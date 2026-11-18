//! Display lists (CORE-DRAFT-029) and their assembly for a space or a sheet viewport.
//!
//! A display list is a sequence of exact [`Primitive`]s in draw order, each tagged with the
//! object it comes from. The list of one entity ([`EntityList`]) is derived data: the node
//! [`DISPLAY`] of the entity in the `arc-doc` dependency graph, registered by
//! [`register_evaluators`]. It reads the entity, its layer, its linetype and, for a block
//! reference, the definition, the definition's entities and their lists, so the graph rebuilds
//! exactly the lists whose inputs changed. [`space_list`] and [`viewport_list`] assemble the
//! list of a space or a viewport from the entity lists. The rules are stated in the crate
//! specification, section "Layouts, viewports and display lists as implemented".
//!
//! The types are neutral: a later part maps them to render items.

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_base::math::{Matrix3, Point2, Transform2};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Graph, NodeKey, StateKind,
    content_hash,
};
use arc_doc::relationship::ReferenceField;
use arc_doc::{ColorValue, Snapshot, TypeKey, Value};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Loop2, Polyline2};

use crate::appearance::{
    Level, Resolved, Setting, Source, Sourced, Style, default_color, layer_style_of, resolve,
    style_of,
};
use crate::clip::{Piece, cut, keeps, piece};
use crate::dimension::{DIMENSION, Resolution};
use crate::error::codes;
use crate::fonts::{Fonts, Substitutions, font_table_id};
use crate::layout::{Overrides, view};
use crate::leader::{LEADER, MULTILEADER};
use crate::linetype::{DEFAULT_MAX_DASHES, DashOptions, Dashes, Pattern, dashes};
use crate::text::{RICH_TEXT, TEXT};
use crate::text_layout::{
    Columns, HAlign, LayoutOptions, PositionedGlyph, RunKind, TextInput, TextLayout, TextStyle,
    VAlign,
};
use crate::types::{
    BLOCK, BLOCK_REF, ENTITIES, POINT, POLYLINE, SPACE, SpaceUnit, WIPEOUT, entity, layer,
};
use crate::validate::DEFAULT_MAX_NESTING;

/// The derived key of an entity's display list.
pub const DISPLAY: &str = "core.draft.display";

/// The names of the common entity fields, in declared order.
const COMMON: [&str; 16] = [
    "layer",
    "color_by",
    "color",
    "linetype_by",
    "linetype",
    "linetype_scale",
    "lineweight_by",
    "lineweight",
    "transparency_by",
    "transparency",
    "plot_style",
    "visible",
    "draw_order",
    "elevation",
    "thickness",
    "extrusion",
];

/// The names of the layer fields that display lists read, in declared order up to
/// `transparency`.
const LAYER_FIELDS: [&str; 5] = ["name", "color", "linetype", "lineweight", "transparency"];

/// The parameters of display-list building.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayOptions {
    /// The tolerance of dash generation, transforms and cuts.
    pub tolerance: Tolerance,
    /// The deepest nesting of block references; deeper lists fail with
    /// `draft.block.nesting_too_deep`.
    pub max_nesting: u32,
    /// The most primitives of one list; more fail with `draft.display.too_large`.
    pub max_primitives: usize,
    /// The bound of [`DashOptions::max_dashes`] per curve.
    pub max_dashes: usize,
    /// The fonts of text entities: the provider, the user's substitution table and the default
    /// stroke font.
    pub fonts: Fonts,
    /// The current annotation scale of [`space_list`]: annotative primitives of other scales
    /// are left out. Viewport lists use the viewport's annotation scale instead.
    pub annotation_scale: f64,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::DEFAULT,
            max_nesting: DEFAULT_MAX_NESTING,
            max_primitives: usize::try_from(Limits::DEFAULT.array_length).unwrap_or(usize::MAX),
            max_dashes: DEFAULT_MAX_DASHES,
            fonts: Fonts::default(),
            annotation_scale: 1.0,
        }
    }
}

/// The geometry of a primitive.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A point entity.
    Point(Point2),
    /// A curve: a line, ray, construction line, circle, arc, ellipse, elliptical arc or
    /// polyline with widths. `drawn` lists what is drawn when that is not the whole curve:
    /// the dashes of a linetype, or what a clip or wipeout left.
    Stroke {
        /// The whole curve.
        curve: Curve2,
        /// The drawn pieces; `None` draws the whole curve.
        drawn: Option<Vec<Piece>>,
    },
    /// A filled region bounded by a closed polyline.
    Fill(Polyline2),
    /// A wipeout mask: the region inside the closed polyline is painted in the background
    /// color.
    Mask(Polyline2),
    /// Outline text glyphs for the renderer and plot stream: glyph positions in text space,
    /// mapped to the list's coordinates by `placement`.
    Glyphs {
        /// The family name the font provider registered the font under.
        family: String,
        /// The face index.
        index: u32,
        /// The em size in text space.
        size: f64,
        /// The glyphs.
        glyphs: Vec<PositionedGlyph>,
        /// Text space to the list's coordinates.
        placement: Transform2,
    },
}

/// One primitive with its resolved appearance and its source.
///
/// An appearance value is `None` while it waits for an inserting block reference (set to
/// `block` in a block definition); the lists of spaces resolve every value.
#[derive(Debug, Clone, PartialEq)]
pub struct Primitive {
    /// The geometry.
    pub shape: Shape,
    /// The color.
    pub color: Option<Sourced<ColorValue>>,
    /// The linetype; `None` inside is continuous. The dashes are already in `shape`.
    pub linetype: Option<Sourced<Option<ObjectId>>>,
    /// The lineweight in millimeters; `None` inside is the default lineweight.
    pub lineweight: Option<Sourced<Option<f64>>>,
    /// The transparency.
    pub transparency: Option<Sourced<f64>>,
    /// The drawing entity's `linetype_scale`.
    pub linetype_scale: f64,
    /// Whether a polyline runs one pattern across its vertices.
    pub continuous: bool,
    /// The layers of the drawing entity and of every block reference inserting it, innermost
    /// first.
    pub layers: Vec<ObjectId>,
    /// The draw-order key of the outermost entity in its container.
    pub draw_order: i64,
    /// The outermost entity: the entity in the list's space or block definition.
    pub source: ObjectId,
    /// The sub-object key: the nested entities from below `source` down to the drawing entity,
    /// outermost first; empty when `source` draws it.
    pub path: Vec<ObjectId>,
    /// The part of the drawing entity: 0 for its shape, 1 for the frame of a wipeout; for
    /// dimensions and leaders the code of [`crate::annotation::Part`].
    pub part: u32,
    /// The annotation scale of the representation the primitive belongs to; `None` for a
    /// primitive shown at every scale.
    pub annotation_scale: Option<f64>,
}

