//! Writing a package by the atomic full rewrite (CORE-DOC-044): a complete package is written to
//! a temporary file beside the target, verified, flushed and renamed over the target. A failure
//! or cancellation before the rename leaves the target as it was and removes the temporary file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, params};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};

use super::asset::Carry;
use super::preview::{self, Preview, THUMBNAIL};
use super::read::{
    KEY_CREATED_BY, KEY_CRS, KEY_DOCUMENT_ID, KEY_FORMAT, KEY_PARTITIONS, KEY_RETENTION,
    KEY_SAVE_COUNTER, KEY_SAVED_BY, KEY_THUMBNAIL, KEY_UNITS, KEYS, THUMBNAIL_NONE, check_header,
    hex, io_error, sql_error,
};
use super::{APPLICATION_ID, FORMAT, MetaValue, Metadata, display, schema};
use crate::descriptor::descriptor;
use crate::error::codes;
use crate::load::{Loaded, StoredObject, StoredType};
use crate::registry::TypeRegistry;
use crate::relationship::references;
use crate::schema::{RelationshipRule, TypeKey, Value};
use crate::snapshot::{Object, Snapshot};

/// A step of a full rewrite, reported before it starts. Returning an error from the progress
/// callback cancels the save there; every step up to and including [`SaveStep::Replace`] can be
/// cancelled and leaves the target untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveStep {
    /// Creating the temporary file beside the target.
    Prepare,
    /// Creating the schema.
    Schema,
    /// Writing `arc_meta`.
    Metadata,
    /// Writing `arc_type`.
    Types,
    /// Writing `arc_object`.
    Objects,
    /// Writing the derived indexes `arc_reference` and `arc_property`.
    Indexes,
    /// Writing `arc_preview`.
    Previews,
    /// Copying the assets and chunks of the package saved from (a full rewrite through
    /// [`super::Package::save`] or [`super::Package::save_to`]); nothing otherwise.
    Assets,
    /// Committing the temporary file.
    Commit,
    /// Verifying the temporary file.
    Verify,
    /// Flushing the temporary file to stable storage.
    Flush,
    /// Renaming the temporary file over the target.
    Replace,
}

impl SaveStep {
    /// Every step, in order.
    pub const ALL: [Self; 12] = [
        Self::Prepare,
        Self::Schema,
        Self::Metadata,
        Self::Types,
        Self::Objects,
        Self::Indexes,
        Self::Previews,
        Self::Assets,
        Self::Commit,
        Self::Verify,
        Self::Flush,
        Self::Replace,
    ];
}

/// What a save reports while it runs: the step about to start, and the number of rows written
/// so far in the current step (reported every [`ROWS_PER_TICK`] rows). Returning an error from
/// the callback cancels the save there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SaveProgress {
    /// A step is about to start.
    Step(SaveStep),
    /// Rows written so far in the current step.
    Rows(u64),
}

/// How many rows a save writes between two [`SaveProgress::Rows`] reports.
pub const ROWS_PER_TICK: u64 = 256;

/// The progress callback of the internal save functions.
pub(super) type Progress<'a> = &'a mut dyn FnMut(SaveProgress) -> Result<()>;

/// Counts rows and reports every [`ROWS_PER_TICK`]th.
pub(super) fn tick(
    progress: &mut dyn FnMut(SaveProgress) -> Result<()>,
    rows: &mut u64,
) -> Result<()> {
    *rows += 1;
    if rows.is_multiple_of(ROWS_PER_TICK) {
        progress(SaveProgress::Rows(*rows))?;
    }
    Ok(())
}

/// The complete contents of a package, written as given by [`write`].
#[derive(Debug, Clone, Copy)]
pub struct Contents<'a> {
    /// The metadata; `format_version` is the format this version writes and `thumbnail` follows
    /// from the previews.
    pub metadata: &'a Metadata,
    /// The `arc_type` rows.
    pub types: &'a [StoredType],
    /// The `arc_object` rows, unchanged: identifiers, hashes and payloads are written as given.
    pub objects: &'a [StoredObject],
    /// The previews.
    pub previews: &'a [Preview],
    /// The snapshot the derived indexes (`arc_reference`, `arc_property`) are built from; none
    /// leaves them empty.
    pub snapshot: Option<&'a Snapshot>,
}

