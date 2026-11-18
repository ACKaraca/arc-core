//! The schema of package format 1.0: the statements that create it and the check of an opened
//! file against it. The statements are the specification's; `docs/architecture/document-package.md`
//! lists them verbatim.

use rusqlite::Connection;

use arc_base::{Error, Result};

use super::read::sql_error;
use crate::error::codes;

/// Every table of format 1.0, in creation order, with its statement.
pub(crate) const TABLES: &[(&str, &str)] = &[
    (
        "arc_meta",
        "CREATE TABLE arc_meta (\n  key TEXT PRIMARY KEY NOT NULL,\n  value\n) WITHOUT ROWID",
    ),
    (
        "arc_type",
        "CREATE TABLE arc_type (\n  type_key TEXT NOT NULL,\n  major INTEGER NOT NULL,\n  minor INTEGER NOT NULL,\n  descriptor TEXT NOT NULL,\n  PRIMARY KEY (type_key, major, minor)\n) WITHOUT ROWID",
    ),
    (
        "arc_object",
        "CREATE TABLE arc_object (\n  id BLOB PRIMARY KEY NOT NULL,\n  type_key TEXT NOT NULL,\n  schema_major INTEGER NOT NULL,\n  schema_minor INTEGER NOT NULL,\n  owner BLOB,\n  name TEXT,\n  partition TEXT,\n  flags INTEGER NOT NULL,\n  encoding INTEGER NOT NULL,\n  payload BLOB NOT NULL,\n  payload_hash BLOB NOT NULL,\n  last_txn BLOB NOT NULL\n) WITHOUT ROWID",
    ),
    (
        "arc_property",
        "CREATE TABLE arc_property (\n  object BLOB NOT NULL,\n  key TEXT NOT NULL,\n  value,\n  unit TEXT,\n  PRIMARY KEY (object, key)\n) WITHOUT ROWID",
    ),
    (
        "arc_reference",
        "CREATE TABLE arc_reference (\n  source BLOB NOT NULL,\n  field TEXT NOT NULL,\n  target BLOB NOT NULL,\n  target_link BLOB,\n  rule TEXT NOT NULL,\n  PRIMARY KEY (source, field, target)\n) WITHOUT ROWID",
    ),
    (
        "arc_source_map",
        "CREATE TABLE arc_source_map (\n  object BLOB NOT NULL,\n  source_format TEXT NOT NULL,\n  source_document TEXT NOT NULL,\n  source_id TEXT NOT NULL,\n  PRIMARY KEY (source_format, source_document, source_id)\n) WITHOUT ROWID",
    ),
    (
        "arc_journal",
        "CREATE TABLE arc_journal (\n  seq INTEGER PRIMARY KEY,\n  txn_id BLOB NOT NULL UNIQUE,\n  record BLOB NOT NULL\n)",
    ),
    (
        "arc_asset",
        "CREATE TABLE arc_asset (\n  hash BLOB PRIMARY KEY NOT NULL,\n  media_type TEXT NOT NULL,\n  byte_length INTEGER NOT NULL,\n  chunked INTEGER NOT NULL,\n  encoding INTEGER NOT NULL\n) WITHOUT ROWID",
    ),
    (
        "arc_asset_chunk",
        "CREATE TABLE arc_asset_chunk (\n  asset BLOB NOT NULL,\n  ordinal INTEGER NOT NULL,\n  chunk BLOB NOT NULL,\n  PRIMARY KEY (asset, ordinal)\n) WITHOUT ROWID",
    ),
    (
        "arc_chunk",
        "CREATE TABLE arc_chunk (\n  hash BLOB PRIMARY KEY NOT NULL,\n  encoding INTEGER NOT NULL,\n  bytes BLOB NOT NULL\n) WITHOUT ROWID",
    ),
    (
        "arc_preview",
        "CREATE TABLE arc_preview (\n  kind TEXT PRIMARY KEY NOT NULL,\n  media_type TEXT NOT NULL,\n  width INTEGER NOT NULL,\n  height INTEGER NOT NULL,\n  bytes BLOB NOT NULL\n) WITHOUT ROWID",
    ),
    (
        "arc_foreign",
        "CREATE TABLE arc_foreign (\n  id INTEGER PRIMARY KEY,\n  source_format TEXT NOT NULL,\n  source_version TEXT,\n  source_id TEXT,\n  object BLOB,\n  record_kind TEXT NOT NULL,\n  data BLOB NOT NULL,\n  asset BLOB\n)",
    ),
    (
        "arc_diagnostic",
        "CREATE TABLE arc_diagnostic (\n  id INTEGER PRIMARY KEY,\n  severity TEXT NOT NULL,\n  code TEXT NOT NULL,\n  object BLOB,\n  origin TEXT NOT NULL,\n  arguments TEXT NOT NULL\n)",
    ),
];

/// Every named index of format 1.0, with its statement. `arc_journal.txn_id` has SQLite's
/// automatic unique index besides.
pub(crate) const INDEXES: &[(&str, &str)] = &[
    (
        "arc_object_type_key",
        "CREATE INDEX arc_object_type_key ON arc_object (type_key)",
    ),
    (
        "arc_reference_target",
        "CREATE INDEX arc_reference_target ON arc_reference (target)",
    ),
    (
        "arc_asset_chunk_chunk",
        "CREATE INDEX arc_asset_chunk_chunk ON arc_asset_chunk (chunk)",
    ),
];

/// Creates the schema in an empty database inside the caller's transaction.
pub(crate) fn create(conn: &Connection) -> rusqlite::Result<()> {
    for (_, statement) in TABLES.iter().chain(INDEXES) {
        conn.execute(statement, [])?;
    }
    Ok(())
}

/// The column names of `table` in `conn`, in order; empty when the table does not exist.
fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?;
    let names = statement.query_map([table], |row| row.get::<_, String>(0))?;
    names.collect()
}

/// Checks that every table of [`TABLES`] exists in `conn` with at least its columns. Extra
/// tables and appended columns (a newer minor version, product `arc_ext_` tables) are allowed.
/// Fails with `doc.package.schema_invalid` (argument `table`, detail `column`).
pub(crate) fn verify(conn: &Connection, path: &str) -> Result<()> {
    let reference = Connection::open_in_memory().map_err(|e| sql_error(e, path))?;
    create(&reference).map_err(|e| sql_error(e, path))?;
    for (table, _) in TABLES {
        let expected = columns(&reference, table).map_err(|e| sql_error(e, path))?;
        let present = columns(conn, table).map_err(|e| sql_error(e, path))?;
        if let Some(missing) = expected.iter().find(|name| !present.contains(name)) {
            return Err(Error::new(codes::PACKAGE_SCHEMA_INVALID)
                .with_arg("table", *table)
                .with_detail("column", missing.clone()));
        }
    }
    Ok(())
}
