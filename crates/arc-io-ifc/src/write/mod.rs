//! IFC writing (CORE-IO-016, first part): an IFC4, IFC4X3 or IFC2X3 physical file (or IFC-ZIP) from
//! a neutral description of a building.
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
mod layout;
mod mvd;
mod party;
pub(crate) mod step;
mod target;
mod values;

use std::collections::{BTreeMap, BTreeSet};

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Cancel, Error, Limits, Result, Tolerance};
use arc_io_common::zip::{Method, write_archive};

use crate::error::{codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::Parameter;

pub use global_id::{derived_id, global_id, role};
pub use mvd::{ModelView, VIEW_RULES, ViewCheck, ViewFinding, ViewRule};
pub use party::{Actor, Address, Party};
pub use target::OutputSchema;
use target::Target;
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

/// A spatial element: the project, the site or the building, with the attributes they share and
/// those of one of them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Spatial {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The name.
    pub name: String,
    /// The `LongName`; `None` omits it.
    pub long_name: Option<String>,
    /// The `Description`; `None` omits it.
    pub description: Option<String>,
    /// The `Phase` (project only); on a site or building it is reported, not written.
    pub phase: Option<String>,
    /// The `SiteAddress` (site) or `BuildingAddress` (building); on the project it is reported,
    /// not written.
    pub address: Option<Address>,
    /// The actors assigned to the element with their roles (`IfcRelAssignsToActor`).
    pub actors: Vec<Actor>,
    /// The property sets of the element.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets of the element.
    pub quantity_sets: Vec<QuantitySet>,
}