/// What [`save_as`] saves.
#[derive(Debug, Clone, Copy)]
pub struct Save<'a> {
    /// The state to save.
    pub snapshot: &'a Snapshot,
    /// The load the snapshot descends from: its type rows and side table. `None` for a document
    /// that was never loaded; every object must then be in `touched`.
    pub loaded: Option<&'a Loaded>,
    /// The objects changed since load and the transaction (its stored bytes) that last changed
    /// each; every other object keeps its stored `last_txn`.
    pub touched: &'a BTreeMap<ObjectId, Vec<u8>>,
    /// The metadata; the save writes the next `save_counter`.
    pub metadata: &'a Metadata,
    /// The previews the application rendered; empty when none are available.
    pub previews: &'a [Preview],
    /// The limits payloads are encoded under.
    pub limits: Limits,
}

/// Saves `save` to `target` by the full rewrite, following the write-back contract of loading
/// (`docs/crates/arc-doc.md`, "Write-back contract"), and returns the metadata written.
///
/// A writable snapshot writes every held row and unaddressable row unchanged, every object of
/// the typed stores encoded at the current version of its type with the BLAKE3 hash of the bytes
/// written and its `last_txn` from `touched` or the load, the current type row of every
/// registered type it writes objects of, and the stored type row of every row written
/// unchanged. A read-only snapshot is saved as a copy with every row as stored and every type row
/// as read. Fails like [`write`], and with `doc.package.last_txn_missing` for an object without a
/// last transaction and the encoding errors of [`crate::payload::encode`]; nothing is written
/// then.
pub fn save_as(
    target: &Path,
    save: &Save<'_>,
    progress: &mut dyn FnMut(SaveStep) -> Result<()>,
) -> Result<Metadata> {
    save_full(target, save, None, &mut steps_only(progress))
}

/// Adapts a step callback to [`SaveProgress`].
fn steps_only(
    progress: &mut dyn FnMut(SaveStep) -> Result<()>,
) -> impl FnMut(SaveProgress) -> Result<()> + '_ {
    move |event| match event {
        SaveProgress::Step(step) => progress(step),
        SaveProgress::Rows(_) => Ok(()),
    }
}

/// [`save_as`], copying the assets of `carry` when given.
pub(super) fn save_full(
    target: &Path,
    save: &Save<'_>,
    carry: Option<&Carry<'_>>,
    progress: Progress<'_>,
) -> Result<Metadata> {
    let (types, objects) = rows(save, progress)?;
    let metadata = next_metadata(save);
    let contents = Contents {
        metadata: &metadata,
        types: &types,
        objects: &objects,
        previews: save.previews,
        snapshot: Some(save.snapshot),
    };
    write_with(target, &contents, carry, progress)?;
    Ok(metadata)
}

/// The metadata a save writes: the next `save_counter` and `thumbnail` from the previews.
pub(super) fn next_metadata(save: &Save<'_>) -> Metadata {
    let mut metadata = save.metadata.clone();
    metadata.save_counter = metadata.save_counter.saturating_add(1);
    metadata.thumbnail_missing = !save.previews.iter().any(|p| p.kind == THUMBNAIL);
    metadata
}

/// The row of the typed object `object` of type `key` by the write-back contract: encoded at
/// the current version of its type, the BLAKE3 hash of its payload and its `last_txn` from
/// `save.touched` or the load. Shared by the full rewrite and the incremental save, so both
/// write identical rows.
pub(super) fn object_row(save: &Save<'_>, key: &TypeKey, object: &Object) -> Result<StoredObject> {
    let not_found = || Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", object.id.to_string());
    let decl = save
        .snapshot
        .registry()
        .get(key.as_str())
        .ok_or_else(not_found)?;
    let mut row = StoredObject::of(key, &decl.schema, object, &save.limits)?;
    row.payload_hash = blake3::hash(&row.payload).as_bytes().to_vec();
    row.last_txn = match save.touched.get(&object.id) {
        Some(txn) => txn.clone(),
        None => save
            .loaded
            .and_then(|l| l.rows.last_txn(object.id))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                Error::new(codes::PACKAGE_LAST_TXN_MISSING).with_arg("id", object.id.to_string())
            })?,
    };
    Ok(row)
}

