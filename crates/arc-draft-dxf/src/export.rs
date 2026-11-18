//! Export: a model space with its tables and blocks as a neutral DXF drawing.
//!
//! [`export_drawing`] is a pure function of a snapshot. It builds the structure the format
//! requires (the standard table entries, the model and paper space blocks and their block
//! records, the layout dictionary and the two layouts) around the content. Every record gets a
//! handle and an owner here, so the writer assigns none.

use std::collections::BTreeMap;

use arc_base::{Error, ObjectId, Result};
use arc_doc::{Snapshot, Value};
use arc_draft::style::{DIM_STYLE, TEXT_STYLE};
use arc_draft::types::{BLOCK, LAYER, LINETYPE, SpaceUnit};
use arc_io_dxf::loss::{Argument, Direction};
use arc_io_dxf::model::{
    AppId, Block, BlockBegin, BlockRecord, Dictionary, DictionaryEntry, DimStyle, Layer, Layout,
    Linetype, LinetypeElement, Table, TableEntry, Tables, TextStyle, VPort,
};
use arc_io_dxf::model::{EntityProps, Head, HeaderVariable, SeqEnd};
use arc_io_dxf::{
    Drawing, Entity, EntityKind, Group, Handle, LossReport, Object, ObjectKind, Version,
};

use crate::apply::model_space_unit;
use crate::color::to_true;
use crate::error::codes;
use crate::fields::Reader;
use crate::report::Reporter;
use crate::units::code_of_unit;

/// The result of an export.
#[derive(Debug, Clone)]
pub struct Exported {
    /// The drawing, ready for [`arc_io_dxf::write`].
    pub drawing: Drawing,
    /// The loss report of the mapping.
    pub report: LossReport,
    /// The entities written, in the model space and in blocks.
    pub entities: u64,
}

pub(crate) struct Exporter<'a> {
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) rep: Reporter,
    next: u64,
    pub(crate) layers: BTreeMap<ObjectId, String>,
    pub(crate) linetypes: BTreeMap<ObjectId, String>,
    pub(crate) text_styles: BTreeMap<ObjectId, String>,
    pub(crate) dim_styles: BTreeMap<ObjectId, String>,
    pub(crate) blocks: BTreeMap<ObjectId, String>,
    pub(crate) count: u64,
    style_handles: BTreeMap<String, Handle>,
}

pub(crate) fn invalid(what: &str) -> Error {
    Error::new(codes::DOCUMENT_INVALID).with_detail("what", what)
}

/// The objects of type `key` with their names, in the order of their names then identifiers.
fn named(snapshot: &Snapshot, key: &str) -> Vec<(ObjectId, String)> {
    let mut out: Vec<(ObjectId, String)> = match snapshot.iter_type(key) {
        Ok(iter) => iter
            .map(|(_, o)| {
                let name = Reader::new(snapshot.registry(), key, &o.values)
                    .text("name")
                    .unwrap_or_default()
                    .to_owned();
                (o.id, name)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| (&a.1, a.0).cmp(&(&b.1, b.0)));
    out
}

/// The entry names of a standard table first, then the document's names that are not among
/// them (compared without regard to case), each name once.
fn names_with_standard(
    standard: &[&str],
    found: Vec<(ObjectId, String)>,
) -> (Vec<String>, BTreeMap<ObjectId, String>) {
    let mut names: Vec<String> = standard.iter().map(|s| (*s).to_owned()).collect();
    let mut by_id = BTreeMap::new();
    for (id, name) in found {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name.clone());
        }
        by_id.insert(id, name);
    }
    (names, by_id)
}

impl Exporter<'_> {
    pub(crate) fn handle(&mut self) -> Handle {
        let h = Handle(self.next);
        self.next += 1;
        h
    }

    pub(crate) fn head(&mut self, owner: Handle) -> Head {
        Head {
            handle: Some(self.handle()),
            owner: Some(owner),
            app_groups: Vec::new(),
        }
    }

    fn table<T: Default>(&mut self, owner: Handle) -> Table<T> {
        Table {
            head: self.head(owner),
            extras: Vec::new(),
            xdata: Vec::new(),
            entries: Vec::new(),
        }
    }

    fn entry<T>(&mut self, table: Handle, name: &str, flags: i16, data: T) -> TableEntry<T> {
        TableEntry {
            head: self.head(table),
            name: name.to_owned(),
            flags,
            data,
            extras: Vec::new(),
            xdata: Vec::new(),
        }
    }
}

