//! Inspection and check of a package (CORE-DOC-050): a JSON dump of the metadata, statistics
//! and objects decoded through the descriptors stored in the package, and a check of SQLite
//! integrity, payload hashes, references, derived indexes and assets. Both open the file
//! read-only and never write to it or its directory.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, params};
use serde_json::{Map, Value as Json, json};

use arc_base::{Error, Limits, Result};

use super::read::{self, Cells, hash_mismatch, hex, rows, sql_error, stored_object};
use super::write::{property, rule_text};
use super::{MetaValue, asset, display};
use crate::error::codes;
use crate::load::{StoredObject, parse_header, parse_id, parse_version};
use crate::payload::{self, Encoding};
use crate::relationship::{OWNER_SPEC, field_references};
use crate::schema::{RelationshipRule, Schema, SchemaVersion, Value};

/// The most lines of `PRAGMA integrity_check` a check reports.
const INTEGRITY_LINES: u32 = 64;

/// The schemas of the stored descriptors by type key and version, or why one does not parse.
type Schemas = BTreeMap<(String, i64, i64), Result<Schema>>;

/// Dumps the package at `path` as JSON without any registry: every object is decoded with the
/// descriptor stored for its type and version, so objects of types the caller does not know
/// decode too.
///
/// The result is an object with `format_version`, `metadata` (every `arc_meta` entry: text,
/// numbers and `null` as they are, a blob as lowercase hexadecimal), `statistics` (`file_bytes`,
/// counts of objects, type rows, references, properties, journal records, assets, chunks and
/// previews, the bytes of payloads, assets and chunks, and per type key its `objects` and
/// `payload_bytes`), `types` (each type row with its descriptor as JSON, or `error`) and
/// `objects` in identifier order: `id`, `type_key`, `version`, `owner`, `name`, `partition`,
/// `flags`, `encoding`, `payload_bytes`, `hash_matches`, `last_txn`, and either `values` (field
/// name to the canonical JSON form of the value, `null` when absent) or `error` (`code` and
/// `arguments`) when the row does not decode with its descriptor.
///
/// The file is opened as [`super::Package::open`] opens it read-only, with its failures
/// (`doc.package.not_a_package`, `doc.package.newer_format`, `doc.package.schema_invalid`,
/// `doc.package.recovery_needed` for a hot journal); `record_count` bounds the rows of each
/// table and the other limits every descriptor and payload.
pub fn inspect(path: &Path, limits: &Limits) -> Result<Json> {
    let (conn, format, _) = read::open(path, limits)?;
    let name = display(path);
    let schemas = schemas(&conn, &name, limits)?;
    let metadata: Map<String, Json> = read::meta_entries(&conn, &name, limits)?
        .into_iter()
        .map(|(key, value)| (key, meta_json(value)))
        .collect();
    let types: Vec<Json> = stored_types(&conn, &name, limits)?
        .into_iter()
        .map(|(key, major, minor, text)| {
            let parsed = schemas.get(&(key.clone(), major, minor));
            let (member, value) = match (parsed, serde_json::from_str::<Json>(&text)) {
                (Some(Ok(_)), Ok(descriptor)) => ("descriptor", descriptor),
                (Some(Err(error)), _) => ("error", error_json(error)),
                _ => ("error", Json::Null),
            };
            json!({ "type_key": key, "major": major, "minor": minor, member: value })
        })
        .collect();
    let mut per_type: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let objects = rows(
        &conn,
        "SELECT id, type_key, schema_major, schema_minor, owner, name, partition, flags, \
         encoding, payload, payload_hash, last_txn FROM arc_object ORDER BY id",
        "arc_object",
        &name,
        limits,
        |cells| {
            let row = stored_object(cells)?;
            let entry = per_type.entry(row.type_key.clone()).or_default();
            entry.0 += 1;
            entry.1 = entry.1.saturating_add(len(row.payload.len()));
            Ok(object_json(&row, &schemas, limits))
        },
    )?;
    let payload_bytes: u64 = per_type.values().map(|(_, bytes)| *bytes).sum();
    let count = |sql: &str| -> Result<i64> {
        conn.query_row(sql, [], |row| row.get(0))
            .map_err(|e| sql_error(e, &name))
    };
    let file_bytes = std::fs::metadata(path)
        .map_err(|e| read::io_error(e, path))?
        .len();
    let statistics = json!({
        "file_bytes": file_bytes,
        "objects": objects.len(),
        "payload_bytes": payload_bytes,
        "types": types.len(),
        "references": count("SELECT count(*) FROM arc_reference")?,
        "properties": count("SELECT count(*) FROM arc_property")?,
        "journal_records": count("SELECT count(*) FROM arc_journal")?,
        "assets": count("SELECT count(*) FROM arc_asset")?,
        "asset_bytes": count("SELECT coalesce(sum(byte_length), 0) FROM arc_asset")?,
        "chunks": count("SELECT count(*) FROM arc_chunk")?,
        "chunk_bytes": count("SELECT coalesce(sum(length(bytes)), 0) FROM arc_chunk")?,
        "previews": count("SELECT count(*) FROM arc_preview")?,
        "per_type": per_type
            .into_iter()
            .map(|(key, (objects, bytes))| (key, json!({ "objects": objects, "payload_bytes": bytes })))
            .collect::<Map<String, Json>>(),
    });
    Ok(json!({
        "format_version": format.to_string(),
        "metadata": metadata,
        "statistics": statistics,
        "types": types,
        "objects": objects,
    }))
}