/// The current `arc_type` row of the registered type `key`.
pub(super) fn current_type(registry: &TypeRegistry, key: &str) -> Result<Option<StoredType>> {
    let Some(decl) = registry.get(key) else {
        return Ok(None);
    };
    let version = decl.schema.version;
    let text = descriptor(decl, version)
        .ok_or_else(|| Error::new(codes::TYPE_FIELD_INVALID).with_arg("type", key))?;
    Ok(Some(StoredType {
        type_key: key.to_owned(),
        major: version.major.into(),
        minor: version.minor.into(),
        descriptor: text,
    }))
}

/// The type and object rows of a save by the write-back contract.
fn rows(save: &Save<'_>, progress: Progress<'_>) -> Result<(Vec<StoredType>, Vec<StoredObject>)> {
    let snapshot = save.snapshot;
    let registry = snapshot.registry();
    let mut count = 0;
    let read_only = !snapshot.read_only().is_empty();
    let loaded = save.loaded;
    let mut kept: Vec<StoredObject> = snapshot
        .held()
        .map(|(_, held)| held.row().clone())
        .collect();
    let mut rows = Vec::with_capacity(snapshot.len());
    let mut written = BTreeSet::new();
    for (handle, object) in snapshot.iter() {
        let not_found =
            || Error::new(codes::OBJECT_NOT_FOUND).with_arg("id", object.id.to_string());
        let key = snapshot.type_key(handle).ok_or_else(not_found)?;
        if read_only && let Some(original) = loaded.and_then(|l| l.rows.get(object.id)) {
            kept.push(original.stored.clone());
            continue;
        }
        rows.push(object_row(save, key, object)?);
        written.insert(key.clone());
        tick(progress, &mut count)?;
    }
    if let Some(loaded) = loaded {
        kept.extend(loaded.rows.unaddressable().iter().cloned());
    }
    let stored_types = loaded.map_or(&[][..], |l| l.types.as_slice());
    if read_only {
        rows.extend(kept);
        return Ok((stored_types.to_vec(), rows));
    }
    let mut types: BTreeMap<(String, i64, i64), StoredType> = BTreeMap::new();
    for key in &written {
        if let Some(row) = current_type(registry, key.as_str())? {
            types.insert((row.type_key.clone(), row.major, row.minor), row);
        }
    }
    for row in &kept {
        let stored = stored_types.iter().find(|t| {
            (t.type_key.as_str(), t.major, t.minor) == (row.type_key.as_str(), row.major, row.minor)
        });
        if let Some(stored) = stored {
            types
                .entry((stored.type_key.clone(), stored.major, stored.minor))
                .or_insert_with(|| stored.clone());
        }
    }
    rows.extend(kept);
    Ok((types.into_values().collect(), rows))
}

/// Distinguishes the temporary files of concurrent saves of one process.
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// The temporary file of a save to `target`: `~<file name>.<process id>-<n>.tmp` beside it.
fn temporary(target: &Path) -> Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| Error::new(codes::PACKAGE_IO_FAILED).with_arg("path", display(target)))?;
    let n = TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    Ok(target.with_file_name(format!("~{}.{pid}-{n}.tmp", name.to_string_lossy())))
}

