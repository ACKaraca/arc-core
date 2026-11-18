//! Applying a plan to a document: every object is created through the transaction of the
//! running command, so one undo removes the whole import.

use std::collections::BTreeMap;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Vector3};
use arc_base::{Error, ObjectId, Result};
use arc_command::Context;
use arc_doc::relationship::ReferenceField;
use arc_doc::{ColorValue, GeometryBytes, Header, Snapshot, StoredForeign, Value};
use arc_draft::dimension::DefinitionPoint;
use arc_draft::text::paragraphs_value;
use arc_draft::types::{
    BLOCK, BLOCK_REF, CIRCLE, CONSTRUCTION_LINE, ELLIPSE, ELLIPTICAL_ARC, ENTITIES, LAYER, LINE,
    LINETYPE, POINT, POLYLINE, RAY, SPACE, SPLINE, SpaceKind, SpaceUnit, WIPEOUT,
};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use arc_io_dxf::Handle;
use serde::Serialize;

use crate::error::codes;
use crate::fields::{Builder, Reader};
use crate::plan::{
    Appearance, AttributeSpec, BlockSpec, By, Container, DimStyleSpec, ElementSpec, EntitySpec,
    FOREIGN_FORMAT, FamilySpec, ForeignSpec, HatchSpec, LayerSpec, LinetypeSpec, Op, Plan, Shape,
    TextStyleSpec,
};

/// How many objects of each kind an import created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Applied {
    /// Linetypes.
    pub linetypes: u64,
    /// Layers.
    pub layers: u64,
    /// Text styles.
    pub text_styles: u64,
    /// Dimension styles.
    pub dim_styles: u64,
    /// Block definitions.
    pub blocks: u64,
    /// Entities, in the space and in blocks.
    pub entities: u64,
}

fn invalid(what: &str) -> Error {
    Error::new(codes::DOCUMENT_INVALID).with_detail("what", what)
}

/// The kind and unit of the space `id`, `None` when `id` is not a space.
///
/// # Errors
///
/// `draft.dxf.space_invalid` unless `id` is a model space.
pub fn model_space_unit(snapshot: &Snapshot, id: ObjectId) -> Result<SpaceUnit> {
    let handle = snapshot
        .resolve(id)
        .map_err(|_| Error::new(codes::SPACE_INVALID))?;
    if snapshot.type_key(handle).map(arc_doc::TypeKey::as_str) != Some(SPACE) {
        return Err(Error::new(codes::SPACE_INVALID));
    }
    let object = snapshot.get(handle)?;
    let reader = Reader::new(snapshot.registry(), SPACE, &object.values);
    if reader.enum_code("kind") != Some(SpaceKind::Model as u32) {
        return Err(Error::new(codes::SPACE_INVALID));
    }
    Ok(reader
        .enum_code("unit")
        .and_then(SpaceUnit::of)
        .unwrap_or(SpaceUnit::Unitless))
}

#[derive(Default)]
struct Ids {
    layers: BTreeMap<String, ObjectId>,
    linetypes: BTreeMap<String, ObjectId>,
    text_styles: BTreeMap<String, ObjectId>,
    dim_styles: BTreeMap<String, ObjectId>,
    blocks: BTreeMap<String, ObjectId>,
}

/// The objects of type `key` by lowercase name.
fn existing(snapshot: &Snapshot, key: &str) -> BTreeMap<String, ObjectId> {
    let mut out = BTreeMap::new();
    if let Ok(iter) = snapshot.iter_type(key) {
        for (_, object) in iter {
            let reader = Reader::new(snapshot.registry(), key, &object.values);
            if let Some(name) = reader.text("name") {
                out.entry(name.to_lowercase()).or_insert(object.id);
            }
        }
    }
    out
}

/// The first free draw-order key of `space`.
fn next_order(snapshot: &Snapshot, space: ObjectId) -> i64 {
    let mut next = 0_i64;
    for r in snapshot.referrers(space) {
        if r.field != ReferenceField::Owner {
            continue;
        }
        let Ok(handle) = snapshot.resolve(r.source) else {
            continue;
        };
        let Some(key) = snapshot.type_key(handle) else {
            continue;
        };
        if !ENTITIES.contains(&key.as_str()) {
            continue;
        }
        if let Ok(object) = snapshot.get(handle) {
            let reader = Reader::new(snapshot.registry(), key.as_str(), &object.values);
            if let Some(Value::I64(k)) = reader.get("draw_order") {
                next = next.max(k.saturating_add(1));
            }
        }
    }
    next
}

