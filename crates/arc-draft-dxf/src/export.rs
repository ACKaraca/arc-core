//! Export: a model space with its tables and blocks as a neutral DXF drawing.
//!
//! [`export_drawing`] is a pure function of a snapshot. It builds the structure the format
//! requires (the standard table entries, the model and paper space blocks and their block
//! records, the layout dictionary and the two layouts) around the content. Every record gets a
//! handle and an owner here, so the writer assigns none.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};
use arc_doc::source_map::RECORD_KIND as SOURCE_RECORD_KIND;
use arc_doc::{Snapshot, StoredForeign, Value};
use arc_draft::style::{DIM_STYLE, TEXT_STYLE};
use arc_draft::types::{BLOCK, LAYER, LINETYPE, SpaceUnit};
use arc_io_dxf::loss::{Argument, Direction};
use arc_io_dxf::model::{
    AppId, Block, BlockBegin, BlockRecord, Class, Dictionary, DictionaryEntry, DimStyle, Layer,
    Layout, Linetype, LinetypeElement, Table, TableEntry, Tables, TextStyle, VPort, XData,
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
use crate::plan::{
    FOREIGN_APPID, FOREIGN_CLASS, FOREIGN_DICTIONARY, FOREIGN_ENTITY, FOREIGN_FORMAT,
    FOREIGN_HEADER, FOREIGN_OBJECT, FOREIGN_XDATA, decode_record,
};
use crate::records;
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
    /// The extended data of each object, as the rows hold it (bounded and checked for the form).
    pub(crate) xdata: BTreeMap<ObjectId, Vec<XData>>,
    /// The source number of each record of the file (a preserved record, a dictionary, a registered
    /// application, the root, or a drafted object with a binding) and the handle the export writes
    /// for it. A number that two records hold is `None`: it is ambiguous and names nothing.
    map: BTreeMap<u64, Option<Handle>>,
    /// The entries of the file's named-object root, merged into the root the export writes.
    root_entries: Vec<DictionaryEntry>,
    /// The source number of the file's named-object root.
    named_root_source: Option<u64>,
    /// The dictionaries of the file other than the named-object root, in row order.
    dictionaries: Vec<Object>,
    /// The class records of the file, in row order.
    class_rows: Vec<PreservedRecord>,
    /// The registered application records of the file, in row order.
    appid_rows: Vec<PreservedRecord>,
    /// The source bindings whose numbers are written through the map: `(object, source number)`.
    source_bindings: Vec<(ObjectId, u64)>,
    /// Owners the export replaced by the root (reported).
    owner_substituted: u64,
    /// Dictionary entries and dictionaries the export dropped (reported).
    dropped_entries: u64,
    /// References written as the null handle (reported in one item).
    nulled_references: u64,
    /// Dictionary entries the export may still write: the record limit, shared by all of them.
    entry_budget: u64,
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

    /// The handle a stored record of the file is written with: its own number when no record of the
    /// output holds it, else a new one (reported). A missing or zero number gets a new handle. The
    /// number is noted in the source map (see [`Self::note_source`]).
    pub(crate) fn claim_at(&mut self, wanted: Option<Handle>, kind: &str) -> Handle {
        let Some(wanted) = wanted.filter(|h| h.0 != 0) else {
            return self.handle();
        };
        let got = if self.taken.insert(wanted.0) {
            wanted
        } else {
            let fresh = self.handle();
            self.rep
                .note(reasons::HANDLE_REASSIGNED, kind, Some(wanted.to_hex()), &[]);
            fresh
        };
        self.note_source(wanted.0, got);
        got
    }

    /// Notes that the source number `source` of the file is written as `written`. A number noted
    /// twice names two records: it is ambiguous, removed from the map, and a reference to it is
    /// written as the null handle.
    pub(crate) fn note_source(&mut self, source: u64, written: Handle) {
        if source == 0 {
            return;
        }
        let value = if self.map.contains_key(&source) {
            None
        } else {
            Some(written)
        };
        self.map.insert(source, value);
    }

    /// Claims the handle of a preserved record (see [`Self::claim_at`]) and writes it into the
    /// record's group 5, which a record without one receives.
    pub(crate) fn claim_record(&mut self, record: &mut PreservedRecord) {
        let got = self.claim_at(record.handle(), &record.kind);
        let text = got.to_hex();
        match record.groups.iter_mut().find(|g| g.code == 5) {
            Some(group) => group.value = arc_io_dxf::Value::Text(text),
            None => record.groups.insert(0, Group::text(5, text)),
        }
    }

    /// The extended data the export writes with the object `id`.
    pub(crate) fn xdata_of(&self, id: ObjectId) -> Vec<XData> {
        self.xdata.get(&id).cloned().unwrap_or_default()
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
            entry.xdata = self.xdata_of(id);
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

/// The stored rows of format `dxf` and record kind `kind` attached to the space `space`, in id
/// order.
fn stored_rows<'s>(snapshot: &'s Snapshot, space: ObjectId, kind: &str) -> Vec<&'s StoredForeign> {
    let space_bytes = space.as_u128().to_be_bytes();
    snapshot
        .foreign_rows()
        .filter(|row| {
            row.source_format == FOREIGN_FORMAT
                && row.record_kind == kind
                && row.object.as_deref() == Some(space_bytes.as_slice())
        })
        .map(|row| row.as_ref())
        .collect()
}