impl Spatial {
    /// A project, site or building with only a name.
    #[must_use]
    pub fn new(id: u128, name: &str) -> Self {
        Self {
            id,
            name: name.to_owned(),
            long_name: None,
            description: None,
            phase: None,
            address: None,
            actors: Vec::new(),
            property_sets: Vec::new(),
            quantity_sets: Vec::new(),
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
    /// The `LongName`; `None` omits it.
    pub long_name: Option<String>,
    /// The `Description`; `None` omits it.
    pub description: Option<String>,
    /// The actors assigned to the storey with their roles.
    pub actors: Vec<Actor>,
    /// The property sets of the storey.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets of the storey.
    pub quantity_sets: Vec<QuantitySet>,
}

impl Storey {
    /// A storey at the given elevation.
    #[must_use]
    pub fn new(id: u128, name: &str, elevation: f64) -> Self {
        Self {
            id,
            name: name.to_owned(),
            elevation,
            long_name: None,
            description: None,
            actors: Vec::new(),
            property_sets: Vec::new(),
            quantity_sets: Vec::new(),
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
    /// `IfcFurniture` (`PredefinedType` from `IfcFurnitureTypeEnum`: `CHAIR`, `TABLE`, `DESK`,
    /// `BED`, `FILECABINET`, `SHELF`, `SOFA`); its type is an `IfcFurnitureType`.
    Furniture,
    /// `IfcSanitaryTerminal` (`PredefinedType` from `IfcSanitaryTerminalTypeEnum`, for example
    /// `SINK`, `TOILETPAN`, `SHOWER`); its type is an `IfcSanitaryTerminalType`.
    SanitaryTerminal,
    /// `IfcLightFixture` (`PredefinedType` from `IfcLightFixtureTypeEnum`: `POINTSOURCE`,
    /// `DIRECTIONSOURCE`, `SECURITYLIGHTING`); its type is an `IfcLightFixtureType`.
    LightFixture,
    /// `IfcElectricAppliance` (`PredefinedType` from `IfcElectricApplianceTypeEnum`, for example
    /// `MICROWAVE`, `FRIDGE_FREEZER`); its type is an `IfcElectricApplianceType`.
    ElectricAppliance,
    /// `IfcUnitaryEquipment` (`PredefinedType` from `IfcUnitaryEquipmentTypeEnum`: `AIRHANDLER`,
    /// `AIRCONDITIONINGUNIT`, `DEHUMIDIFIER`, `SPLITSYSTEM`, `ROOFTOPUNIT`); its type is an
    /// `IfcUnitaryEquipmentType`.
    UnitaryEquipment,
    /// `IfcColumn` (`PredefinedType` from `IfcColumnTypeEnum`: `COLUMN`, `PILASTER`); its type is
    /// an `IfcColumnType`.
    Column,
    /// `IfcBeam` (`PredefinedType` from `IfcBeamTypeEnum`, for example `BEAM`, `JOIST`, `LINTEL`,
    /// `T_BEAM`); its type is an `IfcBeamType`.
    Beam,
    /// `IfcBuildingElementProxy` (`PredefinedType` from `IfcBuildingElementProxyTypeEnum`:
    /// `COMPLEX`, `ELEMENT`, `PARTIAL`, `PROVISIONFORVOID`, `PROVISIONFORSPACE`). The writer never
    /// substitutes it for a kind the output schema or the model does not carry: a caller asks for
    /// it by name. Its type is an `IfcBuildingElementProxyType`.
    BuildingElementProxy,
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
            Self::Furniture => "IFCFURNITURE",
            Self::SanitaryTerminal => "IFCSANITARYTERMINAL",
            Self::LightFixture => "IFCLIGHTFIXTURE",
            Self::ElectricAppliance => "IFCELECTRICAPPLIANCE",
            Self::UnitaryEquipment => "IFCUNITARYEQUIPMENT",
            Self::Column => "IFCCOLUMN",
            Self::Beam => "IFCBEAM",
            Self::BuildingElementProxy => "IFCBUILDINGELEMENTPROXY",
        }
    }

    /// The values of `PredefinedType` besides `USERDEFINED` and `NOTDEFINED`.
    #[allow(
        clippy::too_many_lines,
        reason = "one list of the IFC4 enumeration per kind"
    )]
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
            Self::Furniture => &[
                "CHAIR",
                "TABLE",
                "DESK",
                "BED",
                "FILECABINET",
                "SHELF",
                "SOFA",
            ],
            Self::SanitaryTerminal => &[
                "BATH",
                "BIDET",
                "CISTERN",
                "SHOWER",
                "SINK",
                "SANITARYFOUNTAIN",
                "TOILETPAN",
                "URINAL",
                "WASHHANDBASIN",
                "WCSEAT",
            ],
            Self::LightFixture => &["POINTSOURCE", "DIRECTIONSOURCE", "SECURITYLIGHTING"],
            Self::ElectricAppliance => &[
                "DISHWASHER",
                "ELECTRICCOOKER",
                "FREESTANDINGELECTRICHEATER",
                "FREESTANDINGFAN",
                "FREESTANDINGWATERHEATER",
                "FREESTANDINGWATERCOOLER",
                "FREEZER",
                "FRIDGE_FREEZER",
                "HANDDRYER",
                "KITCHENMACHINE",
                "MICROWAVE",
                "PHOTOCOPIER",
                "REFRIGERATOR",
                "TUMBLEDRYER",
                "VENDINGMACHINE",
                "WASHINGMACHINE",
            ],
            Self::UnitaryEquipment => &[
                "AIRHANDLER",
                "AIRCONDITIONINGUNIT",
                "DEHUMIDIFIER",
                "SPLITSYSTEM",
                "ROOFTOPUNIT",
            ],
            Self::Column => &["COLUMN", "PILASTER"],
            Self::Beam => &[
                "BEAM",
                "JOIST",
                "HOLLOWCORE",
                "LINTEL",
                "SPANDREL",
                "T_BEAM",
            ],
            Self::BuildingElementProxy => &[
                "COMPLEX",
                "ELEMENT",
                "PARTIAL",
                "PROVISIONFORVOID",
                "PROVISIONFORSPACE",
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

/// An element type (`IfcWallType`, `IfcDoorType`, `IfcFurnitureType` and the other types of
/// [`ElementKind`]), assigned to the elements whose [`Element::element_type`] names it
/// (`IfcRelDefinesByType`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ElementType {
    /// The identifier its `GlobalId` is made from.
    pub id: u128,
    /// The kind of its occurrences, which selects the type's entity; an opening, a space and the
    /// stair and ramp kinds have no type.
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
    /// The property sets of the type, written in its `HasPropertySets`; an empty set is reported.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets of the type, written in its `HasPropertySets` after the property sets.
    pub quantity_sets: Vec<QuantitySet>,
    /// The shared geometry: written as an `IfcRepresentationMap` in the type's `RepresentationMaps`
    /// (the body is in the coordinates of the map's origin, the identity). An occurrence uses it
    /// through [`Element::mapping`]; `None` writes no map.
    pub representation: Option<Body>,
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
            property_sets: Vec::new(),
            quantity_sets: Vec::new(),
            representation: None,
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
            ElementKind::Furniture => Some("IFCFURNITURETYPE"),
            ElementKind::SanitaryTerminal => Some("IFCSANITARYTERMINALTYPE"),
            ElementKind::LightFixture => Some("IFCLIGHTFIXTURETYPE"),
            ElementKind::ElectricAppliance => Some("IFCELECTRICAPPLIANCETYPE"),
            ElementKind::UnitaryEquipment => Some("IFCUNITARYEQUIPMENTTYPE"),
            ElementKind::Column => Some("IFCCOLUMNTYPE"),
            ElementKind::Beam => Some("IFCBEAMTYPE"),
            ElementKind::BuildingElementProxy => Some("IFCBUILDINGELEMENTPROXYTYPE"),
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
    /// window; required. Doors and windows: the opening they fill (`IfcRelFillsElement`).
    /// Flights, landings (slabs) and railings: the stair they are a part of; plates and members:
    /// the curtain wall. Any element of the kinds furniture, sanitary terminal, light fixture,
    /// electric appliance, unitary equipment, column, beam, building element proxy, transport
    /// element and pipe segment may name as its host any element except an opening or a space;
    /// the host aggregates it (`IfcRelAggregates`). A part is not contained in the storey. Must be
    /// `None` for walls, spaces, roofs, coverings, stairs, ramps and curtain walls, and for a slab
    /// that is not part of a stair or ramp. A chain of hosts must not return to its start.
    pub host: Option<u128>,
    /// Stair flights: the `NumberOfRisers`, `NumberOfTreads`, `RiserHeight` and `TreadLength`;
    /// `None` omits them. Ignored for the other kinds.
    pub flight: Option<FlightDimensions>,
    /// The property sets.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets.
    pub quantity_sets: Vec<QuantitySet>,
    /// The identifier of the [`ElementType`] of the model the element is an occurrence of; it
    /// must be a type of the same kind. `None` assigns no type.
    pub element_type: Option<u128>,
    /// Walls and slabs: how the layers of the element's type lie in it. Written when the type
    /// has layers; otherwise it is reported, not written.
    pub layer_usage: Option<LayerSetUsage>,
    /// The occurrence of the type's shared geometry: the element uses the `IfcRepresentationMap` of
    /// its [`Element::element_type`] (which must have an [`ElementType::representation`]) through
    /// an `IfcMappedItem`, placed by this transform (origin and axes) in the element's placement.
    /// Must be `None` when [`Element::body`] is set; `None` gives the element no representation of
    /// its own.
    pub mapping: Option<Placement>,
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
            mapping: None,
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
    /// The model view definition named in `FILE_DESCRIPTION` when `view_check` is
    /// [`ViewCheck::Off`].
    pub view_definition: String,
    /// The schema of the file: IFC4 by default (the bytes the writer always wrote), IFC4X3 ADD2 or
    /// IFC2X3. Content the schema cannot carry is reported in the loss report.
    pub schema: OutputSchema,
    /// What happens when the written file does not conform to the schema tables (levels 1 to 3);
    /// [`SelfCheck::Fail`] by default. Without tables for the schema (IFC4 and IFC2X3 in a build
    /// without the generated tables) nothing is checked.
    pub self_check: SelfCheck,
    /// The model view the file is checked against and named after; [`ViewCheck::Off`] by default.
    pub view_check: ViewCheck,
    /// The container of the output bytes.
    pub container: Container,
}

/// What the writer does with the result of checking its own output against the schema tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelfCheck {
    /// A finding fails the write with `io.ifc.write_not_conformant`; nothing is returned.
    #[default]
    Fail,
    /// The findings are returned in [`WriteOutput::findings`].
    Report,
    /// No check.
    Off,
}

