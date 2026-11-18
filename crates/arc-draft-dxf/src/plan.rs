//! The import plan: the creation operations that turn a neutral DXF drawing into drafting
//! objects, as plain data, with the loss report of the mapping.
//!
//! [`plan`] is a pure function of a drawing and [`ImportOptions`]. Operations refer to each
//! other by name (a layer, linetype, style or block name; the drawing's names are case
//! insensitive, so each reference carries the spelling of the definition). The applying side
//! creates the objects in the order of [`Plan::ops`] inside one transaction.

use arc_base::math::Point2;
use arc_base::{Limits, Tolerance};
use arc_doc::ColorValue;
use arc_draft::dimension::Kind as DimensionKind;
use arc_draft::text::Paragraph;
use arc_draft::types::{ElementKind, SpaceUnit};
use arc_geom::{Curve2, Nurbs2, Polyline2};
use arc_io_dxf::loss::{Direction, LossReport};
use arc_io_dxf::{Drawing, Handle};

use crate::mapper::Mapper;

/// What a plan needs to know about its target.
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// The unit of the model space the content goes to.
    pub target_unit: SpaceUnit,
    /// The limits profile of the import; counts and text sizes of the plan stay within it.
    pub limits: Limits,
    /// The tolerance for closing hatch boundaries and interpolating fit points.
    pub tolerance: Tolerance,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            target_unit: SpaceUnit::Millimeter,
            limits: Limits::DEFAULT,
            tolerance: Tolerance::DEFAULT,
        }
    }
}

/// A value of an appearance property: taken from the layer, from the block reference, or the
/// entity's own.
#[derive(Debug, Clone, PartialEq)]
pub enum By<T> {
    /// From the layer.
    Layer,
    /// From the block reference that inserts the entity.
    Block,
    /// The entity's own value.
    Object(T),
}

/// The fields every entity has.
#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    /// The layer name.
    pub layer: String,
    /// The color.
    pub color: By<ColorValue>,
    /// The linetype name; `None` inside `Object` is continuous.
    pub linetype: By<Option<String>>,
    /// The linetype scale, positive.
    pub linetype_scale: f64,
    /// The lineweight in millimeters; `None` inside `Object` is the default lineweight.
    pub lineweight: By<Option<f64>>,
    /// The transparency, 0 (opaque) to 1.
    pub transparency: By<f64>,
    /// Whether the entity is shown.
    pub visible: bool,
    /// The elevation.
    pub elevation: f64,
    /// The thickness.
    pub thickness: f64,
    /// The extrusion direction.
    pub extrusion: [f64; 3],
}

/// A single-line text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    /// The content, one line.
    pub content: String,
    /// The position.
    pub position: Point2,
    /// The rotation in radians.
    pub rotation: f64,
    /// The text style name.
    pub style: Option<String>,
    /// The height, 0 for the style's.
    pub height: f64,
    /// Horizontal alignment: 0 left, 1 center, 2 right.
    pub halign: u32,
    /// Vertical alignment: 0 baseline, 1 bottom, 2 middle, 3 top.
    pub valign: u32,
}

/// A multi-line text.
#[derive(Debug, Clone, PartialEq)]
pub struct RichTextSpec {
    /// The paragraphs, plain.
    pub paragraphs: Vec<Paragraph>,
    /// The position.
    pub position: Point2,
    /// The rotation in radians.
    pub rotation: f64,
    /// The text style name.
    pub style: Option<String>,
    /// The height, 0 for the style's.
    pub height: f64,
    /// The wrapping width, 0 for none.
    pub width: f64,
    /// The attachment point, 0 (top left) to 8 (bottom right).
    pub attachment: u32,
}

/// A dimension with free definition points.
#[derive(Debug, Clone, PartialEq)]
pub struct DimensionSpec {
    /// The kind.
    pub kind: DimensionKind,
    /// The definition points, as many as the kind needs.
    pub points: Vec<Point2>,
    /// A point of the dimension line.
    pub line_position: Point2,
    /// The rotation of a linear dimension in radians.
    pub rotation: f64,
    /// The text template (`{}` stands for the measurement).
    pub text_override: Option<String>,
    /// The text position when the file fixes it.
    pub text_position: Option<Point2>,
    /// The dimension style name.
    pub style: Option<String>,
}

/// One family of parallel pattern lines in the final frame of the hatch.
#[derive(Debug, Clone, PartialEq)]
pub struct FamilySpec {
    /// The angle in degrees.
    pub angle: f64,
    /// A point of the first line.
    pub origin: Point2,
    /// The displacement of consecutive lines along their direction.
    pub shift: f64,
    /// The distance of consecutive lines.
    pub spacing: f64,
    /// Dashes (positive), gaps (negative) and dots (zero).
    pub dashes: Vec<f64>,
}

/// A hatch.
#[derive(Debug, Clone, PartialEq)]
pub struct HatchSpec {
    /// Whether the fill is solid; otherwise the families draw it.
    pub solid: bool,
    /// The boundary loops.
    pub loops: Vec<Curve2>,
    /// The island mode: 0 alternating, 1 outermost, 2 ignore.
    pub island: u32,
    /// The inline pattern families.
    pub families: Vec<FamilySpec>,
}

/// A block reference.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertSpec {
    /// The block name.
    pub block: String,
    /// The position.
    pub position: Point2,
    /// The scale per axis.
    pub scale: [f64; 3],
    /// The rotation in radians.
    pub rotation: f64,
    /// Attribute values by tag.
    pub attributes: Vec<(String, String)>,
}