/// Checks the package at `path` without writing to it and returns the findings, each an
/// error with a stable code; an empty list means no problem was found.
///
/// - `PRAGMA integrity_check` must report `ok`; otherwise each reported line (at most 64) is a
///   `doc.package.corrupt` finding (detail `message`) and the other checks do not run.
/// - Every type row's descriptor must parse ([`Schema::from_descriptor`]); every object row
///   must have a valid header (`doc.load.header_invalid`), a version with a type row
///   (`doc.load.version_undeclared`), a payload that decodes with that descriptor (the
///   `doc.payload.*` code) and, for encoding 0, a `payload_hash` equal to the BLAKE3 hash of
///   the payload (`doc.package.hash_mismatch`). Each finding about a row carries the row's
///   identifier in hexadecimal as detail `id`.
/// - Every reference (the owner and, for a decoded row, every reference value) must name a
///   stored object of a type its field allows (`doc.reference.dangling`, as loading reports it).
/// - The derived indexes must equal those recomputed from the rows (`doc.package.index_mismatch`,
///   arguments `table` and `id`): for each object either the full rows (owner and reference
///   values in `arc_reference`, indexed scalar fields in `arc_property`) or, as a save writes
///   them for an object whose type it did not have registered or whose payload does not
///   decode, the owner row alone and no property row; and no index row may name a missing
///   object.
/// - Every asset must read back with matching chunk and asset hashes (`doc.package.asset_corrupt`,
///   or the code its reader fails with).
///
/// Fails, without findings, as [`inspect`] fails to open the file, and with
/// `doc.package.io_failed`, `doc.package.corrupt` or `doc.load.limit_exceeded` when reading
/// fails.
pub fn check(path: &Path, limits: &Limits) -> Result<Vec<Error>> {
    let (conn, _, _) = read::open(path, limits)?;
    let name = display(path);
    let sql = |e| sql_error(e, &name);
    let corrupt = integrity(&conn, &name)?;
    if !corrupt.is_empty() {
        return Ok(corrupt);
    }
    let mut findings = Vec::new();
    let schemas = schemas(&conn, &name, limits)?;
    for ((key, major, minor), schema) in &schemas {
        if let Err(error) = schema {
            findings.push(
                copy(error)
                    .with_detail("type", key.clone())
                    .with_detail("version", format!("{major}.{minor}")),
            );
        }
    }
    // The type of every stored identifier, for the reference checks.
    let types: BTreeMap<Vec<u8>, String> = rows(
        &conn,
        "SELECT id, type_key FROM arc_object ORDER BY id",
        "arc_object",
        &name,
        limits,
        |cells| Ok((cells.blob(0, "id")?, cells.text(1, "type_key")?)),
    )?
    .into_iter()
    .collect();
    let mut select_references = conn
        .prepare("SELECT field, target, target_link, rule FROM arc_reference WHERE source = ?1")
        .map_err(sql)?;
    let mut select_properties = conn
        .prepare("SELECT key, value, unit FROM arc_property WHERE object = ?1")
        .map_err(sql)?;
    let mut statement = conn
        .prepare(
            "SELECT id, type_key, schema_major, schema_minor, owner, name, partition, flags, \
             encoding, payload, payload_hash, last_txn FROM arc_object ORDER BY id",
        )
        .map_err(sql)?;
    let mut cursor = statement.query([]).map_err(sql)?;
    while let Some(row) = cursor.next().map_err(sql)? {
        let row = stored_object(&Cells {
            row,
            table: "arc_object",
            path: &name,
        })?;
        let stored = StoredIndex {
            references: index_rows(&mut select_references, &row.id).map_err(sql)?,
            properties: index_rows(&mut select_properties, &row.id).map_err(sql)?,
        };
        check_object(&row, &schemas, &types, &stored, limits, &mut findings);
    }
    for (table, column) in [("arc_reference", "source"), ("arc_property", "object")] {
        let orphans = rows(
            &conn,
            &format!(
                "SELECT DISTINCT {column} FROM {table} \
                 WHERE {column} NOT IN (SELECT id FROM arc_object) ORDER BY {column}"
            ),
            table,
            &name,
            limits,
            |cells| match cells.any(0, column)? {
                MetaValue::Blob(bytes) => Ok(hex(&bytes)),
                _ => Ok(String::new()),
            },
        )?;
        findings.extend(orphans.into_iter().map(|id| index_mismatch(table, id)));
    }
    let assets = rows(
        &conn,
        "SELECT hash FROM arc_asset ORDER BY hash",
        "arc_asset",
        &name,
        limits,
        |cells| cells.any(0, "hash"),
    )?;
    drop(cursor);
    drop(statement);
    drop(select_references);
    drop(select_properties);
    drop(conn);
    findings.extend(
        assets
            .into_iter()
            .filter_map(|hash| check_asset(path, hash, limits).err()),
    );
    Ok(findings)
}

