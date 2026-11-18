//! The stored history of a package (CORE-DOC-040): the retained journal records
//! (`arc_journal`), the preserved foreign data (`arc_foreign`) and the diagnostics present at
//! save (`arc_diagnostic`). A save writes them, a load reads them back as stored, `inspect`
//! dumps them and `check` verifies them. The rules are specified in `docs/crates/arc-doc.md`,
//! section "Journal, foreign data and diagnostics in the package".

use std::collections::{BTreeSet, HashSet};

use rusqlite::{Connection, params};
use serde_json::{Value as Json, json};

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId, Result};

use super::Save;
use super::read::{Cells, hex, rows, sql_error};
use crate::diagnostic::{Diagnostic, Origin};
use crate::error::codes;
use crate::load::{StoredDiagnostic, StoredForeign, StoredRecord, parse_id};
use crate::record::TxnRecord;
use crate::schema::Severity;

/// How many records of `arc_journal` a save keeps (`arc_meta` key `history_retention`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Retention {
    /// `none`: no record.
    None,
    /// `full`, absent, or a value this version does not interpret: every record.
    Full,
    /// `saves:<n>`: the records stored by the last `n` saves, this one included.
    Saves(u64),
}

impl Retention {
    pub(super) fn parse(text: Option<&str>) -> Self {
        match text {
            Some("none") => Self::None,
            Some(text) => text
                .strip_prefix("saves:")
                .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|n| n.parse().ok())
                .filter(|n| *n >= 1)
                .map_or(Self::Full, Self::Saves),
            None => Self::Full,
        }
    }
}

/// The save a `seq` was assigned by: its high 32 bits.
fn band(seq: i64) -> u64 {
    u64::try_from(seq >> 32).unwrap_or(0)
}

/// The first `seq` of the save with counter `counter`.
fn first_seq(counter: u64) -> i64 {
    // shortcut: counters beyond 2^31 - 1 share the last band, which only weakens `saves:<n>`.
    i64::try_from(counter.min(0x7fff_ffff)).unwrap_or(0) << 32
}

/// The transaction identifier and parent a record's header names, when it decodes and its
/// identifier is the one of its `txn_id` column.
fn chain_link(record: &StoredRecord, limits: &Limits) -> Option<(u128, Option<u128>)> {
    let header = TxnRecord::decode_header(&record.record, limits).ok()?;
    let id = header.txn_id.as_u128();
    (record.txn_id.as_slice() == id.to_be_bytes()).then(|| (id, header.parent.map(|p| p.as_u128())))
}

/// The journal records the save with counter `counter` writes: `records` in commit order, those
/// without `seq` numbered after the others from the first `seq` of this save, filtered by the
/// retention, at most `record_count` of the newest, and of those the longest newest run in
/// which every record decodes and names the one before as its parent, with unique identifiers.
pub(super) fn retained(
    records: &[StoredRecord],
    retention: Retention,
    counter: u64,
    limits: &Limits,
) -> Vec<StoredRecord> {
    let mut next = first_seq(counter);
    let mut numbered: Vec<StoredRecord> = records
        .iter()
        .map(|record| {
            let seq = record.seq.unwrap_or(next);
            next = next.max(seq.saturating_add(1));
            StoredRecord {
                seq: Some(seq),
                ..record.clone()
            }
        })
        .collect();
    numbered.retain(|record| match retention {
        Retention::None => false,
        Retention::Full => true,
        Retention::Saves(n) => band(record.seq.unwrap_or(0)).saturating_add(n) > counter,
    });
    let cap = usize::try_from(limits.record_count).unwrap_or(usize::MAX);
    let mut kept: Vec<StoredRecord> = Vec::new();
    let mut seen = HashSet::new();
    let mut expected: Option<Option<u128>> = None;
    for record in numbered.into_iter().rev() {
        let Some((id, parent)) = chain_link(&record, limits) else {
            break;
        };
        if kept.len() >= cap || expected.is_some_and(|e| e != Some(id)) || !seen.insert(id) {
            break;
        }
        expected = Some(parent);
        kept.push(record);
    }
    kept.reverse();
    kept
}

/// The rows of the history tables a save writes.
pub(super) struct HistoryRows<'a> {
    /// The retained records, each with its `seq`.
    pub(super) journal: Vec<StoredRecord>,
    /// The preserved foreign data, unchanged.
    pub(super) foreign: &'a [StoredForeign],
    /// The diagnostics, numbered from 1.
    pub(super) diagnostics: Vec<StoredDiagnostic>,
}