/// The records of the rows [`stored_rows`] finds that decode and that the writer accepts in the
/// output form; the others are reported and skipped.
fn stored_records(
    snapshot: &Snapshot,
    space: ObjectId,
    kind: &str,
    binary: bool,
    rep: &mut Reporter,
) -> Vec<PreservedRecord> {
    let mut out = Vec::new();
    for row in stored_rows(snapshot, space, kind) {
        match decode_record(&row.data).filter(|r| {
            r.groups
                .iter()
                .all(|g| arc_io_dxf::value_writable(g, binary))
        }) {
            Some(record) => out.push(record),
            None => rep.note(reasons::EXPORT_UNMAPPED, "foreign data", None, &[]),
        }
    }
    out
}

/// The extended data rows of the snapshot, by object. A row is bounded like an import's: at most
/// `string_bytes` bytes for one object and `input_bytes` in all. A row that does not decode or
/// holds a value the output form refuses is reported (`draft.dxf.export_unmapped`); one that is not
/// well formed (unbalanced braces, a chunk over the form's bound, a name that is not a table name) or
/// is over a bound is reported as `draft.dxf.xdata_omitted`. A row of an object that is no longer
/// live is stale: it is not read and counts against no bound.
fn stored_xdata(
    snapshot: &Snapshot,
    binary: bool,
    limits: &Limits,
    rep: &mut Reporter,
) -> BTreeMap<ObjectId, Vec<XData>> {
    let mut out = BTreeMap::new();
    let mut total = 0_u64;
    for row in snapshot.foreign_rows() {
        if row.source_format != FOREIGN_FORMAT || row.record_kind != FOREIGN_XDATA {
            continue;
        }
        let Some(object) = row
            .object
            .as_deref()
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
            .map(|b| ObjectId::from_u128(u128::from_be_bytes(b)))
        else {
            continue;
        };
        if snapshot.resolve(object).is_err() {
            continue;
        }
        let source = row.source_id.clone();
        let Some(record) = decode_record(&row.data).filter(|r| r.kind == records::XDATA) else {
            rep.note(reasons::EXPORT_UNMAPPED, "XDATA", source, &[]);
            continue;
        };
        if !record
            .groups
            .iter()
            .all(|g| arc_io_dxf::value_writable(g, binary))
        {
            rep.note(reasons::EXPORT_UNMAPPED, "XDATA", source, &[]);
            continue;
        }
        let size = u64::try_from(row.data.len()).unwrap_or(u64::MAX);
        let total_next = total.saturating_add(size);
        let Some(xdata) = records::xdata_from_groups(&record.groups) else {
            rep.note(reasons::EXPORT_UNMAPPED, "XDATA", source, &[]);
            continue;
        };
        if !records::xdata_well_formed(&record.groups, binary)
            || size > limits.string_bytes
            || total_next > limits.input_bytes
        {
            rep.note(reasons::XDATA_OMITTED, "XDATA", source, &[]);
            continue;
        }
        total = total_next;
        out.insert(object, xdata);
    }
    out
}