/// The lines of `PRAGMA integrity_check` as `doc.package.corrupt` findings; none when it
/// reports `ok`.
fn integrity(conn: &Connection, name: &str) -> Result<Vec<Error>> {
    let sql = |e| sql_error(e, name);
    let mut statement = conn
        .prepare(&format!("PRAGMA integrity_check({INTEGRITY_LINES})"))
        .map_err(sql)?;
    let lines: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .map_err(sql)?
        .collect::<rusqlite::Result<_>>()
        .map_err(sql)?;
    if lines == ["ok"] {
        return Ok(Vec::new());
    }
    Ok(lines
        .into_iter()
        .map(|line| {
            Error::new(codes::PACKAGE_CORRUPT)
                .with_arg("path", name.to_owned())
                .with_detail("message", line)
        })
        .collect())
}

/// Reads the asset `hash` to its end, verifying every chunk and the whole.
fn check_asset(path: &Path, hash: MetaValue, limits: &Limits) -> Result<()> {
    let MetaValue::Blob(bytes) = hash else {
        return Err(asset_corrupt(""));
    };
    let raw = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| asset_corrupt(&hex(&bytes)))?;
    let mut reader = asset::open(path, &crate::schema::AssetHash(raw), limits)?;
    io::copy(&mut reader, &mut io::sink())
        .map(drop)
        .map_err(|e| reader_error(e, path))
}

/// The index rows stored for one object, each as the text of its columns, sorted.
struct StoredIndex {
    references: Vec<Vec<SqlValue>>,
    properties: Vec<Vec<SqlValue>>,
}

fn index_rows(
    statement: &mut rusqlite::Statement<'_>,
    id: &[u8],
) -> rusqlite::Result<Vec<Vec<SqlValue>>> {
    let columns = statement.column_count();
    let mut rows: Vec<Vec<SqlValue>> = statement
        .query_map(params![id], |row| {
            (0..columns).map(|at| row.get(at)).collect()
        })?
        .collect::<rusqlite::Result<_>>()?;
    rows.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    Ok(rows)
}