/// Writes `contents` to `target` by the full rewrite: a new package in a temporary file beside
/// the target (created exclusively), verified (`PRAGMA integrity_check`, header, object count
/// and a BLAKE3 summary of every object's identifier, stored hash and payload), flushed to
/// stable storage and renamed over the target; on Unix the directory is flushed afterwards,
/// best effort. `progress` is called before each [`SaveStep`] and cancels the save by returning
/// an error.
///
/// Fails with `doc.package.io_failed` when a file operation fails, `doc.package.verify_failed`
/// when the written file does not verify, `doc.package.preview_invalid` for an invalid or
/// repeated preview kind and `doc.object.duplicate_id` for two rows with one identifier. On
/// every failure before the rename the target is unchanged and the temporary file is removed.
pub fn write(
    target: &Path,
    contents: &Contents<'_>,
    progress: &mut dyn FnMut(SaveStep) -> Result<()>,
) -> Result<()> {
    write_with(target, contents, None, &mut steps_only(progress))
}

/// Checks every preview and that no kind repeats (`doc.package.preview_invalid`).
pub(super) fn check_previews(previews: &[Preview]) -> Result<()> {
    let mut kinds = BTreeSet::new();
    for preview in previews {
        preview::check(preview)?;
        if !kinds.insert(preview.kind.as_str()) {
            return Err(Error::new(codes::PACKAGE_PREVIEW_INVALID)
                .with_arg("kind", preview.kind.clone())
                .with_detail("reason", "duplicate"));
        }
    }
    Ok(())
}

/// [`write`], copying the assets of `carry` when given.
fn write_with(
    target: &Path,
    contents: &Contents<'_>,
    carry: Option<&Carry<'_>>,
    progress: Progress<'_>,
) -> Result<()> {
    check_previews(contents.previews)?;
    let mut objects: Vec<&StoredObject> = contents.objects.iter().collect();
    objects.sort_by(|a, b| a.id.cmp(&b.id));
    if let Some(pair) = objects
        .windows(2)
        .find(|pair| pair.first().map(|r| &r.id) == pair.get(1).map(|r| &r.id))
    {
        let id = pair.first().map(|row| hex(&row.id)).unwrap_or_default();
        return Err(Error::new(codes::OBJECT_DUPLICATE_ID).with_arg("id", id));
    }
    progress(SaveProgress::Step(SaveStep::Prepare))?;
    let temp = temporary(target)?;
    File::create_new(&temp).map_err(|e| io_error(e, &temp))?;
    // Removes the temporary file on every exit before the rename, an unwinding one included.
    let mut guard = Temporary(Some(temp.clone()));
    build(&temp, contents, &objects, carry, progress)?;
    progress(SaveProgress::Step(SaveStep::Verify))?;
    verify(&temp, contents, &objects)?;
    progress(SaveProgress::Step(SaveStep::Flush))?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temp)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_error(e, &temp))?;
    progress(SaveProgress::Step(SaveStep::Replace))?;
    // Replaces the target atomically on the same file system; on Windows `std` replaces an
    // existing file, which fails while another process has it open without delete sharing.
    fs::rename(&temp, target).map_err(|e| io_error(e, target))?;
    guard.0 = None;
    sync_directory(target);
    Ok(())
}

/// A temporary file and its journal, removed when dropped unless taken.
struct Temporary(Option<PathBuf>);

impl Drop for Temporary {
    fn drop(&mut self) {
        if let Some(temp) = self.0.take() {
            let _ = fs::remove_file(&temp);
            let mut journal = temp.into_os_string();
            journal.push("-journal");
            let _ = fs::remove_file(PathBuf::from(journal));
        }
    }
}

/// Flushes the directory entry of the rename, best effort: the new file is complete either way.
fn sync_directory(target: &Path) {
    #[cfg(unix)]
    {
        let dir = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        let _ = File::open(dir).and_then(|dir| dir.sync_all());
    }
    #[cfg(not(unix))]
    let _ = target;
}

fn meta_value(value: &MetaValue) -> SqlValue {
    match value {
        MetaValue::Null => SqlValue::Null,
        MetaValue::Integer(value) => SqlValue::Integer(*value),
        MetaValue::Real(value) => SqlValue::Real(*value),
        MetaValue::Text(value) => SqlValue::Text(value.clone()),
        MetaValue::Blob(value) => SqlValue::Blob(value.clone()),
    }
}