/// The display list of one entity, the result of its [`DISPLAY`] node.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityList {
    /// The entity's draw-order key in its container.
    pub order: i64,
    /// The deepest nesting of block references below the entity: 0 for other entities.
    pub depth: u32,
    /// The primitives in draw order, in the coordinates of the entity's container.
    pub primitives: Vec<Primitive>,
}

impl Derived for EntityList {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text (floats print round-trip); a `Hash` impl over
        // the primitives would avoid the allocation when profiles show it matters.
        content_hash(&format!("{self:?}"))
    }
}

/// An assembled display list of a space or viewport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    /// The primitives in draw order.
    pub primitives: Vec<Primitive>,
    /// The clip boundary of a viewport list on the sheet. Curves and points are already cut
    /// to it; fills and masks that cross it are kept whole for the consumer to clip.
    pub clip: Option<Polyline2>,
    /// The entities whose list failed, or whose primitives could not be cut or transformed;
    /// they are left out.
    pub failed: Vec<ObjectId>,
}

/// What building a list reads: the dependency graph, or a snapshot directly.
pub(crate) trait Reader {
    fn field(&self, id: ObjectId, name: &str) -> Result<Option<Value>>;
    fn type_key(&self, id: ObjectId) -> Option<String>;
    fn owner(&self, id: ObjectId) -> Result<Option<ObjectId>>;
    fn members(&self, container: ObjectId) -> Vec<ObjectId>;
    fn list(&self, id: ObjectId) -> Result<DerivedValue>;
    /// The dimension styles whose `parent` is `style`.
    fn children(&self, style: ObjectId) -> Vec<ObjectId>;
    /// The resolution of the dimension `id`.
    fn resolved(&self, id: ObjectId) -> Result<Resolution>;
}

pub(crate) struct GraphReader<'a, 'b> {
    pub(crate) ctx: &'a EvalContext<'b>,
}

fn parent_field() -> ReferenceField {
    ReferenceField::Field(arc_doc::FieldPath::new("parent"))
}

impl Reader for GraphReader<'_, '_> {
    fn field(&self, id: ObjectId, name: &str) -> Result<Option<Value>> {
        Ok(self.ctx.field(id, name)?.cloned())
    }
    fn type_key(&self, id: ObjectId) -> Option<String> {
        self.ctx.type_key(id).map(|k| k.as_str().to_owned())
    }
    fn owner(&self, id: ObjectId) -> Result<Option<ObjectId>> {
        Ok(self.ctx.header(id)?.owner)
    }
    fn members(&self, container: ObjectId) -> Vec<ObjectId> {
        let owned: Vec<ObjectId> = self
            .ctx
            .referrers(container)
            .filter(|r| r.field == ReferenceField::Owner)
            .map(|r| r.source)
            .collect();
        owned
            .into_iter()
            .filter(|id| {
                self.type_key(*id)
                    .is_some_and(|k| ENTITIES.contains(&k.as_str()))
            })
            .collect()
    }
    fn list(&self, id: ObjectId) -> Result<DerivedValue> {
        self.ctx.node(&node(id)?)
    }
    fn children(&self, style: ObjectId) -> Vec<ObjectId> {
        let field = parent_field();
        self.ctx
            .referrers(style)
            .filter(|r| r.field == field)
            .map(|r| r.source)
            .collect()
    }
    fn resolved(&self, id: ObjectId) -> Result<Resolution> {
        let value = self.ctx.node(&crate::dimension::resolved_node(id)?)?;
        value
            .downcast_ref::<Resolution>()
            .cloned()
            .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))
    }
}

struct SnapshotReader<'a> {
    snapshot: &'a Snapshot,
    options: &'a DisplayOptions,
    depth: u32,
}

impl Reader for SnapshotReader<'_> {
    fn field(&self, id: ObjectId, name: &str) -> Result<Option<Value>> {
        let handle = self.snapshot.resolve(id)?;
        let key = self.type_key(id).unwrap_or_default();
        let object = self.snapshot.get(handle)?;
        let position = self
            .snapshot
            .registry()
            .get(&key)
            .and_then(|decl| decl.schema.fields.iter().position(|f| f.name == name));
        Ok(position.and_then(|p| object.values.get(p).cloned().flatten()))
    }
    fn type_key(&self, id: ObjectId) -> Option<String> {
        let handle = self.snapshot.resolve(id).ok()?;
        self.snapshot
            .type_key(handle)
            .map(|k| k.as_str().to_owned())
    }
    fn owner(&self, id: ObjectId) -> Result<Option<ObjectId>> {
        Ok(self.snapshot.get(self.snapshot.resolve(id)?)?.header.owner)
    }
    fn members(&self, container: ObjectId) -> Vec<ObjectId> {
        members(self.snapshot, container)
    }
    fn list(&self, id: ObjectId) -> Result<DerivedValue> {
        // A cycle or a chain deeper than the bound stops here instead of recursing.
        if self.depth > self.options.max_nesting {
            return Err(nesting(self.options.max_nesting));
        }
        let inner = SnapshotReader {
            depth: self.depth + 1,
            ..*self
        };
        Ok(Arc::new(build(&inner, id, self.options)?))
    }
    fn children(&self, style: ObjectId) -> Vec<ObjectId> {
        let field = parent_field();
        self.snapshot
            .referrers(style)
            .filter(|r| r.field == field)
            .map(|r| r.source)
            .collect()
    }
    fn resolved(&self, id: ObjectId) -> Result<Resolution> {
        crate::dimension::resolve_with(self, id, &self.options.tolerance)
    }
}