/// The container of the bytes [`write()`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Container {
    /// The physical file (ISO 10303-21 clear text).
    #[default]
    Step,
    /// An IFC-ZIP archive with one deflated `.ifc` entry named after `WriteOptions::file_name`
    /// (`model.ifc` when that is not a `.ifc` name).
    Zip,
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
            schema: OutputSchema::Ifc4,
            self_check: SelfCheck::Fail,
            view_check: ViewCheck::Off,
            container: Container::Step,
        }
    }
}

/// The result of a write.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct WriteOutput {
    /// The bytes of the physical file, or of the IFC-ZIP archive holding it.
    pub bytes: Vec<u8>,
    /// The loss report of the write.
    pub report: LossReport,
    /// The conformance findings of the written file under [`SelfCheck::Report`]; empty otherwise.
    pub findings: Vec<crate::Finding>,
    /// The findings of the model view check under [`ViewCheck::Report`]; empty otherwise.
    pub view_findings: Vec<ViewFinding>,
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

/// Writes the model as a physical file (or an IFC-ZIP archive) in `options.schema`, IFC4 by
/// default.
///
/// Every record is built through the writer's layout and, for IFC4X3 and IFC2X3, rebuilt through
/// the tables of the output schema; what the schema cannot carry is reported in the loss report.
/// With tables for the schema the written file is read back and checked at conformance levels 1 to 3
/// (`options.self_check`), and against a model view (`options.view_check`).
///
/// # Errors
///
/// `io.ifc.write_value_invalid` for a number that is not finite, a negative quantity, a door or
/// window size that is not positive, a placement whose axes or a true north that is zero, shorter
/// than about 1.5E-8 or parallel, a NUL in a text, two properties or quantities of one set with
/// the same name, two sets of one element with the same name, an address without any location
/// field or an actor without a name;
/// `io.ifc.write_reference_invalid` for a storey, host or element type that is not in the model or
/// of the wrong kind, an element type of an opening or space, an opening without a host, or a chain
/// of hosts that returns to its start; `io.ifc.write_value_invalid` also for a layer thickness that
/// is negative or not finite and a layer set usage offset that is not finite;
/// `io.ifc.write_id_duplicate` when two objects give the same `GlobalId`;
/// `io.ifc.write_schema_unavailable` for a schema whose tables are not part of the build;
/// `io.ifc.write_not_conformant` when the self-check finds the output non-conformant (a defect);
/// `io.ifc.write_view_violated` under [`ViewCheck::Enforce`]; `io.limit_exceeded` for
/// `record_count` (instances), `string_bytes`, `array_length`, `nesting_depth` and `memory_bytes`
/// (the size of the output); `base.cancelled` when the token was cancelled.
pub fn write(model: &WriteModel, options: &WriteOptions) -> Result<WriteOutput> {
    options.cancel.check()?;
    let target = Target::new(options.schema)?;
    let kinds = check_references(model)?;
    let mut writer = Writer {
        data: String::new(),
        header_end: 0,
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
        target,
        owner_history: None,
        skipped: BTreeSet::new(),
        decomposed: model
            .elements
            .iter()
            .filter(|e| is_part(e.kind))
            .filter_map(|e| e.host)
            .collect(),
    };
    writer.header(options, &options.view_definition)?;
    if options.schema == OutputSchema::Ifc2x3 {
        writer.owner_history(options)?;
    }
    let mut context = writer.context(model)?;
    // Representation maps come before the elements that are mapped to them.
    for element_type in &model.types {
        options.cancel.check()?;
        writer.representation_map(element_type, &mut context)?;
    }
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
    let (findings, view_findings) = writer.verify(options)?;
    let report = writer.loss.finish(LossHeader::new(
        "ifc",
        options.schema.identifier(),
        Direction::Export,
        Some(options.limits),
    ));
    let mut bytes = writer.data.into_bytes();
    if options.container == Container::Zip {
        let entry = options
            .file_name
            .rsplit(['/', '\\'])
            .next()
            .filter(|n| n.to_ascii_lowercase().ends_with(".ifc") && n.len() > 4)
            .unwrap_or("model.ifc");
        bytes = ifc_zip(entry, &bytes)?;
    }
    Ok(WriteOutput {
        bytes,
        report,
        findings,
        view_findings,
    })
}