/// The kind data of an entity.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A point.
    Point(Point2),
    /// A line, ray, construction line, circle, arc, ellipse, elliptical arc or polyline.
    Curve(Curve2),
    /// A spline with the fit points it came from.
    Spline {
        /// The curve.
        curve: Nurbs2,
        /// The fit points, empty when the file has control points only.
        fit: Vec<Point2>,
    },
    /// A single-line text.
    Text(TextSpec),
    /// A multi-line text.
    RichText(RichTextSpec),
    /// A dimension.
    Dimension(DimensionSpec),
    /// A hatch.
    Hatch(HatchSpec),
    /// A wipeout.
    Wipeout(Polyline2),
    /// A block reference.
    Insert(InsertSpec),
}

/// An entity to create.
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySpec {
    /// The common fields.
    pub appearance: Appearance,
    /// The kind data.
    pub shape: Shape,
    /// The DXF handle of the source record.
    pub source: Option<Handle>,
}

/// A layer to create.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    /// The name.
    pub name: String,
    /// The color.
    pub color: ColorValue,
    /// The linetype name.
    pub linetype: Option<String>,
    /// The lineweight in millimeters.
    pub lineweight: Option<f64>,
    /// The transparency, 0 to 1.
    pub transparency: f64,
    /// Whether the layer is on.
    pub on: bool,
    /// Whether the layer is frozen.
    pub frozen: bool,
    /// Whether the layer is locked.
    pub locked: bool,
    /// Whether the layer plots.
    pub plottable: bool,
}

/// An element of a linetype pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementSpec {
    /// The kind.
    pub kind: ElementKind,
    /// The length, not negative.
    pub length: f64,
    /// The text of a text element.
    pub content: Option<String>,
    /// The text style name of a text or shape element.
    pub style: Option<String>,
    /// The scale, positive.
    pub scale: f64,
    /// The rotation in radians.
    pub rotation: f64,
    /// The offset from the pattern line.
    pub offset: Option<[f64; 2]>,
}

/// A linetype to create.
#[derive(Debug, Clone, PartialEq)]
pub struct LinetypeSpec {
    /// The name.
    pub name: String,
    /// The description.
    pub description: String,
    /// The elements.
    pub elements: Vec<ElementSpec>,
}

/// A text style to create.
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyleSpec {
    /// The name.
    pub name: String,
    /// The font name.
    pub font: Option<String>,
    /// The fixed height, 0 when not fixed.
    pub height: f64,
    /// The width factor, positive.
    pub width_factor: f64,
    /// The oblique angle in radians.
    pub oblique: f64,
    /// Whether the text is mirrored left to right.
    pub backwards: bool,
    /// Whether the text is upside down.
    pub upside_down: bool,
    /// Whether the text runs vertically.
    pub vertical: bool,
}

/// A dimension style to create: its name and the values of the fields the file's dimension
/// variables map to, by field name.
#[derive(Debug, Clone, PartialEq)]
pub struct DimStyleSpec {
    /// The name.
    pub name: String,
    /// `(field, value)` pairs; reals and integers.
    pub values: Vec<(&'static str, f64)>,
}

/// An attribute definition of a block.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeSpec {
    /// The tag.
    pub tag: String,
    /// The prompt.
    pub prompt: Option<String>,
    /// The default value.
    pub default: Option<String>,
    /// Whether the value is constant.
    pub constant: bool,
    /// Whether the attribute is invisible.
    pub invisible: bool,
    /// Whether the attribute is preset.
    pub preset: bool,
    /// The position in the block.
    pub position: Point2,
}

/// A block definition to create.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockSpec {
    /// The name.
    pub name: String,
    /// The base point.
    pub base: Point2,
    /// The unit of the block's coordinates.
    pub unit: SpaceUnit,
    /// The attribute definitions.
    pub attributes: Vec<AttributeSpec>,
}

/// Where an entity goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Container {
    /// The target model space.
    Space,
    /// The block definition of that name.
    Block(String),
}

/// One creation operation.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A linetype.
    Linetype(LinetypeSpec),
    /// A layer.
    Layer(LayerSpec),
    /// A text style.
    TextStyle(TextStyleSpec),
    /// A dimension style.
    DimStyle(DimStyleSpec),
    /// A block definition, without its entities.
    Block(BlockSpec),
    /// An entity of a space or block.
    Entity {
        /// The container.
        container: Container,
        /// The entity.
        spec: Box<EntitySpec>,
    },
}

/// The result of planning an import.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The operations in creation order: linetypes, layers, text styles, dimension styles,
    /// block definitions, then entities (blocks first, then the model space, each in file
    /// order).
    pub ops: Vec<Op>,
    /// The `$INSUNITS` code of the file.
    pub source_unit: i64,
    /// The factor applied to model space lengths.
    pub factor: f64,
    /// The loss report of the mapping, including the report of the reader when given to
    /// [`plan_with_report`].
    pub report: LossReport,
}

/// Plans the import of `drawing`.
#[must_use]
pub fn plan(drawing: &Drawing, options: &ImportOptions) -> Plan {
    let mut mapper = Mapper::new(drawing, options);
    mapper.run();
    mapper.finish(None)
}

/// Plans the import of `drawing` and merges `reader` (the report of reading it) into the
/// plan's report.
#[must_use]
pub fn plan_with_report(drawing: &Drawing, options: &ImportOptions, reader: &LossReport) -> Plan {
    let mut mapper = Mapper::new(drawing, options);
    mapper.run();
    mapper.finish(Some(reader))
}

pub(crate) const DIRECTION: Direction = Direction::Import;
