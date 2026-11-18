//! Content-addressed assets (CORE-DOC-047): large values stored once by the BLAKE3 hash of
//! their bytes. Assets up to [`WHOLE_MAX`] bytes are stored whole as one chunk; larger ones are
//! split by content-defined chunking (FastCDC, [`CHUNK_MIN`], [`CHUNK_AVG`], [`CHUNK_MAX`]) into
//! chunks stored once each, however many assets contain them. Every chunk and every whole asset
//! is verified against its hash on read; a mismatch is `doc.package.asset_corrupt` and the
//! damaged bytes are never returned. Writing and reading stream with bounded buffers.

use std::collections::{BTreeSet, HashSet};
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};

use fastcdc::v2020::StreamCDC;
use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use arc_base::{Error, Limits, Result};

use super::display;
use super::incremental::connect;
use super::read::{hex, io_error, sql_error};
use super::write::{Progress, Save, tick};
use crate::error::codes;
use crate::schema::{AssetHash, Value};
use crate::snapshot::Snapshot;

const MIB: usize = 1024 * 1024;
/// The largest asset stored whole, as a single chunk: 4 MiB. Larger assets are chunked.
pub const WHOLE_MAX: usize = 4 * MIB;
/// The smallest chunk of a chunked asset, except its last: 256 KiB.
pub const CHUNK_MIN: usize = 256 * 1024;
/// The average chunk size the chunker aims at: 1 MiB.
pub const CHUNK_AVG: usize = MIB;
/// The largest chunk: 4 MiB.
pub const CHUNK_MAX: usize = 4 * MIB;

fn not_found(hash: &AssetHash) -> Error {
    Error::new(codes::PACKAGE_ASSET_NOT_FOUND).with_arg("hash", hex(&hash.0))
}

fn corrupt(hash: &AssetHash, reason: &'static str) -> Error {
    Error::new(codes::PACKAGE_ASSET_CORRUPT)
        .with_arg("hash", hex(&hash.0))
        .with_detail("reason", reason)
}

fn limit(name: &'static str) -> Error {
    Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", name)
}

fn usize_of(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// The asset hashes the typed objects of `snapshot` hold in their field values, nested lists,
/// maps and records included.
#[must_use]
pub fn asset_values(snapshot: &Snapshot) -> BTreeSet<AssetHash> {
    let mut found = BTreeSet::new();
    for (_, object) in snapshot.iter() {
        let mut work: Vec<&Value> = object.values.iter().flatten().collect();
        while let Some(value) = work.pop() {
            match value {
                Value::Asset(hash) => {
                    found.insert(**hash);
                }
                Value::List(values) => work.extend(values),
                Value::Map(entries) => work.extend(entries.values()),
                Value::Record(values) => work.extend(values.iter().flatten()),
                _ => {}
            }
        }
    }
    found
}

/// Stores the bytes `reader` yields as an asset of `media_type` in the package at `path`, in
/// one write transaction, and returns its hash. An asset that is already stored is not stored
/// again; chunks already stored are shared.
pub(super) fn put(
    path: &Path,
    media_type: &str,
    reader: &mut dyn Read,
    limits: &Limits,
) -> Result<AssetHash> {
    if media_type.is_empty() || media_type.len() as u64 > limits.string_bytes {
        return Err(Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "media_type"));
    }
    let name = display(path);
    let sql = |e| sql_error(e, &name);
    let read_error = |e: io::Error| io_error(e, path);
    let mut first = Vec::new();
    let bound = u64::try_from(WHOLE_MAX)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    reader
        .take(bound)
        .read_to_end(&mut first)
        .map_err(read_error)?;
    let mut conn = connect(path, limits, true)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sql)?;
    let mut insert_chunk = tx
        .prepare("INSERT OR IGNORE INTO arc_chunk (hash, encoding, bytes) VALUES (?1, 0, ?2)")
        .map_err(sql)?;
    let (hash, length, chunks) = if first.len() <= WHOLE_MAX {
        let hash = *blake3::hash(&first).as_bytes();
        insert_chunk.execute(params![hash, first]).map_err(sql)?;
        (hash, first.len() as u64, None)
    } else {
        let mut whole = blake3::Hasher::new();
        let mut length = 0_u64;
        let mut list = Vec::new();
        let source = Cursor::new(first).chain(reader);
        for chunk in StreamCDC::new(source, CHUNK_MIN, CHUNK_AVG, CHUNK_MAX) {
            let chunk = chunk.map_err(|e| read_error(e.into()))?;
            length = length.saturating_add(chunk.data.len() as u64);
            if length > limits.input_bytes {
                return Err(limit("input_bytes"));
            }
            if list.len() as u64 >= limits.record_count {
                return Err(limit("record_count"));
            }
            whole.update(&chunk.data);
            let hash = *blake3::hash(&chunk.data).as_bytes();
            insert_chunk
                .execute(params![hash, chunk.data])
                .map_err(sql)?;
            list.push(hash);
        }
        (*whole.finalize().as_bytes(), length, Some(list))
    };
    drop(insert_chunk);
    let exists = tx
        .query_row(
            "SELECT 1 FROM arc_asset WHERE hash = ?1",
            [hash],
            |_| Ok(()),
        )
        .optional()
        .map_err(sql)?
        .is_some();
    if exists {
        // Stored before: its chunks are there already; nothing this call wrote is kept.
        return Ok(AssetHash(hash));
    }
    let length_value = i64::try_from(length).map_err(|_| limit("input_bytes"))?;
    tx.execute(
        "INSERT INTO arc_asset (hash, media_type, byte_length, chunked, encoding) \
         VALUES (?1, ?2, ?3, ?4, 0)",
        params![hash, media_type, length_value, i64::from(chunks.is_some())],
    )
    .map_err(sql)?;
    if let Some(list) = chunks {
        let mut insert = tx
            .prepare("INSERT INTO arc_asset_chunk (asset, ordinal, chunk) VALUES (?1, ?2, ?3)")
            .map_err(sql)?;
        for (ordinal, chunk) in (0_i64..).zip(&list) {
            insert.execute(params![hash, ordinal, chunk]).map_err(sql)?;
        }
    }
    tx.commit().map_err(sql)?;
    conn.close().map_err(|(_, e)| sql(e))?;
    Ok(AssetHash(hash))
}