/// The history rows of `save` written with the save counter `counter`.
pub(super) fn rows_of<'a>(save: &Save<'a>, counter: u64) -> HistoryRows<'a> {
    let retention = Retention::parse(save.metadata.history_retention.as_deref());
    let foreign = save.loaded.map_or(&[][..], |loaded| loaded.rows.foreign());
    let stored = |id: ObjectId| save.snapshot.resolve(id).is_ok();
    HistoryRows {
        journal: retained(save.journal, retention, counter, &save.limits),
        foreign,
        diagnostics: save
            .diagnostics
            .iter()
            .zip(1..)
            .map(|(diagnostic, id)| diagnostic_row(diagnostic, id, &stored))
            .collect(),
    }
}

/// The `arc_diagnostic` row of `diagnostic`; `object` is the first object it names that is
/// stored.
fn diagnostic_row(
    diagnostic: &Diagnostic,
    id: i64,
    stored: &dyn Fn(ObjectId) -> bool,
) -> StoredDiagnostic {
    StoredDiagnostic {
        id,
        severity: match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
        .to_owned(),
        code: diagnostic.code().to_owned(),
        object: diagnostic
            .objects
            .iter()
            .find(|object| stored(**object))
            .map(|object| object.as_u128().to_be_bytes().to_vec()),
        origin: match diagnostic.origin {
            Origin::Validator => "validator",
            Origin::Node(_) => "node",
            Origin::Load(_) => "load",
            Origin::Executor => "executor",
        }
        .to_owned(),
        arguments: serde_json::to_string(diagnostic.error.arguments())
            .unwrap_or_else(|_| "{}".to_owned()),
    }
}

const INSERT_JOURNAL: &str = "INSERT INTO arc_journal (seq, txn_id, record) VALUES (?1, ?2, ?3)";
const INSERT_DIAGNOSTIC: &str = "INSERT INTO arc_diagnostic \
    (id, severity, code, object, origin, arguments) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";

fn insert_diagnostics(conn: &Connection, rows: &[StoredDiagnostic]) -> rusqlite::Result<()> {
    let mut insert = conn.prepare(INSERT_DIAGNOSTIC)?;
    for row in rows {
        insert.execute(params![
            row.id,
            row.severity,
            row.code,
            row.object,
            row.origin,
            row.arguments
        ])?;
    }
    Ok(())
}

/// Writes every history row into a new package.
pub(super) fn write_full(conn: &Connection, rows: &HistoryRows<'_>) -> rusqlite::Result<()> {
    let mut insert = conn.prepare(INSERT_JOURNAL)?;
    for record in &rows.journal {
        insert.execute(params![record.seq, record.txn_id, record.record])?;
    }
    let mut insert = conn.prepare(
        "INSERT INTO arc_foreign (id, source_format, source_version, source_id, object, \
         record_kind, data, asset) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for row in rows.foreign {
        insert.execute(params![
            row.id,
            row.source_format,
            row.source_version,
            row.source_id,
            row.object,
            row.record_kind,
            row.data,
            row.asset
        ])?;
    }
    insert_diagnostics(conn, &rows.diagnostics)
}

/// Brings the history tables of a package saved in place to `rows`: deletes the journal rows
/// not retained, inserts the new ones, and replaces the diagnostics. Foreign data is not
/// touched: it never changes.
pub(super) fn write_incremental(conn: &Connection, rows: &HistoryRows<'_>) -> rusqlite::Result<()> {
    let present: BTreeSet<i64> = conn
        .prepare("SELECT seq FROM arc_journal")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let kept: BTreeSet<i64> = rows.journal.iter().filter_map(|r| r.seq).collect();
    let mut delete = conn.prepare("DELETE FROM arc_journal WHERE seq = ?1")?;
    for seq in present.difference(&kept) {
        delete.execute([seq])?;
    }
    let mut insert = conn.prepare(INSERT_JOURNAL)?;
    for record in &rows.journal {
        if record.seq.is_some_and(|seq| !present.contains(&seq)) {
            insert.execute(params![record.seq, record.txn_id, record.record])?;
        }
    }
    conn.execute("DELETE FROM arc_diagnostic", [])?;
    insert_diagnostics(conn, &rows.diagnostics)
}