/// Checks one object row and its index rows.
fn check_object(
    row: &StoredObject,
    schemas: &Schemas,
    types: &BTreeMap<Vec<u8>, String>,
    stored: &StoredIndex,
    limits: &Limits,
    findings: &mut Vec<Error>,
) {
    let id = hex(&row.id);
    let mut report = |error: Error| findings.push(error.with_detail("id", id.clone()));
    if parse_id(&row.id).is_none() {
        report(Error::new(codes::LOAD_HEADER_INVALID).with_arg("column", "id"));
    }
    let (header, invalid) = parse_header(row, limits);
    if let Some(error) = invalid {
        report(error);
    }
    if hash_mismatch(row) {
        report(Error::new(codes::PACKAGE_HASH_MISMATCH).with_arg("id", id.clone()));
    }
    let decoded = match decode(row, schemas, limits) {
        Ok(decoded) => Some(decoded),
        Err(error) => {
            report(error);
            None
        }
    };
    // The owner first, then the reference values of a decoded row.
    let mut owner = Vec::new();
    let mut checked = Vec::new();
    if let Some(target) = header.owner {
        let target = target_bytes(target);
        owner.push(reference_row("(owner)", &target, &OWNER_SPEC.rule));
        checked.push(("(owner)".to_owned(), target, &OWNER_SPEC.targets));
    }
    let mut full = owner.clone();
    let mut properties = Vec::new();
    if let Some((schema, values)) = &decoded {
        for reference in field_references(&schema.fields, values) {
            let field = reference.field.to_string();
            let target = target_bytes(reference.target);
            full.push(reference_row(&field, &target, &reference.spec.rule));
            checked.push((field, target, &reference.spec.targets));
        }
        for (field, value) in schema.fields.iter().zip(values) {
            if let (true, Some((value, unit))) = (field.indexed, value.as_ref().and_then(property))
            {
                let unit = unit.map_or(SqlValue::Null, SqlValue::Text);
                properties.push(vec![SqlValue::Text(field.name.clone()), value, unit]);
            }
        }
    }
    for (field, target, allowed) in checked {
        let kind = types.get(&target);
        let allowed = kind.is_some_and(|kind| {
            allowed.is_empty() || allowed.iter().any(|key| key.as_str() == kind)
        });
        if !allowed {
            report(
                Error::new(codes::REFERENCE_DANGLING)
                    .with_arg("field", field)
                    .with_detail("source", id.clone())
                    .with_detail("target", hex(&target)),
            );
        }
    }
    let sort = |rows: &mut Vec<Vec<SqlValue>>| {
        rows.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    };
    sort(&mut owner);
    sort(&mut full);
    sort(&mut properties);
    let as_full = stored.references == full;
    let as_owner = stored.references == owner;
    let properties_match = match (as_full, as_owner) {
        (true, false) => stored.properties == properties,
        (false, true) => stored.properties.is_empty(),
        _ => stored.properties == properties || stored.properties.is_empty(),
    };
    if !as_full && !as_owner {
        report(index_mismatch("arc_reference", id.clone()));
    }
    if !properties_match {
        report(index_mismatch("arc_property", id.clone()));
    }
}

/// Decodes `row` with the descriptor stored for its type and version: the schema and the
/// values, one per field.
fn decode<'s>(
    row: &StoredObject,
    schemas: &'s Schemas,
    limits: &Limits,
) -> Result<(&'s Schema, Vec<Option<Value>>)> {
    let version = parse_version(row)?;
    let schema = match schemas.get(&(row.type_key.clone(), row.major, row.minor)) {
        Some(Ok(schema)) => schema,
        Some(Err(error)) => return Err(copy(error)),
        None => {
            return Err(Error::new(codes::LOAD_VERSION_UNDECLARED)
                .with_arg("type", row.type_key.clone())
                .with_arg("version", version.to_string()));
        }
    };
    let encoding = u8::try_from(row.encoding)
        .ok()
        .and_then(Encoding::from_code)
        .ok_or_else(|| {
            Error::new(codes::PAYLOAD_ENCODING_UNSUPPORTED).with_detail("encoding", row.encoding)
        })?;
    let decoded = payload::decode(schema, version.minor, encoding, &row.payload, limits)?;
    Ok((schema, decoded.values))
}

/// The JSON entry of one object row in [`inspect`].
fn object_json(row: &StoredObject, schemas: &Schemas, limits: &Limits) -> Json {
    let uuid = |bytes: &[u8]| parse_id(bytes).map_or_else(|| hex(bytes), |id| id.to_string());
    let mut entry = json!({
        "id": uuid(&row.id),
        "type_key": row.type_key,
        "version": SchemaVersion::new(
            u32::try_from(row.major).unwrap_or(0),
            u32::try_from(row.minor).unwrap_or(0)
        ).to_string(),
        "owner": row.owner.as_deref().map(uuid),
        "name": row.name,
        "partition": row.partition,
        "flags": row.flags,
        "encoding": row.encoding,
        "payload_bytes": row.payload.len(),
        "hash_matches": !hash_mismatch(row),
        "last_txn": hex(&row.last_txn),
    });
    let result = match decode(row, schemas, limits) {
        Ok((schema, values)) => (
            "values",
            schema
                .fields
                .iter()
                .zip(values)
                .map(|(field, value)| {
                    let value = serde_json::to_value(value).unwrap_or(Json::Null);
                    (field.name.clone(), value)
                })
                .collect::<Map<String, Json>>()
                .into(),
        ),
        Err(error) => ("error", error_json(&error)),
    };
    if let Some(entry) = entry.as_object_mut() {
        entry.insert(result.0.to_owned(), result.1);
    }
    entry
}