/// The foreign rows of the kinds `entity` and `object` by the object they are attached to, in row
/// order.
fn foreign_by_object(
    snapshot: &Snapshot,
) -> BTreeMap<(ObjectId, &'static str), Vec<&StoredForeign>> {
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
    foreign
}

/// What a source number of the file names in the output.
enum Target {
    /// The number zero: it names nothing and is written as is.
    Zero,
    /// The handle the export writes for the record that held the number.
    Written(Handle),
    /// A number no record of the output holds, or one two records hold.
    Unknown,
}

/// A name for a generated root entry that no entry of `names` holds (compared without regard to
/// case); the name is added to `names`.
fn generated_name(names: &mut BTreeSet<String>, handle: Handle) -> String {
    let base = format!("ARC_{:X}", handle.0);
    let mut name = base.clone();
    let mut n = 2_u32;
    while !names.insert(name.to_lowercase()) {
        name = format!("{base}_{n}");
        n = n.saturating_add(1);
    }
    name
}

/// The source bindings of the snapshot: each live object with the source number it is bound to.
/// With a label, only the bindings of that document (the file of the export's policy); without one,
/// the bindings of every document.
fn binding_sources(snapshot: &Snapshot, label: Option<&str>) -> Vec<(ObjectId, u64)> {
    snapshot
        .foreign_rows()
        .filter_map(|row| {
            if row.source_format != FOREIGN_FORMAT || row.record_kind != SOURCE_RECORD_KIND {
                return None;
            }
            if label.is_some_and(|l| row.data.as_slice() != l.as_bytes()) {
                return None;
            }
            let source = u64::from_str_radix(row.source_id.as_deref()?, 16).ok()?;
            let bytes = <[u8; 16]>::try_from(row.object.as_deref()?).ok()?;
            let id = ObjectId::from_u128(u128::from_be_bytes(bytes));
            snapshot.resolve(id).is_ok().then_some((id, source))
        })
        .collect()
}

/// The rows of the space that the export reads, decoded once: the class and application records,
/// the dictionaries (the named-object root apart), and the extended data by object. The handles of
/// the applications and of the other dictionaries are added to `used`.
struct StoredRows {
    classes: Vec<PreservedRecord>,
    appids: Vec<PreservedRecord>,
    named_root: Option<Object>,
    dictionaries: Vec<Object>,
    xdata: BTreeMap<ObjectId, Vec<XData>>,
}

/// The named-object root of the file: the ownerless dictionary with handle 0xC, else the first
/// ownerless dictionary in file order.
fn named_root_index(dictionaries: &[Object]) -> Option<usize> {
    dictionaries
        .iter()
        .position(|o| records::is_root(o) && o.head.handle == Some(Handle(ROOT_HANDLE)))
        .or_else(|| dictionaries.iter().position(records::is_root))
}

fn read_rows(
    snapshot: &Snapshot,
    space: ObjectId,
    binary: bool,
    limits: &Limits,
    used: &mut BTreeSet<u64>,
    rep: &mut Reporter,
) -> StoredRows {
    let classes = stored_records(snapshot, space, FOREIGN_CLASS, binary, rep);
    let appids = stored_records(snapshot, space, FOREIGN_APPID, binary, rep);
    let mut dictionaries = Vec::new();
    for record in stored_records(snapshot, space, FOREIGN_DICTIONARY, binary, rep) {
        match records::dictionary_object(&record) {
            Some(object) => dictionaries.push(object),
            None => rep.note(reasons::EXPORT_UNMAPPED, "DICTIONARY", None, &[]),
        }
    }
    let named_root = named_root_index(&dictionaries).map(|at| dictionaries.remove(at));
    let xdata = stored_xdata(snapshot, binary, limits, rep);
    for record in &appids {
        used.extend(record.handle().map(|h| h.0));
    }
    for object in &dictionaries {
        used.extend(object.head.handle.map(|h| h.0));
    }
    StoredRows {
        classes,
        appids,
        named_root,
        dictionaries,
        xdata,
    }
}

