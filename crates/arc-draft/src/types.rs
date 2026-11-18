//! The drafting object types (CORE-DRAFT-001, 002, 009, 016, 018, 022) and their registration in
//! the `arc-doc` type registry.
//!
//! Every type is version 1.0. Entities share the [common fields](entity) at the start of their
//! field list, followed by the fields of their kind. An entity belongs to a space or a block
//! definition through its header owner, so it is deleted with its container.

use arc_base::math::{Point2, Vector3};
use arc_base::{Color, Error, Limits, ObjectId};
use arc_doc::{
    Bound, Cardinality, ColorValue, EnumVariant, FieldDecl, Finding, GeometryEncoding, OnCopy,
    OnDelete, ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Strength, TypeDecl,
    TypeKey, TypeRegistry, TypeValidator, Value, ValueType,
};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Polyline2, Ray2, Segment2};

use crate::error::codes;

/// A model space or layout: `name`, `kind`, `unit`.
pub const SPACE: &str = "core.draft.space";
/// A layer: see [`layer`] for the field indices.
pub const LAYER: &str = "core.draft.layer";
/// A linetype definition: `name`, `description`, `elements`.
pub const LINETYPE: &str = "core.draft.linetype";
/// A block definition: `name`, `base_point`, `unit`, `description`, `attributes`.
pub const BLOCK: &str = "core.draft.block";
/// A point entity: common fields, then `position`.
pub const POINT: &str = "core.draft.point";
/// A line (segment) entity: common fields, then `geometry`.
pub const LINE: &str = "core.draft.line";
/// A ray entity: common fields, then `geometry`.
pub const RAY: &str = "core.draft.ray";
/// A construction line (infinite in both directions): common fields, then `geometry`.
pub const CONSTRUCTION_LINE: &str = "core.draft.construction_line";
/// A circle entity: common fields, then `geometry`.
pub const CIRCLE: &str = "core.draft.circle";
/// An arc entity: common fields, then `geometry`.
pub const ARC: &str = "core.draft.arc";
/// A full ellipse entity: common fields, then `geometry`.
pub const ELLIPSE: &str = "core.draft.ellipse";
/// An elliptical arc entity: common fields, then `geometry`.
pub const ELLIPTICAL_ARC: &str = "core.draft.elliptical_arc";
/// A bulge polyline with per-vertex widths: common fields, then `geometry` and
/// `continuous_pattern`.
pub const POLYLINE: &str = "core.draft.polyline";
/// A spline: common fields, then `geometry` (a NURBS curve, encoding version 2) and
/// `fit_points`.
pub const SPLINE: &str = "core.draft.spline";
/// A block reference: common fields, then `block`, `position`, `scale`, `rotation`,
/// `attributes`.
pub const BLOCK_REF: &str = "core.draft.block_ref";

/// A wipeout: common fields, then `geometry` (a closed polyline) and `frame`. It masks what is
/// beneath it in draw order (CORE-DRAFT-004).
pub const WIPEOUT: &str = "core.draft.wipeout";

/// Every entity type key: objects with the common fields that live in a space or block.
pub const ENTITIES: &[&str] = &[
    POINT,
    LINE,
    RAY,
    CONSTRUCTION_LINE,
    CIRCLE,
    ARC,
    ELLIPSE,
    ELLIPTICAL_ARC,
    POLYLINE,
    SPLINE,
    BLOCK_REF,
    WIPEOUT,
    crate::text::TEXT,
    crate::text::RICH_TEXT,
    crate::dimension::DIMENSION,
    crate::leader::LEADER,
    crate::leader::MULTILEADER,
];

/// The types that contain entities: spaces and block definitions.
pub const CONTAINERS: &[&str] = &[SPACE, BLOCK];

/// The most bytes of a name or plot style name.
pub const NAME_BYTES: u64 = 255;
/// The most bytes of a description or attribute text.
pub const TEXT_BYTES: u64 = 4096;
/// The most elements of a linetype pattern.
pub const MAX_PATTERN_ELEMENTS: u64 = 256;
/// The most attribute definitions of a block, and attribute values of a reference.
pub const MAX_ATTRIBUTES: u64 = 1024;
/// The largest lineweight in millimeters.
pub const MAX_LINEWEIGHT_MM: f64 = 10.0;

