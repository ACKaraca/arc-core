//! The import mapper: walks a drawing and fills the plan. The tables, entities, text and hatch
//! mappings are in the sibling modules; this one holds the shared state and the orchestration.

use std::collections::{BTreeMap, BTreeSet};

use arc_draft::types::SpaceUnit;
use arc_draft::validate::DEFAULT_MAX_NESTING;
use arc_io_dxf::loss::{Argument, LossReport, Reason};
use arc_io_dxf::model::HeaderVariable;
use arc_io_dxf::{Drawing, Entity, EntityKind, Group, Handle, PreservedRecord, Value};

use crate::depth::too_deep;
use crate::error::reasons;
use crate::header;
use crate::plan::{
    Appearance, BlockSpec, Container, DimStyleSpec, EntitySpec, FOREIGN_ENTITY, FOREIGN_HEADER,
    FOREIGN_OBJECT, ForeignSpec, ImportOptions, LayerSpec, LinetypeSpec, MAX_FOREIGN_RECORDS,
    MAX_HEADER_VARIABLES, NamedSource, Op, Plan, Shape, TextStyleSpec, encode_record,
};
use crate::report::Reporter;
use crate::units;

/// Definitions of one kind, found by case-insensitive name (DXF names do not distinguish case).
#[derive(Debug)]
pub(crate) struct Defs<T> {
    pub(crate) items: Vec<T>,
    index: BTreeMap<String, usize>,
}

impl<T> Default for Defs<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            index: BTreeMap::new(),
        }
    }
}

impl<T> Defs<T> {
    pub(crate) fn find(&self, name: &str) -> Option<&T> {
        self.index
            .get(&name.to_lowercase())
            .and_then(|i| self.items.get(*i))
    }

    /// Adds `item` under `name` unless a definition of that name exists; whether it was added.
    pub(crate) fn add(&mut self, name: &str, item: T) -> bool {
        let key = name.to_lowercase();
        if self.index.contains_key(&key) {
            return false;
        }
        self.index.insert(key, self.items.len());
        self.items.push(item);
        true
    }
}

/// The result of mapping the shape of an entity: the shape and the z coordinates it dropped.
pub(crate) struct Built {
    pub(crate) shape: crate::plan::Shape,
    pub(crate) z: Vec<f64>,
}

impl Built {
    pub(crate) fn with_z(shape: crate::plan::Shape, z: Vec<f64>) -> Option<Self> {
        Some(Self { shape, z })
    }
}

pub(crate) struct Mapper<'a> {
    pub(crate) d: &'a Drawing,
    pub(crate) o: &'a ImportOptions,
    pub(crate) rep: Reporter,
    pub(crate) source_unit: i64,
    pub(crate) factor: f64,
    pub(crate) layers: Defs<LayerSpec>,
    pub(crate) linetypes: Defs<LinetypeSpec>,
    pub(crate) text_styles: Defs<TextStyleSpec>,
    pub(crate) dim_styles: Defs<DimStyleSpec>,
    pub(crate) blocks: Defs<BlockSpec>,
    pub(crate) style_by_handle: BTreeMap<u64, String>,
    pub(crate) entities: Vec<(Container, EntitySpec)>,
    pub(crate) foreign: Vec<ForeignSpec>,
    pub(crate) edges: BTreeMap<String, BTreeSet<String>>,
    pub(crate) standard_linetypes: Vec<NamedSource>,
}

/// The header variables the import consumes itself; every other one is kept as a foreign row.
const CONSUMED_HEADER: &[&str] = &["$INSUNITS"];

/// The node of a block definition in the reference graph, numbered from 1 in order of first use;
/// names are case insensitive, as the definitions are.
fn block_node(ids: &mut BTreeMap<String, usize>, name: &str) -> usize {
    let next = ids.len() + 1;
    *ids.entry(name.to_lowercase()).or_insert(next)
}

pub(crate) fn hex(handle: Option<Handle>) -> Option<String> {
    handle.map(Handle::to_hex)
}

