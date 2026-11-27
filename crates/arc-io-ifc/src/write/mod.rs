//! IFC writing (CORE-IO-016, first part): an IFC4 physical file from a neutral description of a
//! building.
//!
//! [`WriteModel`] describes a project, a site, a building, its storeys and elements (walls, slabs,
//! doors, windows, openings, spaces, roofs and coverings) with their placement, body geometry, voids and fills, property
//! sets and base quantities, and the element types of walls, slabs, doors, windows, roofs and coverings
//! ([`ElementType`]) with the material layers of wall, slab, roof and covering types and the layer set usage of
//! their occurrences ([`LayerSetUsage`]). [`write()`] turns it into the bytes of an ISO 10303-21 clear text file
//! with the IFC4 schema and a loss report of what it did not write. It knows nothing of any
//! product: the caller maps its own objects to this description.
//!
//! The output is deterministic: the same model and options give the same bytes. Nothing reads the
//! clock; the time stamp of the header is the caller's. Every `GlobalId` comes from an identifier
//! of the model ([`global_id`]) or, for the relationships and sets the writer adds, from the
//! identifier of their object and a role.

mod global_id;
pub(crate) mod step;
mod values;

use std::collections::{BTreeMap, BTreeSet};

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Cancel, Error, Limits, Result, Tolerance};
use arc_io_common::zip::{Method, write_archive};

use crate::error::{codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::Parameter;

pub use global_id::{derived_id, global_id, role};
pub use values::{Measure, PropertyValue, QuantityValue};

/// The length unit of the project. Every length of the model, and the area and volume of
/// quantities and properties, are in this unit (squared or cubed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LengthUnit {
    /// Meters.
    #[default]
    Metre,
    /// Millimeters.
    Millimetre,
}

impl LengthUnit {
    fn meters(self) -> f64 {
        match self {
            Self::Metre => 1.0,
            Self::Millimetre => 1e-3,
        }
    }
}

/// The plane angle unit of the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AngleUnit {
    /// Radians.
    #[default]
    Radian,
    /// Degrees, written as a conversion-based unit of the radian.
    Degree,
}

/// A spatial element without attributes of its own: the project, the site or the building.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Spatial {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name.
    pub name: String,
}

impl Spatial {
    /// A project, site or building.
    #[must_use]
    pub fn new(id: u128, name: &str) -> Self {
        Self {
            id,
            name: name.to_owned(),
        }
    }
}

/// A building storey.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Storey {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name.
    pub name: String,
    /// The elevation of its placement above the building, in the length unit.
    pub elevation: f64,
}

impl Storey {
    /// A storey at the given elevation.
    #[must_use]
    pub fn new(id: u128, name: &str, elevation: f64) -> Self {
        Self {
            id,
            name: name.to_owned(),
            elevation,
        }
    }
}

/// A placement relative to the storey: an origin and the directions of the local z and x axes.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Placement {
    /// The origin, in the length unit.
    pub origin: Point3,
    /// The local z axis; need not be normalized.
    pub axis: Vector3,
    /// The local x axis, made orthogonal to `axis` by a reader; need not be normalized.
    pub ref_direction: Vector3,
}

impl Placement {
    /// A placement at `origin` with the given z and x axes.
    #[must_use]
    pub fn new(origin: Point3, axis: Vector3, ref_direction: Vector3) -> Self {
        Self {
            origin,
            axis,
            ref_direction,
        }
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            origin: Point3::origin(),
            axis: Vector3::z(),
            ref_direction: Vector3::x(),
        }
    }
}

/// The body geometry of an element, in the coordinates of its placement and the length unit.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// A closed polyline profile with optional holes in the plane `z = 0`, extruded along a
    /// direction by a depth. Written as `IfcExtrudedAreaSolid` (`SweptSolid`).
    Extrusion {
        /// The outer loop; a last point equal to the first is dropped.
        outer: Vec<Point2>,
        /// The holes, each a closed loop like `outer`.
        holes: Vec<Vec<Point2>>,
        /// The extrusion direction; it must not lie in the profile plane.
        direction: Vector3,
        /// The extrusion depth along the normalized direction.
        depth: f64,
    },
    /// A closed shell of planar polygon faces. Written as `IfcFacetedBrep` (`Brep`).
    Brep {
        /// The vertices.
        vertices: Vec<Point3>,
        /// The faces, each a loop of indices into `vertices`, counterclockwise seen from outside.
        faces: Vec<Vec<usize>>,
    },
}

/// The kind of an element, which selects its IFC entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    /// `IfcWall`.
    Wall,
    /// `IfcSlab`.
    Slab,
    /// `IfcDoor`, which fills an opening.
    Door,
    /// `IfcWindow`, which fills an opening.
    Window,
    /// `IfcOpeningElement`, which voids a host element.
    Opening,
    /// `IfcRoof`, an element with its own body (`PredefinedType` from `IfcRoofTypeEnum`); its
    /// type is an `IfcRoofType`, whose layers are written as for a slab type.
    Roof,
    /// `IfcCovering`, an element with its own body (`PredefinedType` from `IfcCoveringTypeEnum`,
    /// for example `CEILING`); its type is an `IfcCoveringType`, whose layers are written as for
    /// a slab type. A covering may be the host of an opening.
    Covering,
    /// `IfcSpace`, aggregated by its storey (IFC4 decomposes a storey into its spaces; it does not
    /// contain them). By convention `Element::name` holds the room number and
    /// `Element::long_name` the room name.
    Space,
    /// `IfcStair` (`PredefinedType` from `IfcStairTypeEnum`, for example `STRAIGHT_RUN_STAIR`,
    /// `QUARTER_TURN_STAIR` for an L or `HALF_TURN_STAIR` for a U). A stair decomposes into its
    /// parts: an element whose `host` is the stair (flights, landings and railings) is
    /// aggregated by it (`IfcRelAggregates`) and not contained in the storey. A stair has no
    /// element type.
    Stair,
    /// `IfcStairFlight` (`PredefinedType` from `IfcStairFlightTypeEnum`) with the counts and
    /// dimensions of [`Element::flight`]. A flight whose `host` is a stair is a part of it; one
    /// without a host is contained in its storey. A flight has no element type.
    StairFlight,
    /// `IfcRailing` (`PredefinedType` from `IfcRailingTypeEnum`: `HANDRAIL`, `GUARDRAIL`,
    /// `BALUSTRADE`). A railing whose `host` is a stair is a part of it; one without a host is
    /// contained in its storey. A railing has no element type. A landing is an `IfcSlab` with the
    /// predefined type `LANDING`, a part of a stair in the same way.
    Railing,
    /// `IfcRamp` (`PredefinedType` from `IfcRampTypeEnum`, for example `STRAIGHT_RUN_RAMP`,
    /// `QUARTER_TURN_RAMP` or `HALF_TURN_RAMP`). Like a stair, a ramp decomposes into its parts:
    /// the ramp flights, landings (`IfcSlab` with the predefined type `LANDING`) and railings
    /// that name it as their `host` are aggregated by it (`IfcRelAggregates`) and not contained
    /// in the storey. A ramp has no element type.
    Ramp,
    /// `IfcRampFlight` (`PredefinedType` from `IfcRampFlightTypeEnum`: `STRAIGHT`, `SPIRAL`). A
    /// flight whose `host` is a ramp is a part of it; one without a host is contained in its
    /// storey. IFC4 gives a ramp flight no counts or dimensions: [`Element::flight`] is ignored
    /// and quantities (`Qto_RampFlightBaseQuantities`) are the caller's quantity sets.
    RampFlight,
    /// `IfcCurtainWall` (`PredefinedType` `USERDEFINED` or `NOTDEFINED`). The plates and members
    /// that name it as their `host` are aggregated by it (`IfcRelAggregates`) and not contained
    /// in the storey.
    CurtainWall,
    /// `IfcPlate` (`PredefinedType` from `IfcPlateTypeEnum`: `CURTAIN_PANEL`, `SHEET`). A plate
    /// whose `host` is a curtain wall is a part of it; one without a host is contained in its
    /// storey.
    Plate,
    /// `IfcMember` (`PredefinedType` from `IfcMemberTypeEnum`, for example `MULLION`, `PLATE`;
    /// IFC4 has no transom value: use `MEMBER` or a user-defined text). A member whose
    /// `host` is a curtain wall is a part of it; one without a host is contained in its storey.
    Member,
    /// `IfcTransportElement` (`PredefinedType` from `IfcTransportElementTypeEnum`: `ELEVATOR`,
    /// `ESCALATOR`, `MOVINGWALKWAY`, `CRANEWAY`, `LIFTINGGEAR`), contained in its storey; its type
    /// is an `IfcTransportElementType`.
    TransportElement,
    /// `IfcPipeSegment` (`PredefinedType` from `IfcPipeSegmentTypeEnum`: `CULVERT`,
    /// `FLEXIBLESEGMENT`, `RIGIDSEGMENT`, `GUTTER`, `SPOOL`), contained in its storey; its type is
    /// an `IfcPipeSegmentType`.
    PipeSegment,
}