struct Applier<'c, 'a> {
    ctx: &'c mut Context<'a>,
    snapshot: Snapshot,
    ids: Ids,
    order: BTreeMap<ObjectId, i64>,
    space: ObjectId,
    applied: Applied,
}

/// Creates the objects of `plan` in the transaction of `ctx`: definitions first, then the
/// entities of the blocks and of the model space `space`. Layers, linetypes, text styles and
/// dimension styles that the document already has under the same name (compared without regard
/// to case) are used as they are; a block whose name is taken receives a new one.
///
/// # Errors
///
/// `draft.dxf.space_invalid`, `base.cancelled` and the errors of the transaction.
pub fn apply(ctx: &mut Context<'_>, space: ObjectId, plan: &Plan) -> Result<Applied> {
    let snapshot = ctx.txn()?.snapshot().clone();
    model_space_unit(&snapshot, space)?;
    let mut applier = Applier {
        ids: Ids {
            layers: existing(&snapshot, LAYER),
            linetypes: existing(&snapshot, LINETYPE),
            text_styles: existing(&snapshot, arc_draft::style::TEXT_STYLE),
            dim_styles: existing(&snapshot, arc_draft::style::DIM_STYLE),
            blocks: BTreeMap::new(),
        },
        order: BTreeMap::from([(space, next_order(&snapshot, space))]),
        snapshot,
        ctx,
        space,
        applied: Applied::default(),
    };
    let taken = existing(&applier.snapshot, BLOCK);
    let total = plan.ops.len().max(1);
    for (n, op) in plan.ops.iter().enumerate() {
        if n % 256 == 0 {
            applier.ctx.checkpoint()?;
            #[expect(clippy::cast_precision_loss, reason = "a progress fraction")]
            applier.ctx.progress(
                n as f64 / total as f64,
                crate::commands::IMPORT_PROGRESS.message_id,
            );
        }
        applier.op(op, &taken)?;
    }
    for (n, spec) in plan.foreign.iter().enumerate() {
        if n % 256 == 0 {
            applier.ctx.checkpoint()?;
        }
        applier.foreign(spec, &plan.version)?;
    }
    Ok(applier.applied)
}

impl Applier<'_, '_> {
    fn create(
        &mut self,
        key: &str,
        owner: Option<ObjectId>,
        values: Vec<Option<Value>>,
    ) -> Result<ObjectId> {
        let id: ObjectId = self.ctx.new_id();
        let header = Header {
            owner,
            ..Header::default()
        };
        self.ctx.txn()?.create(key, id, header, values)?;
        Ok(id)
    }

    /// Adds the foreign data row of `spec`, attached to its space or block.
    fn foreign(&mut self, spec: &ForeignSpec, version: &str) -> Result<()> {
        let owner = match &spec.container {
            Container::Space => self.space,
            Container::Block(name) => *self
                .ids
                .blocks
                .get(&name.to_lowercase())
                .ok_or_else(|| invalid("block"))?,
        };
        self.ctx.txn()?.add_foreign(StoredForeign {
            id: 0,
            source_format: FOREIGN_FORMAT.to_owned(),
            source_version: (!version.is_empty()).then(|| version.to_owned()),
            source_id: spec.source.map(Handle::to_hex),
            object: Some(owner.as_u128().to_be_bytes().to_vec()),
            record_kind: spec.record_kind.to_owned(),
            data: spec.data.clone(),
            asset: None,
        })?;
        Ok(())
    }

    fn builder(&self, key: &str) -> Builder {
        Builder::new(self.snapshot.registry(), key)
    }

    fn op(&mut self, op: &Op, taken: &BTreeMap<String, ObjectId>) -> Result<()> {
        match op {
            Op::Linetype(s) => self.linetype(s),
            Op::Layer(s) => self.layer(s),
            Op::TextStyle(s) => self.text_style(s),
            Op::DimStyle(s) => self.dim_style(s),
            Op::Block(s) => self.block(s, taken),
            Op::Entity { container, spec } => self.entity(container, spec),
        }
    }

    fn linetype(&mut self, s: &LinetypeSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if self.ids.linetypes.contains_key(&key) {
            return Ok(());
        }
        let mut b = self.builder(LINETYPE);
        b.set("name", Value::String(s.name.clone()))?
            .set("description", Value::String(s.description.clone()))?
            .set(
                "elements",
                Value::List(s.elements.iter().map(element_value).collect()),
            )?;
        let id = self.create(LINETYPE, None, b.values)?;
        self.ids.linetypes.insert(key, id);
        self.applied.linetypes += 1;
        Ok(())
    }