/// Runs `f` with a reader of `snapshot`.
pub(crate) fn with_snapshot<R>(
    snapshot: &Snapshot,
    options: &DisplayOptions,
    f: impl FnOnce(&dyn Reader) -> R,
) -> R {
    f(&SnapshotReader {
        snapshot,
        options,
        depth: 0,
    })
}

fn nesting(limit: u32) -> Error {
    Error::new(codes::BLOCK_NESTING_TOO_DEEP).with_arg("limit", i64::from(limit))
}

/// The entities a space or block definition contains, in no particular order.
fn members(snapshot: &Snapshot, container: ObjectId) -> Vec<ObjectId> {
    snapshot
        .referrers(container)
        .filter(|r| r.field == ReferenceField::Owner)
        .filter(|r| {
            snapshot
                .resolve(r.source)
                .ok()
                .and_then(|h| snapshot.type_key(h))
                .is_some_and(|k| ENTITIES.contains(&k.as_str()))
        })
        .map(|r| r.source)
        .collect()
}

/// The node of the display list of `entity`.
///
/// # Errors
///
/// None for the constant key; the key check of `arc-doc` is passed on.
pub fn node(entity: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(entity, DerivedKey::new(DISPLAY)?))
}

pub(crate) fn read(
    reader: &dyn Reader,
    id: ObjectId,
    names: &[&str],
) -> Result<Vec<Option<Value>>> {
    names.iter().map(|name| reader.field(id, name)).collect()
}

fn reference(value: Option<&Option<Value>>) -> Option<ObjectId> {
    match value {
        Some(Some(Value::Reference(id))) => Some(*id),
        _ => None,
    }
}

fn number(value: Option<Value>) -> Option<f64> {
    match value {
        Some(Value::F64(x)) => Some(x),
        _ => None,
    }
}

fn pattern(reader: &dyn Reader, linetype: ObjectId) -> Result<Pattern> {
    let elements = reader.field(linetype, "elements")?;
    Pattern::from_values(&[None, None, elements])
}

/// The drawn pieces of `curve` with `linetype`; `None` is solid.
fn dash(
    reader: &dyn Reader,
    curve: &Curve2,
    linetype: Option<ObjectId>,
    primitive: (f64, bool),
    options: &DisplayOptions,
) -> Result<Option<Vec<Piece>>> {
    let Some(linetype) = linetype else {
        return Ok(None);
    };
    let dash_options = DashOptions {
        global_scale: 1.0,
        object_scale: primitive.0,
        continuous: primitive.1,
        max_dashes: options.max_dashes,
    };
    match dashes(
        curve,
        &pattern(reader, linetype)?,
        &dash_options,
        &options.tolerance,
    )? {
        Dashes::Continuous => Ok(None),
        Dashes::Pieces(list) => Ok(Some(
            list.iter()
                // A dash too small to construct has no extent to draw.
                .filter_map(|d| piece(curve, d.t0, d.t1).ok())
                .collect(),
        )),
    }
}

/// The appearance of one level, with values set to `block` left open.
pub(crate) struct Look {
    resolved: Resolved,
    style: Style,
    layer: Option<ObjectId>,
}

impl Look {
    fn of(reader: &dyn Reader, values: &[Option<Value>]) -> Result<Self> {
        let style = style_of(values);
        let layer = reference(values.get(entity::LAYER));
        let layer_style = match layer {
            Some(l) => Some(layer_style_of(l, &read(reader, l, &LAYER_FIELDS)?)),
            None => None,
        };
        let resolved = resolve(
            &[Level {
                style: &style,
                layer: layer_style.as_ref(),
            }],
            &std::collections::BTreeMap::new(),
        );
        Ok(Self {
            resolved,
            style,
            layer,
        })
    }

    fn open<T: Clone>(setting: &Setting<impl Sized>, value: &Sourced<T>) -> Option<Sourced<T>> {
        match setting {
            Setting::Block => None,
            _ => Some(value.clone()),
        }
    }

    pub(crate) fn color(&self) -> Option<Sourced<ColorValue>> {
        Self::open(&self.style.color, &self.resolved.color)
    }
    pub(crate) fn linetype(&self) -> Option<Sourced<Option<ObjectId>>> {
        Self::open(&self.style.linetype, &self.resolved.linetype)
    }
    pub(crate) fn lineweight(&self) -> Option<Sourced<Option<f64>>> {
        Self::open(&self.style.lineweight, &self.resolved.lineweight)
    }
    pub(crate) fn transparency(&self) -> Option<Sourced<f64>> {
        Self::open(&self.style.transparency, &self.resolved.transparency)
    }
}

/// Gives every value still open its default: past the outermost level, `block` is the default.
fn close(primitive: &mut Primitive) {
    fn default<T>(value: T) -> Sourced<T> {
        Sourced {
            value,
            source: Source::Default,
        }
    }
    primitive
        .color
        .get_or_insert_with(|| default(default_color()));
    primitive.linetype.get_or_insert_with(|| default(None));
    primitive.lineweight.get_or_insert_with(|| default(None));
    primitive.transparency.get_or_insert_with(|| default(0.0));
}

pub(crate) fn decode_curve(value: Option<&Option<Value>>) -> Result<Curve2> {
    match value {
        Some(Some(Value::Geometry(bytes))) => Curve2::decode(&bytes.0, &Limits::DEFAULT)
            .map_err(|e| Error::new(codes::GEOMETRY_INVALID).with_source(e)),
        _ => Err(Error::new(codes::GEOMETRY_INVALID)),
    }
}

fn transform_curve(curve: &Curve2, t: &Transform2, tol: &Tolerance) -> Result<Curve2> {
    curve.transformed(t, tol)
}

fn transform_polyline(p: &Polyline2, t: &Transform2, tol: &Tolerance) -> Result<Polyline2> {
    match Curve2::Polyline(p.clone()).transformed(t, tol)? {
        Curve2::Polyline(q) => Ok(q),
        _ => Err(Error::new(codes::GEOMETRY_INVALID)),
    }
}

fn transform_piece(p: &Piece, t: &Transform2, tol: &Tolerance) -> Result<Piece> {
    Ok(match p {
        Piece::Point(q) => Piece::Point(t * q),
        Piece::Curve(c) => Piece::Curve(transform_curve(c, t, tol)?),
    })
}