impl ElementKind {
    fn entity(self) -> &'static str {
        match self {
            Self::Wall => "IFCWALL",
            Self::Slab => "IFCSLAB",
            Self::Door => "IFCDOOR",
            Self::Window => "IFCWINDOW",
            Self::Opening => "IFCOPENINGELEMENT",
            Self::Space => "IFCSPACE",
            Self::Roof => "IFCROOF",
            Self::Covering => "IFCCOVERING",
            Self::Stair => "IFCSTAIR",
            Self::StairFlight => "IFCSTAIRFLIGHT",
            Self::Railing => "IFCRAILING",
            Self::Ramp => "IFCRAMP",
            Self::RampFlight => "IFCRAMPFLIGHT",
            Self::CurtainWall => "IFCCURTAINWALL",
            Self::Plate => "IFCPLATE",
            Self::Member => "IFCMEMBER",
            Self::TransportElement => "IFCTRANSPORTELEMENT",
            Self::PipeSegment => "IFCPIPESEGMENT",
        }
    }

    /// The values of `PredefinedType` besides `USERDEFINED` and `NOTDEFINED`.
    fn predefined(self) -> &'static [&'static str] {
        match self {
            Self::Wall => &[
                "MOVABLE",
                "PARAPET",
                "PARTITIONING",
                "PLUMBINGWALL",
                "SHEAR",
                "SOLIDWALL",
                "STANDARD",
                "POLYGONAL",
            ],
            Self::Slab => &["FLOOR", "ROOF", "LANDING", "BASESLAB"],
            Self::Door => &["DOOR", "GATE", "TRAPDOOR"],
            Self::Window => &["WINDOW", "SKYLIGHT", "LIGHTDOME"],
            Self::Opening => &["OPENING", "RECESS"],
            Self::Space => &["SPACE", "PARKING", "GFA", "INTERNAL", "EXTERNAL"],
            Self::Covering => &[
                "CEILING",
                "FLOORING",
                "CLADDING",
                "ROOFING",
                "MOLDING",
                "SKIRTINGBOARD",
                "INSULATION",
                "MEMBRANE",
                "SLEEVING",
                "WRAPPING",
            ],
            Self::Stair => &[
                "STRAIGHT_RUN_STAIR",
                "TWO_STRAIGHT_RUN_STAIR",
                "QUARTER_WINDING_STAIR",
                "QUARTER_TURN_STAIR",
                "HALF_WINDING_STAIR",
                "HALF_TURN_STAIR",
                "TWO_QUARTER_WINDING_STAIR",
                "TWO_QUARTER_TURN_STAIR",
                "THREE_QUARTER_WINDING_STAIR",
                "THREE_QUARTER_TURN_STAIR",
                "SPIRAL_STAIR",
                "DOUBLE_RETURN_STAIR",
                "CURVED_RUN_STAIR",
                "TWO_CURVED_RUN_STAIR",
            ],
            Self::StairFlight => &["STRAIGHT", "WINDER", "SPIRAL", "CURVED", "FREEFORM"],
            Self::Railing => &["HANDRAIL", "GUARDRAIL", "BALUSTRADE"],
            Self::Ramp => &[
                "STRAIGHT_RUN_RAMP",
                "TWO_STRAIGHT_RUN_RAMP",
                "QUARTER_TURN_RAMP",
                "TWO_QUARTER_TURN_RAMP",
                "HALF_TURN_RAMP",
                "SPIRAL_RAMP",
            ],
            Self::RampFlight => &["STRAIGHT", "SPIRAL"],
            Self::CurtainWall => &[],
            Self::Plate => &["CURTAIN_PANEL", "SHEET"],
            Self::Member => &[
                "BRACE", "CHORD", "COLLAR", "MEMBER", "MULLION", "PLATE", "POST", "PURLIN",
                "RAFTER", "STRINGER", "STRUT", "STUD",
            ],
            Self::TransportElement => &[
                "ELEVATOR",
                "ESCALATOR",
                "MOVINGWALKWAY",
                "CRANEWAY",
                "LIFTINGGEAR",
            ],
            Self::PipeSegment => &[
                "CULVERT",
                "FLEXIBLESEGMENT",
                "RIGIDSEGMENT",
                "GUTTER",
                "SPOOL",
            ],
            Self::Roof => &[
                "FLAT_ROOF",
                "SHED_ROOF",
                "GABLE_ROOF",
                "HIP_ROOF",
                "HIPPED_GABLE_ROOF",
                "GAMBREL_ROOF",
                "MANSARD_ROOF",
                "BARREL_ROOF",
                "RAINBOW_ROOF",
                "BUTTERFLY_ROOF",
                "PAVILION_ROOF",
                "DOME_ROOF",
                "FREEFORM",
            ],
        }
    }
}

/// The door operation types of IFC4 (`IfcDoorTypeOperationEnum`).
const DOOR_OPERATIONS: &[&str] = &[
    "SINGLE_SWING_LEFT",
    "SINGLE_SWING_RIGHT",
    "DOUBLE_DOOR_SINGLE_SWING",
    "DOUBLE_DOOR_SINGLE_SWING_OPPOSITE_LEFT",
    "DOUBLE_DOOR_SINGLE_SWING_OPPOSITE_RIGHT",
    "DOUBLE_SWING_LEFT",
    "DOUBLE_SWING_RIGHT",
    "DOUBLE_DOOR_DOUBLE_SWING",
    "SLIDING_TO_LEFT",
    "SLIDING_TO_RIGHT",
    "DOUBLE_DOOR_SLIDING",
    "FOLDING_TO_LEFT",
    "FOLDING_TO_RIGHT",
    "DOUBLE_DOOR_FOLDING",
    "REVOLVING",
    "ROLLINGUP",
    "SWING_FIXED_LEFT",
    "SWING_FIXED_RIGHT",
];

/// The window partitioning types of IFC4 (`IfcWindowTypePartitioningEnum`).
const WINDOW_PARTITIONINGS: &[&str] = &[
    "SINGLE_PANEL",
    "DOUBLE_PANEL_VERTICAL",
    "DOUBLE_PANEL_HORIZONTAL",
    "TRIPLE_PANEL_VERTICAL",
    "TRIPLE_PANEL_BOTTOM",
    "TRIPLE_PANEL_TOP",
    "TRIPLE_PANEL_LEFT",
    "TRIPLE_PANEL_RIGHT",
    "TRIPLE_PANEL_HORIZONTAL",
];

/// A property of a property set: a single or an enumerated value.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Property {
    /// The name, unique within its set.
    pub name: String,
    /// The value.
    pub value: PropertyValue,
}

impl Property {
    /// A property named `name` with `value`.
    #[must_use]
    pub fn new(name: &str, value: PropertyValue) -> Self {
        Self {
            name: name.to_owned(),
            value,
        }
    }
}

/// A property set (`IfcPropertySet`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PropertySet {
    /// The name, such as `Pset_WallCommon`.
    pub name: String,
    /// The properties.
    pub properties: Vec<Property>,
}

impl PropertySet {
    /// A property set.
    #[must_use]
    pub fn new(name: &str, properties: Vec<Property>) -> Self {
        Self {
            name: name.to_owned(),
            properties,
        }
    }
}

/// A quantity of a quantity set.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Quantity {
    /// The name, unique within its set.
    pub name: String,
    /// The value.
    pub value: QuantityValue,
}

impl Quantity {
    /// A quantity.
    #[must_use]
    pub fn new(name: &str, value: QuantityValue) -> Self {
        Self {
            name: name.to_owned(),
            value,
        }
    }
}

/// A quantity set (`IfcElementQuantity`), such as `Qto_WallBaseQuantities`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct QuantitySet {
    /// The name.
    pub name: String,
    /// The quantities.
    pub quantities: Vec<Quantity>,
}

impl QuantitySet {
    /// A quantity set.
    #[must_use]
    pub fn new(name: &str, quantities: Vec<Quantity>) -> Self {
        Self {
            name: name.to_owned(),
            quantities,
        }
    }
}

/// A layer of a material layer set (`IfcMaterialLayer` with its `IfcMaterial`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MaterialLayer {
    /// The material name. Layers with the same name share one `IfcMaterial`.
    pub material: String,
    /// The thickness in the length unit; zero for a membrane, never negative.
    pub thickness: f64,
    /// The `Name` of the layer; `None` omits it.
    pub name: Option<String>,
    /// The `Category` of the layer, such as `LoadBearing`, `Insulation` or `Finish`; `None` omits
    /// it.
    pub category: Option<String>,
}

impl MaterialLayer {
    /// A layer of `thickness` of the material `material`, without name or category. Whether it
    /// is ventilated is written as unknown.
    #[must_use]
    pub fn new(material: &str, thickness: f64) -> Self {
        Self {
            material: material.to_owned(),
            thickness,
            name: None,
            category: None,
        }
    }
}

/// The axis of an element's placement along which the layers of its type are stacked
/// (`IfcLayerSetDirectionEnum`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDirection {
    /// The local y axis: across a wall whose x axis runs along its axis.
    Axis2,
    /// The local z axis: through the thickness of a slab.
    Axis3,
}

/// How the layers of an element's type lie in the element (`IfcMaterialLayerSetUsage`): the
/// first layer starts at `offset` from the reference line (or plane) of the placement along
/// `direction` and the layers follow toward the positive or the negative side.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LayerSetUsage {
    /// The axis the layers are stacked along.
    pub direction: LayerDirection,
    /// True when the layers follow in the positive direction of the axis.
    pub positive: bool,
    /// Where the first layer starts, in the length unit, along the axis.
    pub offset: f64,
}

impl LayerSetUsage {
    /// A usage along `direction`, starting at `offset`, toward the positive side when `positive`.
    #[must_use]
    pub fn new(direction: LayerDirection, positive: bool, offset: f64) -> Self {
        Self {
            direction,
            positive,
            offset,
        }
    }
}

/// An element type (`IfcWallType`, `IfcSlabType`, `IfcDoorType`, `IfcWindowType`, `IfcRoofType` or
/// `IfcCoveringType`), assigned to
/// the elements whose [`Element::element_type`] names it (`IfcRelDefinesByType`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ElementType {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The kind of its occurrences: a wall, slab, door, window or roof.
    pub kind: ElementKind,
    /// The name.
    pub name: String,
    /// The `PredefinedType`, handled like [`Element::predefined_type`]; the text of another value
    /// is written as the `ElementType`.
    pub predefined_type: Option<String>,
    /// Doors: the `OperationType`; windows: the `PartitioningType`, handled like
    /// [`Element::operation_type`]. Ignored for walls and slabs.
    pub operation_type: Option<String>,
    /// Walls, slabs and roofs: the layers, written as an `IfcMaterialLayerSet` associated with the type;
    /// empty writes no material. Doors and windows: must be empty; layers are reported, not
    /// written.
    pub layers: Vec<MaterialLayer>,
}

impl ElementType {
    /// A type of `kind` without predefined type, operation or layers.
    #[must_use]
    pub fn new(id: u128, kind: ElementKind, name: &str) -> Self {
        Self {
            id,
            kind,
            name: name.to_owned(),
            predefined_type: None,
            operation_type: None,
            layers: Vec::new(),
        }
    }

    fn entity(&self) -> Option<&'static str> {
        match self.kind {
            ElementKind::Wall => Some("IFCWALLTYPE"),
            ElementKind::Slab => Some("IFCSLABTYPE"),
            ElementKind::Door => Some("IFCDOORTYPE"),
            ElementKind::Window => Some("IFCWINDOWTYPE"),
            ElementKind::Roof => Some("IFCROOFTYPE"),
            ElementKind::Covering => Some("IFCCOVERINGTYPE"),
            ElementKind::Plate => Some("IFCPLATETYPE"),
            ElementKind::Member => Some("IFCMEMBERTYPE"),
            ElementKind::CurtainWall => Some("IFCCURTAINWALLTYPE"),
            ElementKind::TransportElement => Some("IFCTRANSPORTELEMENTTYPE"),
            ElementKind::PipeSegment => Some("IFCPIPESEGMENTTYPE"),
            ElementKind::Opening
            | ElementKind::Space
            | ElementKind::Stair
            | ElementKind::StairFlight
            | ElementKind::Railing
            | ElementKind::Ramp
            | ElementKind::RampFlight => None,
        }
    }
}

/// The counts and dimensions of a stair flight (`IfcStairFlight`), in the length unit.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct FlightDimensions {
    /// `NumberOfRisers`.
    pub risers: u32,
    /// `NumberOfTreads`.
    pub treads: u32,
    /// `RiserHeight`, positive.
    pub riser_height: f64,
    /// `TreadLength`, positive.
    pub tread_length: f64,
}

impl FlightDimensions {
    /// The dimensions of a flight.
    #[must_use]
    pub fn new(risers: u32, treads: u32, riser_height: f64, tread_length: f64) -> Self {
        Self {
            risers,
            treads,
            riser_height,
            tread_length,
        }
    }
}