/// A stored asset, read chunk by chunk. Each chunk is read from the package when it is needed
/// and verified against its hash before any of its bytes are returned; at the end the whole
/// asset is verified against its hash and length. Holds no connection between reads, so it
/// never keeps the package file open; at most one chunk ([`CHUNK_MAX`] bytes) is buffered.
///
/// The [`Read`] implementation returns the bytes of verified chunks; a damaged or missing chunk,
/// and a whole that does not match the asset hash, end the reading with an error of kind
/// `InvalidData` whose inner error is the [`arc_base::Error`]. Bytes of earlier chunks were
/// returned by then: a caller that must not use an asset with a mismatching whole reads it to
/// its end before using it, or uses [`super::Package::asset_bytes`].
#[derive(Debug)]
pub struct AssetReader {
    path: PathBuf,
    hash: AssetHash,
    media_type: String,
    length: u64,
    chunks: Vec<[u8; 32]>,
    next: usize,
    whole: blake3::Hasher,
    seen: u64,
    buffer: Vec<u8>,
    at: usize,
    finished: bool,
}

impl AssetReader {
    /// The asset's hash.
    #[must_use]
    pub fn hash(&self) -> AssetHash {
        self.hash
    }

    /// The asset's media type.
    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }

    /// The asset's length in bytes, as stored.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.length
    }

    /// True when the asset is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// The next verified chunk, or `None` after the last one once the whole asset verified.
    /// Fails with `doc.package.asset_corrupt` (detail `reason`) for a missing, oversized or
    /// damaged chunk and for a whole that does not match the asset's hash or length, and with
    /// `doc.payload.encoding_unsupported` for a chunk of an encoding this version cannot read.
    pub fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.finished {
            return Ok(None);
        }
        let Some(chunk) = self.chunks.get(self.next).copied() else {
            if self.seen != self.length {
                return Err(corrupt(&self.hash, "length"));
            }
            if *self.whole.finalize().as_bytes() != self.hash.0 {
                return Err(corrupt(&self.hash, "hash"));
            }
            self.finished = true;
            return Ok(None);
        };
        let bytes = read_chunk(&self.path, &self.hash, &chunk)?;
        self.seen = self.seen.saturating_add(bytes.len() as u64);
        if self.seen > self.length {
            return Err(corrupt(&self.hash, "length"));
        }
        self.whole.update(&bytes);
        self.next += 1;
        Ok(Some(bytes))
    }
}

