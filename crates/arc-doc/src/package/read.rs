//! Reading a package: the header check before SQLite, the read-only connection, metadata, type
//! and object rows and previews. Every value is checked for its storage class, text encoding and
//! size before it is used.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

use arc_base::id::{DocumentId, Identifier};
use arc_base::{Error, Limits, ObjectId, Result};

use super::preview::{self, Preview};
use super::{APPLICATION_ID, FORMAT, FormatVersion, MetaValue, Metadata, display, schema};
use crate::error::codes;
use crate::load::{DiagnosticKind, LoadDiagnostic, LoadOptions, Loaded, StoredObject, StoredType};
use crate::registry::TypeRegistry;

/// The SQLite header string at the start of every database file.
const MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// The `arc_meta` keys the package interprets.
pub(crate) const KEY_FORMAT: &str = "format_version";
pub(crate) const KEY_DOCUMENT_ID: &str = "document_id";
pub(crate) const KEY_CREATED_BY: &str = "created_by";
pub(crate) const KEY_SAVED_BY: &str = "saved_by";
pub(crate) const KEY_UNITS: &str = "units";
pub(crate) const KEY_CRS: &str = "coordinate_reference";
pub(crate) const KEY_SAVE_COUNTER: &str = "save_counter";
pub(crate) const KEY_RETENTION: &str = "history_retention";
pub(crate) const KEY_PARTITIONS: &str = "partitions";
pub(crate) const KEY_THUMBNAIL: &str = "thumbnail";
/// Every interpreted key.
pub(crate) const KEYS: &[&str] = &[
    KEY_FORMAT,
    KEY_DOCUMENT_ID,
    KEY_CREATED_BY,
    KEY_SAVED_BY,
    KEY_UNITS,
    KEY_CRS,
    KEY_SAVE_COUNTER,
    KEY_RETENTION,
    KEY_PARTITIONS,
    KEY_THUMBNAIL,
];
/// The value of `thumbnail` when the last save wrote none.
pub(crate) const THUMBNAIL_NONE: &str = "none";

pub(crate) fn io_error(error: io::Error, path: &Path) -> Error {
    Error::new(codes::PACKAGE_IO_FAILED)
        .with_arg("path", display(path))
        .with_source(error)
}

/// The error of a failed SQLite call on the package at `path`.
pub(crate) fn sql_error(error: rusqlite::Error, path: &str) -> Error {
    use rusqlite::ErrorCode;
    let base = match &error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => {
                Error::new(codes::PACKAGE_CORRUPT).with_arg("path", path)
            }
            ErrorCode::TooBig => {
                Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", "input_bytes")
            }
            _ => Error::new(codes::PACKAGE_IO_FAILED).with_arg("path", path),
        },
        _ => Error::new(codes::PACKAGE_IO_FAILED).with_arg("path", path),
    };
    base.with_source(error)
}

fn value_invalid(table: &'static str, column: &'static str) -> Error {
    Error::new(codes::PACKAGE_VALUE_INVALID)
        .with_arg("table", table)
        .with_arg("column", column)
}

fn limit(name: &'static str) -> Error {
    Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", name)
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes(word.try_into().ok()?))
}

/// Checks the 100-byte database header with plain file reads, before SQLite sees the file: the
/// SQLite magic, the application id, the text encoding (1, UTF-8) and the format major version
/// in `user_version`, which is returned.
pub(crate) fn check_header(path: &Path) -> Result<u32> {
    let not_a_package = || Error::new(codes::PACKAGE_NOT_A_PACKAGE).with_arg("path", display(path));
    let mut header = [0_u8; 100];
    let mut file = File::open(path).map_err(|e| io_error(e, path))?;
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Err(not_a_package()),
        Err(error) => return Err(io_error(error, path)),
    }
    drop(file);
    if header.get(..16) != Some(MAGIC.as_slice()) || be32(&header, 68) != Some(APPLICATION_ID) {
        return Err(not_a_package());
    }
    let major = be32(&header, 60).ok_or_else(not_a_package)?;
    if major > FORMAT.major {
        return Err(Error::new(codes::PACKAGE_NEWER_FORMAT)
            .with_arg("version", major.to_string())
            .with_arg("current", FORMAT.to_string()));
    }
    if be32(&header, 56) != Some(1) {
        return Err(Error::new(codes::PACKAGE_SCHEMA_INVALID)
            .with_arg("table", "header")
            .with_detail("column", "text_encoding"));
    }
    Ok(major)
}

