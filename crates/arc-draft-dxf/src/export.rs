//! Export: a model space with its tables and blocks as a neutral DXF drawing.
//!
//! [`export_drawing`] is a pure function of a snapshot. It builds the structure the format
//! requires (the standard table entries, the model and paper space blocks and their block
//! records, the layout dictionary and the two layouts) around the content. Every record gets a
//! handle and an owner here, so the writer assigns none.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::{Snapshot, StoredForeign, Value};
use arc_draft::style::{DIM_STYLE, TEXT_STYLE};
use arc_draft::types::{BLOCK, LAYER, LINETYPE, SpaceUnit};
use arc_io_dxf::loss::{Argument, Direction};
use arc_io_dxf::model::{
    AppId, Block, BlockBegin, BlockRecord, Dictionary, DictionaryEntry, DimStyle, Layer, Layout,
    Linetype, LinetypeElement, Table, TableEntry, Tables, TextStyle, VPort,
};
use arc_io_dxf::model::{EntityProps, Head, HeaderVariable, SeqEnd};
use arc_io_dxf::{
    Drawing, Entity, EntityKind, Group, Handle, LossReport, Object, ObjectKind, PreservedRecord,
    Version,
};

use crate::apply::{bound_handles, model_space_unit};
use crate::color::to_true;
use crate::error::{codes, reasons};
use crate::fields::Reader;
use crate::header;
use crate::plan::{FOREIGN_ENTITY, FOREIGN_FORMAT, FOREIGN_HEADER, FOREIGN_OBJECT, decode_record};
use crate::report::Reporter;
use crate::units::code_of_unit;

/// The handle of the root dictionary the export writes unless a bound handle takes it.
const ROOT_HANDLE: u64 = 0xC;

/// The lowest handle the export assigns; the structure around the content takes the first ones.
const FIRST_ASSIGNED: u64 = 0x20;

/// The most block references one export writes for arrays, over all arrays of the drawing. An
/// export that needs more fails with `io.limit_exceeded` before it builds them.
pub const ARRAY_REFERENCE_BUDGET: u64 = 1_000_000;

/// The handles that must remain above the first assigned one: an export with fewer than 2^32
/// handles to assign above its start is refused, so that no handle wraps.
const HANDLE_ROOM: u64 = 1 << 32;

fn exhausted() -> Error {
    Error::new(codes::HANDLE_SPACE_EXHAUSTED)
}

/// How the export chooses the handles of the records it writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum HandlePolicy {
    /// Every record gets a new handle above the handles of the preserved records. The default,
    /// and the handles the export wrote before the policies existed.
    #[default]
    Assign,
    /// An object bound to the source document `document` (see [`crate::apply`]) is written with
    /// its bound handle; every other record gets a new handle above all bound handles, the
    /// handles of the preserved records and `floor`. A bound handle that another record of the
    /// file already has is given to the first one written; the other gets a new handle and is
    /// reported (`draft.dxf.handle_reassigned`).
    KeepSources {
        /// The label of the source document the bindings were made with.
        document: String,
        /// Assigned handles are above this one.
        floor: u64,
    },
}