/// The history rows of a package as stored, each table in key order. `record_count` bounds the
/// rows of each table and `memory_bytes`, when set, their bytes together
/// (`doc.load.limit_exceeded`); a value of an unexpected storage class fails with
/// `doc.package.value_invalid`.
pub(super) fn read(
    conn: &Connection,
    path: &str,
    limits: &Limits,
) -> Result<(Vec<StoredRecord>, Vec<StoredForeign>, Vec<StoredDiagnostic>)> {
    let mut bytes = 0_u64;
    let mut charge = |n: usize| -> Result<()> {
        bytes = bytes.saturating_add(u64::try_from(n).unwrap_or(u64::MAX).saturating_add(64));
        match limits.memory_bytes {
            Some(budget) if bytes > budget => {
                Err(Error::new(codes::LOAD_LIMIT_EXCEEDED).with_arg("limit", "memory_bytes"))
            }
            _ => Ok(()),
        }
    };
    let journal = rows(
        conn,
        "SELECT seq, txn_id, record FROM arc_journal ORDER BY seq",
        "arc_journal",
        path,
        limits,
        |cells| {
            let record = StoredRecord {
                seq: Some(cells.int(0, "seq")?),
                txn_id: cells.blob(1, "txn_id")?,
                record: cells.blob(2, "record")?,
            };
            charge(record.txn_id.len().saturating_add(record.record.len()))?;
            Ok(record)
        },
    )?;
    let foreign = rows(
        conn,
        "SELECT id, source_format, source_version, source_id, object, record_kind, data, asset \
         FROM arc_foreign ORDER BY id",
        "arc_foreign",
        path,
        limits,
        |cells| {
            let row = foreign_row(cells)?;
            charge(row.data.len().saturating_add(row.source_format.len()))?;
            Ok(row)
        },
    )?;
    let diagnostics = rows(
        conn,
        "SELECT id, severity, code, object, origin, arguments FROM arc_diagnostic ORDER BY id",
        "arc_diagnostic",
        path,
        limits,
        |cells| {
            let row = StoredDiagnostic {
                id: cells.int(0, "id")?,
                severity: cells.text(1, "severity")?,
                code: cells.text(2, "code")?,
                object: cells.opt_blob(3, "object")?,
                origin: cells.text(4, "origin")?,
                arguments: cells.text(5, "arguments")?,
            };
            charge(row.arguments.len().saturating_add(row.code.len()))?;
            Ok(row)
        },
    )?;
    Ok((journal, foreign, diagnostics))
}

fn foreign_row(cells: &Cells<'_, '_>) -> Result<StoredForeign> {
    Ok(StoredForeign {
        id: cells.int(0, "id")?,
        source_format: cells.text(1, "source_format")?,
        source_version: cells.opt_text(2, "source_version")?,
        source_id: cells.opt_text(3, "source_id")?,
        object: cells.opt_blob(4, "object")?,
        record_kind: cells.text(5, "record_kind")?,
        data: cells.blob(6, "data")?,
        asset: cells.opt_blob(7, "asset")?,
    })
}

fn invalid_value(table: &'static str, column: &'static str, row: i64) -> Error {
    super::read::value_invalid(table, column).with_detail("row", row)
}

/// Checks the history tables and returns the findings:
///
/// - every journal row's `txn_id` is 16 bytes, its record's header decodes
///   ([`TxnRecord::decode_header`]; its code, detail `seq`) and names that identifier
///   (`doc.journal.record_invalid`, reason `txn_id`), and each record after the first names the
///   one before it in `seq` order as its parent (`doc.journal.parent_mismatch`, detail `seq`);
/// - every foreign row's `object` is NULL or 16 identifier bytes and its `asset` NULL or the
///   hash of a stored asset (`doc.package.value_invalid`, `doc.package.asset_not_found`);
/// - every diagnostic row has the severity `error` or `warning`, an origin `validator`,
///   `node`, `load` or `executor`, arguments that are a JSON object, and an `object` that is
///   NULL or names a stored object (`doc.package.value_invalid`, `doc.reference.dangling`).
pub(super) fn check(
    conn: &Connection,
    path: &str,
    limits: &Limits,
    objects: &dyn Fn(&[u8]) -> bool,
) -> Result<Vec<Error>> {
    let mut findings = Vec::new();
    let (journal, foreign, diagnostics) = read(conn, path, limits)?;
    let mut previous: Option<u128> = None;
    for record in &journal {
        let seq = record.seq.unwrap_or(0);
        match TxnRecord::decode_header(&record.record, limits) {
            Err(error) => findings.push(error.with_detail("seq", seq)),
            Ok(header) => {
                let id = header.txn_id.as_u128();
                if record.txn_id.as_slice() != id.to_be_bytes() {
                    findings.push(
                        Error::new(codes::JOURNAL_RECORD_INVALID)
                            .with_detail("reason", "txn_id")
                            .with_detail("seq", seq),
                    );
                }
                if previous.is_some() && header.parent.map(|p| p.as_u128()) != previous {
                    findings.push(
                        Error::new(codes::JOURNAL_PARENT_MISMATCH)
                            .with_arg("id", header.txn_id.to_string())
                            .with_detail("seq", seq),
                    );
                }
                previous = Some(id);
            }
        }
    }
    let mut asset = conn
        .prepare("SELECT count(*) FROM arc_asset WHERE hash = ?1")
        .map_err(|e| sql_error(e, path))?;
    for row in &foreign {
        if row.object.as_deref().is_some_and(|o| parse_id(o).is_none()) {
            findings.push(invalid_value("arc_foreign", "object", row.id));
        }
        if let Some(hash) = &row.asset {
            let count: i64 = asset
                .query_row([hash], |r| r.get(0))
                .map_err(|e| sql_error(e, path))?;
            if count == 0 {
                findings.push(
                    Error::new(codes::PACKAGE_ASSET_NOT_FOUND)
                        .with_arg("hash", hex(hash))
                        .with_detail("row", row.id),
                );
            }
        }
    }
    for row in &diagnostics {
        if !["error", "warning"].contains(&row.severity.as_str()) {
            findings.push(invalid_value("arc_diagnostic", "severity", row.id));
        }
        if !["validator", "node", "load", "executor"].contains(&row.origin.as_str()) {
            findings.push(invalid_value("arc_diagnostic", "origin", row.id));
        }
        if !serde_json::from_str::<Json>(&row.arguments).is_ok_and(|a| a.is_object()) {
            findings.push(invalid_value("arc_diagnostic", "arguments", row.id));
        }
        if let Some(object) = &row.object
            && !objects(object)
        {
            findings.push(
                Error::new(codes::REFERENCE_DANGLING)
                    .with_arg("field", "object")
                    .with_detail("source", format!("arc_diagnostic {}", row.id))
                    .with_detail("target", hex(object)),
            );
        }
    }
    Ok(findings)
}