/// Opens `path` read-only. The connection never writes to the file or its directory; values
/// longer than `input_bytes` fail with `doc.load.limit_exceeded` when they are read.
fn connect(path: &Path, limits: &Limits) -> Result<Connection> {
    let name = display(path);
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| sql_error(e, &name))?;
    let length = i32::try_from(limits.input_bytes).unwrap_or(i32::MAX);
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, length)
        .map_err(|e| sql_error(e, &name))?;
    // Connection settings only: a hostile schema runs no functions, and damaged pages are
    // detected early.
    conn.execute_batch("PRAGMA trusted_schema = OFF; PRAGMA cell_size_check = ON;")
        .map_err(|e| sql_error(e, &name))?;
    Ok(conn)
}

/// Opens and checks the package: header, `arc_meta`, format version, schema and metadata.
fn open(path: &Path, limits: &Limits) -> Result<(Connection, FormatVersion, Metadata)> {
    let major = check_header(path)?;
    let name = display(path);
    let conn = connect(path, limits)?;
    let has_meta: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name = 'arc_meta' AND type = 'table'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| sql_error(e, &name))?;
    if has_meta == 0 {
        return Err(Error::new(codes::PACKAGE_NOT_A_PACKAGE).with_arg("path", name));
    }
    let mut entries = meta_entries(&conn, &name, limits)?;
    let format = match entries.remove(KEY_FORMAT) {
        Some(MetaValue::Text(text)) => FormatVersion::parse(&text),
        _ => None,
    }
    .filter(|format| format.major == major && format.major >= 1)
    .ok_or_else(|| {
        Error::new(codes::PACKAGE_SCHEMA_INVALID)
            .with_arg("table", "arc_meta")
            .with_detail("column", KEY_FORMAT)
    })?;
    schema::verify(&conn, &name)?;
    Ok((conn, format, metadata(entries)?))
}

/// [`open`] without keeping the connection.
pub(crate) fn open_checked(path: &Path, limits: &Limits) -> Result<(FormatVersion, Metadata)> {
    let (_, format, metadata) = open(path, limits)?;
    Ok((format, metadata))
}

/// Reads one column of a row, checking its storage class.
struct Cells<'r, 's> {
    row: &'r Row<'s>,
    table: &'static str,
    path: &'r str,
}

impl Cells<'_, '_> {
    fn get(&self, at: usize) -> Result<ValueRef<'_>> {
        self.row.get_ref(at).map_err(|e| sql_error(e, self.path))
    }

    fn blob(&self, at: usize, column: &'static str) -> Result<Vec<u8>> {
        match self.get(at)? {
            ValueRef::Blob(bytes) => Ok(bytes.to_vec()),
            _ => Err(value_invalid(self.table, column)),
        }
    }

    fn opt_blob(&self, at: usize, column: &'static str) -> Result<Option<Vec<u8>>> {
        match self.get(at)? {
            ValueRef::Null => Ok(None),
            _ => self.blob(at, column).map(Some),
        }
    }

    fn text(&self, at: usize, column: &'static str) -> Result<String> {
        match self.get(at)? {
            ValueRef::Text(bytes) => {
                String::from_utf8(bytes.to_vec()).map_err(|_| value_invalid(self.table, column))
            }
            _ => Err(value_invalid(self.table, column)),
        }
    }

    fn opt_text(&self, at: usize, column: &'static str) -> Result<Option<String>> {
        match self.get(at)? {
            ValueRef::Null => Ok(None),
            _ => self.text(at, column).map(Some),
        }
    }

    fn int(&self, at: usize, column: &'static str) -> Result<i64> {
        match self.get(at)? {
            ValueRef::Integer(value) => Ok(value),
            _ => Err(value_invalid(self.table, column)),
        }
    }

    fn any(&self, at: usize, column: &'static str) -> Result<MetaValue> {
        Ok(match self.get(at)? {
            ValueRef::Null => MetaValue::Null,
            ValueRef::Integer(value) => MetaValue::Integer(value),
            ValueRef::Real(value) => MetaValue::Real(value),
            ValueRef::Text(_) => MetaValue::Text(self.text(at, column)?),
            ValueRef::Blob(bytes) => MetaValue::Blob(bytes.to_vec()),
        })
    }
}