/// Field indices of the common entity fields, in declared order.
pub mod entity {
    /// `layer`: optional reference to a layer; deleting a used layer is blocked.
    pub const LAYER: usize = 0;
    /// `color_by`: [`super::By`] code of the color.
    pub const COLOR_BY: usize = 1;
    /// `color`: optional color used when `color_by` is `object`.
    pub const COLOR: usize = 2;
    /// `linetype_by`.
    pub const LINETYPE_BY: usize = 3;
    /// `linetype`: optional reference to a linetype; absent with `object` is continuous.
    pub const LINETYPE: usize = 4;
    /// `linetype_scale`: the per-object pattern scale, positive.
    pub const LINETYPE_SCALE: usize = 5;
    /// `lineweight_by`.
    pub const LINEWEIGHT_BY: usize = 6;
    /// `lineweight`: millimeters in [0, 10]; absent with `object` is the default lineweight.
    pub const LINEWEIGHT: usize = 7;
    /// `transparency_by`.
    pub const TRANSPARENCY_BY: usize = 8;
    /// `transparency`: 0 (opaque) to 1; absent with `object` is 0.
    pub const TRANSPARENCY: usize = 9;
    /// `plot_style`: optional plot style name; absent takes the layer's.
    pub const PLOT_STYLE: usize = 10;
    /// `visible`.
    pub const VISIBLE: usize = 11;
    /// `draw_order`: the draw-order key within the container.
    pub const DRAW_ORDER: usize = 12;
    /// `elevation`: distance along the extrusion direction (CORE-DRAFT-009).
    pub const ELEVATION: usize = 13;
    /// `thickness`: extrusion thickness (CORE-DRAFT-009).
    pub const THICKNESS: usize = 14;
    /// `extrusion`: the extrusion direction, not zero (CORE-DRAFT-009).
    pub const EXTRUSION: usize = 15;
    /// The first field of the entity kind: `geometry` or `position` or `block`.
    pub const KIND: usize = 16;
    /// `continuous_pattern` of a polyline.
    pub const CONTINUOUS_PATTERN: usize = 17;
    /// `frame` of a wipeout: whether its boundary is drawn.
    pub const WIPEOUT_FRAME: usize = 17;
    /// `fit_points` of a spline.
    pub const SPLINE_FIT_POINTS: usize = 17;
    /// `position` of a block reference.
    pub const REF_POSITION: usize = 17;
    /// `scale` of a block reference: per-axis factors, none zero.
    pub const REF_SCALE: usize = 18;
    /// `rotation` of a block reference in radians.
    pub const REF_ROTATION: usize = 19;
    /// `attributes` of a block reference: tag to value.
    pub const REF_ATTRIBUTES: usize = 20;
}

/// Field indices of a layer.
pub mod layer {
    /// `name`.
    pub const NAME: usize = 0;
    /// `color`.
    pub const COLOR: usize = 1;
    /// `linetype`: optional; absent is continuous.
    pub const LINETYPE: usize = 2;
    /// `lineweight`: optional millimeters; absent is the default lineweight.
    pub const LINEWEIGHT: usize = 3;
    /// `transparency`.
    pub const TRANSPARENCY: usize = 4;
    /// `plot_style`.
    pub const PLOT_STYLE: usize = 5;
    /// `on`.
    pub const ON: usize = 6;
    /// `frozen`.
    pub const FROZEN: usize = 7;
    /// `locked`.
    pub const LOCKED: usize = 8;
    /// `plottable`.
    pub const PLOTTABLE: usize = 9;
    /// `description`.
    pub const DESCRIPTION: usize = 10;
}

/// Field indices of a space, linetype and block definition.
pub mod table {
    /// `name` of a space, linetype or block.
    pub const NAME: usize = 0;
    /// `kind` of a space.
    pub const SPACE_KIND: usize = 1;
    /// `unit` of a space.
    pub const SPACE_UNIT: usize = 2;
    /// `elements` of a linetype.
    pub const LINETYPE_ELEMENTS: usize = 2;
    /// `base_point` of a block.
    pub const BLOCK_BASE: usize = 1;
    /// `unit` of a block.
    pub const BLOCK_UNIT: usize = 2;
    /// `attributes` of a block.
    pub const BLOCK_ATTRIBUTES: usize = 4;
}