/// An element of the building.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Element {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The kind.
    pub kind: ElementKind,
    /// The name. For a space, the room number.
    pub name: String,
    /// Spaces: the `LongName`, by convention the room name; `None` omits it. Ignored for the other
    /// kinds.
    pub long_name: Option<String>,
    /// The identifier of the storey the element is placed relative to and contained in. An
    /// opening is placed relative to the storey and not contained; a space is aggregated by it.
    pub storey: u128,
    /// The placement relative to the storey.
    pub placement: Placement,
    /// The body geometry; `None` writes the element without a representation.
    pub body: Option<Body>,
    /// The `PredefinedType`, compared in upper case with the values IFC4 defines for the kind.
    /// Another text is written as `USERDEFINED` with the text as the object type; `None` is
    /// `NOTDEFINED`.
    pub predefined_type: Option<String>,
    /// Doors: the `OperationType`; windows: the `PartitioningType`. Handled like
    /// `predefined_type`, with the text of another value in the user-defined attribute. Ignored for
    /// the other kinds.
    pub operation_type: Option<String>,
    /// Doors and windows: `OverallWidth`, positive, in the length unit.
    pub overall_width: Option<f64>,
    /// Doors and windows: `OverallHeight`, positive, in the length unit.
    pub overall_height: Option<f64>,
    /// Openings: the element the opening voids (`IfcRelVoidsElement`), a wall, slab, door or
    /// window; required. Doors and windows: the opening they fill (`IfcRelFillsElement`); a chain
    /// of hosts must not return to its start. Flights, landings (slabs) and railings: optionally
    /// the stair they are a part of (`IfcRelAggregates`); a part is not contained in the storey.
    /// Must be `None` for walls, spaces, roofs, coverings and stairs, and for a slab that is
    /// not part of a stair.
    pub host: Option<u128>,
    /// Stair flights: the `NumberOfRisers`, `NumberOfTreads`, `RiserHeight` and `TreadLength`;
    /// `None` omits them. Ignored for the other kinds.
    pub flight: Option<FlightDimensions>,
    /// The property sets.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets.
    pub quantity_sets: Vec<QuantitySet>,
    /// Walls, slabs, doors and windows: the identifier of the [`ElementType`] of the model the
    /// element is an occurrence of; it must be a type of the same kind. `None` assigns no type.
    pub element_type: Option<u128>,
    /// Walls and slabs: how the layers of the element's type lie in it. Written when the type
    /// has layers; otherwise it is reported, not written.
    pub layer_usage: Option<LayerSetUsage>,
}

impl Element {
    /// An element at the storey's origin, without body, types, host or sets.
    #[must_use]
    pub fn new(id: u128, kind: ElementKind, name: &str, storey: u128) -> Self {
        Self {
            id,
            kind,
            name: name.to_owned(),
            long_name: None,
            storey,
            placement: Placement::default(),
            body: None,
            predefined_type: None,
            operation_type: None,
            overall_width: None,
            overall_height: None,
            host: None,
            flight: None,
            property_sets: Vec::new(),
            quantity_sets: Vec::new(),
            element_type: None,
            layer_usage: None,
        }
    }
}

/// The curve of a grid axis, in the plane of the grid's placement and the length unit.
#[derive(Debug, Clone, PartialEq)]
pub enum AxisCurve {
    /// A straight axis from `start` to `end`, written as an `IfcPolyline` of two points.
    Line {
        /// The first point.
        start: Point2,
        /// The last point; at least the linear tolerance away from `start`.
        end: Point2,
    },
    /// A polyline axis through two or more points, written as an `IfcPolyline`; consecutive
    /// points are at least the linear tolerance apart.
    Polyline(Vec<Point2>),
    /// A circular arc counterclockwise from `start_angle` to `end_angle`, written as an
    /// `IfcTrimmedCurve` of an `IfcCircle` trimmed by parameter values (the angles, converted to
    /// the plane angle unit of the project).
    Arc {
        /// The center.
        center: Point2,
        /// The radius, positive.
        radius: f64,
        /// The start angle from the x axis, in radians.
        start_angle: f64,
        /// The end angle in radians; greater than `start_angle` by at most a full turn.
        end_angle: f64,
    },
    /// Lines, polylines and arcs joined end to start, written as an `IfcCompositeCurve`. Each
    /// segment starts within the linear tolerance of where the previous one ends; when the last
    /// one ends at the start of the first, the curve is closed.
    Composite(Vec<AxisSegment>),
}

/// A segment of a composite axis (`IfcCompositeCurveSegment`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AxisSegment {
    /// The curve: a line, a polyline or an arc, not a composite.
    pub curve: AxisCurve,
    /// `SameSense`: `false` runs the segment from the end of its curve to its start, as for an
    /// arc that turns clockwise.
    pub same_sense: bool,
}

impl AxisSegment {
    /// A segment that runs along its curve.
    #[must_use]
    pub fn new(curve: AxisCurve) -> Self {
        Self {
            curve,
            same_sense: true,
        }
    }

    /// A segment that runs against its curve.
    #[must_use]
    pub fn reversed(curve: AxisCurve) -> Self {
        Self {
            curve,
            same_sense: false,
        }
    }
}

/// A group of elements and grids (`IfcGroup`), assigned by one `IfcRelAssignsToGroup`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Group {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name.
    pub name: String,
    /// The `Description`; `None` omits it.
    pub description: Option<String>,
    /// The `ObjectType`, the kind of grouping; `None` omits it.
    pub object_type: Option<String>,
    /// The identifiers of the elements and grids in the group, in order; a repeat is written
    /// once. An empty group is written without the relationship.
    pub members: Vec<u128>,
}

impl Group {
    /// A group named `name` of `members`.
    #[must_use]
    pub fn new(id: u128, name: &str, members: Vec<u128>) -> Self {
        Self {
            id,
            name: name.to_owned(),
            description: None,
            object_type: None,
            members,
        }
    }
}

/// The georeferencing of the model: an `IfcMapConversion` from the model representation context
/// to an `IfcProjectedCRS`. Eastings, northings and the orthogonal height are in meters, the map
/// unit written with the reference system.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Georeference {
    /// The `Name` of the coordinate reference system, such as `EPSG:32635`; not empty.
    pub crs_name: String,
    /// The `Description`; `None` omits it.
    pub description: Option<String>,
    /// The `GeodeticDatum`, such as `WGS84`; `None` omits it.
    pub geodetic_datum: Option<String>,
    /// The `VerticalDatum`; `None` omits it.
    pub vertical_datum: Option<String>,
    /// The `MapProjection`, such as `UTM`; `None` omits it.
    pub map_projection: Option<String>,
    /// The `MapZone`, such as `35N`; `None` omits it.
    pub map_zone: Option<String>,
    /// `Eastings` of the model origin.
    pub eastings: f64,
    /// `Northings` of the model origin.
    pub northings: f64,
    /// `OrthogonalHeight` of the model origin.
    pub orthogonal_height: f64,
    /// The direction of the model x axis in the map (`XAxisAbscissa`, `XAxisOrdinate`); `None`
    /// omits both (no rotation).
    pub x_axis: Option<Vector2>,
    /// `Scale` from model to map lengths, positive; `None` omits it (1).
    pub scale: Option<f64>,
}

impl Georeference {
    /// A reference system named `crs_name` with the map coordinates of the model origin.
    #[must_use]
    pub fn new(crs_name: &str, eastings: f64, northings: f64, orthogonal_height: f64) -> Self {
        Self {
            crs_name: crs_name.to_owned(),
            description: None,
            geodetic_datum: None,
            vertical_datum: None,
            map_projection: None,
            map_zone: None,
            eastings,
            northings,
            orthogonal_height,
            x_axis: None,
            scale: None,
        }
    }
}

/// An axis of a grid (`IfcGridAxis`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct GridAxis {
    /// The `AxisTag`, the label of the axis; empty omits it.
    pub tag: String,
    /// The curve.
    pub curve: AxisCurve,
    /// `SameSense`: whether the direction of the axis agrees with the direction of the curve.
    pub same_sense: bool,
}

impl GridAxis {
    /// An axis labelled `tag` whose direction agrees with its curve.
    #[must_use]
    pub fn new(tag: &str, curve: AxisCurve) -> Self {
        Self {
            tag: tag.to_owned(),
            curve,
            same_sense: true,
        }
    }
}

/// A grid (`IfcGrid`), contained in its storey, with its axes in three directions. Its
/// representation is a `FootPrint` `GeometricCurveSet` of the axis curves.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Grid {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name.
    pub name: String,
    /// The identifier of the storey the grid is placed relative to and contained in.
    pub storey: u128,
    /// The placement relative to the storey; the axis curves lie in its xy plane.
    pub placement: Placement,
    /// The `UAxes`; at least one.
    pub axes_u: Vec<GridAxis>,
    /// The `VAxes`; at least one.
    pub axes_v: Vec<GridAxis>,
    /// The optional `WAxes`; empty omits the attribute.
    pub axes_w: Vec<GridAxis>,
    /// The `PredefinedType`, handled like [`Element::predefined_type`] with the values of
    /// `IfcGridTypeEnum` (`RECTANGULAR`, `RADIAL`, `TRIANGULAR`, `IRREGULAR`).
    pub predefined_type: Option<String>,
}

impl Grid {
    /// A grid at the storey's origin with the given U and V axes and no W axes.
    #[must_use]
    pub fn new(
        id: u128,
        name: &str,
        storey: u128,
        axes_u: Vec<GridAxis>,
        axes_v: Vec<GridAxis>,
    ) -> Self {
        Self {
            id,
            name: name.to_owned(),
            storey,
            placement: Placement::default(),
            axes_u,
            axes_v,
            axes_w: Vec::new(),
            predefined_type: None,
        }
    }
}

/// The values of `IfcGridTypeEnum`.
const GRID_TYPES: &[&str] = &["RECTANGULAR", "RADIAL", "TRIANGULAR", "IRREGULAR"];

/// The neutral description of a building to write.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WriteModel {
    /// The project.
    pub project: Spatial,
    /// The site, aggregated by the project.
    pub site: Spatial,
    /// The building, aggregated by the site.
    pub building: Spatial,
    /// The storeys, aggregated by the building in this order.
    pub storeys: Vec<Storey>,
    /// The elements, written in this order.
    pub elements: Vec<Element>,
    /// The element types, written in this order after the elements.
    pub types: Vec<ElementType>,
    /// The grids, written in this order after the elements.
    pub grids: Vec<Grid>,
    /// The groups, written in this order after the grids.
    pub groups: Vec<Group>,
    /// The georeferencing; `None` writes none.
    pub georeference: Option<Georeference>,
    /// The length unit.
    pub length_unit: LengthUnit,
    /// The plane angle unit.
    pub angle_unit: AngleUnit,
    /// The direction of true north in the xy plane of the model, if known.
    pub true_north: Option<Vector2>,
}

impl WriteModel {
    /// A model of a project, a site and a building, without storeys or elements, in meters and
    /// radians, without true north.
    #[must_use]
    pub fn new(project: Spatial, site: Spatial, building: Spatial) -> Self {
        Self {
            project,
            site,
            building,
            storeys: Vec::new(),
            elements: Vec::new(),
            types: Vec::new(),
            grids: Vec::new(),
            groups: Vec::new(),
            georeference: None,
            length_unit: LengthUnit::Metre,
            angle_unit: AngleUnit::Radian,
            true_north: None,
        }
    }
}

/// Options of the writer.
#[derive(Debug, Clone)]
pub struct WriteOptions {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling it makes the write return `base.cancelled` before the next element.
    pub cancel: Cancel,
    /// The tolerance: `linear` is the precision of the representation context and the smallest
    /// extrusion depth, `angular` the smallest angle between an extrusion direction and its
    /// profile plane and between the axes of a placement.
    pub tolerance: Tolerance,
    /// The `name` of `FILE_NAME`.
    pub file_name: String,
    /// The `time_stamp` of `FILE_NAME`, an ISO 8601 date and time chosen by the caller.
    pub timestamp: String,
    /// The `author` of `FILE_NAME`.
    pub author: String,
    /// The `organization` of `FILE_NAME`.
    pub organization: String,
    /// The `originating_system` of `FILE_NAME`.
    pub originating_system: String,
    /// The model view definition named in `FILE_DESCRIPTION`.
    pub view_definition: String,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            tolerance: Tolerance::DEFAULT,
            file_name: String::new(),
            timestamp: String::new(),
            author: String::new(),
            organization: String::new(),
            originating_system: String::new(),
            view_definition: "DesignTransferView_V1.0".to_owned(),
        }
    }
}