impl Read for AssetReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.at >= self.buffer.len() {
            match self.next_chunk() {
                Ok(Some(bytes)) => {
                    self.buffer = bytes;
                    self.at = 0;
                }
                Ok(None) => return Ok(0),
                Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
            }
        }
        let available = self.buffer.get(self.at..).unwrap_or_default();
        let n = available.len().min(out.len());
        if let (Some(target), Some(source)) = (out.get_mut(..n), available.get(..n)) {
            target.copy_from_slice(source);
        }
        self.at += n;
        Ok(n)
    }
}

/// Reads one chunk on a connection of its own, verified against its hash.
fn read_chunk(path: &Path, asset: &AssetHash, chunk: &[u8; 32]) -> Result<Vec<u8>> {
    let name = display(path);
    let limits = Limits {
        input_bytes: CHUNK_MAX as u64,
        ..Limits::DEFAULT
    };
    let conn = connect(path, &limits, false)?;
    let row = conn
        .query_row(
            "SELECT encoding, bytes FROM arc_chunk WHERE hash = ?1",
            [chunk],
            |row| Ok((row.get::<_, SqlValue>(0)?, row.get::<_, SqlValue>(1)?)),
        )
        .optional();
    let row = match row {
        Ok(row) => row,
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if failure.code == rusqlite::ErrorCode::TooBig =>
        {
            return Err(corrupt(asset, "chunk_size"));
        }
        Err(error) => return Err(sql_error(error, &name)),
    };
    drop(conn);
    let (encoding, bytes) = row.ok_or_else(|| corrupt(asset, "chunk_missing"))?;
    match encoding {
        SqlValue::Integer(0) => {}
        SqlValue::Integer(code) => {
            return Err(Error::new(codes::PAYLOAD_ENCODING_UNSUPPORTED).with_arg("encoding", code));
        }
        _ => return Err(corrupt(asset, "chunk_row")),
    }
    let SqlValue::Blob(bytes) = bytes else {
        return Err(corrupt(asset, "chunk_row"));
    };
    if bytes.len() > CHUNK_MAX {
        return Err(corrupt(asset, "chunk_size"));
    }
    if blake3::hash(&bytes).as_bytes() != chunk {
        return Err(corrupt(asset, "chunk_hash"));
    }
    Ok(bytes)
}