/// Where an appearance value comes from; the code of a `*_by` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// From the entity's layer.
    Layer = 0,
    /// From the block reference that inserts the entity.
    Block = 1,
    /// The entity's own value.
    Object = 2,
}

impl By {
    /// The mode of an enum code; unknown codes read as [`By::Layer`].
    #[must_use]
    pub fn of(code: Option<u32>) -> Self {
        match code {
            Some(1) => Self::Block,
            Some(2) => Self::Object,
            _ => Self::Layer,
        }
    }
}

/// The length unit a space or block declares (CORE-DRAFT-001), with its enum code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SpaceUnit {
    /// No declared unit; converting to or from it uses the factor 1.
    Unitless = 0,
    /// Millimeter.
    Millimeter = 1,
    /// Centimeter.
    Centimeter = 2,
    /// Meter.
    Meter = 3,
    /// Kilometer.
    Kilometer = 4,
    /// Inch.
    Inch = 5,
    /// Foot.
    Foot = 6,
    /// Yard.
    Yard = 7,
    /// Mile.
    Mile = 8,
    /// United States survey foot.
    UsSurveyFoot = 9,
}

/// Every space unit with its stable name, in code order.
const UNITS: [(SpaceUnit, &str); 10] = [
    (SpaceUnit::Unitless, "unitless"),
    (SpaceUnit::Millimeter, "millimeter"),
    (SpaceUnit::Centimeter, "centimeter"),
    (SpaceUnit::Meter, "meter"),
    (SpaceUnit::Kilometer, "kilometer"),
    (SpaceUnit::Inch, "inch"),
    (SpaceUnit::Foot, "foot"),
    (SpaceUnit::Yard, "yard"),
    (SpaceUnit::Mile, "mile"),
    (SpaceUnit::UsSurveyFoot, "us_survey_foot"),
];

impl SpaceUnit {
    /// The enum code stored in a `unit` field.
    #[must_use]
    pub fn code(self) -> u32 {
        self as u32
    }

    /// The unit of an enum code.
    #[must_use]
    pub fn of(code: u32) -> Option<Self> {
        UNITS
            .iter()
            .find(|(u, _)| u.code() == code)
            .map(|(u, _)| *u)
    }

    /// The `arc-base` length unit of the same name; `None` for [`SpaceUnit::Unitless`].
    #[must_use]
    pub fn unit(self) -> Option<&'static arc_base::Unit> {
        let name = UNITS.iter().find(|(u, _)| *u == self).map(|(_, n)| *n)?;
        arc_base::Unit::by_name(name).ok()
    }

    /// The factor that converts a length in `self` to a length in `to`: the explicit
    /// conversion of a block inserted into a space with another unit. 1 when either unit is
    /// unitless.
    #[must_use]
    pub fn factor_to(self, to: Self) -> f64 {
        match (self.unit(), to.unit()) {
            (Some(from), Some(to)) => from.scale / to.scale,
            _ => 1.0,
        }
    }
}

/// The kind of a space, with its enum code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    /// A model space.
    Model = 0,
    /// A layout (sheet space).
    Layout = 1,
}

/// The kind of a linetype pattern element, with its enum code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// A drawn piece of the given length.
    Dash = 0,
    /// A blank piece of the given length.
    Gap = 1,
    /// A point: a dash of length zero; its length advances like a gap.
    Dot = 2,
    /// Embedded text, stored as data; its length advances like a gap.
    Text = 3,
    /// An embedded shape, stored as data; its length advances like a gap.
    Shape = 4,
}

impl ElementKind {
    /// The kind of an enum code; unknown codes read as [`ElementKind::Gap`].
    #[must_use]
    pub fn of(code: Option<u32>) -> Self {
        match code {
            Some(0) => Self::Dash,
            Some(2) => Self::Dot,
            Some(3) => Self::Text,
            Some(4) => Self::Shape,
            _ => Self::Gap,
        }
    }
}

pub(crate) fn variants(names: &[&str]) -> ValueType {
    ValueType::Enum(
        names
            .iter()
            .zip(0_u32..)
            .map(|(name, code)| EnumVariant::new(*name, code))
            .collect(),
    )
}

