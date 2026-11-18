//! The atomic incremental save (CORE-DOC-043): the objects touched since the last save are
//! written into the package in place, in one SQLite write transaction with the rollback journal
//! (`DELETE`) and `synchronous = FULL`, so that an interrupted save leaves the previous state.
//! Also the save strategies [`Package::save`] chooses between, and the recovery of a hot
//! rollback journal when a package opens for editing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};

use super::preview::Preview;
use super::read::{KEY_SAVE_COUNTER, check_header, sql_error};
use super::write::{
    INSERT_PROPERTY, INSERT_REFERENCE, Progress, SaveProgress, SaveStep, check_previews,
    current_type, id_bytes, insert_properties, insert_references, meta_entries, next_metadata,
    object_row, tick,
};
use super::{FORMAT, Metadata, Package, Save, display};
use crate::error::codes;
use crate::schema::TypeKey;
use crate::snapshot::{Object, Snapshot};

/// How an object changed since the last save: the input of the incremental save.
///
/// The document derives the map from its journal since the last save; [`touched_between`]
/// derives it from two snapshots. An object created and deleted again since the last save has
/// no entry; an object deleted and created again with the same identifier is [`Touch::Modified`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Touch {
    /// Live now, not in the package: its row is inserted.
    Created,
    /// Live now and in the package: its row is updated.
    Modified,
    /// In the package, not live now: its row is deleted.
    Deleted,
}

/// How [`Package::save`] saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Strategy {
    /// In place, writing only the rows of the objects in the map; when the incremental save is
    /// not allowed (`doc.package.incremental_refused`), by the full rewrite in place instead.
    Incremental(BTreeMap<ObjectId, Touch>),
    /// The full rewrite in place, keeping every stored asset.
    Rewrite,
    /// The full rewrite in place, keeping only the assets the saved state refers to (see
    /// [`Package::save`]): the compaction of assets.
    Compact,
    /// The full rewrite to another file (Save As, Save a Copy), keeping every stored asset; the
    /// package itself is unchanged.
    Copy(std::path::PathBuf),
}

/// The result of [`Package::save`].
#[derive(Debug)]
pub struct Saved {
    /// The metadata written. For a save in place it is the package's new metadata and its
    /// `save_counter` the new save baseline.
    pub metadata: Metadata,
    /// True when the incremental save wrote the file.
    pub incremental: bool,
    /// Why an incremental save was refused, when the full rewrite ran instead.
    pub refused: Option<Error>,
}

/// The objects touched between the saved state `before` and the state `after`, by structural
/// comparison of the typed objects of both: type, version, header and values (floating-point
/// numbers by their bits). Held objects never change and are not compared. Costs one lookup per
/// object of either snapshot. An object changed and changed back since `before` has no entry,
/// so its row keeps the `last_txn` of the last save; the map the document derives from its
/// journal lists it as [`Touch::Modified`], which writes its new last transaction.
#[must_use]
pub fn touched_between(before: &Snapshot, after: &Snapshot) -> BTreeMap<ObjectId, Touch> {
    let mut touched = BTreeMap::new();
    for (handle, object) in after.iter() {
        match typed(before, object.id) {
            None => {
                touched.insert(object.id, Touch::Created);
            }
            Some((key, old)) => {
                if Some(key) != after.type_key(handle) || !same(old, object) {
                    touched.insert(object.id, Touch::Modified);
                }
            }
        }
    }
    for (_, object) in before.iter() {
        if typed(after, object.id).is_none() {
            touched.insert(object.id, Touch::Deleted);
        }
    }
    touched
}

/// True when two objects encode to the same row.
fn same(a: &Arc<Object>, b: &Arc<Object>) -> bool {
    Arc::ptr_eq(a, b)
        || (a.version == b.version
            && a.header == b.header
            && a.values.len() == b.values.len()
            && a.values.iter().zip(&b.values).all(|pair| match pair {
                (Some(x), Some(y)) => x.bits_eq(y),
                (None, None) => true,
                _ => false,
            }))
}

/// The live typed object `id` of `snapshot` with its type key; `None` for an unknown, deleted
/// or held identifier.
pub(super) fn typed(snapshot: &Snapshot, id: ObjectId) -> Option<(&TypeKey, &Arc<Object>)> {
    let handle = snapshot.resolve(id).ok()?;
    if snapshot.held_object(handle).is_some() {
        return None;
    }
    Some((snapshot.type_key(handle)?, snapshot.get(handle).ok()?))
}

