//! Applying a plan to a document: every object is created through the transaction of the
//! running command, so one undo removes the whole import.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::id::Identifier;
use arc_base::math::{Point2, Vector3};
use arc_base::{Error, ObjectId, Result};
use arc_command::Context;
use arc_doc::relationship::ReferenceField;
use arc_doc::source_map::RECORD_KIND as SOURCE_RECORD_KIND;
use arc_doc::{ColorValue, GeometryBytes, Header, Snapshot, SourceMap, StoredForeign, Value};
use arc_draft::dimension::DefinitionPoint;
use arc_draft::text::paragraphs_value;
use arc_draft::types::{
    BLOCK, BLOCK_REF, CIRCLE, CONSTRUCTION_LINE, ELLIPSE, ELLIPTICAL_ARC, ENTITIES, LAYER, LINE,
    LINETYPE, POINT, POLYLINE, RAY, SPACE, SPLINE, SpaceKind, SpaceUnit, WIPEOUT,
};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use arc_io_dxf::{Group, Handle, PreservedRecord};

use crate::error::codes;
use crate::fields::{Builder, Reader};
use crate::plan::{
    Appearance, AttributeSpec, BlockSpec, By, Container, DimStyleSpec, ElementSpec, EntitySpec,
    FOREIGN_APPID, FOREIGN_CLASS, FOREIGN_DICTIONARY, FOREIGN_FORMAT, FOREIGN_HEADER,
    FOREIGN_XDATA, FamilySpec, ForeignSpec, HatchSpec, LayerSpec, LinetypeSpec, Op, Plan, Shape,
    TextStyleSpec,
};

/// An object an import created, in creation order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    /// The identifier of the object.
    pub id: ObjectId,
    /// The type key of the object: `LAYER`, `LINETYPE`, `STYLE`, `DIMSTYLE`, `BLOCK`, or the
    /// drafting type of the entity.
    pub type_key: &'static str,
    /// The DXF handle the object is bound to in the source map (format `dxf`, document the
    /// plan's `source_document`), `None` when the file gave none or the handle occurs earlier.
    pub source: Option<Handle>,
}

/// What an import created: how many objects of each kind, and every created object in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Existing objects of the document that the import bound to the file's handle, because they
    /// had no binding under the source document label yet (not counted in the object counts).
    pub adopted: u64,
    /// Source handles that moved from an object of an earlier import to an object of this one
    /// (the same label and handle). The earlier object loses its binding.
    pub rebound: u64,
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
    /// Every object created, in creation order (definitions first, then entities).
    pub created: Vec<Created>,
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

/// The handles of the objects of `snapshot` bound to `document` in the source map (format `dxf`),
/// by object. A binding whose object is not live is ignored; a zero handle is no binding. Of
/// several records for one identifier, the first in row order counts, as in
/// [`arc_doc::SourceMap`].
pub(crate) fn bound_handles(snapshot: &Snapshot, document: &str) -> BTreeMap<ObjectId, Handle> {
    let mut identifiers = BTreeSet::new();
    let mut out = BTreeMap::new();
    for row in snapshot
        .foreign_rows()
        .filter(|r| r.source_format == FOREIGN_FORMAT && r.record_kind == SOURCE_RECORD_KIND)
    {
        let (Some(id), Some(bytes), Ok(label)) = (
            row.source_id.as_deref(),
            row.object
                .as_deref()
                .and_then(|b| <[u8; 16]>::try_from(b).ok()),
            std::str::from_utf8(&row.data),
        ) else {
            continue;
        };
        if label != document || !identifiers.insert(id) {
            continue;
        }
        let Ok(handle) = u64::from_str_radix(id, 16) else {
            continue;
        };
        let object = ObjectId::from_u128(u128::from_be_bytes(bytes));
        if handle != 0 && snapshot.resolve(object).is_ok() {
            out.insert(object, Handle(handle));
        }
    }
    out
}