/// Exports the model space `space` with every layer, linetype, style and block of the document.
///
/// # Errors
///
/// `draft.dxf.space_invalid` unless `space` is a model space; `draft.dxf.document_invalid` when
/// a stored object does not have the shape of its type.
pub fn export_drawing(snapshot: &Snapshot, space: ObjectId) -> Result<Exported> {
    let unit = model_space_unit(snapshot, space)?;
    let (_, layers) = names_with_standard(&["0"], named(snapshot, LAYER));
    let (_, linetypes) = names_with_standard(&[], named(snapshot, LINETYPE));
    let mut ex = Exporter {
        snapshot,
        rep: Reporter::new(),
        next: 0x20,
        layers,
        linetypes,
        text_styles: BTreeMap::new(),
        dim_styles: BTreeMap::new(),
        blocks: BTreeMap::new(),
        count: 0,
        style_handles: BTreeMap::new(),
    };
    ex.text_styles = named(snapshot, TEXT_STYLE).into_iter().collect();
    ex.dim_styles = named(snapshot, DIM_STYLE).into_iter().collect();
    ex.blocks = named(snapshot, BLOCK).into_iter().collect();
    let drawing = ex.build(space, unit)?;
    let entities = ex.count;
    let report = ex.rep.finish("2018", Direction::Export);
    Ok(Exported {
        drawing,
        report,
        entities,
    })
}