pub(crate) fn extra_f64(extras: &[Group], code: i16) -> Option<f64> {
    extras
        .iter()
        .find(|g| g.code == code)
        .and_then(|g| match g.value {
            Value::Double(v) => Some(v),
            _ => None,
        })
}

pub(crate) fn extra_int(extras: &[Group], code: i16) -> Option<i64> {
    extras
        .iter()
        .find(|g| g.code == code)
        .and_then(|g| match g.value {
            Value::Int(v) => Some(v),
            Value::Bool(b) => Some(i64::from(b)),
            _ => None,
        })
}

impl<'a> Mapper<'a> {
    pub(crate) fn new(d: &'a Drawing, o: &'a ImportOptions) -> Self {
        Self {
            d,
            o,
            rep: Reporter::new(),
            source_unit: 0,
            factor: 1.0,
            layers: Defs::default(),
            linetypes: Defs::default(),
            text_styles: Defs::default(),
            dim_styles: Defs::default(),
            blocks: Defs::default(),
            style_by_handle: BTreeMap::new(),
            entities: Vec::new(),
            foreign: Vec::new(),
            edges: BTreeMap::new(),
            standard_linetypes: Vec::new(),
        }
    }

    pub(crate) fn note(&mut self, reason: Reason, kind: &str, source: Option<Handle>) {
        self.rep.note(reason, kind, hex(source), &[]);
    }

    pub(crate) fn note_what(&mut self, reason: Reason, e: &Entity, what: &str) {
        self.rep.note(
            reason,
            e.kind.type_name(),
            hex(e.source_id()),
            &[("what", Argument::Text(what.to_owned()))],
        );
    }

    pub(crate) fn run(&mut self) {
        self.setup_units();
        self.map_tables();
        self.map_blocks();
        self.map_model_space();
        self.limit_depth();
        self.skip_rest();
    }

    /// Drops the block references that would make a chain deeper than the drafting model allows
    /// (see [`crate::depth`]), notes each dropped reference and takes it back out of the mapped
    /// count.
    fn limit_depth(&mut self) {
        let limit = usize::try_from(DEFAULT_MAX_NESTING).unwrap_or(usize::MAX);
        let mut ids: BTreeMap<String, usize> = BTreeMap::new();
        let mut positions = Vec::new();
        let mut edges = Vec::new();
        for (at, (container, spec)) in self.entities.iter().enumerate() {
            let Shape::Insert(insert) = &spec.shape else {
                continue;
            };
            let from = match container {
                Container::Space => 0,
                Container::Block(name) => block_node(&mut ids, name),
            };
            positions.push(at);
            edges.push((from, block_node(&mut ids, &insert.block)));
        }
        let flags = too_deep(ids.len() + 1, &edges, limit);
        let mut drop_at = vec![false; self.entities.len()];
        for (at, flag) in positions.into_iter().zip(flags) {
            if let Some(slot) = drop_at.get_mut(at) {
                *slot = flag;
            }
        }
        if !drop_at.contains(&true) {
            return;
        }
        let limit_arg = i64::from(DEFAULT_MAX_NESTING);
        for (at, (container, spec)) in std::mem::take(&mut self.entities).into_iter().enumerate() {
            match &spec.shape {
                Shape::Insert(insert) if drop_at.get(at) == Some(&true) => {
                    self.rep.note(
                        reasons::REFERENCE_DEPTH_EXCEEDED,
                        "INSERT",
                        hex(spec.source),
                        &[
                            ("block", Argument::Text(insert.block.clone())),
                            ("limit", Argument::Int(limit_arg)),
                        ],
                    );
                    self.rep.unmap(1);
                }
                _ => self.entities.push((container, spec)),
            }
        }
    }