/// The result of an export.
#[derive(Debug, Clone)]
pub struct Exported {
    /// The drawing, ready for [`arc_io_dxf::write`].
    pub drawing: Drawing,
    /// The loss report of the mapping.
    pub report: LossReport,
    /// The entities written, in the model space and in blocks.
    pub entities: u64,
    /// The handle each exported object is written with, in writing order: the objects of the
    /// document (entities, layers, linetypes, styles, block records) and not the structure.
    pub handles: Vec<(ObjectId, Handle)>,
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
    /// Whether bound handles are kept ([`HandlePolicy::KeepSources`]).
    keep: bool,
    /// The bound handle of each object written with one.
    pub(crate) bound: BTreeMap<ObjectId, Handle>,
    /// Every handle a record of the output holds, so that no two records share one.
    pub(crate) taken: BTreeSet<u64>,
    /// The handles of the preserved records and the bound handles: the ones a new handle avoids.
    used: BTreeSet<u64>,
    /// The handle each object was written with.
    pub(crate) written: Vec<(ObjectId, Handle)>,
    /// The foreign rows of the kinds `entity` and `object` by the object they are attached to,
    /// in row order; built once per export.
    pub(crate) foreign: BTreeMap<(ObjectId, &'static str), Vec<&'a StoredForeign>>,
    /// Set when a handle could not be given without wrapping; the export then fails.
    exhausted: bool,
    /// The block references the arrays may still expand to in this export.
    pub(crate) array_budget: u64,
    /// Whether the output is binary: a stored value the writer refuses in that form is omitted.
    pub(crate) binary: bool,
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

/// The objects by lowercase name, the first object of each name in identifier order: the name
/// merge of the tables compares names without regard to case, so the lookups do too.
fn by_lowercase(names: &BTreeMap<ObjectId, String>) -> BTreeMap<String, ObjectId> {
    let mut out = BTreeMap::new();
    for (id, name) in names {
        out.entry(name.to_lowercase()).or_insert(*id);
    }
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
    /// A new handle, above every handle the export has taken or was given.
    pub(crate) fn handle(&mut self) -> Handle {
        loop {
            let h = self.next;
            let Some(next) = h.checked_add(1) else {
                self.exhausted = true;
                return Handle(h);
            };
            self.next = next;
            if self.taken.insert(h) {
                return Handle(h);
            }
        }
    }

    /// The handle of a record of `object` (`None` for structure): its bound handle when the
    /// policy keeps sources and no other record has it, else `fresh`. The caller lists the
    /// object as written when it writes the record.
    pub(crate) fn handle_for(
        &mut self,
        fresh: Handle,
        object: Option<ObjectId>,
        kind: &str,
    ) -> Handle {
        let Some(wanted) = object.and_then(|id| self.bound.get(&id).copied()) else {
            return fresh;
        };
        if self.taken.insert(wanted.0) {
            return wanted;
        }
        self.rep
            .note(reasons::HANDLE_REASSIGNED, kind, Some(wanted.to_hex()), &[]);
        fresh
    }

    /// Claims the handle of a preserved record in a keeping export; a handle that another record
    /// has is replaced by a new one, which is reported.
    pub(crate) fn claim_record(&mut self, record: &mut PreservedRecord) {
        if !self.keep {
            return;
        }
        let Some(wanted) = record.handle() else {
            return;
        };
        if self.taken.insert(wanted.0) {
            return;
        }
        let fresh = self.handle();
        if let Some(group) = record.groups.iter_mut().find(|g| g.code == 5) {
            group.value = arc_io_dxf::Value::Text(fresh.to_hex());
        }
        self.rep.note(
            reasons::HANDLE_REASSIGNED,
            &record.kind,
            Some(wanted.to_hex()),
            &[],
        );
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

    /// An entry of `object`, with its bound handle when the policy keeps it.
    fn object_entry<T>(
        &mut self,
        table: Handle,
        name: &str,
        flags: i16,
        data: T,
        object: Option<ObjectId>,
        kind: &str,
    ) -> TableEntry<T> {
        let mut entry = self.entry(table, name, flags, data);
        let fresh = entry.head.handle.unwrap_or_default();
        let handle = self.handle_for(fresh, object, kind);
        entry.head.handle = Some(handle);
        if let Some(id) = object {
            self.written.push((id, handle));
        }
        entry
    }
}

/// Exports the model space `space` with every layer, linetype, style and block of the document,
/// with every handle assigned by the export ([`HandlePolicy::Assign`]).
///
/// # Errors
///
/// `draft.dxf.space_invalid` unless `space` is a model space; `draft.dxf.document_invalid` when
/// a stored object does not have the shape of its type.
pub fn export_drawing(snapshot: &Snapshot, space: ObjectId) -> Result<Exported> {
    export_drawing_with(snapshot, space, &HandlePolicy::Assign)
}

/// Exports the model space `space` as [`export_drawing`] does, choosing the handles by `policy`.
///
/// # Errors
///
/// As [`export_drawing`].
pub fn export_drawing_with(
    snapshot: &Snapshot,
    space: ObjectId,
    policy: &HandlePolicy,
) -> Result<Exported> {
    export_drawing_limited(snapshot, space, policy, &Limits::DEFAULT)
}

/// The stored header rows of the space `space`, in id order.
fn stored_header_rows(snapshot: &Snapshot, space: ObjectId) -> Vec<&StoredForeign> {
    let space_bytes = space.as_u128().to_be_bytes();
    snapshot
        .foreign_rows()
        .filter(|row| {
            row.source_format == FOREIGN_FORMAT
                && row.record_kind == FOREIGN_HEADER
                && row.object.as_deref() == Some(space_bytes.as_slice())
        })
        .map(|row| row.as_ref())
        .collect()
}

/// Sets the header of `drawing`: the stored rows of the space, checked by [`header::write_back`],
/// with the variables the export owns. A stored handle is kept only when the export writes its
/// object under that handle.
fn set_header(ex: &mut Exporter<'_>, rows: &[&StoredForeign], drawing: &mut Drawing) {
    let kept: BTreeSet<u64> = ex
        .written
        .iter()
        .filter(|(id, handle)| ex.bound.get(id) == Some(handle))
        .map(|(_, handle)| handle.0)
        .collect();
    let stored: Vec<HeaderVariable> = rows
        .iter()
        .filter_map(|row| decode_record(&row.data))
        .map(|record| HeaderVariable {
            name: record.kind,
            values: record.groups,
        })
        .collect();
    let owned = std::mem::take(&mut drawing.header);
    drawing.header = header::write_back(stored, owned, drawing, &kept, ex.binary, &mut ex.rep);
}

/// Exports the model space `space` as [`export_drawing_with`] does, with the limits `limits`
/// bounding the block references the arrays of the drawing expand to: at most the smaller of
/// `limits.record_count` and [`ARRAY_REFERENCE_BUDGET`].
///
/// # Errors
///
/// As [`export_drawing`], and `io.limit_exceeded` (argument `limit` = `record_count`) when the
/// arrays need more block references than the budget allows.
pub fn export_drawing_limited(
    snapshot: &Snapshot,
    space: ObjectId,
    policy: &HandlePolicy,
    limits: &Limits,
) -> Result<Exported> {
    export_drawing_for_mode(snapshot, space, policy, limits, false)
}

/// Exports the model space `space` as [`export_drawing_limited`] does, for the output form the
/// writer is given: `binary` as [`arc_io_dxf::WriteOptions::binary`]. A stored value the writer
/// refuses in that form is omitted and reported, so the write does not fail.
///
/// # Errors
///
/// As [`export_drawing_limited`].
pub fn export_drawing_for_mode(
    snapshot: &Snapshot,
    space: ObjectId,
    policy: &HandlePolicy,
    limits: &Limits,
    binary: bool,
) -> Result<Exported> {
    let unit = model_space_unit(snapshot, space)?;
    let (_, layers) = names_with_standard(&["0"], named(snapshot, LAYER));
    let (_, linetypes) = names_with_standard(&[], named(snapshot, LINETYPE));
    // The handles of the preserved records, which keep theirs.
    let mut used: BTreeSet<u64> = snapshot
        .foreign_rows()
        .filter(|row| {
            row.source_format == FOREIGN_FORMAT
                && matches!(row.record_kind.as_str(), FOREIGN_ENTITY | FOREIGN_OBJECT)
        })
        .filter_map(|row| u64::from_str_radix(row.source_id.as_deref()?, 16).ok())
        .collect();
    let (keep, bound, floor) = match policy {
        HandlePolicy::Assign => (false, BTreeMap::new(), None),
        HandlePolicy::KeepSources { document, floor } => {
            let bound = bound_handles(snapshot, document);
            used.extend(bound.values().map(|h| h.0));
            (true, bound, Some(*floor))
        }
    };
    // Above every handle in use, the floor and the first assigned handle.
    let above_used = match used.last() {
        Some(top) => top.checked_add(1).ok_or_else(exhausted)?,
        None => FIRST_ASSIGNED,
    };
    let above_floor = match floor {
        Some(f) => f.checked_add(1).ok_or_else(exhausted)?,
        None => FIRST_ASSIGNED,
    };
    let next = above_used.max(above_floor).max(FIRST_ASSIGNED);
    if u64::MAX - next < HANDLE_ROOM {
        return Err(exhausted());
    }
    let mut foreign: BTreeMap<(ObjectId, &'static str), Vec<&StoredForeign>> = BTreeMap::new();
    for row in snapshot.foreign_rows() {
        if row.source_format != FOREIGN_FORMAT {
            continue;
        }
        let kind = match row.record_kind.as_str() {
            FOREIGN_ENTITY => FOREIGN_ENTITY,
            FOREIGN_OBJECT => FOREIGN_OBJECT,
            _ => continue,
        };
        let Some(bytes) = row
            .object
            .as_deref()
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
        else {
            continue;
        };
        foreign
            .entry((ObjectId::from_u128(u128::from_be_bytes(bytes)), kind))
            .or_default()
            .push(row);
    }
    let header_rows = stored_header_rows(snapshot, space);
    let mut ex = Exporter {
        snapshot,
        rep: Reporter::new(),
        next,
        layers,
        linetypes,
        text_styles: BTreeMap::new(),
        dim_styles: BTreeMap::new(),
        blocks: BTreeMap::new(),
        count: 0,
        style_handles: BTreeMap::new(),
        keep,
        bound,
        taken: BTreeSet::new(),
        used,
        written: Vec::new(),
        foreign,
        exhausted: false,
        array_budget: limits.record_count.min(ARRAY_REFERENCE_BUDGET),
        binary,
    };
    ex.text_styles = named(snapshot, TEXT_STYLE).into_iter().collect();
    ex.dim_styles = named(snapshot, DIM_STYLE).into_iter().collect();
    ex.blocks = named(snapshot, BLOCK).into_iter().collect();
    let mut drawing = ex.build(space, unit)?;
    set_header(&mut ex, &header_rows, &mut drawing);
    if ex.exhausted {
        return Err(exhausted());
    }
    let entities = ex.count;
    let handles = std::mem::take(&mut ex.written);
    let report = ex.rep.finish("2018", Direction::Export);
    Ok(Exported {
        drawing,
        report,
        entities,
        handles,
    })
}

impl Exporter<'_> {
    fn build(&mut self, space: ObjectId, unit: SpaceUnit) -> Result<Drawing> {
        // A bound handle takes the root's place: the file does not fix it.
        let root = if self.keep && self.used.contains(&ROOT_HANDLE) {
            let fresh = self.handle();
            self.rep.note(
                reasons::HANDLE_REASSIGNED,
                "DICTIONARY",
                Some(Handle(ROOT_HANDLE).to_hex()),
                &[],
            );
            fresh
        } else {
            self.taken.insert(ROOT_HANDLE);
            Handle(ROOT_HANDLE)
        };
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
        let mut objects = self.layout_objects(root, model_record, paper_record);
        for mut record in self.preserved_records(space, FOREIGN_OBJECT) {
            self.claim_record(&mut record);
            objects.push(Object::new(ObjectKind::Preserved(record)));
        }
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
            let fresh = self.handle();
            let handle = self.handle_for(fresh, Some(id), "BLOCK_RECORD");
            self.written.push((id, handle));
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
        let by_name = by_lowercase(&self.layers);
        for name in names {
            let entry = match by_name.get(&name.to_lowercase()) {
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
        let mut entry = self.object_entry(owner, name, flags, data, Some(id), "LAYER");
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
        let by_name = by_lowercase(&self.linetypes);
        for name in ["ByBlock", "ByLayer", "Continuous"] {
            let object = by_name.get(&name.to_lowercase()).copied();
            let data = Linetype {
                description: if name == "Continuous" {
                    "Solid line".to_owned()
                } else {
                    String::new()
                },
                alignment: 65,
                ..Linetype::default()
            };
            table
                .entries
                .push(self.object_entry(owner, name, 0, data, object, "LTYPE"));
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
            table
                .entries
                .push(self.object_entry(owner, &name, 0, data, Some(id), "LTYPE"));
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
        let by_name = by_lowercase(&self.text_styles);
        for name in names {
            let (data, flags) = match by_name.get(&name.to_lowercase()) {
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
            let object = by_name.get(&name.to_lowercase()).copied();
            let entry = self.object_entry(owner, &name, flags, data, object, "STYLE");
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
        let by_name = by_lowercase(&self.dim_styles);
        for name in names {
            let object = by_name.get(&name.to_lowercase()).copied();
            let mut entry =
                self.object_entry(owner, &name, 0, DimStyle::default(), object, "DIMSTYLE");
            if let Some(id) = by_name.get(&name.to_lowercase()) {
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