/// Opens the package at `path` for writing (`writable`) or reading, with the settings of every
/// connection to a package: values up to `input_bytes`, no trusted schema, cell size checks,
/// and for writing the rollback journal and full synchronization the in-place save needs. A
/// connection waits up to five seconds for another process's SQLite lock.
pub(super) fn connect(path: &Path, limits: &Limits, writable: bool) -> Result<Connection> {
    let name = display(path);
    let sql = |e| sql_error(e, &name);
    let access = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn =
        Connection::open_with_flags(path, access | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(sql)?;
    let length = i32::try_from(limits.input_bytes).unwrap_or(i32::MAX);
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, length)
        .map_err(sql)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(sql)?;
    conn.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA cell_size_check = ON;")
        .map_err(sql)?;
    if writable {
        conn.execute_batch("PRAGMA journal_mode = DELETE; PRAGMA synchronous = FULL;")
            .map_err(sql)?;
    }
    Ok(conn)
}

/// Rolls back the hot rollback journal an interrupted save may have left beside the package,
/// so that the file holds the previous state again; does nothing when there is none. The header
/// is checked first, so a file that is not a package or has a newer major format is not
/// touched. SQLite decides whether a journal is hot under its own file locks, so the journal of
/// a save running in another process is left alone.
pub(super) fn recover(path: &Path, limits: &Limits) -> Result<()> {
    check_header(path)?;
    let name = display(path);
    let conn = connect(path, limits, true)?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|e| sql_error(e, &name))?;
    conn.close().map_err(|(_, e)| sql_error(e, &name))
}

fn refused(reason: &'static str) -> Error {
    Error::new(codes::PACKAGE_INCREMENTAL_REFUSED).with_arg("reason", reason)
}

/// Saves `save` into `package` in place, writing only the rows of the objects in `changes` and
/// of the objects stored at an older version of a registered type (see
/// [`Package::save`]). Returns the metadata written.
pub(super) fn save(
    package: &Package,
    save: &Save<'_>,
    changes: &BTreeMap<ObjectId, Touch>,
    progress: Progress<'_>,
) -> Result<Metadata> {
    if !package.is_editable() {
        return Err(refused("read_only_package"));
    }
    if package.format != FORMAT {
        return Err(refused("format_upgrade"));
    }
    if !save.snapshot.read_only().is_empty() {
        return Err(refused("read_only_document"));
    }
    check_previews(save.previews)?;
    progress(SaveProgress::Step(SaveStep::Prepare))?;
    let path = &package.path;
    let name = display(path);
    let sql = |e| sql_error(e, &name);
    let mut conn = connect(path, &package.limits, true)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    // The changes are relative to the last save: the file must still hold it.
    let stored: Option<SqlValue> = tx
        .query_row(
            "SELECT value FROM arc_meta WHERE key = ?1",
            [KEY_SAVE_COUNTER],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql)?;
    let counter = match stored {
        None => Some(0),
        Some(SqlValue::Integer(value)) => u64::try_from(value).ok(),
        Some(_) => None,
    };
    if counter != Some(save.metadata.save_counter) {
        return Err(refused("base_changed"));
    }

    progress(SaveProgress::Step(SaveStep::Metadata))?;
    let metadata = next_metadata(save);
    write_metadata(&tx, &metadata).map_err(sql)?;

    progress(SaveProgress::Step(SaveStep::Objects))?;
    let snapshot = save.snapshot;
    let mut work: BTreeMap<ObjectId, Touch> = changes.clone();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    // Objects still stored at an older version of their registered type are written at the
    // current version by the first save, as the write-back contract requires. Every stored
    // type key is checked, so the type rows end as the full rewrite writes them.
    let (stored_keys, stale) = stale(&tx, snapshot).map_err(sql)?;
    keys.extend(stored_keys);
    for id in stale {
        work.entry(id).or_insert(Touch::Modified);
    }
    write_objects(&tx, save, &work, &mut keys, &name, progress)?;

    progress(SaveProgress::Step(SaveStep::Types))?;
    for key in &keys {
        write_types(&tx, save, key, &name)?;
    }

    progress(SaveProgress::Step(SaveStep::Previews))?;
    write_previews(&tx, save.previews).map_err(sql)?;

    progress(SaveProgress::Step(SaveStep::Commit))?;
    tx.commit().map_err(sql)?;
    conn.close().map_err(|(_, e)| sql(e))?;
    Ok(metadata)
}