fn transform_shape(shape: &Shape, t: &Transform2, tol: &Tolerance) -> Result<Shape> {
    Ok(match shape {
        Shape::Point(p) => Shape::Point(t * p),
        Shape::Stroke { curve, drawn } => Shape::Stroke {
            curve: transform_curve(curve, t, tol)?,
            drawn: match drawn {
                Some(pieces) => Some(
                    pieces
                        .iter()
                        .map(|p| transform_piece(p, t, tol))
                        .collect::<Result<_>>()?,
                ),
                None => None,
            },
        },
        Shape::Fill(p) => Shape::Fill(transform_polyline(p, t, tol)?),
        Shape::Mask(p) => Shape::Mask(transform_polyline(p, t, tol)?),
        Shape::Glyphs {
            family,
            index,
            size,
            glyphs,
            placement,
        } => Shape::Glyphs {
            family: family.clone(),
            index: *index,
            size: *size,
            glyphs: glyphs.clone(),
            placement: t * placement,
        },
    })
}

fn is_space(reader: &dyn Reader, container: Option<ObjectId>) -> bool {
    container.and_then(|c| reader.type_key(c)).as_deref() == Some(SPACE)
}

/// The list of one entity, as described in the crate specification.
#[expect(clippy::too_many_lines, reason = "one match over every entity type")]
fn build(reader: &dyn Reader, id: ObjectId, options: &DisplayOptions) -> Result<EntityList> {
    let key = reader
        .type_key(id)
        .ok_or_else(|| Error::new(arc_doc::error::codes::OBJECT_NOT_FOUND))?;
    if !ENTITIES.contains(&key.as_str()) {
        return Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", id.to_string()));
    }
    let mut values = read(reader, id, &COMMON)?;
    let order = match values.get(entity::DRAW_ORDER) {
        Some(Some(Value::I64(k))) => *k,
        _ => 0,
    };
    let mut list = EntityList {
        order,
        depth: 0,
        primitives: Vec::new(),
    };
    if matches!(values.get(entity::VISIBLE), Some(Some(Value::Bool(false)))) {
        return Ok(list);
    }
    let look = Look::of(reader, &values)?;
    let top = is_space(reader, reader.owner(id)?);
    let scale = number(values.get(entity::LINETYPE_SCALE).cloned().flatten()).unwrap_or(1.0);
    let base = Primitive {
        shape: Shape::Point(Point2::origin()),
        color: look.color(),
        linetype: look.linetype(),
        lineweight: look.lineweight(),
        transparency: look.transparency(),
        linetype_scale: scale,
        continuous: false,
        layers: look.layer.into_iter().collect(),
        draw_order: order,
        source: id,
        path: Vec::new(),
        part: 0,
        annotation_scale: None,
    };
    let linetype = base.linetype.as_ref().and_then(|l| l.value);
    match key.as_str() {
        BLOCK_REF => {
            values.extend(read(
                reader,
                id,
                &["block", "position", "scale", "rotation"],
            )?);
            return reference_list(reader, &values, base, &look, top, options);
        }
        POINT => {
            if let Some(Value::Point2(p)) = reader.field(id, "position")? {
                list.primitives.push(Primitive {
                    shape: Shape::Point(p),
                    ..base
                });
            }
        }
        TEXT | RICH_TEXT => {
            let layout = text_layout_of(reader, id, &key, options)?;
            let placement = text_placement(reader, id)?;
            list.primitives = text_primitives(&layout, &placement, &base, &options.tolerance)?;
        }
        DIMENSION | LEADER | MULTILEADER => {
            list.primitives =
                crate::annotation_display::primitives(reader, id, &key, &base, &look, options)?;
        }
        WIPEOUT => {
            let Curve2::Polyline(boundary) = decode_curve(Some(&reader.field(id, "geometry")?))?
            else {
                return Err(Error::new(codes::GEOMETRY_INVALID));
            };
            let frame = matches!(reader.field(id, "frame")?, Some(Value::Bool(true)));
            let curve = Curve2::Polyline(boundary.clone());
            list.primitives.push(Primitive {
                shape: Shape::Mask(boundary),
                ..base.clone()
            });
            if frame {
                let drawn = dash(reader, &curve, linetype, (scale, false), options)?;
                list.primitives.push(Primitive {
                    shape: Shape::Stroke { curve, drawn },
                    part: 1,
                    ..base
                });
            }
        }
        _ => {
            let curve = decode_curve(Some(&reader.field(id, "geometry")?))?;
            let continuous = key == POLYLINE
                && matches!(
                    reader.field(id, "continuous_pattern")?,
                    Some(Value::Bool(true))
                );
            let drawn = dash(reader, &curve, linetype, (scale, continuous), options)?;
            list.primitives.push(Primitive {
                shape: Shape::Stroke { curve, drawn },
                continuous,
                ..base
            });
        }
    }
    if top {
        list.primitives.iter_mut().for_each(close);
    }
    Ok(list)
}