/// The `arc_meta` entries of `metadata`.
pub(super) fn meta_entries(metadata: &Metadata, thumbnail: bool) -> Vec<(&str, SqlValue)> {
    let text =
        |key, value: &Option<String>| value.clone().map(|value| (key, SqlValue::Text(value)));
    let mut entries: Vec<(&str, SqlValue)> = [
        Some((KEY_FORMAT, SqlValue::Text(FORMAT.to_string()))),
        metadata
            .document_id
            .map(|id| (KEY_DOCUMENT_ID, SqlValue::Text(id.to_string()))),
        text(KEY_CREATED_BY, &metadata.created_by),
        text(KEY_SAVED_BY, &metadata.saved_by),
        text(KEY_UNITS, &metadata.units),
        text(KEY_CRS, &metadata.coordinate_reference),
        Some((
            KEY_SAVE_COUNTER,
            SqlValue::Integer(i64::try_from(metadata.save_counter).unwrap_or(i64::MAX)),
        )),
        text(KEY_RETENTION, &metadata.history_retention),
        (!metadata.partitions.is_empty()).then(|| {
            let json = serde_json::to_string(&metadata.partitions).unwrap_or_default();
            (KEY_PARTITIONS, SqlValue::Text(json))
        }),
        (!thumbnail).then(|| (KEY_THUMBNAIL, SqlValue::Text(THUMBNAIL_NONE.to_owned()))),
    ]
    .into_iter()
    .flatten()
    .collect();
    for (key, value) in &metadata.other {
        if !KEYS.contains(&key.as_str()) {
            entries.push((key.as_str(), meta_value(value)));
        }
    }
    entries
}

/// The `arc_property` value and unit of an indexed field's value: scalars only.
pub(super) fn property(value: &Value) -> Option<(SqlValue, Option<String>)> {
    Some(match value {
        Value::Bool(value) => (SqlValue::Integer(i64::from(*value)), None),
        Value::I64(value) => (SqlValue::Integer(*value), None),
        Value::F64(value) => (SqlValue::Real(*value), None),
        Value::String(value) => (SqlValue::Text(value.clone()), None),
        Value::Enum(code) => (SqlValue::Integer(i64::from(*code)), None),
        Value::Quantity(quantity) => {
            let unit = serde_json::to_string(&quantity.dimension().exponents()).ok();
            (SqlValue::Real(quantity.value()), unit)
        }
        _ => return None,
    })
}

pub(super) fn id_bytes(id: ObjectId) -> [u8; 16] {
    id.as_u128().to_be_bytes()
}

/// The statement inserting one `arc_reference` row.
pub(super) const INSERT_REFERENCE: &str = "INSERT INTO arc_reference \
    (source, field, target, target_link, rule) VALUES (?1, ?2, ?3, NULL, ?4)";
/// The statement inserting one `arc_property` row.
pub(super) const INSERT_PROPERTY: &str =
    "INSERT INTO arc_property (object, key, value, unit) VALUES (?1, ?2, ?3, ?4)";

/// Writes the `arc_reference` rows of `object` of type `key` (empty for a held object whose
/// type is not registered: then only its owner) with `insert` ([`INSERT_REFERENCE`]).
pub(super) fn insert_references(
    insert: &mut rusqlite::Statement<'_>,
    registry: &TypeRegistry,
    key: &str,
    object: &Object,
) -> rusqlite::Result<()> {
    for reference in references(registry, key, object) {
        insert.execute(params![
            id_bytes(object.id),
            reference.field.to_string(),
            id_bytes(reference.target),
            rule_text(&reference.spec.rule)
        ])?;
    }
    Ok(())
}

/// The `arc_reference.rule` text of `rule`.
pub(super) fn rule_text(rule: &RelationshipRule) -> String {
    serde_json::json!({
        "strength": rule.strength,
        "on_delete": rule.on_delete,
        "on_copy": rule.on_copy,
    })
    .to_string()
}