/// The seconds since 1970-01-01T00:00:00 of an ISO 8601 `YYYY-MM-DDTHH:MM:SS` text (the time zone
/// is ignored); 0 when the text has another form.
fn unix_seconds(text: &str) -> i64 {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let parse = || {
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        // Days from civil (proleptic Gregorian).
        let y = if month <= 2 { year - 1 } else { year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (month + 9) % 12;
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
    };
    parse().filter(|s| *s >= 0).unwrap_or(0)
}

/// Whether an element of this kind with a host is a part of it: a flight, landing or railing of a
/// stair or ramp, a plate or member of a curtain wall, or an element of a generic kind (see
/// [`is_generic_part`]). A part is aggregated by its host and not contained in the storey.
fn is_part(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Slab
            | ElementKind::StairFlight
            | ElementKind::RampFlight
            | ElementKind::Railing
            | ElementKind::Plate
            | ElementKind::Member
    ) || is_generic_part(kind)
}

/// The kinds that any element may name as its host, whatever the kind of the host (except an
/// opening or a space): the kinds that have no decomposition rule of their own.
fn is_generic_part(kind: ElementKind) -> bool {
    matches!(
        kind,
        ElementKind::Furniture
            | ElementKind::SanitaryTerminal
            | ElementKind::LightFixture
            | ElementKind::ElectricAppliance
            | ElementKind::UnitaryEquipment
            | ElementKind::Column
            | ElementKind::Beam
            | ElementKind::BuildingElementProxy
            | ElementKind::TransportElement
            | ElementKind::PipeSegment
    )
}