/// The result of a write.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOutput {
    /// The bytes of the physical file.
    pub bytes: Vec<u8>,
    /// The loss report of the write.
    pub report: LossReport,
}

/// Packs a physical file as an IFC-ZIP archive: one deflated entry named `entry_name`, which
/// [`crate::read`] reads back. The physical file comes from [`write()`] or
/// [`crate::IfcFile::write_step`].
///
/// # Errors
///
/// `io.ifc.archive_entry_missing` when `entry_name` does not end in `.ifc` or names metadata
/// (`__MACOSX/`, `._`), so the reader would not find it; `io.zip.name_invalid` or
/// `io.zip.path_unsafe` for a name the archive reader refuses; `io.limit_exceeded` for a file of
/// 4 GiB or more.
pub fn ifc_zip(entry_name: &str, physical: &[u8]) -> Result<Vec<u8>> {
    if !entry_name.to_ascii_lowercase().ends_with(".ifc") || crate::read::is_metadata(entry_name) {
        return Err(Error::new(codes::IFC_ARCHIVE_ENTRY_MISSING));
    }
    write_archive(&[(entry_name, physical)], Method::Deflate)
}

/// Writes the model as an IFC4 physical file.
///
/// # Errors
///
/// `io.ifc.write_value_invalid` for a number that is not finite, a negative quantity, a door or
/// window size that is not positive, a placement whose axes or a true north that is zero, shorter
/// than about 1.5E-8 or parallel, a NUL in a text, two properties or quantities of one set with
/// the same name, or two sets of one element with the same name;
/// `io.ifc.write_reference_invalid` for a storey, host or element type that is not in the model or
/// of the wrong kind, an element type of an opening or space, an opening without a host, or a chain
/// of hosts that returns to its start; `io.ifc.write_value_invalid` also for a layer thickness that
/// is negative or not finite and a layer set usage offset that is not finite;
/// `io.ifc.write_id_duplicate` when two objects give the same `GlobalId`; `io.limit_exceeded` for
/// `record_count` (instances), `string_bytes`, `array_length`, `nesting_depth` and `memory_bytes`
/// (the size of the output); `base.cancelled` when the token was cancelled.
pub fn write(model: &WriteModel, options: &WriteOptions) -> Result<WriteOutput> {
    options.cancel.check()?;
    let kinds = check_references(model)?;
    let mut writer = Writer {
        data: String::new(),
        next: 1,
        limits: &options.limits,
        tolerance: options.tolerance,
        meters: model.length_unit.meters(),
        angle_scale: match model.angle_unit {
            AngleUnit::Radian => 1.0,
            AngleUnit::Degree => 180.0 / std::f64::consts::PI,
        },
        ids: BTreeSet::new(),
        loss: LossLog::default(),
        enumerations: BTreeMap::new(),
    };
    writer.header(options)?;
    let mut context = writer.context(model)?;
    for element in &model.elements {
        options.cancel.check()?;
        writer.element(element, &mut context)?;
    }
    for element_type in &model.types {
        options.cancel.check()?;
        writer.element_type(element_type, &mut context)?;
    }
    for grid in &model.grids {
        options.cancel.check()?;
        writer.grid(grid, &mut context)?;
    }
    for group in &model.groups {
        options.cancel.check()?;
        writer.group(group, &context)?;
    }
    writer.relationships(model, &context, &kinds)?;
    writer.materials(model, &context)?;
    writer.data.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    writer.loss.add_mapped(len64(
        model.elements.len() + model.types.len() + model.grids.len() + model.groups.len(),
    ));
    let report = writer.loss.finish(LossHeader::new(
        "ifc",
        "IFC4",
        Direction::Export,
        Some(options.limits),
    ));
    Ok(WriteOutput {
        bytes: writer.data.into_bytes(),
        report,
    })
}

/// Whether an element of this kind with a host is a part of that decomposing container (a stair,
/// a ramp or a curtain wall).
fn is_part(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Slab
            | ElementKind::StairFlight
            | ElementKind::RampFlight
            | ElementKind::Railing
            | ElementKind::Plate
            | ElementKind::Member
    )
}

/// Whether an element of this kind decomposes into parts that name it as their host.
fn is_container(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Stair | ElementKind::Ramp | ElementKind::CurtainWall
    )
}

/// Whether a part of kind `part` may be hosted by a container of kind `host`.
fn part_fits(part: ElementKind, host: Option<ElementKind>) -> bool {
    match part {
        ElementKind::Slab | ElementKind::Railing => {
            matches!(host, Some(ElementKind::Stair | ElementKind::Ramp))
        }
        ElementKind::StairFlight => host == Some(ElementKind::Stair),
        ElementKind::RampFlight => host == Some(ElementKind::Ramp),
        ElementKind::Plate | ElementKind::Member => host == Some(ElementKind::CurtainWall),
        _ => false,
    }
}

/// The shortest direction the writer accepts. A shorter one has components whose squares lose
/// their precision or vanish, so its angle is not defined; this is the square root of the machine
/// epsilon of `f64`.
const MIN_DIRECTION_LENGTH: f64 = 1.490_116_119_384_765_6e-8;

/// The direction normalized, or `None` when it is not finite or shorter than
/// [`MIN_DIRECTION_LENGTH`].
fn unit(direction: &Vector3) -> Option<Vector3> {
    let length = direction.norm();
    (length.is_finite() && length > MIN_DIRECTION_LENGTH).then(|| direction / length)
}

/// The start and end of a line, polyline or arc; `None` for a composite or an empty polyline.
fn curve_ends(curve: &AxisCurve) -> Option<(Point2, Point2)> {
    match curve {
        AxisCurve::Line { start, end } => Some((*start, *end)),
        AxisCurve::Polyline(points) => Some((*points.first()?, *points.last()?)),
        &AxisCurve::Arc {
            center,
            radius,
            start_angle,
            end_angle,
        } => {
            let at = |a: f64| center + Vector2::new(a.cos(), a.sin()) * radius;
            Some((at(start_angle), at(end_angle)))
        }
        AxisCurve::Composite(_) => None,
    }
}

fn len64(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

fn invalid(what: &'static str) -> Error {
    Error::new(codes::IFC_WRITE_VALUE_INVALID).with_arg("value", what)
}

fn bad_reference(id: u128) -> Error {
    Error::new(codes::IFC_WRITE_REFERENCE_INVALID).with_arg("global_id", global_id(id))
}

/// Checks that element identifiers are unique, that every storey and host exists and has a kind
/// the relationship allows, that every opening has a host, that no chain of hosts returns to its
/// start, and that the set names of each element are unique; returns the kind of every element.
fn check_references(model: &WriteModel) -> Result<BTreeMap<u128, ElementKind>> {
    let storeys: BTreeSet<u128> = model.storeys.iter().map(|s| s.id).collect();
    let mut kinds = BTreeMap::new();
    for element in &model.elements {
        if kinds.insert(element.id, element.kind).is_some() {
            return Err(Error::new(codes::IFC_WRITE_ID_DUPLICATE)
                .with_arg("global_id", global_id(element.id)));
        }
    }
    let mut type_kinds = BTreeMap::new();
    for element_type in &model.types {
        if element_type.entity().is_none() {
            return Err(bad_reference(element_type.id));
        }
        if kinds.contains_key(&element_type.id)
            || type_kinds
                .insert(element_type.id, element_type.kind)
                .is_some()
        {
            return Err(Error::new(codes::IFC_WRITE_ID_DUPLICATE)
                .with_arg("global_id", global_id(element_type.id)));
        }
    }
    for element in &model.elements {
        if !storeys.contains(&element.storey) {
            return Err(bad_reference(element.storey));
        }
        if let Some(id) = element.element_type
            && type_kinds.get(&id) != Some(&element.kind)
        {
            return Err(bad_reference(id));
        }
        let mut set_names = BTreeSet::new();
        let names = element.property_sets.iter().map(|p| &p.name);
        if !names
            .chain(element.quantity_sets.iter().map(|q| &q.name))
            .all(|name| set_names.insert(name))
        {
            return Err(invalid("property set name"));
        }
        let Some(host) = element.host else {
            if element.kind == ElementKind::Opening {
                // An opening voids exactly one element (`VoidsElements` is 1:1).
                return Err(bad_reference(element.id));
            }
            continue;
        };
        let host_kind = kinds.get(&host).copied();
        let allowed = match element.kind {
            ElementKind::Opening => host_kind.is_some_and(|k| k != ElementKind::Opening),
            ElementKind::Door | ElementKind::Window => host_kind == Some(ElementKind::Opening),
            kind if is_part(kind) => part_fits(kind, host_kind),
            _ => false,
        };
        if !allowed {
            return Err(bad_reference(host));
        }
    }
    check_host_cycles(model)?;
    Ok(kinds)
}

/// Refuses a chain of hosts that returns to an element on it, such as an opening hosted by the
/// door that fills it. Each element has at most one host, so one walk per element suffices and
/// every element is visited once.
fn check_host_cycles(model: &WriteModel) -> Result<()> {
    let hosts: BTreeMap<u128, u128> = model
        .elements
        .iter()
        .filter_map(|e| e.host.map(|h| (e.id, h)))
        .collect();
    // `false`: on the current walk; `true`: finished.
    let mut state: BTreeMap<u128, bool> = BTreeMap::new();
    for element in &model.elements {
        let mut walk = Vec::new();
        let mut current = Some(element.id);
        while let Some(id) = current {
            match state.get(&id) {
                Some(true) => break,
                Some(false) => return Err(bad_reference(id)),
                None => {
                    state.insert(id, false);
                    walk.push(id);
                    current = hosts.get(&id).copied();
                }
            }
        }
        for id in walk {
            state.insert(id, true);
        }
    }
    Ok(())
}

/// An enumeration value from a caller's text: a known value (or `NOTDEFINED`) as it is, another
/// text as `USERDEFINED` with the text returned for the user-defined attribute.
fn choice(value: Option<&str>, known: &[&str]) -> (Parameter, Option<Parameter>) {
    let Some(text) = value.filter(|t| !t.is_empty()) else {
        return (enumeration("NOTDEFINED"), None);
    };
    let upper = text.to_ascii_uppercase();
    if upper == "NOTDEFINED" || known.contains(&upper.as_str()) {
        (Parameter::Enumeration(upper), None)
    } else {
        (enumeration("USERDEFINED"), Some(label(text)))
    }
}

fn enumeration(value: &str) -> Parameter {
    Parameter::Enumeration(value.to_owned())
}

fn boolean(value: bool) -> Parameter {
    enumeration(if value { "T" } else { "F" })
}

fn label(value: &str) -> Parameter {
    Parameter::Text(value.to_owned())
}

fn reference(name: u64) -> Parameter {
    Parameter::Reference(name)
}

fn references(names: &[u64]) -> Parameter {
    Parameter::List(names.iter().copied().map(reference).collect())
}

fn reals(values: &[f64]) -> Parameter {
    Parameter::List(values.iter().copied().map(Parameter::Real).collect())
}

fn typed(type_name: &str, value: Parameter) -> Parameter {
    Parameter::Typed {
        type_name: type_name.to_owned(),
        value: Box::new(value),
    }
}

/// The instances shared by the elements and the spatial structure.
struct Context {
    project: u64,
    site: u64,
    building: u64,
    wcs: u64,
    body: u64,
    /// The `FootPrint` subcontext, written when the model has grids.
    footprint: Option<u64>,
    storeys: BTreeMap<u128, (u64, u64)>,
    elements: BTreeMap<u128, u64>,
    grids: BTreeMap<u128, u64>,
    types: BTreeMap<u128, u64>,
}

struct Writer<'a> {
    data: String,
    next: u64,
    limits: &'a Limits,
    tolerance: Tolerance,
    meters: f64,
    /// The factor from radians to the plane angle unit of the project.
    angle_scale: f64,
    ids: BTreeSet<u128>,
    loss: LossLog,
    /// The `IfcPropertyEnumeration` written per name, with its items.
    enumerations: BTreeMap<String, (Vec<String>, u64)>,
}