/// The extended data of a drawing's objects and records, and of its preserved groups: every
/// registered application name it uses, in order of first use.
fn applications(d: &Drawing) -> Vec<String> {
    fn xdata(x: &[XData], out: &mut Vec<String>) {
        out.extend(x.iter().map(|x| x.app.clone()));
    }
    fn preserved(r: &PreservedRecord, out: &mut Vec<String>) {
        out.extend(
            r.groups
                .iter()
                .filter(|g| g.code == 1001)
                .filter_map(|g| g.as_text().map(str::to_owned)),
        );
    }
    fn table<T>(t: &Option<Table<T>>, out: &mut Vec<String>) {
        if let Some(t) = t {
            xdata(&t.xdata, out);
            for e in &t.entries {
                xdata(&e.xdata, out);
            }
        }
    }
    let mut out = Vec::new();
    let entity = |e: &Entity, out: &mut Vec<String>| {
        xdata(&e.xdata, out);
        if let EntityKind::Preserved(r) = &e.kind {
            preserved(r, out);
        }
    };
    for b in &d.blocks {
        entity(&b.begin, &mut out);
        entity(&b.end, &mut out);
        for e in &b.entities {
            entity(e, &mut out);
        }
    }
    for e in &d.entities {
        entity(e, &mut out);
    }
    for o in &d.objects {
        xdata(&o.xdata, &mut out);
        if let ObjectKind::Preserved(r) = &o.kind {
            preserved(r, &mut out);
        }
    }
    let t = &d.tables;
    table(&t.vports, &mut out);
    table(&t.linetypes, &mut out);
    table(&t.layers, &mut out);
    table(&t.styles, &mut out);
    table(&t.views, &mut out);
    table(&t.ucs, &mut out);
    table(&t.appids, &mut out);
    table(&t.dimstyles, &mut out);
    table(&t.block_records, &mut out);
    out
}