    fn layer(&mut self, s: &LayerSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if self.ids.layers.contains_key(&key) {
            return Ok(());
        }
        let linetype = s
            .linetype
            .as_ref()
            .and_then(|n| self.ids.linetypes.get(&n.to_lowercase()))
            .map(|id| Value::Reference(*id));
        let mut b = self.builder(LAYER);
        b.set("name", Value::String(s.name.clone()))?
            .set("color", Value::Color(Box::new(s.color.clone())))?
            .set_opt("linetype", linetype)?
            .set_opt("lineweight", s.lineweight.map(Value::F64))?
            .set("transparency", Value::F64(s.transparency))?
            .set("on", Value::Bool(s.on))?
            .set("frozen", Value::Bool(s.frozen))?
            .set("locked", Value::Bool(s.locked))?
            .set("plottable", Value::Bool(s.plottable))?;
        let id = self.create(LAYER, None, b.values)?;
        self.ids.layers.insert(key, id);
        self.applied.layers += 1;
        Ok(())
    }

    fn text_style(&mut self, s: &TextStyleSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if self.ids.text_styles.contains_key(&key) {
            return Ok(());
        }
        let mut b = self.builder(arc_draft::style::TEXT_STYLE);
        b.set("name", Value::String(s.name.clone()))?
            .set_opt("font", s.font.clone().map(Value::String))?
            .set("height", Value::F64(s.height))?
            .set("width_factor", Value::F64(s.width_factor))?
            .set("oblique", Value::F64(s.oblique))?
            .set("backwards", Value::Bool(s.backwards))?
            .set("upside_down", Value::Bool(s.upside_down))?
            .set("vertical", Value::Bool(s.vertical))?;
        let id = self.create(arc_draft::style::TEXT_STYLE, None, b.values)?;
        self.ids.text_styles.insert(key, id);
        self.applied.text_styles += 1;
        Ok(())
    }

    fn dim_style(&mut self, s: &DimStyleSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if self.ids.dim_styles.contains_key(&key) {
            return Ok(());
        }
        let mut b = self.builder(arc_draft::style::DIM_STYLE);
        b.set("name", Value::String(s.name.clone()))?;
        for (field, value) in &s.values {
            let v = if *field == "precision" {
                #[expect(clippy::cast_possible_truncation, reason = "a value of 0 to 8")]
                Value::I64(*value as i64)
            } else {
                Value::F64(*value)
            };
            b.set(field, v)?;
        }
        let id = self.create(arc_draft::style::DIM_STYLE, None, b.values)?;
        self.ids.dim_styles.insert(key, id);
        self.applied.dim_styles += 1;
        Ok(())
    }

    fn block(&mut self, s: &BlockSpec, taken: &BTreeMap<String, ObjectId>) -> Result<()> {
        let mut name = s.name.clone();
        let mut n = 2_u32;
        while taken.contains_key(&name.to_lowercase()) {
            name = format!("{} ({n})", s.name);
            n += 1;
        }
        let mut b = self.builder(BLOCK);
        b.set("name", Value::String(name))?
            .set("base_point", Value::Point2(s.base))?
            .set("unit", Value::Enum(s.unit.code()))?
            .set(
                "attributes",
                Value::List(s.attributes.iter().map(attribute_value).collect()),
            )?;
        let id = self.create(BLOCK, None, b.values)?;
        self.ids.blocks.insert(s.name.to_lowercase(), id);
        self.order.insert(id, 0);
        self.applied.blocks += 1;
        Ok(())
    }

    fn entity(&mut self, container: &Container, spec: &EntitySpec) -> Result<()> {
        let owner = match container {
            Container::Space => self.space,
            Container::Block(name) => *self
                .ids
                .blocks
                .get(&name.to_lowercase())
                .ok_or_else(|| invalid("block"))?,
        };
        let (key, mut b) = self.shape(&spec.shape)?;
        self.common(&mut b, &spec.appearance)?;
        let order = self.order.entry(owner).or_insert(0);
        b.set("draw_order", Value::I64(*order))?;
        *order += 1;
        self.create(key, Some(owner), b.values)?;
        self.applied.entities += 1;
        Ok(())
    }

