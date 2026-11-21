//! IFC writing (CORE-IO-016, first part): an IFC4 physical file from a neutral description of a
//! building.
//!
//! [`WriteModel`] describes a project, a site, a building, its storeys and elements (walls, slabs,
//! doors, windows, openings and spaces) with their placement, body geometry, voids and fills, property
//! sets and base quantities. [`write()`] turns it into the bytes of an ISO 10303-21 clear text file
//! with the IFC4 schema and a loss report of what it did not write. It knows nothing of any
//! product: the caller maps its own objects to this description.
//!
//! The output is deterministic: the same model and options give the same bytes. Nothing reads the
//! clock; the time stamp of the header is the caller's. Every `GlobalId` comes from an identifier
//! of the model ([`global_id`]) or, for the relationships and sets the writer adds, from the
//! identifier of their object and a role.

mod global_id;
mod step;

use std::collections::{BTreeMap, BTreeSet};

use arc_base::math::{Point2, Point3, Vector2, Vector3};
use arc_base::{Cancel, Error, Limits, Result, Tolerance};

use crate::error::{codes, limit, reasons};
use crate::loss::{Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::Parameter;

pub use global_id::{derived_id, global_id, role};

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
    /// `IfcSpace`, aggregated by its storey (IFC4 decomposes a storey into its spaces; it does not
    /// contain them). By convention `Element::name` holds the room number and
    /// `Element::long_name` the room name.
    Space,
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

/// A single value of a property, written with its IFC measure type.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// `IfcLabel`.
    Label(String),
    /// `IfcLengthMeasure`, in the length unit.
    Length(f64),
    /// `IfcAreaMeasure`, in the length unit squared.
    Area(f64),
    /// `IfcVolumeMeasure`, in the length unit cubed.
    Volume(f64),
    /// `IfcBoolean`.
    Boolean(bool),
}

/// A property with a single value (`IfcPropertySingleValue`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Property {
    /// The name, unique within its set.
    pub name: String,
    /// The value.
    pub value: PropertyValue,
}

impl Property {
    /// A property with a single value.
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

/// The value of a quantity; it must not be negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuantityValue {
    /// `IfcQuantityLength`, in the length unit.
    Length(f64),
    /// `IfcQuantityArea`, in the length unit squared.
    Area(f64),
    /// `IfcQuantityVolume`, in the length unit cubed.
    Volume(f64),
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
    /// of hosts must not return to its start. Must be `None`
    /// for walls and slabs.
    pub host: Option<u128>,
    /// The property sets.
    pub property_sets: Vec<PropertySet>,
    /// The quantity sets.
    pub quantity_sets: Vec<QuantitySet>,
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
            property_sets: Vec::new(),
            quantity_sets: Vec::new(),
        }
    }
}

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

/// Writes the model as an IFC4 physical file.
///
/// # Errors
///
/// `io.ifc.write_value_invalid` for a number that is not finite, a negative quantity, a door or
/// window size that is not positive, a placement whose axes or a true north that is zero, shorter
/// than about 1.5E-8 or parallel, a NUL in a text, two properties or quantities of one set with
/// the same name, or two sets of one element with the same name;
/// `io.ifc.write_reference_invalid` for a storey or host that is not in the model or of the wrong
/// kind, an opening without a host, or a chain of hosts that returns to its start;
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
        ids: BTreeSet::new(),
        loss: LossLog::default(),
    };
    writer.header(options)?;
    let mut context = writer.context(model)?;
    for element in &model.elements {
        options.cancel.check()?;
        writer.element(element, &mut context)?;
    }
    writer.relationships(model, &context, &kinds)?;
    writer.data.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    writer.loss.add_mapped(len64(model.elements.len()));
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
    for element in &model.elements {
        if !storeys.contains(&element.storey) {
            return Err(bad_reference(element.storey));
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
            ElementKind::Wall | ElementKind::Slab | ElementKind::Space => false,
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
    storeys: BTreeMap<u128, (u64, u64)>,
    elements: BTreeMap<u128, u64>,
}

struct Writer<'a> {
    data: String,
    next: u64,
    limits: &'a Limits,
    tolerance: Tolerance,
    meters: f64,
    ids: BTreeSet<u128>,
    loss: LossLog,
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
        let body = self.add(
            "IFCGEOMETRICREPRESENTATIONSUBCONTEXT",
            &[
                label("Body"),
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
        )?;
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
            storeys,
            elements: BTreeMap::new(),
        })
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
        } else {
            parameters.push(predefined);
        }
        let instance = self.add(element.kind.entity(), &parameters)?;
        context.elements.insert(element.id, instance);
        self.property_sets(element, instance)?;
        self.quantity_sets(element, instance)
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
                let value = match &property.value {
                    PropertyValue::Label(text) => typed("IFCLABEL", label(text)),
                    PropertyValue::Length(v) => typed("IFCLENGTHMEASURE", Parameter::Real(*v)),
                    PropertyValue::Area(v) => typed("IFCAREAMEASURE", Parameter::Real(*v)),
                    PropertyValue::Volume(v) => typed("IFCVOLUMEMEASURE", Parameter::Real(*v)),
                    PropertyValue::Boolean(v) => {
                        typed("IFCBOOLEAN", enumeration(if *v { "T" } else { "F" }))
                    }
                };
                properties.push(self.add(
                    "IFCPROPERTYSINGLEVALUE",
                    &[
                        label(&property.name),
                        Parameter::Omitted,
                        value,
                        Parameter::Omitted,
                    ],
                )?);
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
                let (type_name, value) = match quantity.value {
                    QuantityValue::Length(v) => ("IFCQUANTITYLENGTH", v),
                    QuantityValue::Area(v) => ("IFCQUANTITYAREA", v),
                    QuantityValue::Volume(v) => ("IFCQUANTITYVOLUME", v),
                };
                if !value.is_finite() || value < 0.0 {
                    return Err(invalid("quantity"));
                }
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
    /// and the voids and fills.
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
        for storey in &model.storeys {
            let contained: Vec<u64> = model
                .elements
                .iter()
                .filter(|e| e.storey == storey.id)
                .filter(|e| !matches!(e.kind, ElementKind::Opening | ElementKind::Space))
                .filter_map(|e| context.elements.get(&e.id).copied())
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
        Ok(())
    }
}