/// Runs `query` and maps every row with `map`, failing with `doc.load.limit_exceeded` beyond
/// `record_count` rows.
fn rows<T>(
    conn: &Connection,
    query: &str,
    table: &'static str,
    path: &str,
    limits: &Limits,
    mut map: impl FnMut(&Cells<'_, '_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut statement = conn.prepare(query).map_err(|e| sql_error(e, path))?;
    let mut rows = statement.query([]).map_err(|e| sql_error(e, path))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| sql_error(e, path))? {
        if u64::try_from(out.len()).unwrap_or(u64::MAX) >= limits.record_count {
            return Err(limit("record_count"));
        }
        out.push(map(&Cells { row, table, path })?);
    }
    Ok(out)
}

fn meta_entries(
    conn: &Connection,
    path: &str,
    limits: &Limits,
) -> Result<BTreeMap<String, MetaValue>> {
    let entries = rows(
        conn,
        "SELECT key, value FROM arc_meta ORDER BY key",
        "arc_meta",
        path,
        limits,
        |cells| Ok((cells.text(0, "key")?, cells.any(1, "value")?)),
    )?;
    Ok(entries.into_iter().collect())
}

/// The metadata of the entries of `arc_meta` other than `format_version`.
fn metadata(mut entries: BTreeMap<String, MetaValue>) -> Result<Metadata> {
    let invalid =
        |key: &str| Error::new(codes::PACKAGE_META_INVALID).with_arg("key", key.to_owned());
    let mut text = |key: &'static str| match entries.remove(key) {
        None => Ok(None),
        Some(MetaValue::Text(text)) => Ok(Some(text)),
        Some(_) => Err(invalid(key)),
    };
    let document_id = text(KEY_DOCUMENT_ID)?
        .map(|text| DocumentId::from_str(&text).map_err(|_| invalid(KEY_DOCUMENT_ID)))
        .transpose()?;
    let created_by = text(KEY_CREATED_BY)?;
    let saved_by = text(KEY_SAVED_BY)?;
    let units = text(KEY_UNITS)?;
    let coordinate_reference = text(KEY_CRS)?;
    let history_retention = text(KEY_RETENTION)?;
    let partitions = text(KEY_PARTITIONS)?
        .map(|text| serde_json::from_str::<Vec<String>>(&text).map_err(|_| invalid(KEY_PARTITIONS)))
        .transpose()?
        .unwrap_or_default();
    let thumbnail_missing = match text(KEY_THUMBNAIL)? {
        None => false,
        Some(value) if value == THUMBNAIL_NONE => true,
        Some(_) => return Err(invalid(KEY_THUMBNAIL)),
    };
    let save_counter = match entries.remove(KEY_SAVE_COUNTER) {
        None => 0,
        Some(MetaValue::Integer(value)) => {
            u64::try_from(value).map_err(|_| invalid(KEY_SAVE_COUNTER))?
        }
        Some(_) => return Err(invalid(KEY_SAVE_COUNTER)),
    };
    Ok(Metadata {
        document_id,
        created_by,
        saved_by,
        units,
        coordinate_reference,
        save_counter,
        history_retention,
        partitions,
        thumbnail_missing,
        other: entries,
    })
}