fn by_field(name: &str) -> FieldDecl {
    FieldDecl::new(name, variants(&["layer", "block", "object"]))
        .with_default(Value::Enum(By::Layer as u32))
        .in_aspect("appearance")
}

fn unit_type() -> ValueType {
    variants(&UNITS.map(|(_, name)| name))
}

pub(crate) fn reference(
    target: &str,
    strength: Strength,
    on_delete: OnDelete,
    optional: bool,
) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: TypeKey::new(target).into_iter().collect(),
        rule: RelationshipRule {
            strength,
            on_delete,
            on_copy: OnCopy::ShareTarget,
            cardinality: if optional {
                Cardinality::Optional
            } else {
                Cardinality::One
            },
        },
    })
}

pub(crate) fn name_field() -> FieldDecl {
    FieldDecl::new("name", ValueType::String)
        .with_bound(Bound::Length { max: NAME_BYTES })
        .in_aspect("identity")
}

pub(crate) fn text(name: &str, max: u64) -> FieldDecl {
    FieldDecl::new(name, ValueType::String)
        .with_bound(Bound::Length { max })
        .optional()
}

pub(crate) fn real(name: &str, default: f64, min: f64, max: f64) -> FieldDecl {
    FieldDecl::new(name, ValueType::F64)
        .with_default(Value::F64(default))
        .with_bound(Bound::Real { min, max })
}

pub(crate) fn flag(name: &str, default: bool) -> FieldDecl {
    FieldDecl::new(name, ValueType::Bool).with_default(Value::Bool(default))
}

fn origin() -> Value {
    Value::Point2(Point2::new(0.0, 0.0))
}

/// The common entity fields (indices in [`entity`]).
fn common() -> Vec<FieldDecl> {
    let appearance = |f: FieldDecl| f.in_aspect("appearance");
    vec![
        appearance(
            FieldDecl::new(
                "layer",
                reference(LAYER, Strength::Associative, OnDelete::Block, true),
            )
            .optional(),
        ),
        by_field("color_by"),
        appearance(FieldDecl::new("color", ValueType::Color).optional()),
        by_field("linetype_by"),
        appearance(
            FieldDecl::new(
                "linetype",
                reference(LINETYPE, Strength::Associative, OnDelete::Block, true),
            )
            .optional(),
        ),
        appearance(real("linetype_scale", 1.0, f64::MIN_POSITIVE, f64::MAX)),
        by_field("lineweight_by"),
        appearance(
            FieldDecl::new("lineweight", ValueType::F64)
                .with_bound(Bound::Real {
                    min: 0.0,
                    max: MAX_LINEWEIGHT_MM,
                })
                .optional(),
        ),
        by_field("transparency_by"),
        appearance(
            FieldDecl::new("transparency", ValueType::F64)
                .with_bound(Bound::Real { min: 0.0, max: 1.0 })
                .optional(),
        ),
        appearance(text("plot_style", NAME_BYTES)),
        appearance(flag("visible", true)),
        FieldDecl::new("draw_order", ValueType::I64)
            .with_default(Value::I64(0))
            .in_aspect("order"),
        real("elevation", 0.0, f64::MIN, f64::MAX).in_aspect("geometry"),
        real("thickness", 0.0, f64::MIN, f64::MAX).in_aspect("geometry"),
        FieldDecl::new("extrusion", ValueType::Vector3)
            .with_default(Value::Vector3(Vector3::new(0.0, 0.0, 1.0)))
            .in_aspect("geometry"),
    ]
}

pub(crate) fn geometry() -> FieldDecl {
    let encoding = arc_geom::GeomEncoding::CURRENT;
    FieldDecl::new(
        "geometry",
        ValueType::Geometry(GeometryEncoding {
            name: encoding.name.to_owned(),
            version: u32::from(encoding.version),
        }),
    )
    .in_aspect("geometry")
}

pub(crate) fn finding(error: Error) -> Finding {
    Finding {
        severity: Severity::Error,
        error,
    }
}