/// Writes the object rows of `work` and their derived index rows, collecting the type keys
/// whose type rows may change.
fn write_objects(
    tx: &Connection,
    save: &Save<'_>,
    work: &BTreeMap<ObjectId, Touch>,
    keys: &mut BTreeSet<String>,
    name: &str,
    progress: Progress<'_>,
) -> Result<()> {
    let sql = |e| sql_error(e, name);
    let snapshot = save.snapshot;
    let registry = snapshot.registry();
    let mut count = 0;
    let mut insert = tx
        .prepare(
            "INSERT INTO arc_object (id, type_key, schema_major, schema_minor, owner, name, \
             partition, flags, encoding, payload, payload_hash, last_txn) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .map_err(sql)?;
    let mut update = tx
        .prepare(
            "UPDATE arc_object SET type_key = ?2, schema_major = ?3, schema_minor = ?4, \
             owner = ?5, name = ?6, partition = ?7, flags = ?8, encoding = ?9, \
             payload = ?10, payload_hash = ?11, last_txn = ?12 WHERE id = ?1",
        )
        .map_err(sql)?;
    let mut stored_key = tx
        .prepare("SELECT type_key FROM arc_object WHERE id = ?1")
        .map_err(sql)?;
    let mut delete = tx
        .prepare("DELETE FROM arc_object WHERE id = ?1")
        .map_err(sql)?;
    let mut delete_references = tx
        .prepare("DELETE FROM arc_reference WHERE source = ?1")
        .map_err(sql)?;
    let mut delete_properties = tx
        .prepare("DELETE FROM arc_property WHERE object = ?1")
        .map_err(sql)?;
    let mut insert_reference = tx.prepare(INSERT_REFERENCE).map_err(sql)?;
    let mut insert_property = tx.prepare(INSERT_PROPERTY).map_err(sql)?;
    for (&id, &touch) in work {
        let bytes = id_bytes(id);
        let live = typed(snapshot, id);
        match (touch, live) {
            (Touch::Created | Touch::Modified, Some((key, object))) => {
                let row = object_row(save, key, object)?;
                let values = params![
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
                ];
                let written = if touch == Touch::Created {
                    // An existing row means the changes do not describe this file.
                    match insert.execute(values) {
                        Err(rusqlite::Error::SqliteFailure(failure, _))
                            if failure.code == rusqlite::ErrorCode::ConstraintViolation =>
                        {
                            0
                        }
                        other => other.map_err(sql)?,
                    }
                } else {
                    update.execute(values).map_err(sql)?
                };
                if written != 1 {
                    return Err(refused("changes"));
                }
                if let Some(old) = stored_type_of(&mut stored_key, &bytes).map_err(sql)? {
                    keys.insert(old);
                }
                keys.insert(key.as_str().to_owned());
                delete_references.execute([bytes]).map_err(sql)?;
                delete_properties.execute([bytes]).map_err(sql)?;
                insert_references(&mut insert_reference, registry, key.as_str(), object)
                    .map_err(sql)?;
                insert_properties(&mut insert_property, registry, key.as_str(), object)
                    .map_err(sql)?;
            }
            (Touch::Deleted, None) if snapshot.resolve(id).is_err() => {
                let Some(old) = stored_type_of(&mut stored_key, &bytes).map_err(sql)? else {
                    return Err(refused("changes"));
                };
                keys.insert(old);
                delete.execute([bytes]).map_err(sql)?;
                delete_references.execute([bytes]).map_err(sql)?;
                delete_properties.execute([bytes]).map_err(sql)?;
            }
            _ => return Err(refused("changes")),
        }
        tick(progress, &mut count)?;
    }
    Ok(())
}

/// The stored type key of the row `id`, if there is one.
fn stored_type_of(
    statement: &mut rusqlite::Statement<'_>,
    id: &[u8; 16],
) -> rusqlite::Result<Option<String>> {
    statement.query_row([id], |row| row.get(0)).optional()
}

/// The type keys of the stored type rows, and the typed objects of `snapshot` whose rows are
/// stored at a version of their registered type other than the current one.
#[allow(clippy::type_complexity, reason = "a private pair of results")]
fn stale(
    conn: &Connection,
    snapshot: &Snapshot,
) -> rusqlite::Result<(BTreeSet<String>, Vec<ObjectId>)> {
    let registry = snapshot.registry();
    let versions: Vec<(String, i64, i64)> = {
        let mut statement = conn.prepare("SELECT type_key, major, minor FROM arc_type")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut select = conn.prepare(
        "SELECT id FROM arc_object WHERE type_key = ?1 AND schema_major = ?2 \
         AND schema_minor = ?3",
    )?;
    let keys = versions.iter().map(|(key, _, _)| key.clone()).collect();
    let mut found = Vec::new();
    for (key, major, minor) in versions {
        let Some(decl) = registry.get(&key) else {
            continue;
        };
        let current = decl.schema.version;
        if (major, minor) == (i64::from(current.major), i64::from(current.minor)) {
            continue;
        }
        let mut rows = select.query(params![key, major, minor])?;
        while let Some(row) = rows.next()? {
            let Ok(SqlValue::Blob(bytes)) = row.get::<_, SqlValue>(0) else {
                continue;
            };
            let Ok(raw) = <[u8; 16]>::try_from(bytes.as_slice()) else {
                continue;
            };
            let id = ObjectId::from_u128(u128::from_be_bytes(raw));
            if typed(snapshot, id).is_some() {
                found.push(id);
            }
        }
    }
    Ok((keys, found))
}

/// Brings the `arc_type` rows of `key` in line with the object rows, as the full rewrite
/// writes them: the current row of a registered type with the registry's descriptor while a
/// typed object of it exists; otherwise the row as loaded (`Loaded::types`) while a held or
/// unaddressable row of that version is stored; and no row of a version no object is stored at.
fn write_types(conn: &Connection, save: &Save<'_>, key: &str, path: &str) -> Result<()> {
    let sql = |e| sql_error(e, path);
    let snapshot = save.snapshot;
    if let Some(current) = current_type(snapshot.registry(), key)? {
        let typed = snapshot
            .iter_type(key)
            .is_ok_and(|mut objects| objects.next().is_some());
        let row = if typed {
            Some(current.clone())
        } else {
            save.loaded.and_then(|loaded| {
                loaded
                    .types
                    .iter()
                    .find(|t| {
                        (t.type_key.as_str(), t.major, t.minor)
                            == (key, current.major, current.minor)
                    })
                    .cloned()
            })
        };
        match row {
            Some(row) => conn.execute(
                "INSERT INTO arc_type (type_key, major, minor, descriptor) \
                 SELECT ?1, ?2, ?3, ?4 WHERE EXISTS (SELECT 1 FROM arc_object \
                   WHERE type_key = ?1 AND schema_major = ?2 AND schema_minor = ?3) \
                 ON CONFLICT (type_key, major, minor) DO UPDATE \
                 SET descriptor = excluded.descriptor WHERE descriptor IS NOT excluded.descriptor",
                params![row.type_key, row.major, row.minor, row.descriptor],
            ),
            None => conn.execute(
                "DELETE FROM arc_type WHERE type_key = ?1 AND major = ?2 AND minor = ?3",
                params![current.type_key, current.major, current.minor],
            ),
        }
        .map_err(sql)?;
    }
    conn.execute(
        "DELETE FROM arc_type WHERE type_key = ?1 AND NOT EXISTS (SELECT 1 FROM arc_object o \
         WHERE o.type_key = ?1 AND o.schema_major = arc_type.major \
         AND o.schema_minor = arc_type.minor)",
        [key],
    )
    .map_err(sql)?;
    Ok(())
}

/// Writes the `arc_meta` entries of `metadata` that differ from the stored ones and deletes
/// the stored keys it does not have.
fn write_metadata(conn: &Connection, metadata: &Metadata) -> rusqlite::Result<()> {
    let thumbnail = !metadata.thumbnail_missing;
    let entries = meta_entries(metadata, thumbnail);
    let mut upsert = conn.prepare(
        "INSERT INTO arc_meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE \
         SET value = excluded.value WHERE value IS NOT excluded.value \
         OR typeof(value) IS NOT typeof(excluded.value)",
    )?;
    for (key, value) in &entries {
        upsert.execute(params![key, value])?;
    }
    let keep: BTreeSet<&str> = entries.iter().map(|(key, _)| *key).collect();
    let stored: Vec<String> = {
        let mut statement = conn.prepare("SELECT key FROM arc_meta")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for key in stored.iter().filter(|key| !keep.contains(key.as_str())) {
        conn.execute("DELETE FROM arc_meta WHERE key = ?1", [key])?;
    }
    Ok(())
}

/// Writes the previews that differ from the stored ones and deletes the stored kinds not among
/// them.
pub(super) fn write_previews(conn: &Connection, previews: &[Preview]) -> rusqlite::Result<()> {
    let mut upsert = conn.prepare(
        "INSERT INTO arc_preview (kind, media_type, width, height, bytes) \
         VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (kind) DO UPDATE SET \
         media_type = excluded.media_type, width = excluded.width, height = excluded.height, \
         bytes = excluded.bytes WHERE media_type IS NOT excluded.media_type \
         OR width IS NOT excluded.width OR height IS NOT excluded.height \
         OR bytes IS NOT excluded.bytes",
    )?;
    for preview in previews {
        upsert.execute(params![
            preview.kind,
            preview.media_type,
            preview.width,
            preview.height,
            preview.bytes
        ])?;
    }
    let keep: BTreeSet<&str> = previews.iter().map(|p| p.kind.as_str()).collect();
    let stored: Vec<String> = {
        let mut statement = conn.prepare("SELECT kind FROM arc_preview")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for kind in stored.iter().filter(|kind| !keep.contains(kind.as_str())) {
        conn.execute("DELETE FROM arc_preview WHERE kind = ?1", [kind])?;
    }
    Ok(())
}