pub(crate) fn read_number(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<Option<f64>> {
    Ok(number(reader.field(id, name)?))
}

pub(crate) fn read_enum(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<u32> {
    Ok(match reader.field(id, name)? {
        Some(Value::Enum(code)) => code,
        _ => 0,
    })
}

pub(crate) fn read_flag(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<bool> {
    Ok(matches!(reader.field(id, name)?, Some(Value::Bool(true))))
}

/// The style values of a text and its placement flags: (style, height, backwards, upside down).
fn text_style(reader: &dyn Reader, id: ObjectId) -> Result<(TextStyle, f64, bool, bool)> {
    let own = read_number(reader, id, "height")?.unwrap_or(0.0);
    let Some(Value::Reference(style)) = reader.field(id, "style")? else {
        let height = if own > 0.0 {
            own
        } else {
            crate::text::DEFAULT_HEIGHT
        };
        return Ok((TextStyle::default(), height, false, false));
    };
    let font = match reader.field(style, "font")? {
        Some(Value::String(s)) => Some(s),
        _ => None,
    };
    let style_height = read_number(reader, style, "height")?.unwrap_or(0.0);
    let height = if own > 0.0 {
        own
    } else if style_height > 0.0 {
        style_height
    } else {
        crate::text::DEFAULT_HEIGHT
    };
    Ok((
        TextStyle {
            font,
            width_factor: read_number(reader, style, "width_factor")?.unwrap_or(1.0),
            oblique: read_number(reader, style, "oblique")?.unwrap_or(0.0),
        },
        height,
        read_flag(reader, style, "backwards")?,
        read_flag(reader, style, "upside_down")?,
    ))
}

/// The layout of the text entity `id` of type `key`, reading the entity, its style and the
/// document's substitution table through `reader`.
fn text_layout_of(
    reader: &dyn Reader,
    id: ObjectId,
    key: &str,
    options: &DisplayOptions,
) -> Result<TextLayout> {
    let (style, height, _, _) = text_style(reader, id)?;
    let paragraphs;
    let mut input = TextInput {
        paragraphs: &[],
        height,
        width: 0.0,
        halign: HAlign::Left,
        valign: VAlign::Baseline,
        columns: Columns::default(),
        style: &style,
    };
    if key == TEXT {
        let content = match reader.field(id, "content")? {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        paragraphs = vec![crate::text::Paragraph {
            runs: vec![crate::text::Run {
                text: content,
                ..crate::text::Run::default()
            }],
            ..crate::text::Paragraph::default()
        }];
        input.halign = match read_enum(reader, id, "halign")? {
            1 => HAlign::Center,
            2 => HAlign::Right,
            _ => HAlign::Left,
        };
        input.valign = match read_enum(reader, id, "valign")? {
            1 => VAlign::Bottom,
            2 => VAlign::Middle,
            3 => VAlign::Top,
            _ => VAlign::Baseline,
        };
    } else {
        let stored = reader.field(id, "paragraphs")?;
        paragraphs = match &stored {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(crate::text::Paragraph::from_value)
                .collect(),
            _ => Vec::new(),
        };
        input.width = read_number(reader, id, "width")?.unwrap_or(0.0);
        let attachment = read_enum(reader, id, "attachment")?;
        input.halign = match attachment % 3 {
            1 => HAlign::Center,
            2 => HAlign::Right,
            _ => HAlign::Left,
        };
        input.valign = match attachment / 3 {
            1 => VAlign::Middle,
            2 => VAlign::Bottom,
            _ => VAlign::Top,
        };
        let count = match reader.field(id, "columns")? {
            Some(Value::I64(n)) => u32::try_from(n.clamp(1, crate::text::MAX_COLUMNS)).unwrap_or(1),
            _ => 1,
        };
        input.columns = Columns {
            count,
            gutter: read_number(reader, id, "column_gutter")?.unwrap_or(0.0),
            height: read_number(reader, id, "column_height")?.unwrap_or(0.0),
        };
    }
    input.paragraphs = &paragraphs;
    lay_out(reader, &input, options)
}

/// Lays out `input` with the document's substitution table, read through `reader`, and the
/// fonts of `options`.
pub(crate) fn lay_out(
    reader: &dyn Reader,
    input: &TextInput<'_>,
    options: &DisplayOptions,
) -> Result<TextLayout> {
    // A document without a table depends on its creation through the failed read.
    let table = match reader.field(font_table_id(), "entries") {
        Ok(entries) => Substitutions::from_value(entries.as_ref()),
        Err(_) => Substitutions::default(),
    };
    crate::text_layout::layout(
        input,
        &table,
        &options.fonts,
        &LayoutOptions {
            tolerance: options.tolerance,
            max_items: options.max_primitives,
        },
    )
}

/// The style values of the text style `style` (the defaults without one).
pub(crate) fn text_style_of(reader: &dyn Reader, style: Option<ObjectId>) -> Result<TextStyle> {
    let Some(style) = style else {
        return Ok(TextStyle::default());
    };
    Ok(TextStyle {
        font: match reader.field(style, "font")? {
            Some(Value::String(s)) => Some(s),
            _ => None,
        },
        width_factor: read_number(reader, style, "width_factor")?.unwrap_or(1.0),
        oblique: read_number(reader, style, "oblique")?.unwrap_or(0.0),
    })
}

/// The map of a text from text space to its container: translation to `position`, rotation,
/// and the mirror flags of its style.
fn text_placement(reader: &dyn Reader, id: ObjectId) -> Result<Transform2> {
    let (_, _, backwards, upside_down) = text_style(reader, id)?;
    let position = match reader.field(id, "position")? {
        Some(Value::Point2(p)) => p,
        _ => Point2::origin(),
    };
    let rotation = read_number(reader, id, "rotation")?.unwrap_or(0.0);
    let (sin, cos) = rotation.sin_cos();
    let fx = if backwards { -1.0 } else { 1.0 };
    let fy = if upside_down { -1.0 } else { 1.0 };
    Ok(Transform2::from_matrix_unchecked(Matrix3::new(
        cos * fx,
        -sin * fy,
        position.x,
        sin * fx,
        cos * fy,
        position.y,
        0.0,
        0.0,
        1.0,
    )))
}

/// The primitives of a laid-out text mapped by `placement`: one stroke per curve of a stroke
/// run, one glyph primitive per outline run. Text strokes are continuous whatever the
/// linetype; a run with its own color takes it with the source `Object`.
pub(crate) fn text_primitives(
    layout: &TextLayout,
    placement: &Transform2,
    base: &Primitive,
    tolerance: &Tolerance,
) -> Result<Vec<Primitive>> {
    let placement = *placement;
    let continuous = Some(Sourced {
        value: None,
        source: Source::Object,
    });
    let mut out = Vec::new();
    for run in &layout.runs {
        let color = match run.color {
            Some(rgba) => Some(Sourced {
                value: ColorValue { rgba, source: None },
                source: Source::Object,
            }),
            None => base.color.clone(),
        };
        let template = Primitive {
            color,
            linetype: continuous.clone(),
            ..base.clone()
        };
        match &run.kind {
            RunKind::Stroke(curves) => {
                for curve in curves {
                    out.push(Primitive {
                        shape: Shape::Stroke {
                            curve: transform_curve(curve, &placement, tolerance)?,
                            drawn: None,
                        },
                        ..template.clone()
                    });
                }
            }
            RunKind::Outline {
                family,
                index,
                size,
                glyphs,
            } => out.push(Primitive {
                shape: Shape::Glyphs {
                    family: family.clone(),
                    index: *index,
                    size: *size,
                    glyphs: glyphs.clone(),
                    placement,
                },
                ..template
            }),
        }
    }
    Ok(out)
}

/// The layout of the text entity `entity` in `snapshot` with the fonts of `options`: the
/// layout its display list draws, for plot and export.
///
/// # Errors
///
/// `doc.object.not_found`; `draft.object.wrong_type` for an object that is not a text; the
/// errors of [`crate::text_layout::layout`].
pub fn text_layout(
    snapshot: &Snapshot,
    entity: ObjectId,
    options: &DisplayOptions,
) -> Result<TextLayout> {
    let reader = SnapshotReader {
        snapshot,
        options,
        depth: 0,
    };
    let key = reader
        .type_key(entity)
        .ok_or_else(|| Error::new(arc_doc::error::codes::OBJECT_NOT_FOUND))?;
    if key != TEXT && key != RICH_TEXT {
        return Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", entity.to_string()));
    }
    text_layout_of(&reader, entity, &key, options)
}

/// The map of a block reference: position · rotation · scale (with the unit factor of the
/// definition to the container) · translation by the negated base point.
fn reference_transform(
    reader: &dyn Reader,
    values: &[Option<Value>],
    block: ObjectId,
    container: Option<ObjectId>,
) -> Result<Transform2> {
    let at = |i| values.get(i).cloned().flatten();
    let position = match at(entity::REF_POSITION) {
        Some(Value::Point2(p)) => p,
        _ => Point2::origin(),
    };
    let (sx, sy) = match at(entity::REF_SCALE) {
        Some(Value::Vector3(v)) => (v.x, v.y),
        _ => (1.0, 1.0),
    };
    let rotation = number(at(entity::REF_ROTATION)).unwrap_or(0.0);
    let base = match reader.field(block, "base_point")? {
        Some(Value::Point2(p)) => p,
        _ => Point2::origin(),
    };
    let unit = |id: Option<ObjectId>| -> Result<SpaceUnit> {
        Ok(match id {
            Some(id) => match reader.field(id, "unit")? {
                Some(Value::Enum(code)) => SpaceUnit::of(code).unwrap_or(SpaceUnit::Unitless),
                _ => SpaceUnit::Unitless,
            },
            None => SpaceUnit::Unitless,
        })
    };
    let factor = unit(Some(block))?.factor_to(unit(container)?);
    let (sin, cos) = rotation.sin_cos();
    let (fx, fy) = (sx * factor, sy * factor);
    let (a, b, c, d) = (cos * fx, -sin * fy, sin * fx, cos * fy);
    let tx = position.x - (a * base.x + b * base.y);
    let ty = position.y - (c * base.x + d * base.y);
    Ok(Transform2::from_matrix_unchecked(Matrix3::new(
        a, b, tx, c, d, ty, 0.0, 0.0, 1.0,
    )))
}

/// The block definition the block reference `id` inserts and the map of the reference.
pub(crate) fn block_ref_transform(
    reader: &dyn Reader,
    id: ObjectId,
) -> Result<(ObjectId, Transform2)> {
    if reader.type_key(id).as_deref() != Some(BLOCK_REF) {
        return Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", id.to_string()));
    }
    let mut values = vec![None; entity::KIND];
    values.extend(read(
        reader,
        id,
        &["block", "position", "scale", "rotation"],
    )?);
    let block = reference(values.get(entity::KIND))
        .ok_or_else(|| Error::new(codes::FIELD_INVALID).with_arg("field", "block"))?;
    let map = reference_transform(reader, &values, block, reader.owner(id)?)?;
    Ok((block, map))
}

/// Fills an open value of an inserted primitive with the reference's own value: a value of
/// the reference itself becomes the source `Block { level }`.
fn fill<T: Clone>(slot: &mut Option<Sourced<T>>, own: Option<Sourced<T>>, level: usize) {
    if slot.is_none() {
        *slot = own.map(|s| Sourced {
            source: match s.source {
                Source::Object => Source::Block { level },
                other => other,
            },
            value: s.value,
        });
    }
}

pub(crate) fn reference_list(
    reader: &dyn Reader,
    values: &[Option<Value>],
    base: Primitive,
    look: &Look,
    top: bool,
    options: &DisplayOptions,
) -> Result<EntityList> {
    let id = base.source;
    let block = reference(values.get(entity::KIND))
        .ok_or_else(|| Error::new(codes::FIELD_INVALID).with_arg("field", "block"))?;
    if reader.type_key(block).as_deref() != Some(BLOCK) {
        return Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", block.to_string()));
    }
    let transform = reference_transform(reader, values, block, reader.owner(id)?)?;
    let mut children = Vec::new();
    for member in reader.members(block) {
        children.push((member, reader.list(member)?));
    }
    let mut lists = Vec::with_capacity(children.len());
    for (member, value) in &children {
        let list = value
            .downcast_ref::<EntityList>()
            .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))?;
        lists.push((list.order, *member, list));
    }
    lists.sort_by_key(|(order, member, _)| (*order, *member));
    let depth = lists
        .iter()
        .map(|(_, _, l)| l.depth)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    if depth > options.max_nesting {
        return Err(nesting(options.max_nesting));
    }
    let count: usize = lists.iter().map(|(_, _, l)| l.primitives.len()).sum();
    if count > options.max_primitives {
        return Err(Error::new(codes::DISPLAY_TOO_LARGE).with_arg(
            "limit",
            i64::try_from(options.max_primitives).unwrap_or(i64::MAX),
        ));
    }
    let own_linetype = look.linetype();
    let mut primitives = Vec::with_capacity(count);
    for (_, member, list) in lists {
        for child in &list.primitives {
            let mut p = child.clone();
            p.path.insert(0, member);
            let level = p.path.len();
            if p.linetype.is_none()
                && let Some(lt) = &own_linetype
                && let Shape::Stroke { curve, drawn } = &mut p.shape
            {
                *drawn = dash(
                    reader,
                    curve,
                    lt.value,
                    (p.linetype_scale, p.continuous),
                    options,
                )?;
            }
            fill(&mut p.color, look.color(), level);
            fill(&mut p.linetype, own_linetype.clone(), level);
            fill(&mut p.lineweight, look.lineweight(), level);
            fill(&mut p.transparency, look.transparency(), level);
            p.shape = transform_shape(&p.shape, &transform, &options.tolerance)?;
            p.source = id;
            p.draw_order = base.draw_order;
            p.layers.extend(look.layer);
            if top {
                close(&mut p);
            }
            primitives.push(p);
        }
    }
    Ok(EntityList {
        order: base.draw_order,
        depth,
        primitives,
    })
}

/// The evaluator of [`DISPLAY`] with `options`.
pub fn evaluator(
    options: DisplayOptions,
) -> impl Fn(&EvalContext<'_>) -> Result<DerivedValue> + Send + Sync + Clone + 'static {
    move |ctx| {
        let id = ctx
            .object_id()
            .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))?;
        let list = build(&GraphReader { ctx }, id, &options)?;
        Ok(Arc::new(list) as DerivedValue)
    }
}

/// Registers the [`DISPLAY`] evaluator for every entity type.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when one is registered already.
pub fn register_evaluators(evaluators: &mut Evaluators, options: DisplayOptions) -> Result<()> {
    for key in ENTITIES {
        evaluators.register_object(
            TypeKey::new(*key)?,
            DerivedKey::new(DISPLAY)?,
            evaluator(options.clone()),
        )?;
    }
    crate::dimension::register_evaluators(evaluators, &options)
}

/// The list of `entity` evaluated directly from `snapshot`, without the graph: the same rules
/// as its [`DISPLAY`] node.
///
/// # Errors
///
/// As the node: `doc.object.not_found`, `draft.object.wrong_type`, `draft.geometry.invalid`,
/// `draft.block.nesting_too_deep`, `draft.display.too_large`, and the errors of dash
/// generation and transforms.
pub fn entity_list(
    snapshot: &Snapshot,
    entity: ObjectId,
    options: &DisplayOptions,
) -> Result<EntityList> {
    build(
        &SnapshotReader {
            snapshot,
            options,
            depth: 0,
        },
        entity,
        options,
    )
}

/// Where assembly takes entity lists from.
#[derive(Debug, Clone, Copy)]
pub enum Lists<'a> {
    /// The nodes of the graph, evaluated when not clean; the graph's snapshot must be the one
    /// passed to the assembly.
    Graph(&'a Graph),
    /// Direct evaluation with [`entity_list`].
    Direct,
}

fn layer_hidden(snapshot: &Snapshot, layer: ObjectId) -> bool {
    snapshot
        .resolve(layer)
        .ok()
        .and_then(|h| snapshot.get(h).ok())
        .is_some_and(|o| {
            matches!(o.values.get(layer::ON), Some(Some(Value::Bool(false))))
                || matches!(o.values.get(layer::FROZEN), Some(Some(Value::Bool(true))))
        })
}

/// The visible primitives of the entities of `space`, in draw order.
fn gather(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    space: ObjectId,
    frozen: &BTreeSet<ObjectId>,
    scale: f64,
    options: &DisplayOptions,
    out: &mut DisplayList,
) -> Result<Vec<Primitive>> {
    let mut entries = Vec::new();
    for member in members(snapshot, space) {
        let list: Option<DerivedValue> = match lists {
            Lists::Graph(graph) => graph
                .evaluate_now(&node(member)?)
                .ok()
                .filter(|v| v.state.kind() == StateKind::Clean)
                .and_then(|v| v.result),
            Lists::Direct => entity_list(snapshot, member, options)
                .ok()
                .map(|l| Arc::new(l) as DerivedValue),
        };
        match list {
            Some(value) if value.downcast_ref::<EntityList>().is_some() => {
                entries.push((member, value));
            }
            _ => out.failed.push(member),
        }
    }
    let mut ordered: Vec<(i64, ObjectId, &EntityList)> = entries
        .iter()
        .filter_map(|(id, v)| v.downcast_ref::<EntityList>().map(|l| (l.order, *id, l)))
        .collect();
    ordered.sort_by_key(|(order, id, _)| (*order, *id));
    let mut hidden = std::collections::BTreeMap::new();
    let mut is_hidden = |layer: ObjectId| {
        *hidden
            .entry(layer)
            .or_insert_with(|| frozen.contains(&layer) || layer_hidden(snapshot, layer))
    };
    let mut primitives = Vec::new();
    for (_, _, list) in ordered {
        for p in &list.primitives {
            // Annotation scales match exactly: both are the stored values.
            #[expect(clippy::float_cmp, reason = "stored scales are compared exactly")]
            let shown = p.annotation_scale.is_none_or(|s| s == scale);
            if shown && !p.layers.iter().any(|l| is_hidden(*l)) {
                primitives.push(p.clone());
            }
        }
    }
    Ok(primitives)
}

fn boundary_loop(boundary: &Polyline2, tol: &Tolerance) -> Result<Loop2> {
    Loop2::new(vec![Curve2::Polyline(boundary.clone())], tol)
}

/// Cuts the curves and points of `p` to `boundary`; `false` when nothing is left.
fn cut_primitive(
    p: &mut Primitive,
    boundary: &Loop2,
    inside: bool,
    tol: &Tolerance,
) -> Result<bool> {
    match &mut p.shape {
        Shape::Point(q) => keeps(*q, boundary, inside, tol),
        Shape::Stroke { curve, drawn } => {
            let mut kept = Vec::new();
            match drawn {
                None => kept = cut(curve, boundary, inside, tol)?,
                Some(pieces) => {
                    for piece in pieces.iter() {
                        match piece {
                            Piece::Point(q) => {
                                if keeps(*q, boundary, inside, tol)? {
                                    kept.push(piece.clone());
                                }
                            }
                            Piece::Curve(c) => kept.extend(cut(c, boundary, inside, tol)?),
                        }
                    }
                }
            }
            let whole = matches!(&kept[..], [Piece::Curve(c)] if c == curve);
            if !whole {
                *drawn = Some(kept);
            }
            Ok(!matches!(drawn, Some(v) if v.is_empty()))
        }
        // Glyphs are kept whole; the consumer clips them like fills.
        Shape::Glyphs { .. } => Ok(true),
        Shape::Fill(region) | Shape::Mask(region) => {
            let keep = !inside
                || region
                    .bounding_box()
                    .intersects(&boundary.bounding_box().expanded(tol.linear));
            Ok(keep)
        }
    }
}

/// Applies the wipeouts in draw order: each mask cuts away what earlier curves and points
/// have inside it. Fills and masks beneath stay and are painted over by the mask.
fn apply_masks(primitives: &mut Vec<Primitive>, tol: &Tolerance, failed: &mut Vec<ObjectId>) {
    let mut out: Vec<Primitive> = Vec::with_capacity(primitives.len());
    for p in primitives.drain(..) {
        if let Shape::Mask(boundary) = &p.shape {
            match boundary_loop(boundary, tol) {
                Ok(mask) => out.retain_mut(|q| match cut_primitive(q, &mask, false, tol) {
                    Ok(keep) => keep,
                    Err(_) => {
                        failed.push(q.source);
                        false
                    }
                }),
                Err(_) => failed.push(p.source),
            }
        }
        out.push(p);
    }
    *primitives = out;
}

fn dedup(failed: &mut Vec<ObjectId>) {
    failed.sort();
    failed.dedup();
}

/// The display list of `space`: the visible primitives of its entities in draw order, with
/// the wipeouts applied. A primitive is hidden when a layer of its chain is off or frozen.
///
/// # Errors
///
/// `doc.node.not_found` and `doc.graph.malformed_key` are passed on; a failed entity list does
/// not fail the assembly but is listed in [`DisplayList::failed`].
pub fn space_list(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    space: ObjectId,
    options: &DisplayOptions,
) -> Result<DisplayList> {
    let mut out = DisplayList::default();
    let mut primitives = gather(
        snapshot,
        lists,
        space,
        &BTreeSet::new(),
        options.annotation_scale,
        options,
        &mut out,
    )?;
    apply_masks(&mut primitives, &options.tolerance, &mut out.failed);
    out.primitives = primitives;
    dedup(&mut out.failed);
    Ok(out)
}

/// Applies the viewport's appearance overrides to a primitive whose value came from an
/// overridden layer: the result `appearance::resolve` gives with the overrides. A linetype
/// override generates the dashes again on the curve in model coordinates.
fn override_primitive(
    p: &mut Primitive,
    overrides: &Overrides,
    reader: &SnapshotReader<'_>,
    options: &DisplayOptions,
) -> Result<()> {
    fn layer_of<T>(value: Option<&Sourced<T>>) -> Option<ObjectId> {
        match value.map(|v| v.source) {
            Some(Source::Layer(l)) => Some(l),
            _ => None,
        }
    }
    fn swap<T>(slot: &mut Option<Sourced<T>>, layer: ObjectId, value: Option<T>) {
        if let Some(value) = value {
            *slot = Some(Sourced {
                value,
                source: Source::ViewportOverride(layer),
            });
        }
    }
    let get = |l: Option<ObjectId>| l.and_then(|l| overrides.layers.get(&l).map(|o| (l, o)));
    if let Some((l, o)) = get(layer_of(p.color.as_ref())) {
        swap(&mut p.color, l, o.color.clone());
    }
    if let Some((l, o)) = get(layer_of(p.lineweight.as_ref())) {
        swap(&mut p.lineweight, l, o.lineweight.map(Some));
    }
    if let Some((l, o)) = get(layer_of(p.transparency.as_ref())) {
        swap(&mut p.transparency, l, o.transparency);
    }
    if let Some((l, o)) = get(layer_of(p.linetype.as_ref()))
        && let Some(linetype) = o.linetype
    {
        swap(&mut p.linetype, l, Some(Some(linetype)));
        if let Shape::Stroke { curve, drawn } = &mut p.shape {
            *drawn = dash(
                reader,
                curve,
                Some(linetype),
                (p.linetype_scale, p.continuous),
                options,
            )?;
        }
    }
    Ok(())
}

/// The display list of the sheet viewport `viewport` on its sheet: the list of the space it
/// shows with the viewport's frozen layers left out and its appearance overrides applied, the
/// wipeouts of that space applied, mapped to the sheet, and cut to the clip boundary.
///
/// # Errors
///
/// The errors of [`crate::layout::view`], and those of [`space_list`].
pub fn viewport_list(
    snapshot: &Snapshot,
    lists: Lists<'_>,
    viewport: ObjectId,
    options: &DisplayOptions,
) -> Result<DisplayList> {
    let view = view(snapshot, viewport)?;
    let tol = &options.tolerance;
    let mut out = DisplayList::default();
    let mut primitives = gather(
        snapshot,
        lists,
        view.model,
        &view.overrides.frozen,
        view.annotation_scale,
        options,
        &mut out,
    )?;
    let reader = SnapshotReader {
        snapshot,
        options,
        depth: 0,
    };
    let mut overridden = Vec::with_capacity(primitives.len());
    for mut p in primitives.drain(..) {
        match override_primitive(&mut p, &view.overrides, &reader, options) {
            Ok(()) => overridden.push(p),
            Err(_) => out.failed.push(p.source),
        }
    }
    let mut primitives = overridden;
    apply_masks(&mut primitives, tol, &mut out.failed);
    let clip = boundary_loop(&view.clip, tol)
        .map_err(|e| Error::new(codes::VIEWPORT_CLIP_INVALID).with_source(e))?;
    for mut p in primitives {
        let mapped = transform_shape(&p.shape, &view.transform, tol).and_then(|shape| {
            p.shape = shape;
            cut_primitive(&mut p, &clip, true, tol)
        });
        match mapped {
            Ok(true) => out.primitives.push(p),
            Ok(false) => {}
            Err(_) => out.failed.push(p.source),
        }
    }
    out.clip = Some(view.clip);
    dedup(&mut out.failed);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filling_open_values_names_the_reference_level() {
        let own = Some(Sourced {
            value: 2.0,
            source: Source::Object,
        });
        let mut slot = None;
        fill(&mut slot, own.clone(), 3);
        assert_eq!(slot.unwrap().source, Source::Block { level: 3 });
        let mut set = Some(Sourced {
            value: 1.0,
            source: Source::Default,
        });
        fill(&mut set, own, 1);
        assert_eq!(set.unwrap().value, 1.0);
    }
}