/// The findings of the common fields: a zero extrusion direction.
pub(crate) fn common_findings(values: &[Option<Value>]) -> Vec<Finding> {
    match values.get(entity::EXTRUSION) {
        Some(Some(Value::Vector3(v))) if v.x == 0.0 && v.y == 0.0 && v.z == 0.0 => {
            vec![finding(Error::new(codes::EXTRUSION_ZERO))]
        }
        _ => Vec::new(),
    }
}

/// The type validator of a curve entity whose geometry is a `T`: the common findings, and
/// `draft.geometry.invalid` unless the bytes decode as a `T` under [`Limits::DEFAULT`].
fn curve_entity<T: GeomCodec>(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    if let Some(Some(Value::Geometry(bytes))) = values.get(entity::KIND)
        && let Err(error) = T::decode(&bytes.0, &Limits::DEFAULT)
    {
        findings.push(finding(
            Error::new(codes::GEOMETRY_INVALID).with_source(error),
        ));
    }
    findings
}

/// The type validator of a wipeout: a polyline boundary that is closed.
fn wipeout_entity(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = curve_entity::<Polyline2>(values);
    if let Some(Some(Value::Geometry(bytes))) = values.get(entity::KIND)
        && let Ok(polyline) = Polyline2::decode(&bytes.0, &Limits::DEFAULT)
        && !polyline.is_closed()
    {
        findings.push(finding(Error::new(codes::WIPEOUT_BOUNDARY_OPEN)));
    }
    findings
}

fn block_ref_entity(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    if let Some(Some(Value::Vector3(s))) = values.get(entity::REF_SCALE)
        && (s.x == 0.0 || s.y == 0.0 || s.z == 0.0)
    {
        findings.push(finding(Error::new(codes::BLOCK_SCALE_ZERO)));
    }
    findings
}

fn space_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let layout = matches!(values.get(table::SPACE_KIND), Some(Some(Value::Enum(1))));
    let unitless = matches!(values.get(table::SPACE_UNIT), Some(Some(Value::Enum(0))));
    if layout && unitless {
        vec![finding(Error::new(codes::SPACE_LAYOUT_UNITLESS))]
    } else {
        Vec::new()
    }
}

fn linetype_findings(values: &[Option<Value>]) -> Vec<Finding> {
    match crate::linetype::Pattern::from_values(values) {
        Ok(_) => Vec::new(),
        Err(error) => vec![finding(error)],
    }
}

fn layer_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        FieldDecl::new("color", ValueType::Color).with_default(Value::Color(Box::new(
            ColorValue {
                rgba: Color::WHITE,
                source: None,
            },
        ))),
        FieldDecl::new(
            "linetype",
            reference(LINETYPE, Strength::Associative, OnDelete::Block, true),
        )
        .optional(),
        FieldDecl::new("lineweight", ValueType::F64)
            .with_bound(Bound::Real {
                min: 0.0,
                max: MAX_LINEWEIGHT_MM,
            })
            .optional(),
        real("transparency", 0.0, 0.0, 1.0),
        text("plot_style", NAME_BYTES),
        flag("on", true),
        flag("frozen", false),
        flag("locked", false),
        flag("plottable", true),
        text("description", TEXT_BYTES),
    ]
}

