//! The stored history of a package (CORE-DOC-040): journal records, preserved foreign data and
//! diagnostics survive saves and reopening as the retention allows; the incremental save and a
//! full rewrite of the same state write the same history rows; `inspect` dumps them and `check`
//! reports seeded corruptions.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;
mod recovery_support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use arc_base::id::{IdGenerator, Identifier};
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::graph::Evaluators;
use arc_doc::package::{Mode, Package, Save, Strategy as SaveStrategy, check, inspect};
use arc_doc::{
    Change, Document, DocumentConfig, DocumentValidator, Finding, LoadOptions, Severity, Snapshot,
    StoredForeign, StoredRecord, Value,
};
use package_support::{TempDir, cases, owner};
use proptest::prelude::*;
use recovery_support::{State, registry, set, write_base};
use rusqlite::Connection;

type Rows = Vec<Vec<rusqlite::types::Value>>;

fn table(path: &Path, sql: &str) -> Rows {
    let conn = Connection::open(path).unwrap();
    let mut statement = conn.prepare(sql).unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|at| row.get(at)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

const JOURNAL: &str = "SELECT seq, txn_id, record FROM arc_journal ORDER BY seq";
const FOREIGN: &str = "SELECT * FROM arc_foreign ORDER BY id";
const DIAGNOSTICS: &str = "SELECT * FROM arc_diagnostic ORDER BY id";

/// Warns about every touched object with a negative value.
fn negative_warning() -> DocumentValidator {
    Arc::new(
        |_: &Snapshot, after: &Snapshot, changes: &BTreeMap<ObjectId, Change>| {
            changes
                .keys()
                .filter(|id| {
                    after.resolve(**id).is_ok_and(|handle| {
                        after.get(handle).is_ok_and(
                            |o| matches!(o.values.first(), Some(Some(Value::I64(v))) if *v < 0),
                        )
                    })
                })
                .map(|id| Finding {
                    severity: Severity::Warning,
                    error: Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("id", id.to_string()),
                })
                .collect()
        },
    )
}

/// Adds foreign rows to the package at `path`: `(object, with asset)` each.
fn seed_foreign(path: &Path, rows: &[(Option<u128>, bool)]) {
    let conn = Connection::open(path).unwrap();
    for ((object, asset), n) in rows.iter().zip(0_u8..) {
        let object = object.map(|o| o.to_be_bytes().to_vec());
        let asset = asset.then(|| {
            let hash = blake3::hash(&[n]).as_bytes().to_vec();
            conn.execute(
                "INSERT OR IGNORE INTO arc_asset VALUES (?1, 'application/octet-stream', 1, 0, 0)",
                [&hash],
            )
            .unwrap();
            conn.execute(
                "INSERT OR IGNORE INTO arc_chunk VALUES (?1, 0, ?2)",
                rusqlite::params![hash, vec![n]],
            )
            .unwrap();
            hash
        });
        conn.execute(
            "INSERT INTO arc_foreign (source_format, source_version, source_id, object, \
             record_kind, data, asset) VALUES ('fmt', '1', ?1, ?2, 'kind', ?3, ?4)",
            rusqlite::params![format!("s{n}"), object, vec![n; 3], asset],
        )
        .unwrap();
    }
}

fn set_retention(path: &Path, retention: Option<&str>) {
    let conn = Connection::open(path).unwrap();
    match retention {
        Some(text) => conn
            .execute(
                "INSERT OR REPLACE INTO arc_meta (key, value) VALUES ('history_retention', ?1)",
                [text],
            )
            .unwrap(),
        None => conn
            .execute("DELETE FROM arc_meta WHERE key = 'history_retention'", [])
            .unwrap(),
    };
}

/// The reference model of retention: which of `all` (identifier and the save that first stored
/// it) a package with save counter `counter` keeps.
fn expected(all: &[(u128, u64)], retention: Option<&str>, counter: u64) -> Vec<u128> {
    all.iter()
        .filter(|(_, saved)| match retention {
            Some("none") => false,
            Some("saves:1") => saved + 1 > counter,
            Some("saves:2") => saved + 2 > counter,
            _ => true,
        })
        .map(|(id, _)| *id)
        .collect()
}

fn txn_ids(rows: &Rows) -> Vec<u128> {
    rows.iter()
        .map(|row| match &row[1] {
            rusqlite::types::Value::Blob(bytes) => {
                u128::from_be_bytes(bytes[..].try_into().unwrap())
            }
            other => panic!("{other:?}"),
        })
        .collect()
}

fn last_txn(document: &Document) -> u128 {
    document.with_journal(|j| j.last().unwrap().record().txn_id.as_u128())
}

fn retention() -> impl proptest::strategy::Strategy<Value = Option<&'static str>> {
    prop::sample::select(vec![
        None,
        Some("none"),
        Some("full"),
        Some("saves:1"),
        Some("saves:2"),
    ])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(10), ..ProptestConfig::default() })]

    #[test]
    fn history_survives_saves_and_reopening_and_both_strategies_agree(
        initial in prop::collection::btree_map(1_u128..12, -3_i64..9, 0..5),
        foreign in prop::collection::vec((prop::option::of(1_u128..12), any::<bool>()), 0..4),
        sessions in prop::collection::vec(
            prop::collection::vec(
                (
                    prop::collection::vec((1_u128..12, -3_i64..9), 0..5),
                    retention(),
                    prop::collection::vec((1_i64..9, prop::option::of((prop::option::of(1_u128..12), any::<u8>()))), 0..4),
                ),
                1..4,
            ),
            1..3,
        ),
    ) {
        let dir = TempDir::new("history");
        let path = dir.path("model.arc");
        write_base(&path, &initial, 1);
        seed_foreign(&path, &foreign);
        // The foreign rows by key with their data, as the table holds them.
        let mut foreign_model: BTreeMap<i64, Vec<u8>> = table(&path, FOREIGN)
            .iter()
            .map(|row| match (&row[0], &row[6]) {
                (rusqlite::types::Value::Integer(id), rusqlite::types::Value::Blob(data)) => {
                    (*id, data.clone())
                }
                other => panic!("{other:?}"),
            })
            .collect();
        // Every committed transaction and the save counter of the save that first stored it.
        let mut all: Vec<(u128, u64)> = Vec::new();
        let mut kept: Vec<u128> = Vec::new();
        let mut model: State = initial.clone();
        for (session, saves) in sessions.iter().enumerate() {
            let mut package =
                Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
            let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
            let config = DocumentConfig {
                ids: IdGenerator::seeded(session as u64 + 1),
                workers: 0,
                ..DocumentConfig::default()
            };
            let document = package
                .open_document(registry(), Arc::new(Evaluators::new()), &LoadOptions::default(), config)
                .unwrap();
            document.add_validator(negative_warning());
            let stored: Vec<u128> = document
                .stored_journal()
                .iter()
                .map(|r| u128::from_be_bytes(r.txn_id[..].try_into().unwrap()))
                .collect();
            prop_assert_eq!(&stored, &kept, "reopened journal");
            prop_assert_eq!(document.loaded_rows().foreign().len(), foreign_model.len());
            prop_assert_eq!(document.snapshot().foreign_len(), foreign_model.len());
            let mut saved_revision = document.snapshot().revision();
            let mut unsaved: Vec<u128> = Vec::new();
            for (edits, retention, foreign_edits) in saves {
                for (id, v) in edits {
                    if model.get(id) != Some(v) {
                        set(&document, *id, *v);
                        model.insert(*id, *v);
                        unsaved.push(last_txn(&document));
                    }
                }
                for (key, edit) in foreign_edits {
                    let (key, job) = (*key, *edit);
                    let done = document
                        .submit(move |txn| match job {
                            Some((object, byte)) => txn.set_foreign(StoredForeign {
                                id: key,
                                source_format: "fmt".to_owned(),
                                source_version: None,
                                source_id: Some(format!("e{byte}")),
                                object: object.map(|o| o.to_be_bytes().to_vec()),
                                record_kind: "kind".to_owned(),
                                data: vec![byte; 2],
                                asset: None,
                            }),
                            None => txn.remove_foreign(key),
                        })
                        .wait();
                    match (edit, done) {
                        (Some((_, byte)), Ok(_)) => {
                            foreign_model.insert(key, vec![*byte; 2]);
                            unsaved.push(last_txn(&document));
                        }
                        (None, Ok(_)) => {
                            prop_assert!(foreign_model.remove(&key).is_some());
                            unsaved.push(last_txn(&document));
                        }
                        (None, Err(error)) => {
                            prop_assert_eq!(error.code(), "doc.foreign.not_found");
                            prop_assert!(!foreign_model.contains_key(&key));
                        }
                        (Some(_), Err(error)) => prop_assert!(false, "{error:?}"),
                    }
                }
                set_retention_meta(&mut package, &path, *retention);
                // The full rewrite of the same state, to another file.
                let mut records = document.stored_journal();
                records.extend(document.with_journal(|j| {
                    j.iter()
                        .filter(|e| e.revision() > saved_revision)
                        .map(|e| StoredRecord {
                            seq: None,
                            txn_id: e.record().txn_id.as_u128().to_be_bytes().to_vec(),
                            record: e.bytes().to_vec(),
                        })
                        .collect::<Vec<_>>()
                }));
                let snapshot = document.snapshot();
                let touched: BTreeMap<ObjectId, Vec<u8>> =
                    snapshot.iter().map(|(_, o)| (o.id, vec![1; 16])).collect();
                let diagnostics = document.diagnostics_snapshot().diagnostics;
                let metadata = package.metadata().clone();
                let save = Save {
                    snapshot: &snapshot,
                    loaded: Some(&loaded),
                    touched: &touched,
                    metadata: &metadata,
                    previews: &[],
                    journal: &records,
                    diagnostics: &diagnostics,
                    limits: Limits::DEFAULT,
                };
                let copy = dir.path("copy.arc");
                let _ = std::fs::remove_file(&copy);
                package.save(&save, &SaveStrategy::Copy(copy.clone()), &mut |_| Ok(())).unwrap();

                let saved = package.save_document(&document, &[], None, &mut |_| Ok(())).unwrap();
                prop_assert!(saved.incremental, "{:?}", saved.refused);
                saved_revision = snapshot.revision();
                let counter = saved.metadata.save_counter;
                all.extend(unsaved.drain(..).map(|id| (id, counter)));
                kept = expected(&all, *retention, counter);
                // A record a save dropped never comes back.
                all.retain(|(id, _)| kept.contains(id));
                let journal = table(&path, JOURNAL);
                prop_assert_eq!(txn_ids(&journal), kept.clone(), "retention {:?}", retention);
                prop_assert_eq!(&journal, &table(&copy, JOURNAL));
                prop_assert_eq!(table(&path, DIAGNOSTICS), table(&copy, DIAGNOSTICS));
                // Both saves write the rows of the snapshot: table by table equal, and equal to
                // the reference map.
                prop_assert_eq!(table(&path, FOREIGN), table(&copy, FOREIGN));
                let stored: BTreeMap<i64, Vec<u8>> = snapshot
                    .foreign_rows()
                    .map(|row| (row.id, row.data.clone()))
                    .collect();
                prop_assert_eq!(&stored, &foreign_model);
                prop_assert_eq!(table(&path, FOREIGN).len(), foreign_model.len());
                let negative = model.values().filter(|v| **v < 0).count();
                prop_assert!(table(&path, DIAGNOSTICS).len() <= negative);
                prop_assert_eq!(check(&path, &Limits::DEFAULT).unwrap().len(), 0);
                prop_assert_eq!(check(&copy, &Limits::DEFAULT).unwrap().len(), 0);
            }
            drop(document);
            drop(package);
            let loaded = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT)
                .unwrap()
                .load(registry(), &LoadOptions::default())
                .unwrap();
            prop_assert_eq!(loaded.rows.diagnostics().len(), table(&path, DIAGNOSTICS).len());
            prop_assert_eq!(loaded.rows.foreign().len(), foreign_model.len());
            prop_assert_eq!(loaded.snapshot.foreign_len(), foreign_model.len());
            let dump = inspect(&path, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(dump["foreign"].as_array().unwrap().len(), foreign_model.len());
            prop_assert_eq!(dump["journal"].as_array().unwrap().len(), kept.len());
            prop_assert!(dump["journal"].as_array().unwrap().iter().all(|r| r.get("header").is_some()));
        }
    }
}