    fn common(&mut self, b: &mut Builder, a: &Appearance) -> Result<()> {
        let layer = self
            .ids
            .layers
            .get(&a.layer.to_lowercase())
            .ok_or_else(|| invalid("layer"))?;
        b.set("layer", Value::Reference(*layer))?;
        match &a.color {
            By::Layer => {}
            By::Block => {
                b.set("color_by", Value::Enum(1))?;
            }
            By::Object(c) => {
                b.set("color_by", Value::Enum(2))?
                    .set("color", Value::Color(Box::new(ColorValue { ..c.clone() })))?;
            }
        }
        match &a.linetype {
            By::Layer => {}
            By::Block => {
                b.set("linetype_by", Value::Enum(1))?;
            }
            By::Object(name) => {
                let id = name
                    .as_ref()
                    .and_then(|n| self.ids.linetypes.get(&n.to_lowercase()))
                    .map(|id| Value::Reference(*id));
                b.set("linetype_by", Value::Enum(2))?
                    .set_opt("linetype", id)?;
            }
        }
        match &a.lineweight {
            By::Layer => {}
            By::Block => {
                b.set("lineweight_by", Value::Enum(1))?;
            }
            By::Object(w) => {
                b.set("lineweight_by", Value::Enum(2))?
                    .set_opt("lineweight", w.map(Value::F64))?;
            }
        }
        match &a.transparency {
            By::Layer => {}
            By::Block => {
                b.set("transparency_by", Value::Enum(1))?;
            }
            By::Object(t) => {
                b.set("transparency_by", Value::Enum(2))?
                    .set("transparency", Value::F64(*t))?;
            }
        }
        let [x, y, z] = a.extrusion;
        b.set("linetype_scale", Value::F64(a.linetype_scale))?
            .set("visible", Value::Bool(a.visible))?
            .set("elevation", Value::F64(a.elevation))?
            .set("thickness", Value::F64(a.thickness))?
            .set("extrusion", Value::Vector3(Vector3::new(x, y, z)))?;
        Ok(())
    }

    fn style_ref(&self, name: Option<&String>, dim: bool) -> Option<Value> {
        let map = if dim {
            &self.ids.dim_styles
        } else {
            &self.ids.text_styles
        };
        name.and_then(|n| map.get(&n.to_lowercase()))
            .map(|id| Value::Reference(*id))
    }

    fn shape(&self, shape: &Shape) -> Result<(&'static str, Builder)> {
        let reg = self.snapshot.registry();
        let geometry = |c: &Curve2| Value::Geometry(GeometryBytes(c.encode()));
        Ok(match shape {
            Shape::Point(p) => {
                let mut b = Builder::new(reg, POINT);
                b.set("position", Value::Point2(*p))?;
                (POINT, b)
            }
            Shape::Curve(c) => {
                let key = curve_key(c);
                let mut b = Builder::new(reg, key);
                b.set("geometry", geometry(c))?;
                (key, b)
            }
            Shape::Spline { curve, fit } => {
                let mut b = Builder::new(reg, SPLINE);
                b.set("geometry", Value::Geometry(GeometryBytes(curve.encode())))?
                    .set(
                        "fit_points",
                        Value::List(fit.iter().map(|p| Value::Point2(*p)).collect()),
                    )?;
                (SPLINE, b)
            }
            Shape::Text(t) => {
                let mut b = Builder::new(reg, arc_draft::text::TEXT);
                b.set("content", Value::String(t.content.clone()))?
                    .set("position", Value::Point2(t.position))?
                    .set("rotation", Value::F64(t.rotation))?
                    .set_opt("style", self.style_ref(t.style.as_ref(), false))?
                    .set("height", Value::F64(t.height))?
                    .set("halign", Value::Enum(t.halign))?
                    .set("valign", Value::Enum(t.valign))?;
                (arc_draft::text::TEXT, b)
            }
            Shape::RichText(t) => {
                let mut b = Builder::new(reg, arc_draft::text::RICH_TEXT);
                b.set("paragraphs", paragraphs_value(&t.paragraphs))?
                    .set("position", Value::Point2(t.position))?
                    .set("rotation", Value::F64(t.rotation))?
                    .set_opt("style", self.style_ref(t.style.as_ref(), false))?
                    .set("height", Value::F64(t.height))?
                    .set("width", Value::F64(t.width))?
                    .set("attachment", Value::Enum(t.attachment))?;
                (arc_draft::text::RICH_TEXT, b)
            }
            Shape::Dimension(d) => {
                let mut b = Builder::new(reg, arc_draft::dimension::DIMENSION);
                b.set("kind", Value::Enum(d.kind as u32))?
                    .set(
                        "points",
                        Value::List(
                            d.points
                                .iter()
                                .map(|p| DefinitionPoint::free(*p).to_value())
                                .collect(),
                        ),
                    )?
                    .set("line_position", Value::Point2(d.line_position))?
                    .set("rotation", Value::F64(d.rotation))?
                    .set_opt("text_override", d.text_override.clone().map(Value::String))?
                    .set_opt("text_position", d.text_position.map(Value::Point2))?
                    .set_opt("style", self.style_ref(d.style.as_ref(), true))?;
                (arc_draft::dimension::DIMENSION, b)
            }
            Shape::Hatch(h) => {
                let mut b = Builder::new(reg, arc_draft::hatch::HATCH);
                hatch_values(&mut b, h)?;
                (arc_draft::hatch::HATCH, b)
            }
            Shape::Wipeout(p) => {
                let mut b = Builder::new(reg, WIPEOUT);
                b.set("geometry", geometry(&Curve2::Polyline(p.clone())))?
                    .set("frame", Value::Bool(false))?;
                (WIPEOUT, b)
            }
            Shape::Insert(i) => {
                let block = self
                    .ids
                    .blocks
                    .get(&i.block.to_lowercase())
                    .ok_or_else(|| invalid("block"))?;
                let attributes = i
                    .attributes
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect();
                let mut b = Builder::new(reg, BLOCK_REF);
                b.set("block", Value::Reference(*block))?
                    .set("position", Value::Point2(i.position))?
                    .set(
                        "scale",
                        Value::Vector3(Vector3::new(i.scale[0], i.scale[1], i.scale[2])),
                    )?
                    .set("rotation", Value::F64(i.rotation))?
                    .set("attributes", Value::Map(attributes))?;
                (BLOCK_REF, b)
            }
        })
    }
}

