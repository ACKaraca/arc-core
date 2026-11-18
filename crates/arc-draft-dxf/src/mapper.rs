//! The import mapper: walks a drawing and fills the plan. The tables, entities, text and hatch
//! mappings are in the sibling modules; this one holds the shared state and the orchestration.

use std::collections::{BTreeMap, BTreeSet};

use arc_draft::types::SpaceUnit;
use arc_io_dxf::loss::{Argument, LossReport, Reason};
use arc_io_dxf::{Drawing, Entity, EntityKind, Group, Handle, Value};

use crate::error::reasons;
use crate::plan::{
    Appearance, BlockSpec, Container, DimStyleSpec, EntitySpec, ImportOptions, LayerSpec,
    LinetypeSpec, Op, Plan, Shape, TextStyleSpec,
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
    pub(crate) edges: BTreeMap<String, BTreeSet<String>>,
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
            edges: BTreeMap::new(),
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
        self.skip_rest();
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

    fn skip_rest(&mut self) {
        let d = self.d;
        if !d.objects.is_empty() {
            let handle = d.objects.iter().find_map(|o| o.source_id());
            let n = u64::try_from(d.objects.len()).unwrap_or(u64::MAX);
            self.rep
                .note_n(reasons::OBJECTS_SKIPPED, "OBJECTS", hex(handle), n, &[]);
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

    pub(crate) fn finish(self, reader: Option<&LossReport>) -> Plan {
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
            source_unit: self.source_unit,
            factor: self.factor,
            report: rep.finish(self.d.version.name(), crate::plan::DIRECTION),
        }
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