/// The JSON of the history tables for `inspect`: `journal` (each row's `seq`, `txn_id`, the
/// decoded header or `error`, and `bytes`), `foreign` (each row with `data` and identifiers as
/// hexadecimal) and `diagnostics` (each row with `arguments` as JSON).
pub(super) fn inspect(
    conn: &Connection,
    path: &str,
    limits: &Limits,
) -> Result<(Json, Json, Json)> {
    let (journal, foreign, diagnostics) = read(conn, path, limits)?;
    let opt_hex = |bytes: &Option<Vec<u8>>| bytes.as_deref().map(hex);
    let journal = journal
        .iter()
        .map(|record| {
            let mut entry = json!({
                "seq": record.seq,
                "txn_id": hex(&record.txn_id),
                "bytes": record.record.len(),
            });
            let (member, value) = match TxnRecord::decode_header(&record.record, limits) {
                Ok(header) => (
                    "header",
                    json!({
                        "txn_id": header.txn_id.to_string(),
                        "parent": header.parent.map(|p| p.to_string()),
                        "session": header.session.to_string(),
                        "author": header.author,
                        "time_unix_ms": header.time_unix_millis,
                        "kind": format!("{:?}", header.kind),
                        "command": header.command,
                        "label": header.label,
                        "group": header.group,
                        "type_keys": header.type_keys.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
                        "operations": header.ops,
                    }),
                ),
                Err(error) => (
                    "error",
                    json!({ "code": error.code(), "arguments": error.arguments() }),
                ),
            };
            if let Some(entry) = entry.as_object_mut() {
                entry.insert(member.to_owned(), value);
            }
            entry
        })
        .collect();
    let foreign = foreign
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "source_format": row.source_format,
                "source_version": row.source_version,
                "source_id": row.source_id,
                "object": opt_hex(&row.object),
                "record_kind": row.record_kind,
                "data": hex(&row.data),
                "asset": opt_hex(&row.asset),
            })
        })
        .collect();
    let diagnostics = diagnostics
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "severity": row.severity,
                "code": row.code,
                "object": opt_hex(&row.object),
                "origin": row.origin,
                "arguments": serde_json::from_str::<Json>(&row.arguments).unwrap_or(Json::Null),
            })
        })
        .collect();
    Ok((
        Json::Array(journal),
        Json::Array(foreign),
        Json::Array(diagnostics),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_values_parse_and_default_to_full() {
        assert_eq!(Retention::parse(Some("none")), Retention::None);
        assert_eq!(Retention::parse(Some("saves:3")), Retention::Saves(3));
        for other in [
            None,
            Some("full"),
            Some("all"),
            Some("saves:0"),
            Some("saves:"),
            Some("saves:-1"),
        ] {
            assert_eq!(Retention::parse(other), Retention::Full, "{other:?}");
        }
    }

    #[test]
    fn seq_bands_follow_the_save_counter() {
        assert_eq!(band(first_seq(7)), 7);
        assert_eq!(band(first_seq(7) + 5), 7);
        assert_eq!(band(first_seq(u64::MAX)), 0x7fff_ffff);
    }
}