/// Writes the `arc_property` rows of the typed object `object` of type `key` with `insert`
/// ([`INSERT_PROPERTY`]): one per indexed field with a scalar value.
pub(super) fn insert_properties(
    insert: &mut rusqlite::Statement<'_>,
    registry: &TypeRegistry,
    key: &str,
    object: &Object,
) -> rusqlite::Result<()> {
    let Some(decl) = registry.get(key) else {
        return Ok(());
    };
    for (field, value) in decl.schema.fields.iter().zip(&object.values) {
        if let (true, Some((value, unit))) = (field.indexed, value.as_ref().and_then(property)) {
            insert.execute(params![id_bytes(object.id), field.name, value, unit])?;
        }
    }
    Ok(())
}

/// Writes the derived indexes of `snapshot`: every reference of every object (held objects:
/// their owner) and every indexed scalar field of the typed stores.
fn indexes(
    conn: &Connection,
    snapshot: &Snapshot,
    path: &str,
    progress: Progress<'_>,
) -> Result<()> {
    let registry = snapshot.registry();
    let sql = |e| sql_error(e, path);
    let mut insert_reference = conn.prepare(INSERT_REFERENCE).map_err(sql)?;
    let mut insert_property = conn.prepare(INSERT_PROPERTY).map_err(sql)?;
    let mut count = 0;
    let typed = snapshot.iter().map(|(handle, object)| (handle, &**object));
    let held = snapshot
        .held()
        .map(|(handle, held)| (handle, &**held.object()));
    for (handle, object) in typed.chain(held) {
        let key = snapshot.type_key(handle).map_or("", |key| key.as_str());
        insert_references(&mut insert_reference, registry, key, object).map_err(sql)?;
        if snapshot.held_object(handle).is_none() {
            insert_properties(&mut insert_property, registry, key, object).map_err(sql)?;
        }
        tick(progress, &mut count)?;
    }
    Ok(())
}