fn linetype_fields() -> Vec<FieldDecl> {
    let element = ValueType::Record(vec![
        FieldDecl::new("kind", variants(&["dash", "gap", "dot", "text", "shape"])),
        real("length", 0.0, 0.0, f64::MAX),
        text("content", TEXT_BYTES),
        text("style", NAME_BYTES),
        real("scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("offset", ValueType::Point2).optional(),
    ]);
    vec![
        name_field(),
        text("description", TEXT_BYTES),
        FieldDecl::new("elements", ValueType::List(Box::new(element))).with_bound(Bound::Length {
            max: MAX_PATTERN_ELEMENTS,
        }),
    ]
}

fn block_fields() -> Vec<FieldDecl> {
    let attribute = ValueType::Record(vec![
        FieldDecl::new("tag", ValueType::String).with_bound(Bound::Length { max: NAME_BYTES }),
        text("prompt", TEXT_BYTES),
        text("default", TEXT_BYTES),
        flag("constant", false),
        flag("invisible", false),
        flag("preset", false),
        flag("multiline", false),
        FieldDecl::new("position", ValueType::Point2).with_default(origin()),
    ]);
    vec![
        name_field(),
        FieldDecl::new("base_point", ValueType::Point2).with_default(origin()),
        FieldDecl::new("unit", unit_type()).with_default(Value::Enum(0)),
        text("description", TEXT_BYTES),
        FieldDecl::new("attributes", ValueType::List(Box::new(attribute))).with_bound(
            Bound::Length {
                max: MAX_ATTRIBUTES,
            },
        ),
    ]
}

fn space_fields() -> Vec<FieldDecl> {
    vec![
        name_field(),
        FieldDecl::new("kind", variants(&["model", "layout"])),
        FieldDecl::new("unit", unit_type()),
    ]
}

pub(crate) fn with_kind(fields: Vec<FieldDecl>) -> Vec<FieldDecl> {
    let mut all = common();
    all.extend(fields);
    all
}

fn block_ref_fields() -> Vec<FieldDecl> {
    with_kind(vec![
        FieldDecl::new(
            "block",
            reference(BLOCK, Strength::Associative, OnDelete::Block, false),
        ),
        FieldDecl::new("position", ValueType::Point2).with_default(origin()),
        FieldDecl::new("scale", ValueType::Vector3)
            .with_default(Value::Vector3(Vector3::new(1.0, 1.0, 1.0))),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("attributes", ValueType::Map(Box::new(ValueType::String))).with_bound(
            Bound::Length {
                max: MAX_ATTRIBUTES,
            },
        ),
    ])
}

/// Every drafting type declaration.
fn declarations() -> arc_base::Result<Vec<TypeDecl>> {
    let curve = || with_kind(vec![geometry()]);
    let table: [(&str, Vec<FieldDecl>, Vec<TypeValidator>); 16] = [
        (SPACE, space_fields(), vec![space_findings]),
        (LAYER, layer_fields(), vec![]),
        (LINETYPE, linetype_fields(), vec![linetype_findings]),
        (BLOCK, block_fields(), vec![]),
        (
            POINT,
            with_kind(vec![FieldDecl::new("position", ValueType::Point2)]),
            vec![common_findings],
        ),
        (LINE, curve(), vec![curve_entity::<Segment2>]),
        (RAY, curve(), vec![curve_entity::<Ray2>]),
        (CONSTRUCTION_LINE, curve(), vec![curve_entity::<Line2>]),
        (CIRCLE, curve(), vec![curve_entity::<Circle2>]),
        (ARC, curve(), vec![curve_entity::<Arc2>]),
        (ELLIPSE, curve(), vec![curve_entity::<Ellipse2>]),
        (
            ELLIPTICAL_ARC,
            curve(),
            vec![curve_entity::<EllipticalArc2>],
        ),
        (
            POLYLINE,
            with_kind(vec![geometry(), flag("continuous_pattern", false)]),
            vec![curve_entity::<Polyline2>],
        ),
        (
            SPLINE,
            with_kind(crate::spline::fields()),
            vec![crate::spline::findings],
        ),
        (BLOCK_REF, block_ref_fields(), vec![block_ref_entity]),
        (
            WIPEOUT,
            with_kind(vec![geometry(), flag("frame", false)]),
            vec![wipeout_entity],
        ),
    ];
    let version = SchemaVersion::new(1, 0);
    table
        .into_iter()
        .chain(crate::layout::declarations())
        .chain(crate::style::declarations())
        .chain(crate::text::declarations())
        .chain(crate::dimension::declarations())
        .chain(crate::leader::declarations())
        .chain(crate::plot_style::declarations())
        .map(|(key, fields, validators)| {
            let mut decl = TypeDecl::new(TypeKey::new(key)?, Schema::new(version, fields));
            decl.validators = validators;
            Ok(decl)
        })
        .collect()
}

/// Registers every drafting type into `registry`.
///
/// # Errors
///
/// The registry's errors: `doc.type.duplicate_key` when a drafting type is already registered.
pub fn register_types(registry: &mut TypeRegistry) -> arc_base::Result<()> {
    for decl in declarations()? {
        registry.register(decl)?;
    }
    Ok(())
}

/// The values a new object of `key` gets: each field's default, absent where it has none.
#[must_use]
pub fn defaults(registry: &TypeRegistry, key: &str) -> Vec<Option<Value>> {
    registry
        .get(key)
        .map(|decl| {
            decl.schema
                .fields
                .iter()
                .map(|f| f.default.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The reference held at `index`, if any.
#[must_use]
pub fn reference_at(values: &[Option<Value>], index: usize) -> Option<ObjectId> {
    match values.get(index) {
        Some(Some(Value::Reference(id))) => Some(*id),
        _ => None,
    }
}

/// The text held at `index`, if any.
#[must_use]
pub fn text_at(values: &[Option<Value>], index: usize) -> Option<&str> {
    match values.get(index) {
        Some(Some(Value::String(s))) => Some(s),
        _ => None,
    }
}

/// The number held at `index`, if any.
#[must_use]
pub fn f64_at(values: &[Option<Value>], index: usize) -> Option<f64> {
    match values.get(index) {
        Some(Some(Value::F64(x))) => Some(*x),
        _ => None,
    }
}

/// The enum code held at `index`, if any.
#[must_use]
pub fn enum_at(values: &[Option<Value>], index: usize) -> Option<u32> {
    match values.get(index) {
        Some(Some(Value::Enum(code))) => Some(*code),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_registers_and_field_indices_match_names() {
        let mut registry = TypeRegistry::new();
        register_types(&mut registry).unwrap();
        assert_eq!(registry.len(), 30);
        let name = |key: &str, i: usize| registry.get(key).unwrap().schema.fields[i].name.clone();
        assert_eq!(name(LINE, entity::EXTRUSION), "extrusion");
        assert_eq!(name(LINE, entity::KIND), "geometry");
        assert_eq!(
            name(POLYLINE, entity::CONTINUOUS_PATTERN),
            "continuous_pattern"
        );
        assert_eq!(name(BLOCK_REF, entity::REF_ATTRIBUTES), "attributes");
        assert_eq!(name(LAYER, layer::DESCRIPTION), "description");
        assert_eq!(name(BLOCK, table::BLOCK_ATTRIBUTES), "attributes");
        assert_eq!(name(LINETYPE, table::LINETYPE_ELEMENTS), "elements");
        assert!(register_types(&mut registry).is_err());
    }

    #[test]
    fn validators_reject_mismatched_geometry_and_zero_vectors() {
        let mut registry = TypeRegistry::new();
        register_types(&mut registry).unwrap();
        let mut values = defaults(&registry, LINE);
        let ray = Ray2::new(
            Point2::new(0.0, 0.0),
            arc_base::math::Vector2::new(1.0, 0.0),
        )
        .unwrap();
        values[entity::KIND] = Some(Value::Geometry(arc_doc::GeometryBytes(ray.encode())));
        values[entity::EXTRUSION] = Some(Value::Vector3(Vector3::new(0.0, 0.0, 0.0)));
        let codes: Vec<_> = curve_entity::<Segment2>(&values)
            .iter()
            .map(|f| f.error.code())
            .collect();
        assert_eq!(codes, ["draft.extrusion.zero", "draft.geometry.invalid"]);
        assert!(curve_entity::<Ray2>(&values[..entity::KIND + 1]).len() == 1);

        let mut r = defaults(&registry, BLOCK_REF);
        r[entity::REF_SCALE] = Some(Value::Vector3(Vector3::new(1.0, 0.0, 1.0)));
        assert_eq!(
            block_ref_entity(&r)[0].error.code(),
            "draft.block.scale_zero"
        );

        let layout = [
            Some(Value::String("a".into())),
            Some(Value::Enum(1)),
            Some(Value::Enum(0)),
        ];
        assert_eq!(space_findings(&layout).len(), 1);
    }

    #[test]
    fn unit_factors_follow_arc_base() {
        assert!((SpaceUnit::Inch.factor_to(SpaceUnit::Millimeter) - 25.4).abs() < 1e-12);
        assert!((SpaceUnit::Unitless.factor_to(SpaceUnit::Meter) - 1.0).abs() < f64::EPSILON);
        for (unit, _) in UNITS {
            assert_eq!(SpaceUnit::of(unit.code()), Some(unit));
            assert_eq!(unit.unit().is_none(), unit == SpaceUnit::Unitless);
        }
    }
}