/// Whether a part of kind `part` may be hosted by an element of kind `host` (`None` when the
/// host is not an element of the model).
fn part_fits(part: ElementKind, host: Option<ElementKind>) -> bool {
    match part {
        ElementKind::Slab | ElementKind::Railing => {
            matches!(host, Some(ElementKind::Stair | ElementKind::Ramp))
        }
        ElementKind::StairFlight => host == Some(ElementKind::Stair),
        ElementKind::RampFlight => host == Some(ElementKind::Ramp),
        ElementKind::Plate | ElementKind::Member => host == Some(ElementKind::CurtainWall),
        kind if is_generic_part(kind) => {
            host.is_some_and(|h| !matches!(h, ElementKind::Opening | ElementKind::Space))
        }
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
    let mut represented = BTreeSet::new();
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
        if element_type.representation.is_some() {
            represented.insert(element_type.id);
        }
        if !unique_set_names(&element_type.property_sets, &element_type.quantity_sets) {
            return Err(invalid("property set name"));
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
        if !unique_set_names(&element.property_sets, &element.quantity_sets) {
            return Err(invalid("property set name"));
        }
        if element.mapping.is_some() {
            if element.body.is_some() {
                return Err(invalid("mapping with a body"));
            }
            let Some(id) = element.element_type else {
                return Err(invalid("mapping without an element type"));
            };
            if !represented.contains(&id) {
                return Err(bad_reference(id));
            }
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

/// Whether the property and quantity sets of one owner have distinct names.
fn unique_set_names(properties: &[PropertySet], quantities: &[QuantitySet]) -> bool {
    let mut names = BTreeSet::new();
    properties
        .iter()
        .map(|p| p.name.as_str())
        .chain(quantities.iter().map(|q| q.name.as_str()))
        .all(|name| names.insert(name))
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
    /// The `IfcRepresentationMap` of each type with a representation, by the type's identifier.
    maps: BTreeMap<u128, u64>,
}

struct Writer<'a> {
    data: String,
    /// The length of the header at the start of `data`.
    header_end: usize,
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
    target: Target,
    /// The `IfcOwnerHistory` of IFC2X3, whose rooted entities require one.
    owner_history: Option<u64>,
    /// Elements and types the output schema cannot carry; nothing refers to them.
    skipped: BTreeSet<u128>,
    /// Elements with parts (a stair, ramp or curtain wall that others name as host).
    decomposed: BTreeSet<u128>,
}

impl Writer<'_> {
    fn add(&mut self, type_name: &str, parameters: &[Parameter]) -> Result<u64> {
        let name = self.next;
        if name > self.limits.record_count {
            return Err(limit("record_count", name));
        }
        if self.target.is_native() {
            step::record(&mut self.data, name, type_name, parameters, self.limits)?;
        } else {
            let target = self.target;
            let (entity, translated) =
                target.translate(type_name, parameters, self.owner_history, &mut self.loss)?;
            step::record(&mut self.data, name, &entity, &translated, self.limits)?;
        }
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

    fn header(&mut self, options: &WriteOptions, view: &str) -> Result<()> {
        let mut text = String::new();
        text.push_str("ISO-10303-21;\nHEADER;\n");
        let view = format!("ViewDefinition [{view}]");
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
            (
                "FILE_SCHEMA",
                vec![Parameter::List(vec![label(options.schema.identifier())])],
            ),
        ];
        for (type_name, parameters) in &records {
            step::header_record(&mut text, type_name, parameters, self.limits)?;
        }
        text.push_str("ENDSEC;\nDATA;\n");
        self.data.replace_range(..self.header_end, &text);
        self.header_end = text.len();
        self.check_size()
    }

    /// Reads the written file back and checks it: conformance levels 1 to 3 against the tables of
    /// the schema and the rule set of the model view. The header names the view only when the
    /// file passes.
    fn verify(
        &mut self,
        options: &WriteOptions,
    ) -> Result<(Vec<crate::Finding>, Vec<ViewFinding>)> {
        let schema = options.schema;
        let tables = crate::schema::SchemaTables::of(&schema.schema()).is_some();
        let conform = tables && options.self_check != SelfCheck::Off;
        let view = match options.view_check {
            ViewCheck::Off => None,
            ViewCheck::Report(v) | ViewCheck::Enforce(v) => Some(v),
        };
        if !conform && view.is_none() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut read_options = crate::ReadOptions {
            limits: options.limits,
            cancel: options.cancel.clone(),
            ..crate::ReadOptions::default()
        };
        let skipped = |e: &Error| e.code() == codes::LIMIT_EXCEEDED.code;
        let file = match crate::read(self.data.as_bytes(), &mut read_options) {
            Ok(output) => output.file,
            Err(e) if skipped(&e) => {
                self.loss
                    .add(reasons::SELF_CHECK_SKIPPED, "conformance", None, 1);
                return Ok((Vec::new(), Vec::new()));
            }
            Err(e) => return Err(e),
        };
        let mut findings = Vec::new();
        if conform {
            match file.conformance(&options.limits, &options.cancel) {
                Ok(result) => {
                    let failed = result.failed() > 0;
                    findings = result.findings;
                    if findings.is_empty() && failed {
                        // Counts beyond the findings kept: still a failure.
                        findings.push(crate::Finding {
                            instance: 0,
                            entity: String::new(),
                            attribute: None,
                            rule: crate::rules::VALUE_TYPE,
                        });
                    }
                }
                Err(e) if skipped(&e) => {
                    self.loss
                        .add(reasons::SELF_CHECK_SKIPPED, "conformance", None, 1);
                }
                Err(e) => return Err(e),
            }
            if options.self_check == SelfCheck::Fail
                && let Some(first) = findings.first()
            {
                return Err(Error::new(codes::IFC_WRITE_NOT_CONFORMANT)
                    .with_arg("count", i64::try_from(findings.len()).unwrap_or(i64::MAX))
                    .with_arg("rule", first.rule.code)
                    .with_arg(
                        "instance",
                        i64::try_from(first.instance).unwrap_or(i64::MAX),
                    )
                    .with_arg("entity", first.entity.clone()));
            }
            if options.self_check != SelfCheck::Report {
                findings.clear();
            }
        }
        let mut view_findings = Vec::new();
        if let Some(view) = view {
            view_findings = mvd::check(&file, view, schema);
            if matches!(options.view_check, ViewCheck::Enforce(_)) && !view_findings.is_empty() {
                return Err(Error::new(codes::IFC_WRITE_VIEW_VIOLATED).with_arg(
                    "count",
                    i64::try_from(view_findings.len()).unwrap_or(i64::MAX),
                ));
            }
            let label = if view_findings.is_empty() {
                view.label(schema).unwrap_or("")
            } else {
                ""
            };
            self.header(options, label)?;
        }
        Ok((findings, view_findings))
    }

    /// The `IfcOwnerHistory` IFC2X3 requires on every rooted entity: the author and organization
    /// of the options (`unknown` when empty), this writer as the application, the time stamp of the
    /// options as the creation date.
    fn owner_history(&mut self, options: &WriteOptions) -> Result<()> {
        let or_unknown = |t: &str| {
            if t.is_empty() {
                "unknown".to_owned()
            } else {
                t.to_owned()
            }
        };
        let organization = self.add(
            "IFCORGANIZATION",
            &[
                Parameter::Omitted,
                label(&or_unknown(&options.organization)),
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
            ],
        )?;
        let person = self.add(
            "IFCPERSON",
            &[
                Parameter::Omitted,
                label(&or_unknown(&options.author)),
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
            ],
        )?;
        let user = self.add(
            "IFCPERSONANDORGANIZATION",
            &[
                reference(person),
                reference(organization),
                Parameter::Omitted,
            ],
        )?;
        let application = self.add(
            "IFCAPPLICATION",
            &[
                reference(organization),
                label(env!("CARGO_PKG_VERSION")),
                label("arc-io-ifc"),
                label("arc-io-ifc"),
            ],
        )?;
        let created = unix_seconds(&options.timestamp);
        let history = self.add(
            "IFCOWNERHISTORY",
            &[
                reference(user),
                reference(application),
                Parameter::Omitted,
                enumeration("ADDED"),
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Omitted,
                Parameter::Integer(created),
            ],
        )?;
        self.owner_history = Some(history);
        Ok(())
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
    #[allow(
        clippy::too_many_lines,
        reason = "one block per record of the structure"
    )]
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
            if self.target.supports("IFCMAPCONVERSION") && self.target.supports("IFCPROJECTEDCRS") {
                self.georeference(georeference, model_context)?;
            } else {
                Target::unrepresentable(&mut self.loss, "IFCMAPCONVERSION", None);
            }
        }
        let body = self.subcontext("Body", model_context)?;
        let footprint = if model.grids.is_empty() {
            None
        } else {
            Some(self.subcontext("FootPrint", model_context)?)
        };
        let optional = |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
        let project_spatial = &model.project;
        if project_spatial.address.is_some() {
            self.loss.attribute(
                reasons::ATTRIBUTE_NOT_REPRESENTABLE,
                "IFCPROJECT.Address",
                1,
            );
        }
        let project_id = self.global_id(project_spatial.id)?;
        let project = self.add(
            "IFCPROJECT",
            &[
                project_id,
                Parameter::Omitted,
                label(&project_spatial.name),
                optional(&project_spatial.description),
                Parameter::Omitted,
                optional(&project_spatial.long_name),
                optional(&project_spatial.phase),
                references(&[model_context]),
                reference(units),
            ],
        )?;
        self.spatial_extras(project_spatial, project)?;
        let site_placement = self.local_placement(None, wcs)?;
        let site_spatial = &model.site;
        if site_spatial.phase.is_some() {
            self.loss
                .attribute(reasons::ATTRIBUTE_NOT_REPRESENTABLE, "IFCSITE.Phase", 1);
        }
        let site_address = match &site_spatial.address {
            Some(address) => Some(self.address(address)?),
            None => None,
        };
        let site_id = self.global_id(site_spatial.id)?;
        let mut site_parameters = self.spatial_head(site_id, site_spatial, site_placement);
        site_parameters.extend(std::iter::repeat_n(Parameter::Omitted, 4));
        site_parameters.push(site_address.map_or(Parameter::Omitted, reference));
        let site = self.add("IFCSITE", &site_parameters)?;
        self.spatial_extras(site_spatial, site)?;
        let building_placement = self.local_placement(Some(site_placement), wcs)?;
        let building_spatial = &model.building;
        if building_spatial.phase.is_some() {
            self.loss
                .attribute(reasons::ATTRIBUTE_NOT_REPRESENTABLE, "IFCBUILDING.Phase", 1);
        }
        let building_address = match &building_spatial.address {
            Some(address) => Some(self.address(address)?),
            None => None,
        };
        let building_id = self.global_id(building_spatial.id)?;
        let mut building_parameters =
            self.spatial_head(building_id, building_spatial, building_placement);
        building_parameters.extend(std::iter::repeat_n(Parameter::Omitted, 2));
        building_parameters.push(building_address.map_or(Parameter::Omitted, reference));
        let building = self.add("IFCBUILDING", &building_parameters)?;
        self.spatial_extras(building_spatial, building)?;
        let mut storeys = BTreeMap::new();
        for storey in &model.storeys {
            let axes = self.axes(&Placement {
                origin: Point3::new(0.0, 0.0, storey.elevation),
                ..Placement::default()
            })?;
            let placement = self.local_placement(Some(building_placement), axes)?;
            let storey_id = self.global_id(storey.id)?;
            let head = Spatial {
                long_name: storey.long_name.clone(),
                description: storey.description.clone(),
                ..Spatial::new(storey.id, &storey.name)
            };
            let mut parameters = self.spatial_head(storey_id, &head, placement);
            parameters.push(Parameter::Real(storey.elevation));
            let instance = self.add("IFCBUILDINGSTOREY", &parameters)?;
            self.actors(storey.id, &storey.actors, instance)?;
            self.property_sets(storey.id, &storey.property_sets, Some(instance))?;
            self.quantity_sets(storey.id, &storey.quantity_sets, Some(instance))?;
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
            maps: BTreeMap::new(),
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
    /// placement, no representation, the long name, and `CompositionType` `ELEMENT`.
    fn spatial_head(&self, id: Parameter, spatial: &Spatial, placement: u64) -> Vec<Parameter> {
        let optional = |t: &Option<String>| t.as_deref().map_or(Parameter::Omitted, label);
        vec![
            id,
            Parameter::Omitted,
            label(&spatial.name),
            optional(&spatial.description),
            Parameter::Omitted,
            reference(placement),
            Parameter::Omitted,
            optional(&spatial.long_name),
            enumeration("ELEMENT"),
        ]
    }

    /// The actors, property sets and quantity sets of the project, site or building.
    fn spatial_extras(&mut self, spatial: &Spatial, instance: u64) -> Result<()> {
        self.actors(spatial.id, &spatial.actors, instance)?;
        self.property_sets(spatial.id, &spatial.property_sets, Some(instance))?;
        self.quantity_sets(spatial.id, &spatial.quantity_sets, Some(instance))
            .map(drop)
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

    #[allow(
        clippy::too_many_lines,
        reason = "one block per record of the structure"
    )]
    fn element(&mut self, element: &Element, context: &mut Context) -> Result<()> {
        if !self.target.supports(element.kind.entity()) {
            Target::unrepresentable(
                &mut self.loss,
                element.kind.entity(),
                Some(&global_id(element.id)),
            );
            self.skipped.insert(element.id);
            return Ok(());
        }
        let id = self.global_id(element.id)?;
        let storey_placement = context
            .storeys
            .get(&element.storey)
            .map(|&(_, placement)| placement)
            .ok_or_else(|| bad_reference(element.storey))?;
        let axes = self.axes(&element.placement)?;
        let placement = self.local_placement(Some(storey_placement), axes)?;
        // IFC2X3 refuses a representation on an element that decomposes into parts.
        let decomposes =
            self.target.schema == OutputSchema::Ifc2x3 && self.decomposed.contains(&element.id);
        if decomposes && (element.body.is_some() || element.mapping.is_some()) {
            self.loss.attribute(
                reasons::ATTRIBUTE_NOT_REPRESENTABLE,
                &format!("{}.Representation", element.kind.entity()),
                1,
            );
        }
        let representation = match element.body.as_ref() {
            Some(body) if !decomposes => self.body(body, element.id, context)?,
            None if !decomposes => self.mapped(element, context)?,
            _ => None,
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
            self.property_sets(element.id, &element.property_sets, Some(instance))?;
            return self
                .quantity_sets(element.id, &element.quantity_sets, Some(instance))
                .map(drop);
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
        self.property_sets(element.id, &element.property_sets, Some(instance))?;
        self.quantity_sets(element.id, &element.quantity_sets, Some(instance))
            .map(drop)
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
            if self.skipped.contains(&member) {
                continue;
            }
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
        if !self.target.supports(entity) {
            Target::unrepresentable(&mut self.loss, entity, Some(&global_id(element_type.id)));
            self.skipped.insert(element_type.id);
            return Ok(());
        }
        let id = self.global_id(element_type.id)?;
        // The sets are written before the type, which lists them in `HasPropertySets`.
        let mut definitions =
            self.property_sets(element_type.id, &element_type.property_sets, None)?;
        definitions.extend(self.quantity_sets(
            element_type.id,
            &element_type.quantity_sets,
            None,
        )?);
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
            if definitions.is_empty() {
                Parameter::Omitted
            } else {
                references(&definitions)
            },
            context
                .maps
                .get(&element_type.id)
                .map_or(Parameter::Omitted, |&map| references(&[map])),
            Parameter::Omitted,
            user_type.unwrap_or(Parameter::Omitted),
        ];
        if element_type.kind == ElementKind::Furniture {
            // `AssemblyPlace` is required by IFC4; the model has no value for it.
            parameters.push(enumeration("NOTDEFINED"));
        }
        parameters.push(predefined);
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
            if element_type.layers.is_empty() || self.skipped.contains(&element_type.id) {
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
                // IFC2X3 layers have a positive thickness: a membrane is not representable.
                if layer.thickness == 0.0 && self.target.schema == OutputSchema::Ifc2x3 {
                    Target::unrepresentable(&mut self.loss, "IFCMATERIALLAYER", None);
                    continue;
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
            if layers.is_empty() {
                continue;
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
            if self.skipped.contains(&element.id) {
                continue;
            }
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
    /// The `IfcProductDefinitionShape` of a body; `None` when the body is not written (reported).
    fn body(&mut self, body: &Body, owner: u128, context: &Context) -> Result<Option<u64>> {
        let Some(representation) = self.body_shape(body, owner, context)? else {
            return Ok(None);
        };
        self.product_shape(representation).map(Some)
    }

    /// The `IfcProductDefinitionShape` of one `IfcShapeRepresentation`.
    fn product_shape(&mut self, representation: u64) -> Result<u64> {
        self.add(
            "IFCPRODUCTDEFINITIONSHAPE",
            &[
                Parameter::Omitted,
                Parameter::Omitted,
                references(&[representation]),
            ],
        )
    }

    /// The `IfcShapeRepresentation` of a body in the body context; `None` when the body is not
    /// written (reported against `owner`).
    fn body_shape(&mut self, body: &Body, owner: u128, context: &Context) -> Result<Option<u64>> {
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
        self.add(
            "IFCSHAPEREPRESENTATION",
            &[
                reference(context.body),
                label("Body"),
                label(kind.0),
                references(&[item]),
            ],
        )
        .map(Some)
    }

    /// The `IfcRepresentationMap` of a type with a representation, with the identity as its origin,
    /// written before the elements: an occurrence names it in its `IfcMappedItem`. A type the
    /// output schema lacks, or a body that is not written, has no map.
    fn representation_map(
        &mut self,
        element_type: &ElementType,
        context: &mut Context,
    ) -> Result<()> {
        let (Some(body), Some(entity)) = (&element_type.representation, element_type.entity())
        else {
            return Ok(());
        };
        if !self.target.supports(entity) {
            return Ok(());
        }
        let Some(representation) = self.body_shape(body, element_type.id, context)? else {
            return Ok(());
        };
        let origin = self.axes(&Placement::default())?;
        let map = self.add(
            "IFCREPRESENTATIONMAP",
            &[reference(origin), reference(representation)],
        )?;
        context.maps.insert(element_type.id, map);
        Ok(())
    }

    /// The representation of an occurrence of a type with shared geometry: an `IfcMappedItem` of the
    /// type's map, its `MappingTarget` the element's [`Element::mapping`], in a
    /// `MappedRepresentation`. `None` when the map is absent (reported).
    fn mapped(&mut self, element: &Element, context: &Context) -> Result<Option<u64>> {
        let Some(mapping) = element.mapping else {
            return Ok(None);
        };
        let Some(&map) = element.element_type.and_then(|t| context.maps.get(&t)) else {
            self.loss.add(
                reasons::NOT_WRITTEN,
                "IFCMAPPEDITEM",
                Some(Source::Text(&global_id(element.id))),
                1,
            );
            return Ok(None);
        };
        let target = self.transform(&mapping)?;
        let item = self.add("IFCMAPPEDITEM", &[reference(map), reference(target)])?;
        let representation = self.add(
            "IFCSHAPEREPRESENTATION",
            &[
                reference(context.body),
                label("Body"),
                label("MappedRepresentation"),
                references(&[item]),
            ],
        )?;
        self.product_shape(representation).map(Some)
    }

    /// An `IfcCartesianTransformationOperator3D` of a placement: the origin, and the z axis and the
    /// x axis made orthogonal to it (the y axis completes them); the scale is one.
    fn transform(&mut self, placement: &Placement) -> Result<u64> {
        let Placement {
            origin,
            axis,
            ref_direction,
        } = placement;
        let finite = origin.iter().chain(axis.iter()).chain(ref_direction.iter());
        if !finite.copied().all(f64::is_finite) {
            return Err(invalid("mapping"));
        }
        let Some(z) = unit(axis) else {
            return Err(invalid("mapping"));
        };
        let Some(x) = unit(&(ref_direction - z * ref_direction.dot(&z))) else {
            return Err(invalid("mapping"));
        };
        let y = z.cross(&x);
        let location = self.point3(origin)?;
        let (x, y, z) = (
            self.direction(&x)?,
            self.direction(&y)?,
            self.direction(&z)?,
        );
        self.add(
            "IFCCARTESIANTRANSFORMATIONOPERATOR3D",
            &[
                reference(x),
                reference(y),
                reference(location),
                Parameter::Omitted,
                reference(z),
            ],
        )
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

    /// The property sets of `owner`, returned as their definitions. Relates each to `instance` when
    /// there is one (the element or spatial structure of an occurrence); a type lists them in its
    /// `HasPropertySets` instead.
    fn property_sets(
        &mut self,
        owner: u128,
        sets: &[PropertySet],
        instance: Option<u64>,
    ) -> Result<Vec<u64>> {
        let mut definitions = Vec::new();
        for (index, set) in (0u32..).zip(sets) {
            if set.properties.is_empty() {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCPROPERTYSET",
                    Some(Source::Text(&global_id(owner))),
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
                    Some(value) => {
                        let Some(value) = self.carried_value(value) else {
                            continue;
                        };
                        self.add(
                            "IFCPROPERTYSINGLEVALUE",
                            &[
                                label(&property.name),
                                Parameter::Omitted,
                                value,
                                Parameter::Omitted,
                            ],
                        )?
                    }
                    None => self.enumerated(property)?,
                };
                properties.push(written);
            }
            if properties.is_empty() {
                continue;
            }
            let set_id = self.global_id(derived_id(owner, role::PROPERTY_SET, index))?;
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
            if let Some(instance) = instance {
                self.defines(owner, role::PROPERTY_REL, index, instance, definition)?;
            }
            definitions.push(definition);
        }
        Ok(definitions)
    }

    /// A typed nominal value in a type the output schema declares: the value itself, in the
    /// nearest wider type where the schema lacks its type, or `None` (reported) where there is
    /// none.
    fn carried_value(&mut self, value: Parameter) -> Option<Parameter> {
        let Parameter::Typed {
            type_name,
            value: inner,
        } = value
        else {
            return Some(value);
        };
        match self.target.value_type(&type_name) {
            Some(carried) => {
                let inner = match *inner {
                    Parameter::Real(v) if self.target.is_integer(&carried) => {
                        Parameter::Integer(target::rounded(v))
                    }
                    other => other,
                };
                Some(typed(&carried, inner))
            }
            None => {
                self.loss
                    .add(reasons::ENTITY_NOT_REPRESENTABLE, &type_name, None, 1);
                None
            }
        }
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

    /// The quantity sets of `owner`, as [`Self::property_sets`] does for property sets.
    fn quantity_sets(
        &mut self,
        owner: u128,
        sets: &[QuantitySet],
        instance: Option<u64>,
    ) -> Result<Vec<u64>> {
        let mut definitions = Vec::new();
        for (index, set) in (0u32..).zip(sets) {
            if set.quantities.is_empty() {
                self.loss.add(
                    reasons::NOT_WRITTEN,
                    "IFCELEMENTQUANTITY",
                    Some(Source::Text(&global_id(owner))),
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
            let set_id = self.global_id(derived_id(owner, role::QUANTITY_SET, index))?;
            // IFC2X3 has no quantity set templates: its convention is one `BaseQuantities` set.
            let set_name =
                if self.target.schema == OutputSchema::Ifc2x3 && set.name.starts_with("Qto_") {
                    self.loss
                        .add(reasons::SET_RENAMED, "IFCELEMENTQUANTITY", None, 1);
                    "BaseQuantities"
                } else {
                    set.name.as_str()
                };
            let definition = self.add(
                "IFCELEMENTQUANTITY",
                &[
                    set_id,
                    Parameter::Omitted,
                    label(set_name),
                    Parameter::Omitted,
                    Parameter::Omitted,
                    references(&quantities),
                ],
            )?;
            if let Some(instance) = instance {
                self.defines(owner, role::QUANTITY_REL, index, instance, definition)?;
            }
            definitions.push(definition);
        }
        Ok(definitions)
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
            if self.skipped.contains(&element.id) || self.skipped.contains(&host) {
                continue;
            }
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

    /// One `IfcRelAggregates` per element with parts: the parts are the elements that name it as
    /// their host, in model order. A host may be any element (see [`Element::host`]).
    fn parts(&mut self, model: &WriteModel, context: &Context) -> Result<()> {
        let mut by_host: BTreeMap<u128, Vec<u64>> = BTreeMap::new();
        for e in model.elements.iter().filter(|e| is_part(e.kind)) {
            if let (Some(host), Some(&part)) = (e.host, context.elements.get(&e.id)) {
                by_host.entry(host).or_default().push(part);
            }
        }
        for element in &model.elements {
            let Some(parts) = by_host.get(&element.id) else {
                continue;
            };
            if let Some(&host) = context.elements.get(&element.id) {
                self.relationship(
                    "IFCRELAGGREGATES",
                    derived_id(element.id, role::AGGREGATES, 0),
                    reference(host),
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