    fn setup_units(&mut self) {
        let code = self
            .d
            .header_variable("$INSUNITS")
            .and_then(|g| {
                g.iter().find_map(|g| {
                    if let Value::Int(v) = g.value {
                        Some(v)
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);
        self.source_unit = code;
        let target = self.o.target_unit;
        let known = units::meters_of_code(code).is_some();
        if known && target != SpaceUnit::Unitless {
            self.factor = units::factor(code, target);
            if (self.factor - 1.0).abs() > f64::EPSILON {
                self.rep.note(
                    reasons::UNITS_CONVERTED,
                    "",
                    None,
                    &[
                        ("from", Argument::Int(code)),
                        ("factor", Argument::Text(format!("{:?}", self.factor))),
                    ],
                );
            }
        } else {
            self.rep.note(
                reasons::UNITS_UNITLESS,
                "",
                None,
                &[("from", Argument::Int(code))],
            );
        }
    }

    pub(crate) fn file_unit(&self) -> SpaceUnit {
        units::unit_of_code(self.source_unit).unwrap_or(SpaceUnit::Unitless)
    }

    /// The names of the entities of block `name` and of the model space, in file order.
    fn map_model_space(&mut self) {
        let d = self.d;
        let model_block = d
            .blocks
            .iter()
            .find(|b| matches!(&b.begin.kind, EntityKind::Block(begin) if begin.name.eq_ignore_ascii_case("*Model_Space")));
        let from_block = model_block
            .map(|b| b.entities.as_slice())
            .unwrap_or_default();
        for e in from_block.iter().chain(d.entities.iter()) {
            if e.props.paper_space {
                self.note(
                    reasons::PAPER_SPACE_SKIPPED,
                    e.kind.type_name(),
                    e.source_id(),
                );
                continue;
            }
            let f = self.factor;
            self.add_entity(e, &Container::Space, f);
        }
    }

    /// Maps `e` and stores it in `container`; `f` scales its lengths.
    pub(crate) fn add_entity(&mut self, e: &Entity, container: &Container, f: f64) {
        if let EntityKind::Preserved(record) = &e.kind {
            self.keep(record, FOREIGN_ENTITY, container);
            return;
        }
        if let EntityKind::Insert(i) = &e.kind {
            self.add_insert(e, i, container, f);
            return;
        }
        let Some(built) = self.map_shape(e, f) else {
            return;
        };
        let appearance = self.appearance(e, f, &built.z);
        if !finite(&built.shape, &appearance) {
            self.note(reasons::GEOMETRY_INVALID, e.kind.type_name(), e.source_id());
            return;
        }
        self.rep.mapped(1);
        self.entities.push((
            container.clone(),
            EntitySpec {
                appearance,
                shape: built.shape,
                source: e.source_id(),
            },
        ));
    }

    /// Keeps `record` as a foreign data row attached to `container` unless the import holds
    /// the most rows it keeps or the record is larger than the limits allow.
    fn keep(&mut self, record: &PreservedRecord, record_kind: &'static str, container: &Container) {
        let data = encode_record(record);
        let handle = record.handle();
        let fits = self.foreign.len() < MAX_FOREIGN_RECORDS
            && u64::try_from(data.len()).is_ok_and(|n| n <= self.o.limits.input_bytes)
            && u64::try_from(self.foreign.len()).is_ok_and(|n| n < self.o.limits.record_count);
        if !fits {
            self.note(reasons::FOREIGN_OVERFLOW, &record.kind, handle);
            return;
        }
        self.note(reasons::RECORD_PRESERVED, &record.kind, handle);
        self.foreign.push(ForeignSpec {
            record_kind,
            container: container.clone(),
            source: handle,
            data,
        });
    }

    fn skip_rest(&mut self) {
        let d = self.d;
        let mut skipped = 0_u64;
        for o in &d.objects {
            if let arc_io_dxf::ObjectKind::Preserved(record) = &o.kind {
                self.keep(record, FOREIGN_OBJECT, &Container::Space);
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            let handle = d
                .objects
                .iter()
                .find(|o| !matches!(o.kind, arc_io_dxf::ObjectKind::Preserved(_)))
                .and_then(|o| o.source_id());
            self.rep.note_n(
                reasons::OBJECTS_SKIPPED,
                "OBJECTS",
                hex(handle),
                skipped,
                &[],
            );
        }
        let t = &d.tables;
        let skipped = [
            ("VPORT", t.vports.as_ref().map(|t| t.entries.len())),
            ("VIEW", t.views.as_ref().map(|t| t.entries.len())),
            ("UCS", t.ucs.as_ref().map(|t| t.entries.len())),
            ("APPID", t.appids.as_ref().map(|t| t.entries.len())),
        ];
        for (name, count) in skipped {
            if count.is_some_and(|c| c > 0) {
                self.rep.note(reasons::TABLE_SKIPPED, name, None, &[]);
            }
        }
        for table in &t.preserved {
            self.rep
                .note(reasons::TABLE_SKIPPED, &table.name, None, &[]);
        }
        for section in &d.preserved_sections {
            self.rep
                .note(reasons::TABLE_SKIPPED, &section.name, None, &[]);
        }
    }

    /// The header variables the mapping does not consume, as foreign rows of the target space in
    /// file order. They are brought into the target's units, and those that are not storable are
    /// reported. Those beyond [`MAX_HEADER_VARIABLES`] or the string bound of the limits are
    /// counted as overflow; the variables kept need no loss item.
    fn header_rows(&mut self) -> Vec<ForeignSpec> {
        let mut vars: Vec<HeaderVariable> = self
            .d
            .header
            .iter()
            .filter(|v| {
                !CONSUMED_HEADER
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(&v.name))
            })
            .cloned()
            .collect();
        let source =
            units::metric(units::unit_of_code(self.source_unit).unwrap_or(SpaceUnit::Unitless));
        let conversion = header::convert(
            &mut vars,
            self.factor,
            units::metric(self.o.target_unit),
            source,
        );
        header::report_conversion(&mut self.rep, &conversion);
        let budget = self.o.limits.string_bytes;
        let mut rows = Vec::new();
        let mut bytes = 0_u64;
        let mut overflow = 0_u64;
        let mut invalid = Vec::new();
        for var in &vars {
            if !header::storable(var) {
                invalid.push(var.name.clone());
                continue;
            }
            let data = encode_record(&PreservedRecord {
                kind: var.name.clone(),
                groups: var.values.clone(),
            });
            let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
            if rows.len() < MAX_HEADER_VARIABLES && bytes.saturating_add(size) <= budget {
                bytes += size;
                rows.push(ForeignSpec {
                    record_kind: FOREIGN_HEADER,
                    container: Container::Space,
                    source: None,
                    data,
                });
            } else {
                overflow += 1;
            }
        }
        if overflow > 0 {
            self.rep
                .note_n(reasons::FOREIGN_OVERFLOW, "HEADER", None, overflow, &[]);
        }
        header::report(&mut self.rep, reasons::HEADER_VALUE_OMITTED, &invalid);
        rows
    }

    pub(crate) fn finish(mut self, reader: Option<&LossReport>) -> Plan {
        self.first_sources();
        let header = self.header_rows();
        let standard_linetypes = std::mem::take(&mut self.standard_linetypes);
        let mut ops = Vec::new();
        ops.extend(self.linetypes.items.into_iter().map(Op::Linetype));
        ops.extend(self.layers.items.into_iter().map(Op::Layer));
        ops.extend(self.text_styles.items.into_iter().map(Op::TextStyle));
        ops.extend(self.dim_styles.items.into_iter().map(Op::DimStyle));
        ops.extend(self.blocks.items.into_iter().map(Op::Block));
        let (mut in_blocks, in_space): (Vec<_>, Vec<_>) = self
            .entities
            .into_iter()
            .partition(|(c, _)| matches!(c, Container::Block(_)));
        in_blocks.extend(in_space);
        ops.extend(in_blocks.into_iter().map(|(container, spec)| Op::Entity {
            container,
            spec: Box::new(spec),
        }));
        let mut rep = self.rep;
        if let Some(reader) = reader {
            rep.absorb(reader);
        }
        Plan {
            ops,
            foreign: header.into_iter().chain(self.foreign).collect(),
            version: self.d.version.name().to_owned(),
            source_unit: self.source_unit,
            factor: self.factor,
            report: rep.finish(self.d.version.name(), crate::plan::DIRECTION),
            source_document: self.o.source_document.clone(),
            standard_linetypes,
        }
    }

    /// Leaves a source handle on the first object that carries it, in file order of the
    /// definitions and then of the entities; a zero handle means none. A later object with the
    /// same handle is created without one and reported (`draft.dxf.handle_duplicate`).
    fn first_sources(&mut self) {
        // The preserved records keep their handles: an object with one of them is not bound.
        let mut seen: BTreeSet<u64> = self
            .foreign
            .iter()
            .filter_map(|f| f.source.map(|h| h.0))
            .filter(|h| *h != 0)
            .collect();
        let rep = &mut self.rep;
        let mut standard = std::mem::take(&mut self.standard_linetypes);
        for named in &mut standard {
            claim_source(&mut seen, rep, "LTYPE", &mut named.source);
        }
        standard.retain(|named| named.source.is_some());
        self.standard_linetypes = standard;
        for layer in &mut self.layers.items {
            claim_source(&mut seen, rep, "LAYER", &mut layer.source);
        }
        for linetype in &mut self.linetypes.items {
            claim_source(&mut seen, rep, "LTYPE", &mut linetype.source);
        }
        for style in &mut self.text_styles.items {
            claim_source(&mut seen, rep, "STYLE", &mut style.source);
        }
        for style in &mut self.dim_styles.items {
            claim_source(&mut seen, rep, "DIMSTYLE", &mut style.source);
        }
        for block in &mut self.blocks.items {
            claim_source(&mut seen, rep, "BLOCK_RECORD", &mut block.source);
        }
        for (_, spec) in &mut self.entities {
            claim_source(&mut seen, rep, "ENTITY", &mut spec.source);
        }
    }
}

/// Keeps `source` when no earlier object of the import carries the same handle.
fn claim_source(
    seen: &mut BTreeSet<u64>,
    rep: &mut Reporter,
    kind: &str,
    source: &mut Option<Handle>,
) {
    let Some(handle) = source.filter(|h| h.0 != 0) else {
        *source = None;
        return;
    };
    if !seen.insert(handle.0) {
        rep.note(reasons::HANDLE_DUPLICATE, kind, Some(handle.to_hex()), &[]);
        *source = None;
    }
}

/// Whether every real of a shape and its appearance is finite, so the document accepts it.
pub(crate) fn finite(shape: &Shape, a: &Appearance) -> bool {
    let p = |q: &arc_base::math::Point2| q.x.is_finite() && q.y.is_finite();
    let own = [a.elevation, a.thickness, a.linetype_scale]
        .iter()
        .chain(a.extrusion.iter())
        .all(|v| v.is_finite());
    own && match shape {
        Shape::Point(q) => p(q),
        Shape::Curve(_) | Shape::Spline { .. } | Shape::Wipeout(_) => true,
        Shape::Text(t) => p(&t.position) && t.rotation.is_finite() && t.height.is_finite(),
        Shape::RichText(t) => {
            p(&t.position)
                && [t.rotation, t.height, t.width]
                    .iter()
                    .all(|v| v.is_finite())
        }
        Shape::Dimension(d) => {
            d.points.iter().all(p)
                && p(&d.line_position)
                && d.text_position.as_ref().is_none_or(p)
                && d.rotation.is_finite()
        }
        Shape::Hatch(h) => h.families.iter().all(|f| {
            p(&f.origin)
                && [f.angle, f.shift, f.spacing].iter().all(|v| v.is_finite())
                && f.dashes.iter().all(|v| v.is_finite())
        }),
        Shape::Insert(i) => {
            p(&i.position) && i.rotation.is_finite() && i.scale.iter().all(|v| v.is_finite())
        }
    }
}