struct Applier<'c, 'a> {
    ctx: &'c mut Context<'a>,
    snapshot: Snapshot,
    ids: Ids,
    order: BTreeMap<ObjectId, i64>,
    space: ObjectId,
    applied: Applied,
    sources: SourceMap,
    document: String,
    /// The objects bound under the document label before the import.
    bound_here: BTreeSet<ObjectId>,
    /// The extended data rows of each object, by key.
    xdata_rows: BTreeMap<ObjectId, Vec<i64>>,
    /// The version name of the file, the rows' `source_version`.
    version: String,
}

/// Creates the objects of `plan` in the transaction of `ctx`: definitions first, then the
/// entities of the blocks and of the model space `space`. Layers, linetypes, text styles and
/// dimension styles that the document already has under the same name (compared without regard
/// to case) are used as they are; a block whose name is taken receives a new one.
///
/// Every created object that the plan gives a source handle is bound to it in the document's
/// source map (format `dxf`, document [`Plan::source_document`], identifier the handle in
/// hexadecimal), in the same transaction: one undo removes the objects and the bindings.
/// Binding a handle that another object of the document has moves the binding to the new
/// object. The result lists every created object with its source handle, in creation order.
///
/// # Errors
///
/// `draft.dxf.space_invalid`, `base.cancelled` and the errors of the transaction.
pub fn apply(ctx: &mut Context<'_>, space: ObjectId, plan: &Plan) -> Result<Applied> {
    let snapshot = ctx.txn()?.snapshot().clone();
    model_space_unit(&snapshot, space)?;
    let mut applier = Applier {
        sources: SourceMap::of(&snapshot),
        document: plan.source_document.clone(),
        bound_here: bound_handles(&snapshot, &plan.source_document)
            .into_keys()
            .collect(),
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
        xdata_rows: BTreeMap::new(),
        version: plan.version.clone(),
    };
    applier.xdata_rows = xdata_rows(&applier.snapshot);
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
    for named in &plan.standard_linetypes {
        let key = named.name.to_lowercase();
        if let Some(id) = applier.ids.linetypes.get(&key).copied() {
            applier.adopt(id, named.source, &[])?;
        }
    }
    // An import replaces the header, class, application and dictionary rows of its space: the rows
    // of an earlier import into the same space are removed in this transaction, found by format,
    // record kind and object. The last import's records win for the space.
    let stale: Vec<i64> = applier
        .snapshot
        .foreign_of(space)
        .filter(|r| {
            r.source_format == FOREIGN_FORMAT
                && [
                    FOREIGN_HEADER,
                    FOREIGN_CLASS,
                    FOREIGN_APPID,
                    FOREIGN_DICTIONARY,
                ]
                .contains(&r.record_kind.as_str())
        })
        .map(|r| r.id)
        .collect();
    for id in stale {
        applier.ctx.txn()?.remove_foreign(id)?;
    }
    for (n, spec) in plan.foreign.iter().enumerate() {
        if n % 256 == 0 {
            applier.ctx.checkpoint()?;
        }
        applier.foreign(spec, &plan.version)?;
    }
    Ok(applier.applied)
}

/// The extended data rows of `snapshot` by the object each is attached to.
fn xdata_rows(snapshot: &Snapshot) -> BTreeMap<ObjectId, Vec<i64>> {
    let mut out: BTreeMap<ObjectId, Vec<i64>> = BTreeMap::new();
    for row in snapshot.foreign_rows() {
        if row.source_format != FOREIGN_FORMAT || row.record_kind != FOREIGN_XDATA {
            continue;
        }
        let Some(bytes) = row
            .object
            .as_deref()
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
        else {
            continue;
        };
        out.entry(ObjectId::from_u128(u128::from_be_bytes(bytes)))
            .or_default()
            .push(row.id);
    }
    out
}

impl Applier<'_, '_> {
    /// Binds `handle` to `id` under the document label. A handle that a live object of an earlier
    /// import holds moves to `id`, and is counted in `rebound`.
    fn bind_source(&mut self, id: ObjectId, handle: Handle) -> Result<()> {
        let hex = handle.to_hex();
        if self
            .sources
            .resolve(FOREIGN_FORMAT, &self.document, &hex)
            .is_some_and(|old| old != id)
        {
            self.applied.rebound += 1;
        }
        let txn = self.ctx.txn()?;
        self.sources
            .bind(txn, FOREIGN_FORMAT, &self.document, &hex, id)
    }