/// Sets the retention in the package's metadata through its file, keeping the package's view
/// in step: the next save writes the metadata it read at open.
fn set_retention_meta(package: &mut Package, path: &Path, retention: Option<&str>) {
    set_retention(path, retention);
    let owner = owner("u", "h");
    // Reopening reads the new metadata; the edit lock passes from the old handle to the new.
    let reopened = {
        let placeholder = Package::open(path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
        std::mem::replace(package, placeholder)
    };
    drop(reopened);
    *package = Package::open(path, Mode::Edit(&owner), &Limits::DEFAULT).unwrap();
}

/// A package with two saves of history, a foreign row with an asset and a diagnostic.
fn history_package(dir: &TempDir) -> std::path::PathBuf {
    let path = dir.path("model.arc");
    write_base(&path, &BTreeMap::from([(1, 1)]), 1);
    seed_foreign(&path, &[(Some(1), true)]);
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let document = package
        .open_document(
            registry(),
            Arc::new(Evaluators::new()),
            &LoadOptions::default(),
            DocumentConfig {
                workers: 0,
                ..DocumentConfig::default()
            },
        )
        .unwrap();
    document.add_validator(negative_warning());
    for v in [2, 3, -4] {
        set(&document, 1, v);
        package
            .save_document(&document, &[], None, &mut |_| Ok(()))
            .unwrap();
    }
    path
}

#[test]
fn check_reports_seeded_corruptions_of_the_history_tables() {
    let dir = TempDir::new("history check");
    let path = history_package(&dir);
    assert_eq!(check(&path, &Limits::DEFAULT).unwrap().len(), 0);
    assert_eq!(table(&path, JOURNAL).len(), 3);
    assert_eq!(table(&path, DIAGNOSTICS).len(), 1);
    let cases: [(&str, &str); 7] = [
        (
            "UPDATE arc_journal SET txn_id = zeroblob(16) WHERE seq = (SELECT max(seq) FROM arc_journal)",
            codes::JOURNAL_RECORD_INVALID.code,
        ),
        (
            "UPDATE arc_journal SET record = substr(record, 1, 20) WHERE seq = (SELECT min(seq) FROM arc_journal)",
            codes::JOURNAL_RECORD_INVALID.code,
        ),
        (
            "DELETE FROM arc_journal WHERE seq = (SELECT min(seq) + 0 FROM arc_journal WHERE seq > (SELECT min(seq) FROM arc_journal))",
            codes::JOURNAL_PARENT_MISMATCH.code,
        ),
        (
            "UPDATE arc_foreign SET asset = zeroblob(32)",
            codes::PACKAGE_ASSET_NOT_FOUND.code,
        ),
        (
            "UPDATE arc_foreign SET object = zeroblob(5)",
            codes::PACKAGE_VALUE_INVALID.code,
        ),
        (
            "UPDATE arc_diagnostic SET severity = 'fatal'",
            codes::PACKAGE_VALUE_INVALID.code,
        ),
        (
            "UPDATE arc_diagnostic SET object = zeroblob(16)",
            codes::REFERENCE_DANGLING.code,
        ),
    ];
    for (n, (sql, code)) in cases.iter().enumerate() {
        let copy = dir.path(&format!("corrupt-{n}.arc"));
        std::fs::copy(&path, &copy).unwrap();
        Connection::open(&copy).unwrap().execute(sql, []).unwrap();
        let findings = check(&copy, &Limits::DEFAULT).unwrap();
        assert!(
            findings.iter().any(|f| f.code() == *code),
            "{sql}: {:?}",
            findings.iter().map(Error::code).collect::<Vec<_>>()
        );
        // A damaged history never blocks opening the document.
        Package::open(&copy, Mode::ReadOnly, &Limits::DEFAULT)
            .unwrap()
            .load(registry(), &LoadOptions::default())
            .unwrap();
    }
}

#[test]
fn a_journal_cut_at_a_damaged_record_keeps_the_newest_chain() {
    let dir = TempDir::new("history cut");
    let path = history_package(&dir);
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE arc_journal SET record = x'00' WHERE seq = (SELECT min(seq) FROM arc_journal)",
            [],
        )
        .unwrap();
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let config = DocumentConfig {
        ids: IdGenerator::seeded(9),
        workers: 0,
        ..DocumentConfig::default()
    };
    let document = package
        .open_document(
            registry(),
            Arc::new(Evaluators::new()),
            &LoadOptions::default(),
            config,
        )
        .unwrap();
    assert_eq!(document.stored_journal().len(), 3);
    set(&document, 1, 7);
    package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    assert_eq!(
        table(&path, JOURNAL).len(),
        3,
        "the damaged oldest record is dropped"
    );
    assert!(check(&path, &Limits::DEFAULT).unwrap().is_empty());
}

#[test]
fn compaction_keeps_the_assets_of_foreign_data_and_the_journal() {
    let dir = TempDir::new("history compact");
    let path = history_package(&dir);
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();
    let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
    let journal = loaded.rows.journal().to_vec();
    let touched = BTreeMap::new();
    let metadata = package.metadata().clone();
    let save = Save {
        snapshot: &loaded.snapshot,
        loaded: Some(&loaded),
        touched: &touched,
        metadata: &metadata,
        previews: &[],
        journal: &journal,
        diagnostics: &[],
        limits: Limits::DEFAULT,
    };
    package
        .save(&save, &SaveStrategy::Compact, &mut |_| Ok(()))
        .unwrap();
    assert_eq!(table(&path, "SELECT hash FROM arc_asset").len(), 1);
    assert_eq!(table(&path, JOURNAL).len(), 3);
    assert_eq!(table(&path, FOREIGN).len(), 1);
    assert!(table(&path, DIAGNOSTICS).is_empty());
    assert!(check(&path, &Limits::DEFAULT).unwrap().is_empty());
}