fn stored_object(cells: &Cells<'_, '_>) -> Result<StoredObject> {
    Ok(StoredObject {
        id: cells.blob(0, "id")?,
        type_key: cells.text(1, "type_key")?,
        major: cells.int(2, "schema_major")?,
        minor: cells.int(3, "schema_minor")?,
        owner: cells.opt_blob(4, "owner")?,
        name: cells.opt_text(5, "name")?,
        partition: cells.opt_text(6, "partition")?,
        flags: cells.int(7, "flags")?,
        encoding: cells.int(8, "encoding")?,
        payload: cells.blob(9, "payload")?,
        payload_hash: cells.blob(10, "payload_hash")?,
        last_txn: cells.blob(11, "last_txn")?,
    })
}

/// Lowercase hexadecimal text of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// True when the stored hash of a row of encoding 0 is not the BLAKE3 hash of its payload.
fn hash_mismatch(row: &StoredObject) -> bool {
    row.encoding == 0 && blake3::hash(&row.payload).as_bytes().as_slice() != row.payload_hash
}

/// Reads and loads the package's rows (see [`super::Package::load`]).
pub(crate) fn load(
    path: &Path,
    registry: Arc<TypeRegistry>,
    options: &LoadOptions,
) -> Result<Loaded> {
    let limits = &options.limits;
    let (conn, _, _) = open(path, limits)?;
    let name = display(path);
    let types = rows(
        &conn,
        "SELECT type_key, major, minor, descriptor FROM arc_type ORDER BY type_key, major, minor",
        "arc_type",
        &name,
        limits,
        |cells| {
            Ok(StoredType {
                type_key: cells.text(0, "type_key")?,
                major: cells.int(1, "major")?,
                minor: cells.int(2, "minor")?,
                descriptor: cells.text(3, "descriptor")?,
            })
        },
    )?;
    let mut statement = conn
        .prepare(
            "SELECT id, type_key, schema_major, schema_minor, owner, name, partition, flags, \
             encoding, payload, payload_hash, last_txn FROM arc_object ORDER BY id",
        )
        .map_err(|e| sql_error(e, &name))?;
    let mut cursor = statement.query([]).map_err(|e| sql_error(e, &name))?;
    let mut failure = None;
    let mut mismatches = Vec::new();
    // Rows stream into the load, which bounds their number and memory as it goes.
    let objects = std::iter::from_fn(|| {
        let next = match cursor.next() {
            Ok(Some(row)) => stored_object(&Cells {
                row,
                table: "arc_object",
                path: &name,
            }),
            Ok(None) => return None,
            Err(error) => Err(sql_error(error, &name)),
        };
        match next {
            Ok(row) => {
                if hash_mismatch(&row) {
                    mismatches.push(row.id.clone());
                }
                Some(row)
            }
            Err(error) => {
                failure = Some(error);
                None
            }
        }
    });
    let loaded = crate::load::load(registry, &types, objects, options);
    if let Some(error) = failure {
        return Err(error);
    }
    let mut loaded = loaded?;
    for id in mismatches {
        let object = <[u8; 16]>::try_from(id.as_slice())
            .ok()
            .map(|raw| ObjectId::from_u128(u128::from_be_bytes(raw)));
        loaded.counts.warnings += 1;
        loaded.diagnostics.push(LoadDiagnostic {
            kind: DiagnosticKind::Warning,
            object,
            error: Error::new(codes::PACKAGE_HASH_MISMATCH).with_arg("id", hex(&id)),
        });
    }
    Ok(loaded)
}

/// Reads the previews (see [`super::Package::previews`]).
pub(crate) fn previews(path: &Path, limits: &Limits) -> Result<Vec<Preview>> {
    let (conn, _, _) = open(path, limits)?;
    let name = display(path);
    rows(
        &conn,
        "SELECT kind, media_type, width, height, bytes FROM arc_preview ORDER BY kind",
        "arc_preview",
        &name,
        limits,
        |cells| {
            let size = |at, column| {
                u32::try_from(cells.int(at, column)?)
                    .map_err(|_| value_invalid("arc_preview", column))
            };
            let preview = Preview {
                kind: cells.text(0, "kind")?,
                media_type: cells.text(1, "media_type")?,
                width: size(2, "width")?,
                height: size(3, "height")?,
                bytes: cells.blob(4, "bytes")?,
            };
            preview::check(&preview)?;
            Ok(preview)
        },
    )
}