    /// Binds the existing object `id` to the file's `source` when it has no binding under the
    /// document label yet; an existing object is not created and is not listed as created. Its
    /// extended data is replaced by the file's (see [`Self::extended`]).
    fn adopt(&mut self, id: ObjectId, source: Option<Handle>, xdata: &[Group]) -> Result<()> {
        self.extended(id, xdata, source)?;
        match source {
            Some(handle) if !self.bound_here.contains(&id) => {
                self.bind_source(id, handle)?;
                self.applied.adopted += 1;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Sets the extended data of the object `id` to `groups` as one row bound to it. The rows the
    /// object held before are removed in this transaction, so the last import's data replaces them,
    /// and an empty `groups` leaves the object without a row.
    fn extended(&mut self, id: ObjectId, groups: &[Group], source: Option<Handle>) -> Result<()> {
        for old in self.xdata_rows.remove(&id).unwrap_or_default() {
            self.ctx.txn()?.remove_foreign(old)?;
        }
        if groups.is_empty() {
            return Ok(());
        }
        let data = crate::plan::encode_record(&PreservedRecord {
            kind: crate::records::XDATA.to_owned(),
            groups: groups.to_vec(),
        });
        let key = self.ctx.txn()?.add_foreign(StoredForeign {
            id: 0,
            source_format: FOREIGN_FORMAT.to_owned(),
            source_version: (!self.version.is_empty()).then(|| self.version.clone()),
            source_id: source.map(Handle::to_hex),
            object: Some(id.as_u128().to_be_bytes().to_vec()),
            record_kind: FOREIGN_XDATA.to_owned(),
            data,
            asset: None,
        })?;
        self.xdata_rows.insert(id, vec![key]);
        Ok(())
    }

    /// Records that `id` was created as an object of `type_key` from the file object `source`:
    /// binds the handle in the source map and lists the object.
    fn bind(&mut self, id: ObjectId, type_key: &'static str, source: Option<Handle>) -> Result<()> {
        if let Some(handle) = source {
            self.bind_source(id, handle)?;
        }
        self.applied.created.push(Created {
            id,
            type_key,
            source,
        });
        Ok(())
    }

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
        if let Some(id) = self.ids.linetypes.get(&key).copied() {
            return self.adopt(id, s.source, &s.xdata);
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
        self.bind(id, LINETYPE, s.source)?;
        self.extended(id, &s.xdata, s.source)
    }

    fn layer(&mut self, s: &LayerSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if let Some(id) = self.ids.layers.get(&key).copied() {
            return self.adopt(id, s.source, &s.xdata);
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
        self.bind(id, LAYER, s.source)?;
        self.extended(id, &s.xdata, s.source)
    }

    fn text_style(&mut self, s: &TextStyleSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if let Some(id) = self.ids.text_styles.get(&key).copied() {
            return self.adopt(id, s.source, &s.xdata);
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
        self.bind(id, arc_draft::style::TEXT_STYLE, s.source)?;
        self.extended(id, &s.xdata, s.source)
    }

    fn dim_style(&mut self, s: &DimStyleSpec) -> Result<()> {
        let key = s.name.to_lowercase();
        if let Some(id) = self.ids.dim_styles.get(&key).copied() {
            return self.adopt(id, s.source, &s.xdata);
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
        self.bind(id, arc_draft::style::DIM_STYLE, s.source)?;
        self.extended(id, &s.xdata, s.source)
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
        self.bind(id, BLOCK, s.source)?;
        self.extended(id, &s.xdata, s.source)
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
        let id = self.create(key, Some(owner), b.values)?;
        self.applied.entities += 1;
        self.bind(id, key, spec.source)?;
        self.extended(id, &spec.xdata, spec.source)
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