/// Writes the complete package into the empty file `temp`.
fn build(
    temp: &Path,
    contents: &Contents<'_>,
    objects: &[&StoredObject],
    carry: Option<&Carry<'_>>,
    progress: Progress<'_>,
) -> Result<()> {
    let name = display(temp);
    let sql = |e| sql_error(e, &name);
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(temp, flags).map_err(sql)?;
    // The temporary file is verified and flushed as a whole before it replaces anything, so it
    // needs neither a journal on disk nor flushes of its own. The rollback-journal mode of the
    // file at rest (`DELETE`) is SQLite's default and not stored in the file.
    conn.execute_batch(&format!(
        "PRAGMA page_size = 4096; PRAGMA encoding = 'UTF-8'; PRAGMA journal_mode = MEMORY; \
         PRAGMA synchronous = OFF; PRAGMA application_id = {}; PRAGMA user_version = {};",
        i64::from(APPLICATION_ID),
        FORMAT.major
    ))
    .map_err(sql)?;
    let tx = conn.transaction().map_err(sql)?;
    progress(SaveProgress::Step(SaveStep::Schema))?;
    schema::create(&tx).map_err(sql)?;
    progress(SaveProgress::Step(SaveStep::Metadata))?;
    let thumbnail = contents.previews.iter().any(|p| p.kind == THUMBNAIL);
    {
        let mut insert = tx
            .prepare("INSERT INTO arc_meta (key, value) VALUES (?1, ?2)")
            .map_err(sql)?;
        for (key, value) in meta_entries(contents.metadata, thumbnail) {
            insert.execute(params![key, value]).map_err(sql)?;
        }
    }
    progress(SaveProgress::Step(SaveStep::Types))?;
    {
        let mut types: Vec<&StoredType> = contents.types.iter().collect();
        types.sort_by(|a, b| (&a.type_key, a.major, a.minor).cmp(&(&b.type_key, b.major, b.minor)));
        let mut insert = tx
            .prepare(
                "INSERT INTO arc_type (type_key, major, minor, descriptor) VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(sql)?;
        for row in types {
            insert
                .execute(params![row.type_key, row.major, row.minor, row.descriptor])
                .map_err(sql)?;
        }
    }
    progress(SaveProgress::Step(SaveStep::Objects))?;
    {
        let mut insert = tx
            .prepare(
                "INSERT INTO arc_object (id, type_key, schema_major, schema_minor, owner, name, \
                 partition, flags, encoding, payload, payload_hash, last_txn) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )
            .map_err(sql)?;
        let mut count = 0;
        for row in objects {
            tick(progress, &mut count)?;
            insert
                .execute(params![
                    row.id,
                    row.type_key,
                    row.major,
                    row.minor,
                    row.owner,
                    row.name,
                    row.partition,
                    row.flags,
                    row.encoding,
                    row.payload,
                    row.payload_hash,
                    row.last_txn
                ])
                .map_err(sql)?;
        }
    }
    progress(SaveProgress::Step(SaveStep::Indexes))?;
    if let Some(snapshot) = contents.snapshot {
        indexes(&tx, snapshot, &name, progress)?;
    }
    progress(SaveProgress::Step(SaveStep::Previews))?;
    super::incremental::write_previews(&tx, contents.previews).map_err(sql)?;
    progress(SaveProgress::Step(SaveStep::Assets))?;
    if let Some(carry) = carry {
        super::asset::copy(carry, &tx, progress)?;
    }
    progress(SaveProgress::Step(SaveStep::Commit))?;
    tx.commit().map_err(sql)?;
    conn.close().map_err(|(_, e)| sql(e))
}

/// The summary of objects verified after writing: count, then for each object in identifier
/// order its identifier, stored hash and the BLAKE3 hash of its payload.
#[derive(Default)]
struct Summary {
    count: u64,
    hasher: blake3::Hasher,
}

impl Summary {
    fn add(&mut self, id: &[u8], hash: &[u8], payload: &[u8]) {
        self.count += 1;
        for part in [id, hash] {
            let len = u64::try_from(part.len()).unwrap_or(u64::MAX);
            self.hasher.update(&len.to_le_bytes());
            self.hasher.update(part);
        }
        self.hasher.update(blake3::hash(payload).as_bytes());
    }

    fn finish(&self) -> (u64, [u8; 32]) {
        (self.count, *self.hasher.finalize().as_bytes())
    }
}

/// Verifies the written temporary file.
fn verify(temp: &Path, contents: &Contents<'_>, objects: &[&StoredObject]) -> Result<()> {
    let name = display(temp);
    let failed = |what: &'static str| {
        Error::new(codes::PACKAGE_VERIFY_FAILED)
            .with_arg("path", name.clone())
            .with_detail("check", what)
    };
    let sql = |e| sql_error(e, &name);
    check_header(temp).map_err(|_| failed("header"))?;
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(temp, flags).map_err(sql)?;
    let integrity: Vec<String> = {
        let mut statement = conn.prepare("PRAGMA integrity_check").map_err(sql)?;
        let rows = statement.query_map([], |row| row.get(0)).map_err(sql)?;
        rows.collect::<rusqlite::Result<_>>().map_err(sql)?
    };
    if integrity != ["ok"] {
        return Err(failed("integrity_check"));
    }
    let types: i64 = conn
        .query_row("SELECT count(*) FROM arc_type", [], |row| row.get(0))
        .map_err(sql)?;
    if usize::try_from(types).ok() != Some(contents.types.len()) {
        return Err(failed("types"));
    }
    let mut expected = Summary::default();
    for row in objects {
        expected.add(&row.id, &row.payload_hash, &row.payload);
    }
    let mut actual = Summary::default();
    let mut statement = conn
        .prepare("SELECT id, payload_hash, payload FROM arc_object ORDER BY id")
        .map_err(sql)?;
    let mut rows = statement.query([]).map_err(sql)?;
    while let Some(row) = rows.next().map_err(sql)? {
        let bytes = |at| match row.get_ref(at) {
            Ok(rusqlite::types::ValueRef::Blob(bytes)) => Some(bytes),
            _ => None,
        };
        let (Some(id), Some(hash), Some(payload)) = (bytes(0), bytes(1), bytes(2)) else {
            return Err(failed("objects"));
        };
        actual.add(id, hash, payload);
    }
    let (expected, actual) = (expected.finish(), actual.finish());
    if actual != expected {
        return Err(failed("objects"));
    }
    Ok(())
}