impl Writer<'_> {
    fn add(&mut self, type_name: &str, parameters: &[Parameter]) -> Result<u64> {
        let name = self.next;
        if name > self.limits.record_count {
            return Err(limit("record_count", name));
        }
        step::record(&mut self.data, name, type_name, parameters, self.limits)?;
        self.check_size()?;
        self.next += 1;
        Ok(name)
    }

    fn check_size(&self) -> Result<()> {
        let size = len64(self.data.len());
        match self.limits.memory_bytes {
            Some(max) if size > max => Err(limit("memory_bytes", size)),
            _ => Ok(()),
        }
    }

    /// The `GlobalId` parameter of an identifier, refused when the file already holds it.
    fn global_id(&mut self, id: u128) -> Result<Parameter> {
        if !self.ids.insert(id) {
            return Err(
                Error::new(codes::IFC_WRITE_ID_DUPLICATE).with_arg("global_id", global_id(id))
            );
        }
        Ok(Parameter::Text(global_id(id)))
    }

    fn header(&mut self, options: &WriteOptions) -> Result<()> {
        self.data.push_str("ISO-10303-21;\nHEADER;\n");
        let view = format!("ViewDefinition [{}]", options.view_definition);
        let records = [
            (
                "FILE_DESCRIPTION",
                vec![Parameter::List(vec![label(&view)]), label("2;1")],
            ),
            (
                "FILE_NAME",
                vec![
                    label(&options.file_name),
                    label(&options.timestamp),
                    Parameter::List(vec![label(&options.author)]),
                    Parameter::List(vec![label(&options.organization)]),
                    label(concat!("arc-io-ifc ", env!("CARGO_PKG_VERSION"))),
                    label(&options.originating_system),
                    label(""),
                ],
            ),
            ("FILE_SCHEMA", vec![Parameter::List(vec![label("IFC4")])]),
        ];
        for (type_name, parameters) in &records {
            step::header_record(&mut self.data, type_name, parameters, self.limits)?;
        }
        self.data.push_str("ENDSEC;\nDATA;\n");
        self.check_size()
    }

    fn point3(&mut self, point: &Point3) -> Result<u64> {
        self.add("IFCCARTESIANPOINT", &[reals(&[point.x, point.y, point.z])])
    }

    fn point2(&mut self, point: &Point2) -> Result<u64> {
        self.add("IFCCARTESIANPOINT", &[reals(&[point.x, point.y])])
    }

    fn direction(&mut self, vector: &Vector3) -> Result<u64> {
        self.add("IFCDIRECTION", &[reals(&[vector.x, vector.y, vector.z])])
    }

    /// An `IfcAxis2Placement3D`; the axes are checked to be finite, non-zero and not parallel.
    fn axes(&mut self, placement: &Placement) -> Result<u64> {
        let Placement {
            origin,
            axis,
            ref_direction,
        } = placement;
        let finite = origin.iter().chain(axis.iter()).chain(ref_direction.iter());
        if !finite.copied().all(f64::is_finite) {
            return Err(invalid("placement"));
        }
        let (Some(z), Some(x)) = (unit(axis), unit(ref_direction)) else {
            return Err(invalid("placement"));
        };
        if z.cross(&x).norm() <= self.tolerance.angular {
            return Err(invalid("placement"));
        }
        let location = self.point3(origin)?;
        let axis = self.direction(axis)?;
        let ref_direction = self.direction(ref_direction)?;
        self.add(
            "IFCAXIS2PLACEMENT3D",
            &[
                reference(location),
                reference(axis),
                reference(ref_direction),
            ],
        )
    }

    fn local_placement(&mut self, relative_to: Option<u64>, axes: u64) -> Result<u64> {
        let relative_to = relative_to.map_or(Parameter::Omitted, reference);
        self.add("IFCLOCALPLACEMENT", &[relative_to, reference(axes)])
    }

    /// Units, the representation contexts, the project, the site, the building and the storeys.
    fn context(&mut self, model: &WriteModel) -> Result<Context> {
        let units = self.units(model)?;
        let origin = self.point3(&Point3::origin())?;
        let wcs = self.add(
            "IFCAXIS2PLACEMENT3D",
            &[reference(origin), Parameter::Omitted, Parameter::Omitted],
        )?;
        let north = match model.true_north {
            Some(north) => {
                if unit(&Vector3::new(north.x, north.y, 0.0)).is_none() {
                    return Err(invalid("true north"));
                }
                reference(self.add("IFCDIRECTION", &[reals(&[north.x, north.y])])?)
            }
            None => Parameter::Omitted,
        };
        let model_context = self.add(
            "IFCGEOMETRICREPRESENTATIONCONTEXT",
            &[
                Parameter::Omitted,
                label("Model"),
                Parameter::Integer(3),
                Parameter::Real(self.tolerance.linear / self.meters),
                reference(wcs),
                north,
            ],
        )?;
        if let Some(georeference) = &model.georeference {
            self.georeference(georeference, model_context)?;
        }
        let body = self.subcontext("Body", model_context)?;
        let footprint = if model.grids.is_empty() {
            None
        } else {
            Some(self.subcontext("FootPrint", model_context)?)
        };
        let project_id = self.global_id(model.project.id)?;
        let project = self.add(
            "IFCPROJECT",
            &[
                project_id,
                Parameter::Omitted,
                label(&model.project.name),
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                references(&[model_context]),
                reference(units),
            ],
        )?;
        let site_placement = self.local_placement(None, wcs)?;
        let site_id = self.global_id(model.site.id)?;
        let mut site_parameters = self.spatial_head(site_id, &model.site.name, site_placement);
        site_parameters.extend(std::iter::repeat_n(Parameter::Omitted, 5));
        let site = self.add("IFCSITE", &site_parameters)?;
        let building_placement = self.local_placement(Some(site_placement), wcs)?;
        let building_id = self.global_id(model.building.id)?;
        let mut building_parameters =
            self.spatial_head(building_id, &model.building.name, building_placement);
        building_parameters.extend(std::iter::repeat_n(Parameter::Omitted, 3));
        let building = self.add("IFCBUILDING", &building_parameters)?;
        let mut storeys = BTreeMap::new();
        for storey in &model.storeys {
            let axes = self.axes(&Placement {
                origin: Point3::new(0.0, 0.0, storey.elevation),
                ..Placement::default()
            })?;
            let placement = self.local_placement(Some(building_placement), axes)?;
            let storey_id = self.global_id(storey.id)?;
            let mut parameters = self.spatial_head(storey_id, &storey.name, placement);
            parameters.push(Parameter::Real(storey.elevation));
            let instance = self.add("IFCBUILDINGSTOREY", &parameters)?;
            storeys.insert(storey.id, (instance, placement));
        }
        Ok(Context {
            project,
            site,
            building,
            wcs,
            body,
            footprint,
            storeys,
            elements: BTreeMap::new(),
            grids: BTreeMap::new(),
            types: BTreeMap::new(),
        })
    }

    /// A `Model` subcontext of the model context for one representation identifier.
    fn subcontext(&mut self, identifier: &str, model_context: u64) -> Result<u64> {
        self.add(
            "IFCGEOMETRICREPRESENTATIONSUBCONTEXT",
            &[
                label(identifier),
                label("Model"),
                Parameter::Derived,
                Parameter::Derived,
                Parameter::Derived,
                Parameter::Derived,
                reference(model_context),
                Parameter::Omitted,
                enumeration("MODEL_VIEW"),
                Parameter::Omitted,
            ],
        )
    }

    /// The first nine attributes of a spatial structure element: the rooted attributes, the
    /// placement, no representation or long name, and `CompositionType` `ELEMENT`.
    fn spatial_head(&self, id: Parameter, name: &str, placement: u64) -> Vec<Parameter> {
        vec![
            id,
            Parameter::Omitted,
            label(name),
            Parameter::Omitted,
            Parameter::Omitted,
            reference(placement),
            Parameter::Omitted,
            Parameter::Omitted,
            enumeration("ELEMENT"),
        ]
    }

    fn units(&mut self, model: &WriteModel) -> Result<u64> {
        let prefix = match model.length_unit {
            LengthUnit::Metre => Parameter::Omitted,
            LengthUnit::Millimetre => enumeration("MILLI"),
        };
        let mut units = Vec::new();
        for (unit_type, name) in [
            ("LENGTHUNIT", "METRE"),
            ("AREAUNIT", "SQUARE_METRE"),
            ("VOLUMEUNIT", "CUBIC_METRE"),
        ] {
            units.push(self.add(
                "IFCSIUNIT",
                &[
                    Parameter::Derived,
                    enumeration(unit_type),
                    prefix.clone(),
                    enumeration(name),
                ],
            )?);
        }
        let radian = self.add(
            "IFCSIUNIT",
            &[
                Parameter::Derived,
                enumeration("PLANEANGLEUNIT"),
                Parameter::Omitted,
                enumeration("RADIAN"),
            ],
        )?;
        let angle = match model.angle_unit {
            AngleUnit::Radian => radian,
            AngleUnit::Degree => {
                let factor = self.add(
                    "IFCMEASUREWITHUNIT",
                    &[
                        typed(
                            "IFCPLANEANGLEMEASURE",
                            Parameter::Real(std::f64::consts::PI / 180.0),
                        ),
                        reference(radian),
                    ],
                )?;
                let dimensions = self.add(
                    "IFCDIMENSIONALEXPONENTS",
                    &[
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                        Parameter::Integer(0),
                    ],
                )?;
                self.add(
                    "IFCCONVERSIONBASEDUNIT",
                    &[
                        reference(dimensions),
                        enumeration("PLANEANGLEUNIT"),
                        label("DEGREE"),
                        reference(factor),
                    ],
                )?
            }
        };
        units.push(angle);
        self.add("IFCUNITASSIGNMENT", &[references(&units)])
    }

    fn element(&mut self, element: &Element, context: &mut Context) -> Result<()> {
        let id = self.global_id(element.id)?;
        let storey_placement = context
            .storeys
            .get(&element.storey)
            .map(|&(_, placement)| placement)
            .ok_or_else(|| bad_reference(element.storey))?;
        let axes = self.axes(&element.placement)?;
        let placement = self.local_placement(Some(storey_placement), axes)?;
        let representation = match &element.body {
            Some(body) => self.body(body, element.id, context)?,
            None => None,
        }
        .map_or(Parameter::Omitted, reference);
        let (predefined, user_type) = choice(
            element.predefined_type.as_deref(),
            element.kind.predefined(),
        );
        let mut parameters = vec![
            id,
            Parameter::Omitted,
            label(&element.name),
            Parameter::Omitted,
            user_type.unwrap_or(Parameter::Omitted),
            reference(placement),
            representation,
        ];
        if element.kind == ElementKind::Space {
            // `LongName`, `CompositionType`, `PredefinedType`, `ElevationWithFlooring`.
            parameters.extend([
                element
                    .long_name
                    .as_deref()
                    .map_or(Parameter::Omitted, label),
                enumeration("ELEMENT"),
                predefined,
                Parameter::Omitted,
            ]);
            let instance = self.add(element.kind.entity(), &parameters)?;
            context.elements.insert(element.id, instance);
            self.property_sets(element, instance)?;
            return self.quantity_sets(element, instance);
        }
        parameters.push(Parameter::Omitted);
        let operations = match element.kind {
            ElementKind::Door => Some(DOOR_OPERATIONS),
            ElementKind::Window => Some(WINDOW_PARTITIONINGS),
            _ => None,
        };
        if let Some(operations) = operations {
            for size in [element.overall_height, element.overall_width] {
                parameters.push(match size {
                    Some(value) if value.is_finite() && value > 0.0 => Parameter::Real(value),
                    Some(_) => return Err(invalid("overall size")),
                    None => Parameter::Omitted,
                });
            }
            let (operation, user_operation) = choice(element.operation_type.as_deref(), operations);
            parameters.extend([
                predefined,
                operation,
                user_operation.unwrap_or(Parameter::Omitted),
            ]);
        } else if element.kind == ElementKind::StairFlight {
            match element.flight {
                Some(flight) => {
                    let sizes = [flight.riser_height, flight.tread_length];
                    if !sizes.iter().all(|v| v.is_finite() && *v > 0.0) {
                        return Err(invalid("flight dimensions"));
                    }
                    parameters.extend([
                        Parameter::Integer(i64::from(flight.risers)),
                        Parameter::Integer(i64::from(flight.treads)),
                        Parameter::Real(flight.riser_height),
                        Parameter::Real(flight.tread_length),
                    ]);
                }
                None => parameters.extend(std::iter::repeat_n(Parameter::Omitted, 4)),
            }
            parameters.push(predefined);
        } else {
            parameters.push(predefined);
        }
        let instance = self.add(element.kind.entity(), &parameters)?;
        context.elements.insert(element.id, instance);
        self.property_sets(element, instance)?;
        self.quantity_sets(element, instance)
    }

    /// An `IfcGrid`: placed relative to the storey, its axes as `IfcGridAxis` lists and a
    /// `FootPrint` `GeometricCurveSet` of the axis curves as representation.
    fn grid(&mut self, grid: &Grid, context: &mut Context) -> Result<()> {
        let id = self.global_id(grid.id)?;
        let storey_placement = context
            .storeys
            .get(&grid.storey)
            .map(|&(_, placement)| placement)
            .ok_or_else(|| bad_reference(grid.storey))?;
        let footprint = context.footprint.ok_or_else(|| bad_reference(grid.id))?;
        if grid.axes_u.is_empty() || grid.axes_v.is_empty() {
            return Err(invalid("grid axes"));
        }
        let axes = self.axes(&grid.placement)?;
        let placement = self.local_placement(Some(storey_placement), axes)?;
        let mut curves = Vec::new();
        let mut lists = Vec::with_capacity(3);
        for list in [&grid.axes_u, &grid.axes_v, &grid.axes_w] {
            let mut instances = Vec::with_capacity(list.len());
            for axis in list {
                let curve = self.axis_curve(&axis.curve)?;
                curves.push(curve);
                instances.push(self.add(
                    "IFCGRIDAXIS",
                    &[
                        if axis.tag.is_empty() {
                            Parameter::Omitted
                        } else {
                            label(&axis.tag)
                        },
                        reference(curve),
                        boolean(axis.same_sense),
                    ],
                )?);
            }
            lists.push(if instances.is_empty() {
                Parameter::Omitted
            } else {
                references(&instances)
            });
        }
        let set = self.add("IFCGEOMETRICCURVESET", &[references(&curves)])?;
        let representation = self.add(
            "IFCSHAPEREPRESENTATION",
            &[
                reference(footprint),
                label("FootPrint"),
                label("GeometricCurveSet"),
                references(&[set]),
            ],
        )?;
        let shape = self.add(
            "IFCPRODUCTDEFINITIONSHAPE",
            &[
                Parameter::Omitted,
                Parameter::Omitted,
                references(&[representation]),
            ],
        )?;
        let (predefined, user_type) = choice(grid.predefined_type.as_deref(), GRID_TYPES);
        let mut parameters = vec![
            id,
            Parameter::Omitted,
            label(&grid.name),
            Parameter::Omitted,
            user_type.unwrap_or(Parameter::Omitted),
            reference(placement),
            reference(shape),
        ];
        parameters.extend(lists);
        parameters.push(predefined);
        let instance = self.add("IFCGRID", &parameters)?;
        context.grids.insert(grid.id, instance);
        Ok(())
    }

    /// The curve of a grid axis in two dimensions.
    fn axis_curve(&mut self, curve: &AxisCurve) -> Result<u64> {
        match curve {
            AxisCurve::Line { start, end } => self.polyline_axis(&[*start, *end]),
            AxisCurve::Polyline(points) => self.polyline_axis(points),
            &AxisCurve::Arc {
                center,
                radius,
                start_angle,
                end_angle,
            } => {
                let sweep = end_angle - start_angle;
                let valid = [center.x, center.y, radius, start_angle, end_angle]
                    .iter()
                    .all(|v| v.is_finite())
                    && radius * self.meters > self.tolerance.linear
                    && sweep > self.tolerance.angular
                    && sweep <= std::f64::consts::TAU;
                if !valid {
                    return Err(invalid("grid axis"));
                }
                let location = self.point2(&center)?;
                let position = self.add(
                    "IFCAXIS2PLACEMENT2D",
                    &[reference(location), Parameter::Omitted],
                )?;
                let circle =
                    self.add("IFCCIRCLE", &[reference(position), Parameter::Real(radius)])?;
                let trim = |angle: f64| {
                    Parameter::List(vec![typed(
                        "IFCPARAMETERVALUE",
                        Parameter::Real(angle * self.angle_scale),
                    )])
                };
                let (first, last) = (trim(start_angle), trim(end_angle));
                self.add(
                    "IFCTRIMMEDCURVE",
                    &[
                        reference(circle),
                        first,
                        last,
                        boolean(true),
                        enumeration("PARAMETER"),
                    ],
                )
            }
            AxisCurve::Composite(segments) => self.composite_axis(segments),
        }
    }

    /// An `IfcPolyline` of two or more points, consecutive points apart by the linear tolerance.
    fn polyline_axis(&mut self, points: &[Point2]) -> Result<u64> {
        let finite = points.iter().all(|p| p.x.is_finite() && p.y.is_finite());
        let apart = points
            .windows(2)
            .all(|w| (w[1] - w[0]).norm() * self.meters > self.tolerance.linear);
        if points.len() < 2 || !finite || !apart {
            return Err(invalid("grid axis"));
        }
        let mut names = Vec::with_capacity(points.len());
        for point in points {
            names.push(self.point2(point)?);
        }
        self.add("IFCPOLYLINE", &[references(&names)])
    }

    /// An `IfcCompositeCurve` of line, polyline and arc segments joined end to start. The last
    /// segment is `DISCONTINUOUS` unless it closes the curve, as the curve's continuity rule
    /// requires.
    fn composite_axis(&mut self, segments: &[AxisSegment]) -> Result<u64> {
        let mut ends = Vec::with_capacity(segments.len());
        for segment in segments {
            let Some((start, end)) = curve_ends(&segment.curve) else {
                return Err(invalid("grid axis"));
            };
            ends.push(if segment.same_sense {
                (start, end)
            } else {
                (end, start)
            });
        }
        let near = |a: Point2, b: Point2| (b - a).norm() * self.meters <= self.tolerance.linear;
        if ends.is_empty() || !ends.windows(2).all(|w| near(w[0].1, w[1].0)) {
            return Err(invalid("grid axis"));
        }
        let closed = match (ends.first(), ends.last()) {
            (Some(first), Some(last)) => near(last.1, first.0),
            _ => false,
        };
        let mut written = Vec::with_capacity(segments.len());
        for (i, segment) in segments.iter().enumerate() {
            let parent = self.axis_curve(&segment.curve)?;
            let transition = if i + 1 < segments.len() || closed {
                "CONTINUOUS"
            } else {
                "DISCONTINUOUS"
            };
            written.push(self.add(
                "IFCCOMPOSITECURVESEGMENT",
                &[
                    enumeration(transition),
                    boolean(segment.same_sense),
                    reference(parent),
                ],
            )?);
        }
        self.add(
            "IFCCOMPOSITECURVE",
            &[references(&written), enumeration("U")],
        )
    }

    /// An `IfcMapConversion` from the model context to an `IfcProjectedCRS` in meters.
    fn georeference(&mut self, georeference: &Georeference, model_context: u64) -> Result<()> {
        let g = georeference;
        let numbers = [g.eastings, g.northings, g.orthogonal_height];
        let texts = [
            Some(&g.crs_name),
            g.description.as_ref(),
            g.geodetic_datum.as_ref(),
            g.vertical_datum.as_ref(),
            g.map_projection.as_ref(),
            g.map_zone.as_ref(),
        ];
        let x_axis = match g.x_axis {
            Some(v) => {
                Some(unit(&Vector3::new(v.x, v.y, 0.0)).ok_or_else(|| invalid("georeference"))?)
            }
            None => None,
        };
        let valid = numbers.iter().all(|v| v.is_finite())
            && !g.crs_name.is_empty()
            && texts
                .iter()
                .flatten()
                .all(|t| !t.contains('\0') && t.chars().count() <= 255)
            && g.scale.is_none_or(|s| s.is_finite() && s > 0.0);
        if !valid {
            return Err(invalid("georeference"));
        }
        let text = |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
        let metre = self.add(
            "IFCSIUNIT",
            &[
                Parameter::Derived,
                enumeration("LENGTHUNIT"),
                Parameter::Omitted,
                enumeration("METRE"),
            ],
        )?;
        let crs = self.add(
            "IFCPROJECTEDCRS",
            &[
                label(&g.crs_name),
                text(&g.description),
                text(&g.geodetic_datum),
                text(&g.vertical_datum),
                text(&g.map_projection),
                text(&g.map_zone),
                reference(metre),
            ],
        )?;
        let (abscissa, ordinate) = match x_axis {
            Some(v) => (Parameter::Real(v.x), Parameter::Real(v.y)),
            None => (Parameter::Omitted, Parameter::Omitted),
        };
        self.add(
            "IFCMAPCONVERSION",
            &[
                reference(model_context),
                reference(crs),
                Parameter::Real(g.eastings),
                Parameter::Real(g.northings),
                Parameter::Real(g.orthogonal_height),
                abscissa,
                ordinate,
                g.scale.map_or(Parameter::Omitted, Parameter::Real),
            ],
        )
        .map(|_| ())
    }

    /// An `IfcGroup` and the `IfcRelAssignsToGroup` of its members.
    fn group(&mut self, group: &Group, context: &Context) -> Result<()> {
        let id = self.global_id(group.id)?;
        let optional = |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
        let instance = self.add(
            "IFCGROUP",
            &[
                id,
                Parameter::Omitted,
                label(&group.name),
                optional(&group.description),
                optional(&group.object_type),
            ],
        )?;
        let mut seen = BTreeSet::new();
        let mut members = Vec::with_capacity(group.members.len());
        for &member in &group.members {
            let written = context
                .elements
                .get(&member)
                .or_else(|| context.grids.get(&member))
                .copied()
                .ok_or_else(|| bad_reference(member))?;
            if seen.insert(written) {
                members.push(written);
            }
        }
        if members.is_empty() {
            return Ok(());
        }
        let rel_id = self.global_id(derived_id(group.id, role::GROUP, 0))?;
        self.add(
            "IFCRELASSIGNSTOGROUP",
            &[
                rel_id,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                references(&members),
                Parameter::Omitted,
                reference(instance),
            ],
        )
        .map(|_| ())
    }

    /// An element type: the rooted attributes, the `ElementType` text of a user-defined
    /// predefined type and the `PredefinedType`; for doors and windows also the operation.
    fn element_type(&mut self, element_type: &ElementType, context: &mut Context) -> Result<()> {
        let entity = element_type
            .entity()
            .ok_or_else(|| bad_reference(element_type.id))?;
        let id = self.global_id(element_type.id)?;
        let (predefined, user_type) = choice(
            element_type.predefined_type.as_deref(),
            element_type.kind.predefined(),
        );
        let mut parameters = vec![
            id,
            Parameter::Omitted,
            label(&element_type.name),
            Parameter::Omitted,
            Parameter::Omitted,
            Parameter::Omitted,
            Parameter::Omitted,
            Parameter::Omitted,
            user_type.unwrap_or(Parameter::Omitted),
            predefined,
        ];
        let operations = match element_type.kind {
            ElementKind::Door => Some(DOOR_OPERATIONS),
            ElementKind::Window => Some(WINDOW_PARTITIONINGS),
            _ => None,
        };
        if let Some(operations) = operations {
            let (operation, user_operation) =
                choice(element_type.operation_type.as_deref(), operations);
            // `OperationType` (or `PartitioningType`), `ParameterTakesPrecedence`, the user text.
            parameters.extend([
                operation,
                Parameter::Omitted,
                user_operation.unwrap_or(Parameter::Omitted),
            ]);
        }
        let instance = self.add(entity, &parameters)?;
        context.types.insert(element_type.id, instance);
        Ok(())
    }

    /// The material layer sets of the wall, slab and roof types, associated with their types, and the
    /// layer set usage of each element that has one, associated with the element. Layers of door
    /// and window types and a usage without a layer set are reported, not written.
    fn materials(&mut self, model: &WriteModel, context: &Context) -> Result<()> {
        let mut materials: BTreeMap<&str, u64> = BTreeMap::new();
        let mut sets: BTreeMap<u128, u64> = BTreeMap::new();
        for element_type in &model.types {
            if element_type.layers.is_empty() {
                continue;
            }
            if !matches!(
                element_type.kind,
                ElementKind::Wall | ElementKind::Slab | ElementKind::Roof | ElementKind::Covering
            ) {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCMATERIALLAYERSET",
                    Some(Source::Text(&global_id(element_type.id))),
                    1,
                );
                continue;
            }
            let mut layers = Vec::with_capacity(element_type.layers.len());
            for layer in &element_type.layers {
                if !layer.thickness.is_finite() || layer.thickness < 0.0 {
                    return Err(invalid("layer thickness"));
                }
                let material = match materials.get(layer.material.as_str()) {
                    Some(&material) => material,
                    None => {
                        let material = self.add(
                            "IFCMATERIAL",
                            &[
                                label(&layer.material),
                                Parameter::Omitted,
                                Parameter::Omitted,
                            ],
                        )?;
                        materials.insert(&layer.material, material);
                        material
                    }
                };
                let optional =
                    |text: &Option<String>| text.as_deref().map_or(Parameter::Omitted, label);
                layers.push(self.add(
                    "IFCMATERIALLAYER",
                    &[
                        reference(material),
                        Parameter::Real(layer.thickness),
                        // `IsVentilated`: unknown.
                        enumeration("U"),
                        optional(&layer.name),
                        Parameter::Omitted,
                        optional(&layer.category),
                        Parameter::Omitted,
                    ],
                )?);
            }
            let set = self.add(
                "IFCMATERIALLAYERSET",
                &[
                    references(&layers),
                    label(&element_type.name),
                    Parameter::Omitted,
                ],
            )?;
            sets.insert(element_type.id, set);
            if let Some(&instance) = context.types.get(&element_type.id) {
                self.relationship(
                    "IFCRELASSOCIATESMATERIAL",
                    derived_id(element_type.id, role::MATERIAL, 0),
                    references(&[instance]),
                    reference(set),
                )?;
            }
        }
        self.layer_usages(model, context, &sets)
    }

    /// The layer set usage of each element that has one and whose type has a layer set in `sets`.
    fn layer_usages(
        &mut self,
        model: &WriteModel,
        context: &Context,
        sets: &BTreeMap<u128, u64>,
    ) -> Result<()> {
        for element in &model.elements {
            let Some(usage) = element.layer_usage else {
                continue;
            };
            if !usage.offset.is_finite() {
                return Err(invalid("layer set usage"));
            }
            let set = element.element_type.and_then(|t| sets.get(&t).copied());
            let (Some(set), Some(&instance)) = (set, context.elements.get(&element.id)) else {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCMATERIALLAYERSETUSAGE",
                    Some(Source::Text(&global_id(element.id))),
                    1,
                );
                continue;
            };
            let direction = match usage.direction {
                LayerDirection::Axis2 => "AXIS2",
                LayerDirection::Axis3 => "AXIS3",
            };
            let sense = if usage.positive {
                "POSITIVE"
            } else {
                "NEGATIVE"
            };
            let usage = self.add(
                "IFCMATERIALLAYERSETUSAGE",
                &[
                    reference(set),
                    enumeration(direction),
                    enumeration(sense),
                    Parameter::Real(usage.offset),
                    Parameter::Omitted,
                ],
            )?;
            self.relationship(
                "IFCRELASSOCIATESMATERIAL",
                derived_id(element.id, role::MATERIAL, 0),
                references(&[instance]),
                reference(usage),
            )?;
        }
        Ok(())
    }

    /// The product definition shape of a body, or `None` with a loss item when the body is
    /// degenerate. A number that is not finite is an error.
    fn body(&mut self, body: &Body, owner: u128, context: &Context) -> Result<Option<u64>> {
        let (item, kind) = match body {
            Body::Extrusion {
                outer,
                holes,
                direction,
                depth,
            } => {
                let mut numbers = outer
                    .iter()
                    .chain(holes.iter().flatten())
                    .flat_map(|p| [p.x, p.y])
                    .chain(direction.iter().copied())
                    .chain([*depth]);
                if !numbers.all(f64::is_finite) {
                    return Err(invalid("geometry"));
                }
                (
                    self.extrusion(outer, holes, direction, *depth, context.wcs)?,
                    ("SweptSolid", "IFCEXTRUDEDAREASOLID"),
                )
            }
            Body::Brep { vertices, faces } => {
                if !vertices
                    .iter()
                    .flat_map(|p| p.iter())
                    .all(|v| v.is_finite())
                {
                    return Err(invalid("geometry"));
                }
                (self.brep(vertices, faces)?, ("Brep", "IFCFACETEDBREP"))
            }
        };
        let Some(item) = item else {
            self.loss.add(
                reasons::NOT_WRITTEN,
                kind.1,
                Some(Source::Text(&global_id(owner))),
                1,
            );
            return Ok(None);
        };
        let representation = self.add(
            "IFCSHAPEREPRESENTATION",
            &[
                reference(context.body),
                label("Body"),
                label(kind.0),
                references(&[item]),
            ],
        )?;
        self.add(
            "IFCPRODUCTDEFINITIONSHAPE",
            &[
                Parameter::Omitted,
                Parameter::Omitted,
                references(&[representation]),
            ],
        )
        .map(Some)
    }

    fn extrusion(
        &mut self,
        outer: &[Point2],
        holes: &[Vec<Point2>],
        direction: &Vector3,
        depth: f64,
        position: u64,
    ) -> Result<Option<u64>> {
        let open = |points: &[Point2]| -> Vec<Point2> {
            let mut points = points.to_vec();
            if points.len() > 1 && points.first() == points.last() {
                points.pop();
            }
            points
        };
        let outer = open(outer);
        let holes: Vec<Vec<Point2>> = holes.iter().map(|h| open(h)).collect();
        let flat = |points: &[Point2]| self.thin(points);
        let degenerate = flat(&outer)
            || holes.iter().any(|h| flat(h))
            || depth * self.meters <= self.tolerance.linear
            || unit(direction).is_none_or(|d| d.z.abs() <= self.tolerance.angular);
        if degenerate {
            return Ok(None);
        }
        let outer_curve = self.polyline(&outer)?;
        let profile = if holes.is_empty() {
            self.add(
                "IFCARBITRARYCLOSEDPROFILEDEF",
                &[
                    enumeration("AREA"),
                    Parameter::Omitted,
                    reference(outer_curve),
                ],
            )?
        } else {
            let mut inner = Vec::with_capacity(holes.len());
            for hole in &holes {
                inner.push(self.polyline(hole)?);
            }
            self.add(
                "IFCARBITRARYPROFILEDEFWITHVOIDS",
                &[
                    enumeration("AREA"),
                    Parameter::Omitted,
                    reference(outer_curve),
                    references(&inner),
                ],
            )?
        };
        let direction = self.direction(direction)?;
        self.add(
            "IFCEXTRUDEDAREASOLID",
            &[
                reference(profile),
                reference(position),
                reference(direction),
                Parameter::Real(depth),
            ],
        )
        .map(Some)
    }

    /// Whether a closed loop has fewer than three points or no area: its area divided by the
    /// diagonal of its bounding box (its mean width) is not above the linear tolerance.
    fn thin(&self, points: &[Point2]) -> bool {
        if points.len() < 3 {
            return true;
        }
        let mut twice_area = 0.0;
        let (mut low, mut high) = (
            Point2::new(f64::MAX, f64::MAX),
            Point2::new(f64::MIN, f64::MIN),
        );
        for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
            twice_area += a.x * b.y - b.x * a.y;
            low = low.inf(a);
            high = high.sup(a);
        }
        let diagonal = (high - low).norm();
        (twice_area.abs() / 2.0) <= self.tolerance.linear / self.meters * diagonal
    }

    /// A closed `IfcPolyline`: the points and the first point again, as the same instance.
    fn polyline(&mut self, points: &[Point2]) -> Result<u64> {
        let mut names = Vec::with_capacity(points.len() + 1);
        for point in points {
            names.push(self.point2(point)?);
        }
        if let Some(&first) = names.first() {
            names.push(first);
        }
        self.add("IFCPOLYLINE", &[references(&names)])
    }

    fn brep(&mut self, vertices: &[Point3], faces: &[Vec<usize>]) -> Result<Option<u64>> {
        // A closing repeat of the first index is dropped; any other repeat makes the face invalid,
        // because the points of an `IfcPolyLoop` are unique.
        let faces: Vec<&[usize]> = faces
            .iter()
            .map(|f| match f.split_last() {
                Some((last, rest)) if rest.first() == Some(last) => rest,
                _ => f.as_slice(),
            })
            .collect();
        let valid = |face: &[usize]| {
            let mut seen = BTreeSet::new();
            face.len() >= 3 && face.iter().all(|&i| i < vertices.len() && seen.insert(i))
        };
        if faces.is_empty() || !faces.iter().all(|f| valid(f)) {
            return Ok(None);
        }
        let mut points = Vec::with_capacity(vertices.len());
        for vertex in vertices {
            points.push(self.point3(vertex)?);
        }
        let mut face_names = Vec::with_capacity(faces.len());
        for face in &faces {
            let indices: Vec<u64> = face
                .iter()
                .filter_map(|&i| points.get(i).copied())
                .collect();
            let polygon = self.add("IFCPOLYLOOP", &[references(&indices)])?;
            let bound = self.add("IFCFACEOUTERBOUND", &[reference(polygon), enumeration("T")])?;
            face_names.push(self.add("IFCFACE", &[references(&[bound])])?);
        }
        let shell = self.add("IFCCLOSEDSHELL", &[references(&face_names)])?;
        self.add("IFCFACETEDBREP", &[reference(shell)]).map(Some)
    }

    fn property_sets(&mut self, element: &Element, instance: u64) -> Result<()> {
        for (index, set) in (0u32..).zip(&element.property_sets) {
            if set.properties.is_empty() {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCPROPERTYSET",
                    Some(Source::Text(&global_id(element.id))),
                    1,
                );
                continue;
            }
            let mut names = BTreeSet::new();
            let mut properties = Vec::with_capacity(set.properties.len());
            for property in &set.properties {
                if !names.insert(property.name.as_str()) {
                    return Err(invalid("property name"));
                }
                property.value.check()?;
                let written = match property.value.nominal(self.angle_scale) {
                    Some(value) => self.add(
                        "IFCPROPERTYSINGLEVALUE",
                        &[
                            label(&property.name),
                            Parameter::Omitted,
                            value,
                            Parameter::Omitted,
                        ],
                    )?,
                    None => self.enumerated(property)?,
                };
                properties.push(written);
            }
            let set_id = self.global_id(derived_id(element.id, role::PROPERTY_SET, index))?;
            let definition = self.add(
                "IFCPROPERTYSET",
                &[
                    set_id,
                    Parameter::Omitted,
                    label(&set.name),
                    Parameter::Omitted,
                    references(&properties),
                ],
            )?;
            self.defines(element.id, role::PROPERTY_REL, index, instance, definition)?;
        }
        Ok(())
    }

    /// An `IfcPropertyEnumeratedValue` and, once per enumeration name, its
    /// `IfcPropertyEnumeration` (whose name is unique in a file). Two enumerations of one name
    /// with other items are refused.
    fn enumerated(&mut self, property: &Property) -> Result<u64> {
        let PropertyValue::Enumerated {
            enumeration,
            items,
            values,
        } = &property.value
        else {
            return Err(invalid("property value"));
        };
        let labels = |texts: &[String]| {
            Parameter::List(texts.iter().map(|t| typed("IFCLABEL", label(t))).collect())
        };
        let reference_to = match self.enumerations.get(enumeration) {
            Some((known, instance)) if known == items => *instance,
            Some(_) => return Err(invalid("property enumeration")),
            None => {
                let instance = self.add(
                    "IFCPROPERTYENUMERATION",
                    &[label(enumeration), labels(items), Parameter::Omitted],
                )?;
                self.enumerations
                    .insert(enumeration.clone(), (items.clone(), instance));
                instance
            }
        };
        self.add(
            "IFCPROPERTYENUMERATEDVALUE",
            &[
                label(&property.name),
                Parameter::Omitted,
                labels(values),
                reference(reference_to),
            ],
        )
    }

    fn quantity_sets(&mut self, element: &Element, instance: u64) -> Result<()> {
        for (index, set) in (0u32..).zip(&element.quantity_sets) {
            if set.quantities.is_empty() {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCELEMENTQUANTITY",
                    Some(Source::Text(&global_id(element.id))),
                    1,
                );
                continue;
            }
            let mut names = BTreeSet::new();
            let mut quantities = Vec::with_capacity(set.quantities.len());
            for quantity in &set.quantities {
                if !names.insert(quantity.name.as_str()) {
                    return Err(invalid("quantity name"));
                }
                quantity.value.check()?;
                let (type_name, value) = quantity.value.parts();
                quantities.push(self.add(
                    type_name,
                    &[
                        label(&quantity.name),
                        Parameter::Omitted,
                        Parameter::Omitted,
                        Parameter::Real(value),
                        Parameter::Omitted,
                    ],
                )?);
            }
            let set_id = self.global_id(derived_id(element.id, role::QUANTITY_SET, index))?;
            let definition = self.add(
                "IFCELEMENTQUANTITY",
                &[
                    set_id,
                    Parameter::Omitted,
                    label(&set.name),
                    Parameter::Omitted,
                    Parameter::Omitted,
                    references(&quantities),
                ],
            )?;
            self.defines(element.id, role::QUANTITY_REL, index, instance, definition)?;
        }
        Ok(())
    }

    fn defines(
        &mut self,
        owner: u128,
        role: u32,
        index: u32,
        instance: u64,
        definition: u64,
    ) -> Result<()> {
        self.relationship(
            "IFCRELDEFINESBYPROPERTIES",
            derived_id(owner, role, index),
            references(&[instance]),
            reference(definition),
        )
    }

    /// A relationship with the rooted attributes, no name or description, and two members.
    fn relationship(
        &mut self,
        type_name: &str,
        id: u128,
        first: Parameter,
        second: Parameter,
    ) -> Result<()> {
        let id = self.global_id(id)?;
        self.add(
            type_name,
            &[
                id,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                first,
                second,
            ],
        )
        .map(|_| ())
    }

    /// The aggregation of the spatial structure, the containment of the elements in their storeys,
    /// the voids and fills, and the type of each typed element.
    fn relationships(
        &mut self,
        model: &WriteModel,
        context: &Context,
        kinds: &BTreeMap<u128, ElementKind>,
    ) -> Result<()> {
        let aggregates = "IFCRELAGGREGATES";
        self.relationship(
            aggregates,
            derived_id(model.project.id, role::AGGREGATES, 0),
            reference(context.project),
            references(&[context.site]),
        )?;
        self.relationship(
            aggregates,
            derived_id(model.site.id, role::AGGREGATES, 0),
            reference(context.site),
            references(&[context.building]),
        )?;
        let storeys: Vec<u64> = model
            .storeys
            .iter()
            .filter_map(|s| context.storeys.get(&s.id).map(|&(instance, _)| instance))
            .collect();
        if !storeys.is_empty() {
            self.relationship(
                aggregates,
                derived_id(model.building.id, role::AGGREGATES, 0),
                reference(context.building),
                references(&storeys),
            )?;
        }
        self.parts(model, context)?;
        for storey in &model.storeys {
            let contained: Vec<u64> = model
                .elements
                .iter()
                .filter(|e| e.storey == storey.id)
                .filter(|e| !matches!(e.kind, ElementKind::Opening | ElementKind::Space))
                .filter(|e| !(e.host.is_some() && is_part(e.kind)))
                .filter_map(|e| context.elements.get(&e.id).copied())
                .chain(
                    model
                        .grids
                        .iter()
                        .filter(|g| g.storey == storey.id)
                        .filter_map(|g| context.grids.get(&g.id).copied()),
                )
                .collect();
            let spaces: Vec<u64> = model
                .elements
                .iter()
                .filter(|e| e.storey == storey.id && e.kind == ElementKind::Space)
                .filter_map(|e| context.elements.get(&e.id).copied())
                .collect();
            let Some(&(instance, _)) = context.storeys.get(&storey.id) else {
                continue;
            };
            if !spaces.is_empty() {
                self.relationship(
                    aggregates,
                    derived_id(storey.id, role::AGGREGATES, 0),
                    reference(instance),
                    references(&spaces),
                )?;
            }
            if !contained.is_empty() {
                self.relationship(
                    "IFCRELCONTAINEDINSPATIALSTRUCTURE",
                    derived_id(storey.id, role::CONTAINS, 0),
                    references(&contained),
                    reference(instance),
                )?;
            }
        }
        for element in &model.elements {
            let Some(host) = element.host else { continue };
            let (Some(&own), Some(&other)) = (
                context.elements.get(&element.id),
                context.elements.get(&host),
            ) else {
                return Err(bad_reference(host));
            };
            match kinds.get(&element.id) {
                Some(kind) if is_part(*kind) => {}
                Some(ElementKind::Opening) => self.relationship(
                    "IFCRELVOIDSELEMENT",
                    derived_id(element.id, role::VOIDS, 0),
                    reference(other),
                    reference(own),
                )?,
                _ => self.relationship(
                    "IFCRELFILLSELEMENT",
                    derived_id(element.id, role::FILLS, 0),
                    reference(other),
                    reference(own),
                )?,
            }
        }
        self.type_relationships(model, context)
    }

    /// One `IfcRelAggregates` per stair, ramp or curtain wall with parts: the elements that name
    /// it as their host, in model order.
    fn parts(&mut self, model: &WriteModel, context: &Context) -> Result<()> {
        // The parts of each host, in model order: aggregated by the container, not contained in a
        // storey.
        let mut by_host: BTreeMap<u128, Vec<u64>> = BTreeMap::new();
        for e in model.elements.iter().filter(|e| is_part(e.kind)) {
            if let (Some(host), Some(&part)) = (e.host, context.elements.get(&e.id)) {
                by_host.entry(host).or_default().push(part);
            }
        }
        for element in model.elements.iter().filter(|e| is_container(e.kind)) {
            let parts = by_host.get(&element.id).map_or(&[][..], Vec::as_slice);
            if let (false, Some(&stair)) = (parts.is_empty(), context.elements.get(&element.id)) {
                self.relationship(
                    "IFCRELAGGREGATES",
                    derived_id(element.id, role::AGGREGATES, 0),
                    reference(stair),
                    references(parts),
                )?;
            }
        }
        Ok(())
    }

    /// One `IfcRelDefinesByType` per element type with occurrences.
    fn type_relationships(&mut self, model: &WriteModel, context: &Context) -> Result<()> {
        let mut occurrences: BTreeMap<u128, Vec<u64>> = BTreeMap::new();
        for element in &model.elements {
            if let (Some(element_type), Some(&instance)) =
                (element.element_type, context.elements.get(&element.id))
            {
                occurrences.entry(element_type).or_default().push(instance);
            }
        }
        for element_type in &model.types {
            // `RelatedObjects` holds at least one object: a type without occurrences stands alone.
            if let (Some(related), Some(&instance)) = (
                occurrences.get(&element_type.id),
                context.types.get(&element_type.id),
            ) {
                self.relationship(
                    "IFCRELDEFINESBYTYPE",
                    derived_id(element_type.id, role::TYPED, 0),
                    references(related),
                    reference(instance),
                )?;
            }
        }
        Ok(())
    }
}