/// Opens the asset `hash` of the package at `path` for reading: its row and chunk list are read
/// and checked, the chunks are read by the [`AssetReader`].
pub(super) fn open(path: &Path, hash: &AssetHash, limits: &Limits) -> Result<AssetReader> {
    let name = display(path);
    let sql = |e| sql_error(e, &name);
    let conn = connect(path, limits, false)?;
    let row = conn
        .query_row(
            "SELECT media_type, byte_length, chunked, encoding FROM arc_asset WHERE hash = ?1",
            [hash.0],
            |row| {
                Ok((
                    row.get::<_, SqlValue>(0)?,
                    row.get::<_, SqlValue>(1)?,
                    row.get::<_, SqlValue>(2)?,
                    row.get::<_, SqlValue>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sql)?
        .ok_or_else(|| not_found(hash))?;
    let (SqlValue::Text(media_type), SqlValue::Integer(length), SqlValue::Integer(chunked)) =
        (row.0, row.1, row.2)
    else {
        return Err(corrupt(hash, "asset_row"));
    };
    match row.3 {
        SqlValue::Integer(0) => {}
        SqlValue::Integer(code) => {
            return Err(Error::new(codes::PAYLOAD_ENCODING_UNSUPPORTED).with_arg("encoding", code));
        }
        _ => return Err(corrupt(hash, "asset_row")),
    }
    let length = u64::try_from(length).map_err(|_| corrupt(hash, "asset_row"))?;
    let chunks = match chunked {
        0 if usize_of(length) <= WHOLE_MAX => vec![hash.0],
        1 if usize_of(length) > WHOLE_MAX => {
            let mut statement = conn
                .prepare(
                    "SELECT ordinal, chunk FROM arc_asset_chunk WHERE asset = ?1 ORDER BY ordinal",
                )
                .map_err(sql)?;
            let mut rows = statement.query([hash.0]).map_err(sql)?;
            let mut list = Vec::new();
            while let Some(row) = rows.next().map_err(sql)? {
                if list.len() as u64 >= limits.record_count {
                    return Err(limit("record_count"));
                }
                let ordinal = match row.get_ref(0).map_err(sql)? {
                    ValueRef::Integer(value) => value,
                    _ => return Err(corrupt(hash, "chunk_list")),
                };
                let chunk = match row.get_ref(1).map_err(sql)? {
                    ValueRef::Blob(bytes) => <[u8; 32]>::try_from(bytes).ok(),
                    _ => None,
                };
                match chunk {
                    Some(chunk) if usize::try_from(ordinal).ok() == Some(list.len()) => {
                        list.push(chunk);
                    }
                    _ => return Err(corrupt(hash, "chunk_list")),
                }
            }
            // Every chunk holds at most `CHUNK_MAX` bytes.
            if (list.len() as u64).saturating_mul(CHUNK_MAX as u64) < length {
                return Err(corrupt(hash, "chunk_list"));
            }
            list
        }
        _ => return Err(corrupt(hash, "asset_row")),
    };
    drop(conn);
    Ok(AssetReader {
        path: path.to_path_buf(),
        hash: *hash,
        media_type,
        length,
        chunks,
        next: 0,
        whole: blake3::Hasher::new(),
        seen: 0,
        buffer: Vec::new(),
        at: 0,
        finished: false,
    })
}

/// The whole asset `hash` in memory, returned only once every chunk and the whole verified.
/// An asset longer than `input_bytes` fails with `doc.load.limit_exceeded`.
pub(super) fn read_all(path: &Path, hash: &AssetHash, limits: &Limits) -> Result<Vec<u8>> {
    let mut reader = open(path, hash, limits)?;
    if reader.len() > limits.input_bytes {
        return Err(limit("input_bytes"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = reader.next_chunk()? {
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Where a full rewrite takes its assets from.
#[derive(Debug, Clone, Copy)]
pub(super) struct Carry<'a> {
    /// The package whose assets are copied.
    pub from: &'a Path,
    /// `None`: every asset, chunk and chunk list row, unchanged. `Some(save)`: only the assets
    /// the saved state refers to (see [`referenced`]), with their chunks.
    pub only_referenced: Option<&'a Save<'a>>,
}

/// Copies the assets of `carry` into the package being written on `conn`.
pub(super) fn copy(carry: &Carry<'_>, conn: &Connection, progress: Progress<'_>) -> Result<()> {
    let source_name = display(carry.from);
    let src = |e| sql_error(e, &source_name);
    let dst = |e| sql_error(e, "");
    let source = connect(carry.from, &Limits::DEFAULT, false)?;
    let mut count = 0;
    let Some(save) = carry.only_referenced else {
        return copy_everything(&source, conn, &source_name, progress);
    };
    let stored: Vec<Vec<u8>> = {
        let mut statement = source.prepare("SELECT hash FROM arc_asset").map_err(src)?;
        let mut rows = statement.query([]).map_err(src)?;
        let mut stored = Vec::new();
        while let Some(row) = rows.next().map_err(src)? {
            if let ValueRef::Blob(bytes) = row.get_ref(0).map_err(src)? {
                stored.push(bytes.to_vec());
            }
        }
        stored
    };
    let keep = referenced(save, &stored);
    let mut asset = source
        .prepare(
            "SELECT hash, media_type, byte_length, chunked, encoding FROM arc_asset \
             WHERE hash = ?1",
        )
        .map_err(src)?;
    let mut list = source
        .prepare("SELECT asset, ordinal, chunk FROM arc_asset_chunk WHERE asset = ?1")
        .map_err(src)?;
    let mut chunk = source
        .prepare("SELECT hash, encoding, bytes FROM arc_chunk WHERE hash = ?1")
        .map_err(src)?;
    let mut insert_asset = conn
        .prepare(
            "INSERT INTO arc_asset (hash, media_type, byte_length, chunked, encoding) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(dst)?;
    let mut insert_list = conn
        .prepare("INSERT INTO arc_asset_chunk (asset, ordinal, chunk) VALUES (?1, ?2, ?3)")
        .map_err(dst)?;
    let mut insert_chunk = conn
        .prepare("INSERT OR IGNORE INTO arc_chunk (hash, encoding, bytes) VALUES (?1, ?2, ?3)")
        .map_err(dst)?;
    let mut copy_chunk = |hash: &SqlValue| -> Result<()> {
        let mut rows = chunk.query([hash]).map_err(src)?;
        if let Some(row) = rows.next().map_err(src)? {
            let values: Vec<ToSqlOutput<'_>> = (0..3)
                .map(|at| row.get_ref(at).map(ToSqlOutput::Borrowed))
                .collect::<rusqlite::Result<_>>()
                .map_err(src)?;
            insert_chunk
                .execute(rusqlite::params_from_iter(values))
                .map_err(dst)?;
        }
        Ok(())
    };
    for hash in stored.iter().filter(|hash| keep.contains(hash.as_slice())) {
        let key = SqlValue::Blob(hash.clone());
        let mut rows = asset.query([&key]).map_err(src)?;
        let Some(row) = rows.next().map_err(src)? else {
            continue;
        };
        let values: Vec<SqlValue> = (0..5)
            .map(|at| row.get(at))
            .collect::<rusqlite::Result<_>>()
            .map_err(src)?;
        drop(rows);
        insert_asset
            .execute(rusqlite::params_from_iter(&values))
            .map_err(dst)?;
        // A whole asset is its own chunk; a chunked one lists its chunks.
        copy_chunk(&key)?;
        let entries: Vec<[SqlValue; 3]> = {
            let mut rows = list.query([&key]).map_err(src)?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next().map_err(src)? {
                entries.push([
                    row.get(0).map_err(src)?,
                    row.get(1).map_err(src)?,
                    row.get(2).map_err(src)?,
                ]);
            }
            entries
        };
        for entry in &entries {
            insert_list
                .execute(rusqlite::params_from_iter(entry))
                .map_err(dst)?;
            copy_chunk(&entry[2])?;
            tick(progress, &mut count)?;
        }
        tick(progress, &mut count)?;
    }
    Ok(())
}

/// Copies every asset, chunk list row and chunk of `source` unchanged.
fn copy_everything(
    source: &Connection,
    conn: &Connection,
    source_name: &str,
    progress: Progress<'_>,
) -> Result<()> {
    let src = |e| sql_error(e, source_name);
    let dst = |e| sql_error(e, "");
    let mut count = 0;
    for (select, insert) in [
        (
            "SELECT hash, media_type, byte_length, chunked, encoding FROM arc_asset",
            "INSERT INTO arc_asset (hash, media_type, byte_length, chunked, encoding) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        ),
        (
            "SELECT asset, ordinal, chunk FROM arc_asset_chunk",
            "INSERT INTO arc_asset_chunk (asset, ordinal, chunk) VALUES (?1, ?2, ?3)",
        ),
        (
            "SELECT hash, encoding, bytes FROM arc_chunk",
            "INSERT INTO arc_chunk (hash, encoding, bytes) VALUES (?1, ?2, ?3)",
        ),
    ] {
        let mut read = source.prepare(select).map_err(src)?;
        let columns = read.column_count();
        let mut write = conn.prepare(insert).map_err(dst)?;
        let mut rows = read.query([]).map_err(src)?;
        while let Some(row) = rows.next().map_err(src)? {
            let values: Vec<ToSqlOutput<'_>> = (0..columns)
                .map(|at| row.get_ref(at).map(ToSqlOutput::Borrowed))
                .collect::<rusqlite::Result<_>>()
                .map_err(src)?;
            write
                .execute(rusqlite::params_from_iter(values))
                .map_err(dst)?;
            tick(progress, &mut count)?;
        }
    }
    Ok(())
}

/// The stored asset hashes (`stored`) that the saved state refers to: the asset values of the
/// typed objects, and every stored hash whose 32 bytes occur in the payload of a held or
/// unaddressable row, whose values are not decoded. When such a payload is not of encoding 0,
/// whose bytes would show the hash, every stored asset is kept.
fn referenced(save: &Save<'_>, stored: &[Vec<u8>]) -> BTreeSet<Vec<u8>> {
    let mut keep: BTreeSet<Vec<u8>> = asset_values(save.snapshot)
        .into_iter()
        .map(|hash| hash.0.to_vec())
        .collect();
    let candidates: HashSet<&[u8]> = stored.iter().map(Vec::as_slice).collect();
    let held = save.snapshot.held().map(|(_, held)| held.row());
    let unaddressable = save
        .loaded
        .map(|loaded| loaded.rows.unaddressable())
        .unwrap_or_default();
    for row in held.chain(unaddressable) {
        if row.encoding != 0 {
            return stored.iter().cloned().collect();
        }
        for window in row.payload.windows(32) {
            if candidates.contains(window) {
                keep.insert(window.to_vec());
            }
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_parameters_fit_the_chunker() {
        use fastcdc::v2020::{AVERAGE_MAX, MAXIMUM_MAX, MINIMUM_MAX};
        const {
            assert!(CHUNK_MIN <= MINIMUM_MAX && CHUNK_AVG <= AVERAGE_MAX);
            assert!(CHUNK_MAX <= MAXIMUM_MAX && CHUNK_MAX == WHOLE_MAX);
            assert!(CHUNK_MIN < CHUNK_AVG && CHUNK_AVG < CHUNK_MAX);
        }
    }
}