/// The type key of an entity holding `curve`.
#[must_use]
pub fn curve_key(curve: &Curve2) -> &'static str {
    match curve {
        Curve2::Segment(_) => LINE,
        Curve2::Ray(_) => RAY,
        Curve2::Line(_) => CONSTRUCTION_LINE,
        Curve2::Circle(_) => CIRCLE,
        Curve2::Arc(_) => arc_draft::types::ARC,
        Curve2::Ellipse(_) => ELLIPSE,
        Curve2::EllipticalArc(_) => ELLIPTICAL_ARC,
        Curve2::Polyline(_) => POLYLINE,
        Curve2::Nurbs(_) => SPLINE,
    }
}

fn hatch_values(b: &mut Builder, h: &HatchSpec) -> Result<()> {
    let loops = h
        .loops
        .iter()
        .map(|c| Value::Geometry(GeometryBytes(c.encode())))
        .collect();
    b.set("kind", Value::Enum(u32::from(h.solid)))?
        .set("loops", Value::List(loops))?
        .set("island", Value::Enum(h.island))?
        .set(
            "inline",
            Value::List(h.families.iter().map(family_value).collect()),
        )?;
    Ok(())
}

fn family_value(f: &FamilySpec) -> Value {
    Value::Record(vec![
        Some(Value::F64(f.angle)),
        Some(Value::Point2(f.origin)),
        Some(Value::F64(f.shift)),
        Some(Value::F64(f.spacing)),
        Some(Value::List(
            f.dashes.iter().map(|d| Value::F64(*d)).collect(),
        )),
    ])
}

fn element_value(e: &ElementSpec) -> Value {
    Value::Record(vec![
        Some(Value::Enum(e.kind as u32)),
        Some(Value::F64(e.length)),
        e.content.clone().map(Value::String),
        e.style.clone().map(Value::String),
        Some(Value::F64(e.scale)),
        Some(Value::F64(e.rotation)),
        e.offset.map(|[x, y]| Value::Point2(Point2::new(x, y))),
    ])
}

fn attribute_value(a: &AttributeSpec) -> Value {
    Value::Record(vec![
        Some(Value::String(a.tag.clone())),
        a.prompt.clone().map(Value::String),
        a.default.clone().map(Value::String),
        Some(Value::Bool(a.constant)),
        Some(Value::Bool(a.invisible)),
        Some(Value::Bool(a.preset)),
        Some(Value::Bool(false)),
        Some(Value::Point2(a.position)),
    ])
}