impl Exporter<'_> {
    fn build(&mut self, space: ObjectId, unit: SpaceUnit) -> Result<Drawing> {
        let root = Handle(0xC);
        let model_record = self.handle();
        let paper_record = self.handle();
        let (tables, block_handles) = self.tables(model_record, paper_record)?;
        let mut blocks = vec![
            self.structural_block("*Model_Space", model_record),
            self.structural_block("*Paper_Space", paper_record),
        ];
        let mut ids: Vec<(ObjectId, Handle)> = block_handles.into_iter().collect();
        ids.sort_by_key(|(id, _)| (self.blocks.get(id).cloned(), *id));
        for (id, record) in ids {
            blocks.push(self.block(id, record)?);
        }
        let entities = self.container_entities(space, model_record)?;
        let objects = self.layout_objects(root, model_record, paper_record);
        Ok(Drawing {
            version: Version::R2018,
            header: vec![
                HeaderVariable {
                    name: "$INSUNITS".to_owned(),
                    values: vec![Group::int(70, code_of_unit(unit))],
                },
                HeaderVariable {
                    name: "$HANDSEED".to_owned(),
                    values: vec![Group::text(5, "1")],
                },
            ],
            classes: Vec::new(),
            tables,
            blocks,
            entities,
            objects,
            preserved_sections: Vec::new(),
        })
    }

    fn structural_block(&mut self, name: &str, record: Handle) -> Block {
        let begin = Entity {
            head: self.head(record),
            props: EntityProps {
                layer: "0".to_owned(),
                ..EntityProps::default()
            },
            kind: EntityKind::Block(BlockBegin {
                name: name.to_owned(),
                flags: 0,
                ..BlockBegin::default()
            }),
            extras: Vec::new(),
            xdata: Vec::new(),
        };
        let end = self.block_end(record);
        Block {
            begin,
            entities: Vec::new(),
            end,
        }
    }

    fn block_end(&mut self, record: Handle) -> Entity {
        Entity {
            head: self.head(record),
            props: EntityProps {
                layer: "0".to_owned(),
                ..EntityProps::default()
            },
            kind: EntityKind::BlockEnd(SeqEnd::default()),
            extras: Vec::new(),
            xdata: Vec::new(),
        }
    }

    fn layout_objects(&mut self, root: Handle, model: Handle, paper: Handle) -> Vec<Object> {
        let layouts_dict = self.handle();
        let model_layout = self.handle();
        let paper_layout = self.handle();
        let object = |handle: Handle, owner: Handle, kind: ObjectKind| {
            let mut o = Object::new(kind);
            o.head.handle = Some(handle);
            o.head.owner = Some(owner);
            o
        };
        let entry = |name: &str, handle: Handle| DictionaryEntry {
            name: name.to_owned(),
            handle,
            hard_owner: true,
        };
        let layout = |name: &str, order: i16, record: Handle| {
            ObjectKind::Layout(Layout {
                name: name.to_owned(),
                flags: 1,
                tab_order: order,
                block_record: Some(record),
                ..Layout::default()
            })
        };
        vec![
            object(
                root,
                Handle(0),
                ObjectKind::Dictionary(Dictionary {
                    cloning: Some(1),
                    entries: vec![entry("ACAD_LAYOUT", layouts_dict)],
                    ..Dictionary::default()
                }),
            ),
            object(
                layouts_dict,
                root,
                ObjectKind::Dictionary(Dictionary {
                    cloning: Some(1),
                    entries: vec![entry("Layout1", paper_layout), entry("Model", model_layout)],
                    ..Dictionary::default()
                }),
            ),
            object(model_layout, layouts_dict, layout("Model", 0, model)),
            object(paper_layout, layouts_dict, layout("Layout1", 1, paper)),
        ]
    }

    fn tables(
        &mut self,
        model: Handle,
        paper: Handle,
    ) -> Result<(Tables, BTreeMap<ObjectId, Handle>)> {
        let mut tables = Tables::default();
        let appids: Table<AppId> = {
            let mut t = self.table(Handle(0));
            let owner = t.head.handle.unwrap_or_default();
            t.entries
                .push(self.entry(owner, "ACAD", 0, AppId::default()));
            t
        };
        tables.appids = Some(appids);
        let mut vports: Table<VPort> = self.table(Handle(0));
        let owner = vports.head.handle.unwrap_or_default();
        vports.entries.push(self.entry(
            owner,
            "*Active",
            0,
            VPort {
                upper_right: [1.0, 1.0],
                direction: [0.0, 0.0, 1.0],
                height: 1.0,
                aspect: 1.0,
                lens: 50.0,
                ..VPort::default()
            },
        ));
        tables.vports = Some(vports);
        tables.styles = Some(self.text_style_table()?);
        tables.linetypes = Some(self.linetype_table()?);
        tables.layers = Some(self.layer_table()?);
        tables.dimstyles = Some(self.dim_style_table()?);
        let (records, handles) = self.block_records(model, paper)?;
        tables.block_records = Some(records);
        Ok((tables, handles))
    }

    fn block_records(
        &mut self,
        model: Handle,
        paper: Handle,
    ) -> Result<(Table<BlockRecord>, BTreeMap<ObjectId, Handle>)> {
        let mut table: Table<BlockRecord> = self.table(Handle(0));
        let owner = table.head.handle.unwrap_or_default();
        let record = |this: &mut Self, name: &str, handle: Handle, units: i16| {
            let mut e = this.entry(owner, name, units, BlockRecord::default());
            e.head.handle = Some(handle);
            e
        };
        table.entries.push(record(self, "*Model_Space", model, 0));
        table.entries.push(record(self, "*Paper_Space", paper, 0));
        let mut handles = BTreeMap::new();
        let ids: Vec<(ObjectId, String)> =
            self.blocks.iter().map(|(i, n)| (*i, n.clone())).collect();
        for (id, name) in ids {
            let handle = self.handle();
            let object = self.snapshot.get(self.snapshot.resolve(id)?)?;
            let units = Reader::new(self.snapshot.registry(), BLOCK, &object.values)
                .enum_code("unit")
                .and_then(SpaceUnit::of)
                .map_or(0, code_of_unit);
            table.entries.push(record(
                self,
                &name,
                handle,
                i16::try_from(units).unwrap_or(0),
            ));
            handles.insert(id, handle);
        }
        Ok((table, handles))
    }

    fn layer_table(&mut self) -> Result<Table<Layer>> {
        let mut table: Table<Layer> = self.table(Handle(0));
        let owner = table.head.handle.unwrap_or_default();
        let (names, _) = names_with_standard(&["0"], named(self.snapshot, LAYER));
        let by_name: BTreeMap<String, ObjectId> =
            self.layers.iter().map(|(i, n)| (n.clone(), *i)).collect();
        for name in names {
            let entry = match by_name.get(&name) {
                Some(id) => self.layer_entry(owner, *id, &name)?,
                None => self.entry(owner, &name, 0, plain_layer()),
            };
            table.entries.push(entry);
        }
        Ok(table)
    }

    fn layer_entry(
        &mut self,
        owner: Handle,
        id: ObjectId,
        name: &str,
    ) -> Result<TableEntry<Layer>> {
        let object = self.snapshot.get(self.snapshot.resolve(id)?)?;
        let r = Reader::new(self.snapshot.registry(), LAYER, &object.values);
        let color = match r.get("color") {
            Some(Value::Color(c)) => Some(c.as_ref().clone()),
            _ => None,
        };
        let aci = color.as_ref().and_then(|c| match c.source {
            Some(arc_doc::ColorSource::Indexed(n)) => Some(i16::from(n)),
            _ => None,
        });
        let on = r.bool_or("on", true);
        let magnitude = aci.unwrap_or(7);
        let data = Layer {
            color: if on { magnitude } else { -magnitude },
            true_color: color.filter(|_| aci.is_none()).map(|c| to_true(&c)),
            linetype: r
                .reference("linetype")
                .and_then(|l| self.linetypes.get(&l).cloned())
                .unwrap_or_else(|| "Continuous".to_owned()),
            plot: Some(r.bool_or("plottable", true)),
            lineweight: Some(lineweight_code(r.f64("lineweight"))),
            ..Layer::default()
        };
        let flags =
            i16::from(r.bool_or("frozen", false)) | (i16::from(r.bool_or("locked", false)) << 2);
        let mut entry = self.entry(owner, name, flags, data);
        let transparency = r.f64_or("transparency", 0.0);
        if transparency > 0.0 {
            entry.extras.push(Group::int(
                440,
                0x0200_0000 | i64::from(alpha_of(transparency)),
            ));
        }
        Ok(entry)
    }

    fn linetype_table(&mut self) -> Result<Table<Linetype>> {
        let mut table: Table<Linetype> = self.table(Handle(0));
        let owner = table.head.handle.unwrap_or_default();
        for name in ["ByBlock", "ByLayer", "Continuous"] {
            let data = Linetype {
                description: if name == "Continuous" {
                    "Solid line".to_owned()
                } else {
                    String::new()
                },
                alignment: 65,
                ..Linetype::default()
            };
            table.entries.push(self.entry(owner, name, 0, data));
        }
        let ids: Vec<(ObjectId, String)> = self
            .linetypes
            .iter()
            .map(|(i, n)| (*i, n.clone()))
            .collect();
        let mut sorted = ids;
        sorted.sort_by(|a, b| (&a.1, a.0).cmp(&(&b.1, b.0)));
        for (id, name) in sorted {
            if ["bylayer", "byblock", "continuous"].contains(&name.to_lowercase().as_str()) {
                continue;
            }
            let data = self.linetype_data(id)?;
            table.entries.push(self.entry(owner, &name, 0, data));
        }
        Ok(table)
    }

    fn linetype_data(&mut self, id: ObjectId) -> Result<Linetype> {
        let object = self.snapshot.get(self.snapshot.resolve(id)?)?;
        let r = Reader::new(self.snapshot.registry(), LINETYPE, &object.values);
        let mut elements = Vec::new();
        if let Some(Value::List(list)) = r.get("elements") {
            for item in list {
                let Value::Record(f) = item else { continue };
                elements.push(self.element(f));
            }
        }
        let total = elements.iter().map(|e| e.length.abs()).sum();
        Ok(Linetype {
            description: r.text("description").unwrap_or_default().to_owned(),
            alignment: 65,
            pattern_length: total,
            elements,
        })
    }

    fn element(&self, f: &[Option<Value>]) -> LinetypeElement {
        let real = |i: usize| match f.get(i) {
            Some(Some(Value::F64(v))) => Some(*v),
            _ => None,
        };
        let text = |i: usize| match f.get(i) {
            Some(Some(Value::String(s))) => Some(s.clone()),
            _ => None,
        };
        let kind = match f.first() {
            Some(Some(Value::Enum(c))) => *c,
            _ => 1,
        };
        let length = real(1).unwrap_or(0.0);
        let style = text(3).and_then(|n| self.style_handles.get(&n.to_lowercase()).copied());
        let offset = match f.get(6) {
            Some(Some(Value::Point2(p))) => Some([p.x, p.y]),
            _ => None,
        };
        LinetypeElement {
            length: match kind {
                1 => -length,
                2 => 0.0,
                _ => length,
            },
            kind: match kind {
                3 => 2,
                4 => 4,
                _ => 0,
            },
            shape: None,
            style,
            scale: real(4),
            rotation: real(5).map(f64::to_degrees),
            offset_x: offset.map(|o| o[0]),
            offset_y: offset.map(|o| o[1]),
            text: text(2),
        }
    }

    fn text_style_table(&mut self) -> Result<Table<TextStyle>> {
        let mut table: Table<TextStyle> = self.table(Handle(0));
        let owner = table.head.handle.unwrap_or_default();
        let (names, _) = names_with_standard(&["Standard"], named(self.snapshot, TEXT_STYLE));
        let by_name: BTreeMap<String, ObjectId> = self
            .text_styles
            .iter()
            .map(|(i, n)| (n.clone(), *i))
            .collect();
        for name in names {
            let (data, flags) = match by_name.get(&name) {
                Some(id) => self.text_style_data(*id)?,
                None => (
                    TextStyle {
                        width_factor: 1.0,
                        font: "txt".to_owned(),
                        ..TextStyle::default()
                    },
                    0,
                ),
            };
            let entry = self.entry(owner, &name, flags, data);
            if let Some(h) = entry.head.handle {
                self.style_handles.insert(name.to_lowercase(), h);
            }
            table.entries.push(entry);
        }
        Ok(table)
    }

    fn text_style_data(&self, id: ObjectId) -> Result<(TextStyle, i16)> {
        let object = self.snapshot.get(self.snapshot.resolve(id)?)?;
        let r = Reader::new(self.snapshot.registry(), TEXT_STYLE, &object.values);
        let generation = i16::from(r.bool_or("backwards", false)) * 2
            + i16::from(r.bool_or("upside_down", false)) * 4;
        Ok((
            TextStyle {
                height: r.f64_or("height", 0.0),
                width_factor: r.f64_or("width_factor", 1.0),
                oblique: r.f64_or("oblique", 0.0).to_degrees(),
                generation,
                last_height: 0.0,
                font: r
                    .text("font")
                    .map(|f| format!("{f}.ttf"))
                    .unwrap_or_default(),
                big_font: None,
            },
            if r.bool_or("vertical", false) { 4 } else { 0 },
        ))
    }

    fn dim_style_table(&mut self) -> Result<Table<DimStyle>> {
        let mut table: Table<DimStyle> = self.table(Handle(0));
        let owner = table.head.handle.unwrap_or_default();
        let (names, _) = names_with_standard(&["Standard"], named(self.snapshot, DIM_STYLE));
        let by_name: BTreeMap<String, ObjectId> = self
            .dim_styles
            .iter()
            .map(|(i, n)| (n.clone(), *i))
            .collect();
        for name in names {
            let mut entry = self.entry(owner, &name, 0, DimStyle::default());
            if let Some(id) = by_name.get(&name) {
                let object = self.snapshot.get(self.snapshot.resolve(*id)?)?;
                let r = Reader::new(self.snapshot.registry(), DIM_STYLE, &object.values);
                for (code, field) in [
                    (40, "overall_scale"),
                    (41, "arrow_size"),
                    (42, "extension_offset"),
                    (44, "extension_beyond"),
                    (140, "text_height"),
                    (144, "measurement_scale"),
                    (147, "text_gap"),
                ] {
                    if let Some(v) = r.f64(field) {
                        entry.extras.push(Group::double(code, v));
                    }
                }
                if let Some(Value::I64(p)) = r.get("precision") {
                    entry.extras.push(Group::int(271, *p));
                }
            }
            table.entries.push(entry);
        }
        Ok(table)
    }

    fn block(&mut self, id: ObjectId, record: Handle) -> Result<Block> {
        let object = self.snapshot.get(self.snapshot.resolve(id)?)?;
        let r = Reader::new(self.snapshot.registry(), BLOCK, &object.values);
        let base = match r.get("base_point") {
            Some(Value::Point2(p)) => [p.x, p.y, 0.0],
            _ => [0.0; 3],
        };
        let name = self.blocks.get(&id).cloned().unwrap_or_default();
        let begin = Entity {
            head: self.head(record),
            props: EntityProps {
                layer: "0".to_owned(),
                ..EntityProps::default()
            },
            kind: EntityKind::Block(BlockBegin {
                name,
                flags: i16::from(
                    matches!(r.get("attributes"), Some(Value::List(l)) if !l.is_empty()),
                ),
                base,
                ..BlockBegin::default()
            }),
            extras: Vec::new(),
            xdata: Vec::new(),
        };
        let mut entities = self.attribute_definitions(&r, record);
        entities.extend(self.container_entities(id, record)?);
        let end = self.block_end(record);
        Ok(Block {
            begin,
            entities,
            end,
        })
    }
}

fn plain_layer() -> Layer {
    Layer {
        color: 7,
        linetype: "Continuous".to_owned(),
        lineweight: Some(-3),
        ..Layer::default()
    }
}

/// The DXF lineweight code of a lineweight in millimeters: hundredths of a millimeter, -3 for
/// the default.
pub(crate) fn lineweight_code(mm: Option<f64>) -> i16 {
    match mm {
        None => -3,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "clamped to the range of i16"
        )]
        Some(v) => (v * 100.0).round().clamp(0.0, 2100.0) as i16,
    }
}

/// The alpha byte (255 opaque) of a transparency of 0 (opaque) to 1.
pub(crate) fn alpha_of(transparency: f64) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    let alpha = ((1.0 - transparency) * 255.0).round().clamp(0.0, 255.0) as u8;
    alpha
}

pub(crate) fn note_what(
    rep: &mut Reporter,
    reason: arc_io_dxf::loss::Reason,
    kind: &str,
    what: &str,
) {
    rep.note(
        reason,
        kind,
        None,
        &[("what", Argument::Text(what.to_owned()))],
    );
}