/// The number of records of each type the writer writes: entities, block records and objects,
/// by their type name (a preserved record by its own type).
fn record_counts(d: &Drawing) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    let mut add = |kind: &str| {
        let n: &mut u64 = counts.entry(kind.to_lowercase()).or_insert(0);
        *n = n.saturating_add(1);
    };
    for b in &d.blocks {
        add(b.begin.kind.type_name());
        add(b.end.kind.type_name());
        for e in &b.entities {
            add(e.kind.type_name());
        }
    }
    for e in &d.entities {
        add(e.kind.type_name());
    }
    for o in &d.objects {
        add(o.kind.type_name());
    }
    counts
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
    let mut pre = Reporter::new();
    let stored = read_rows(snapshot, space, binary, limits, &mut used, &mut pre);
    let (named_root_source, root_entries) = match &stored.named_root {
        Some(Object {
            head,
            kind: ObjectKind::Dictionary(d),
            ..
        }) => (head.handle.map(|h| h.0), d.entries.clone()),
        _ => (None, Vec::new()),
    };
    let source_bindings = binding_sources(
        snapshot,
        match policy {
            HandlePolicy::Assign => None,
            HandlePolicy::KeepSources { document, .. } => Some(document.as_str()),
        },
    );
    let (bound, floor) = match policy {
        HandlePolicy::Assign => (BTreeMap::new(), None),
        HandlePolicy::KeepSources { document, floor } => {
            let bound = bound_handles(snapshot, document);
            used.extend(bound.values().map(|h| h.0));
            (bound, Some(*floor))
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
    let foreign = foreign_by_object(snapshot);
    let header_rows = stored_rows(snapshot, space, FOREIGN_HEADER);
    let mut ex = Exporter {
        snapshot,
        rep: pre,
        next,
        layers,
        linetypes,
        text_styles: BTreeMap::new(),
        dim_styles: BTreeMap::new(),
        blocks: BTreeMap::new(),
        count: 0,
        style_handles: BTreeMap::new(),
        bound,
        taken: BTreeSet::new(),
        used,
        written: Vec::new(),
        foreign,
        exhausted: false,
        array_budget: limits.record_count.min(ARRAY_REFERENCE_BUDGET),
        binary,
        xdata: stored.xdata,
        map: BTreeMap::new(),
        root_entries,
        named_root_source,
        dictionaries: stored.dictionaries,
        class_rows: stored.classes,
        appid_rows: stored.appids,
        source_bindings,
        owner_substituted: 0,
        dropped_entries: 0,
        nulled_references: 0,
        entry_budget: limits.record_count,
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
        // A bound handle takes the root's place: the file does not fix it. A handle that a stored
        // record or a registered application holds is taken as well.
        let root = if self.used.contains(&ROOT_HANDLE) {
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
        if let Some(source) = self.named_root_source {
            self.note_source(source, root);
        }
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
        // The export writes its own layouts dictionary, so the file's layouts dictionary (the
        // target of its root's `ACAD_LAYOUT` entry) is not written and its number names ours.
        let layouts = objects.get(1).and_then(|o| o.head.handle);
        let file_layouts = self
            .root_entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("ACAD_LAYOUT"))
            .map(|e| e.handle.0);
        if let (Some(source), Some(layouts)) = (file_layouts, layouts) {
            self.note_source(source, layouts);
        }
        let mut rows = Vec::new();
        let mut layouts_row_skipped = false;
        for mut object in std::mem::take(&mut self.dictionaries) {
            let number = object.head.handle.map(|h| h.0);
            if !layouts_row_skipped && file_layouts.is_some() && number == file_layouts {
                layouts_row_skipped = true;
                continue;
            }
            object.head.handle = Some(self.claim_at(object.head.handle, "DICTIONARY"));
            rows.push(object);
        }
        for mut record in self.preserved_records(space, FOREIGN_OBJECT) {
            self.claim_record(&mut record);
            rows.push(Object::new(ObjectKind::Preserved(record)));
        }
        let classes = self.classes();
        let mut drawing = Drawing {
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
            classes,
            tables,
            blocks,
            entities,
            objects,
            preserved_sections: Vec::new(),
        };
        self.settle(&mut drawing, rows, root);
        Ok(drawing)
    }

    /// The class records of the file, one per class name (the first one wins), without regard to
    /// case. Each record is decoded once.
    fn classes(&mut self) -> Vec<Class> {
        let mut names = BTreeSet::new();
        let mut out = Vec::new();
        for record in std::mem::take(&mut self.class_rows) {
            match records::class_from_record(&record) {
                Some(class) if names.insert(class.dxf_name.to_lowercase()) => out.push(class),
                Some(_) => {}
                None => self.rep.note(reasons::EXPORT_UNMAPPED, "CLASS", None, &[]),
            }
        }
        out
    }

    /// Makes the records of the file consistent with the written drawing, each reference rewritten
    /// exactly once. The source map is complete when this runs: the drafted objects with a binding
    /// are noted first. Then the rows (dictionaries and preserved objects), the entities, the
    /// tables and the root are rewritten; the applications and the class counts follow.
    fn settle(&mut self, drawing: &mut Drawing, rows: Vec<Object>, root: Handle) {
        let written: BTreeMap<ObjectId, Handle> = self.written.iter().copied().collect();
        for (id, source) in std::mem::take(&mut self.source_bindings) {
            if let Some(handle) = written.get(&id).copied() {
                self.note_source(source, handle);
            }
        }
        // Objects that must be reachable from the root: ownerless dictionaries and records whose
        // owner the export does not write.
        let mut listed = Vec::new();
        for mut object in rows {
            if self.rewrite_row(&mut object, root, &mut listed) {
                drawing.objects.push(object);
            }
        }
        for block in &mut drawing.blocks {
            for e in [&mut block.begin, &mut block.end]
                .into_iter()
                .chain(block.entities.iter_mut())
            {
                self.rewrite_entity(e, root);
            }
        }
        for e in &mut drawing.entities {
            self.rewrite_entity(e, root);
        }
        self.rewrite_tables(&mut drawing.tables);
        self.merge_root(drawing, root);
        self.list_root(drawing, &listed);
        self.settle_appids(drawing);
        let counts = record_counts(drawing);
        for class in &mut drawing.classes {
            if class.instance_count.is_some() {
                let n = counts
                    .get(&class.dxf_name.to_lowercase())
                    .copied()
                    .unwrap_or(0);
                class.instance_count = Some(i32::try_from(n).unwrap_or(i32::MAX));
            }
        }
        if self.owner_substituted > 0 {
            self.rep.note_n(
                reasons::OWNER_SUBSTITUTED,
                "OBJECT",
                None,
                self.owner_substituted,
                &[],
            );
        }
        if self.dropped_entries > 0 {
            self.rep.note_n(
                reasons::EXPORT_UNMAPPED,
                "DICTIONARY",
                None,
                self.dropped_entries,
                &[],
            );
        }
        if self.nulled_references > 0 {
            self.rep.note_n(
                reasons::REFERENCE_NULLED,
                "REFERENCE",
                None,
                self.nulled_references,
                &[],
            );
        }
    }

    /// The written handle a source number names. A zero number names nothing (`Zero`), a number
    /// that no record holds, or one that two records hold, is `Unknown`.
    fn target(&self, handle: Handle) -> Target {
        if handle.0 == 0 {
            return Target::Zero;
        }
        match self.map.get(&handle.0) {
            Some(Some(written)) => Target::Written(*written),
            _ => Target::Unknown,
        }
    }

    /// Takes one dictionary entry from the record budget of the export; `false` when none is left.
    fn take_entry(&mut self) -> bool {
        if self.entry_budget == 0 {
            return false;
        }
        self.entry_budget -= 1;
        true
    }

    /// Rewrites one row of the file (a dictionary or a preserved object). Returns `false` when the
    /// row is not written: a dictionary whose owner is not written has no meaning without it.
    /// `listed` collects the written handles that the root must name.
    fn rewrite_row(&mut self, object: &mut Object, root: Handle, listed: &mut Vec<Handle>) -> bool {
        // A preserved object keeps its handle in its record, a dictionary in its head.
        let own = match &object.kind {
            ObjectKind::Preserved(r) => r.handle(),
            _ => object.head.handle,
        };
        let keep = match &mut object.kind {
            ObjectKind::Dictionary(d) => {
                let keep = self.dictionary_owner(&mut object.head.owner, own, root, listed);
                if keep {
                    let entries = std::mem::take(&mut d.entries);
                    d.entries = self.kept_entries(entries, root);
                }
                keep
            }
            ObjectKind::Preserved(r) => {
                self.rewrite_record(r, true, own, root, listed);
                true
            }
            _ => true,
        };
        self.rewrite_xdata(&mut object.xdata);
        keep
    }

    /// The owner of a dictionary: the written owner, or the root when it has none. A dictionary
    /// that is its own owner, or whose owner is not written, is owned by the root (listed) or
    /// dropped, respectively.
    fn dictionary_owner(
        &mut self,
        owner: &mut Option<Handle>,
        own: Option<Handle>,
        root: Handle,
        listed: &mut Vec<Handle>,
    ) -> bool {
        match owner.filter(|h| h.0 != 0) {
            None => {
                *owner = Some(root);
                listed.extend(own);
                true
            }
            Some(h) => match self.target(h) {
                Target::Written(w) if Some(w) != own => {
                    *owner = Some(w);
                    true
                }
                Target::Written(_) => {
                    *owner = Some(root);
                    self.owner_substituted += 1;
                    listed.extend(own);
                    true
                }
                _ => false,
            },
        }
    }

    /// The entries whose targets are written, with the written handles. An entry to the root
    /// itself, or to nothing written, is dropped; the record budget bounds the count.
    fn kept_entries(
        &mut self,
        entries: Vec<DictionaryEntry>,
        root: Handle,
    ) -> Vec<DictionaryEntry> {
        let mut kept = Vec::with_capacity(entries.len());
        for mut entry in entries {
            match self.target(entry.handle) {
                Target::Written(h) if h != root && self.take_entry() => {
                    entry.handle = h;
                    kept.push(entry);
                }
                _ => self.dropped_entries += 1,
            }
        }
        kept
    }

    fn rewrite_entity(&mut self, e: &mut Entity, root: Handle) {
        if let EntityKind::Preserved(r) = &mut e.kind {
            self.rewrite_record(r, false, None, root, &mut Vec::new());
        }
        self.rewrite_xdata(&mut e.xdata);
    }

    /// Rewrites the references of a preserved record. The owner of an entity is its container and
    /// is not changed here; the owner of an object is written through the map, or replaced by the
    /// root and listed. The owner is found by [`records::owner_position`], which skips application
    /// groups, so a reactor inside one is a reference like the others. Extended data (group 1005
    /// after the first group 1001) is a reference as well.
    fn rewrite_record(
        &mut self,
        r: &mut PreservedRecord,
        object: bool,
        own: Option<Handle>,
        root: Handle,
        listed: &mut Vec<Handle>,
    ) {
        let owner = records::owner_position(&r.groups);
        let xdata_at = r
            .groups
            .iter()
            .position(|g| g.code == 1001)
            .unwrap_or(r.groups.len());
        for (i, g) in r.groups.iter_mut().enumerate() {
            if g.code == 5 {
                continue;
            }
            if Some(i) == owner {
                if object {
                    self.owner_group(g, own, root, listed);
                }
                continue;
            }
            let reference = if i >= xdata_at {
                g.code == 1005
            } else {
                records::is_handle_code(g.code)
            };
            if reference {
                self.rewrite_reference(g);
            }
        }
    }

    /// The owner group of an object: a written owner is written under its new handle; any other
    /// owner (not written, or the object itself) is replaced by the root, which lists the object.
    fn owner_group(
        &mut self,
        g: &mut Group,
        own: Option<Handle>,
        root: Handle,
        listed: &mut Vec<Handle>,
    ) {
        match g.as_handle() {
            Some(h) if h.0 == 0 => {}
            found => match found.map_or(Target::Unknown, |h| self.target(h)) {
                Target::Written(w) if Some(w) != own => {
                    g.value = arc_io_dxf::Value::Text(w.to_hex());
                }
                _ => {
                    g.value = arc_io_dxf::Value::Text(root.to_hex());
                    self.owner_substituted += 1;
                    listed.extend(own);
                }
            },
        }
    }

    /// One reference: a written target is written under its new handle, zero stays zero, and any
    /// other value (a number no record of the output holds, or a malformed one) is written as the
    /// null handle and counted.
    fn rewrite_reference(&mut self, g: &mut Group) {
        let target = g.as_handle().map_or(Target::Unknown, |h| self.target(h));
        match target {
            Target::Zero => {}
            Target::Written(h) => g.value = arc_io_dxf::Value::Text(h.to_hex()),
            Target::Unknown => {
                g.value = arc_io_dxf::Value::Text("0".to_owned());
                self.nulled_references += 1;
            }
        }
    }

    fn rewrite_xdata(&mut self, xdata: &mut [XData]) {
        for x in xdata {
            for g in x.groups.iter_mut() {
                if g.code == 1005 {
                    self.rewrite_reference(g);
                }
            }
        }
    }

    fn rewrite_tables(&mut self, t: &mut Tables) {
        self.rewrite_table(&mut t.vports);
        self.rewrite_table(&mut t.linetypes);
        self.rewrite_table(&mut t.layers);
        self.rewrite_table(&mut t.styles);
        self.rewrite_table(&mut t.views);
        self.rewrite_table(&mut t.ucs);
        self.rewrite_table(&mut t.appids);
        self.rewrite_table(&mut t.dimstyles);
        self.rewrite_table(&mut t.block_records);
    }

    fn rewrite_table<T>(&mut self, table: &mut Option<Table<T>>) {
        let Some(table) = table else {
            return;
        };
        self.rewrite_xdata(&mut table.xdata);
        for entry in &mut table.entries {
            self.rewrite_xdata(&mut entry.xdata);
        }
    }

    /// Merges the entries of the file's named-object root into the root the export writes, first
    /// one per name, without regard to case. An entry whose target is not written, or is the root,
    /// is dropped; `ACAD_LAYOUT` is the export's own.
    fn merge_root(&mut self, drawing: &mut Drawing, root: Handle) {
        let entries = std::mem::take(&mut self.root_entries);
        let mut merged = Vec::new();
        for entry in entries {
            if entry.name.eq_ignore_ascii_case("ACAD_LAYOUT") {
                continue;
            }
            match self.target(entry.handle) {
                Target::Written(h) if h != root => merged.push((entry.name, h, entry.hard_owner)),
                _ => self.dropped_entries += 1,
            }
        }
        let Some(Object {
            kind: ObjectKind::Dictionary(d),
            ..
        }) = drawing.objects.first_mut()
        else {
            return;
        };
        let mut names: BTreeSet<String> = d.entries.iter().map(|e| e.name.to_lowercase()).collect();
        for (name, handle, hard_owner) in merged {
            if !names.insert(name.to_lowercase()) {
                continue;
            }
            if self.take_entry() {
                d.entries.push(DictionaryEntry {
                    name,
                    handle,
                    hard_owner,
                });
            } else {
                self.dropped_entries += 1;
            }
        }
    }

    /// Lists the objects the root must reach under generated names (see [`generated_name`]).
    fn list_root(&mut self, drawing: &mut Drawing, listed: &[Handle]) {
        let mut seen = BTreeSet::new();
        let handles: Vec<Handle> = listed
            .iter()
            .copied()
            .filter(|h| seen.insert(h.0))
            .collect();
        let Some(Object {
            kind: ObjectKind::Dictionary(d),
            ..
        }) = drawing.objects.first_mut()
        else {
            return;
        };
        let mut names: BTreeSet<String> = d.entries.iter().map(|e| e.name.to_lowercase()).collect();
        for handle in handles {
            if self.take_entry() {
                let name = generated_name(&mut names, handle);
                d.entries.push(DictionaryEntry {
                    name,
                    handle,
                    hard_owner: true,
                });
            } else {
                self.dropped_entries += 1;
            }
        }
    }

    /// Adds the registered applications that the drawing's extended data names and the APPID
    /// table lacks, with flags 0. A name that is not a valid table name is not added.
    fn settle_appids(&mut self, drawing: &mut Drawing) {
        let used = applications(drawing);
        let Some(table) = drawing.tables.appids.as_mut() else {
            return;
        };
        let owner = table.head.handle.unwrap_or_default();
        let mut have: BTreeSet<String> = table
            .entries
            .iter()
            .map(|e| e.name.to_lowercase())
            .collect();
        let mut added = Vec::new();
        for name in used {
            if records::valid_name(&name) && have.insert(name.to_lowercase()) {
                added.push(name);
            }
        }
        if added.is_empty() {
            return;
        }
        let names: Vec<&str> = added.iter().take(16).map(String::as_str).collect();
        let text = names.join(", ");
        for name in &added {
            let entry = self.entry(owner, name, 0, AppId::default());
            table.entries.push(entry);
        }
        self.rep.note_n(
            reasons::APPID_ADDED,
            "APPID",
            None,
            u64::try_from(added.len()).unwrap_or(u64::MAX),
            &[("names", Argument::Text(text))],
        );
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
        let mut appids: Table<AppId> = self.table(Handle(0));
        let owner = appids.head.handle.unwrap_or_default();
        let mut names = BTreeSet::new();
        for record in std::mem::take(&mut self.appid_rows) {
            let Some(mut entry) = records::appid_from_record(&record) else {
                self.rep.note(reasons::EXPORT_UNMAPPED, "APPID", None, &[]);
                continue;
            };
            if !records::valid_name(&entry.name) {
                self.rep.note(reasons::EXPORT_UNMAPPED, "APPID", None, &[]);
                continue;
            }
            if !names.insert(entry.name.to_lowercase()) {
                continue;
            }
            entry.head.handle = Some(self.claim_at(entry.head.handle, "APPID"));
            entry.head.owner = Some(owner);
            appids.entries.push(entry);
        }
        if !names.contains("acad") {
            appids
                .entries
                .push(self.entry(owner, "ACAD", 0, AppId::default()));
        }
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
            let mut entry = record(self, &name, handle, i16::try_from(units).unwrap_or(0));
            entry.xdata = self.xdata_of(id);
            table.entries.push(entry);
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