/// The type rows as stored: type key, major, minor and descriptor text.
fn stored_types(
    conn: &Connection,
    name: &str,
    limits: &Limits,
) -> Result<Vec<(String, i64, i64, String)>> {
    rows(
        conn,
        "SELECT type_key, major, minor, descriptor FROM arc_type ORDER BY type_key, major, minor",
        "arc_type",
        name,
        limits,
        |cells| {
            Ok((
                cells.text(0, "type_key")?,
                cells.int(1, "major")?,
                cells.int(2, "minor")?,
                cells.text(3, "descriptor")?,
            ))
        },
    )
}

/// The schema of every type row, parsed from its descriptor; a descriptor whose type key or
/// version disagrees with its row is invalid.
fn schemas(conn: &Connection, name: &str, limits: &Limits) -> Result<Schemas> {
    Ok(stored_types(conn, name, limits)?
        .into_iter()
        .map(|(key, major, minor, text)| {
            let schema = Schema::from_descriptor(&text, limits).and_then(|(parsed, schema)| {
                let version = schema.version;
                if parsed.as_str() == key
                    && i64::from(version.major) == major
                    && i64::from(version.minor) == minor
                {
                    Ok(schema)
                } else {
                    Err(Error::new(codes::DESCRIPTOR_INVALID).with_detail("reason", "row"))
                }
            });
            ((key, major, minor), schema)
        })
        .collect())
}

fn reference_row(field: &str, target: &[u8], rule: &RelationshipRule) -> Vec<SqlValue> {
    vec![
        SqlValue::Text(field.to_owned()),
        SqlValue::Blob(target.to_vec()),
        SqlValue::Null,
        SqlValue::Text(rule_text(rule)),
    ]
}

fn target_bytes(id: arc_base::ObjectId) -> Vec<u8> {
    use arc_base::id::Identifier;
    id.as_u128().to_be_bytes().to_vec()
}

fn index_mismatch(table: &'static str, id: String) -> Error {
    Error::new(codes::PACKAGE_INDEX_MISMATCH)
        .with_arg("table", table)
        .with_arg("id", id)
}

fn asset_corrupt(hash: &str) -> Error {
    Error::new(codes::PACKAGE_ASSET_CORRUPT)
        .with_arg("hash", hash.to_owned())
        .with_detail("reason", "row")
}

/// The error inside an `io::Error` an asset reader returned, or `doc.package.io_failed`.
fn reader_error(error: io::Error, path: &Path) -> Error {
    match error.into_inner() {
        Some(inner) => match inner.downcast::<Error>() {
            Ok(error) => *error,
            Err(other) => read::io_error(io::Error::other(other), path),
        },
        None => Error::new(codes::PACKAGE_IO_FAILED).with_arg("path", display(path)),
    }
}

fn meta_json(value: MetaValue) -> Json {
    match value {
        MetaValue::Null => Json::Null,
        MetaValue::Integer(value) => value.into(),
        MetaValue::Real(value) => value.into(),
        MetaValue::Text(value) => value.into(),
        MetaValue::Blob(value) => hex(&value).into(),
    }
}

fn error_json(error: &Error) -> Json {
    json!({ "code": error.code(), "arguments": error.arguments() })
}

/// A copy of `error` without its source chain (an `Error` is not `Clone`).
fn copy(error: &Error) -> Error {
    let mut copied = Error::new(arc_base::ErrorCode {
        code: error.code(),
        category: error.category(),
        message_id: error.message_id(),
    });
    for (name, value) in error.arguments() {
        copied = copied.with_arg(name, value.clone());
    }
    for (name, value) in error.details() {
        copied = copied.with_detail(name, value.clone());
    }
    copied
}

fn len(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}
