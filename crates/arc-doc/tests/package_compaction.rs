//! Compaction (CORE-DOC-049): after a compaction the document reopens equal to the one saved
//! before, the assets still referenced survive, unreferenced assets and their chunks are removed,
//! the reported size is the real difference of the file sizes, and journal records beyond the
//! retention setting are removed.

#![cfg(feature = "package")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod package_support;

use std::path::Path;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::graph::Evaluators;
use arc_doc::package::{Metadata, Mode, Package};
use arc_doc::{
    AssetHash, Document, DocumentConfig, FieldDecl, Header, LoadOptions, Schema, SchemaVersion,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};
use package_support::{TempDir, objects, owner};

/// Deterministic pseudo-random bytes.
fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[3]
        })
        .collect()
}

fn registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("main", ValueType::Asset).optional(),
            FieldDecl::new("more", ValueType::List(Box::new(ValueType::Asset))).optional(),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("k.asset").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn holder(id: u128, main: Option<AssetHash>, more: Vec<AssetHash>) -> (u128, Vec<Option<Value>>) {
    (
        id,
        vec![
            main.map(|hash| Value::Asset(Box::new(hash))),
            Some(Value::List(
                more.into_iter()
                    .map(|hash| Value::Asset(Box::new(hash)))
                    .collect(),
            )),
        ],
    )
}

fn create_holder(document: &Document, (id, values): (u128, Vec<Option<Value>>)) {
    document
        .submit(move |txn| {
            txn.create(
                "k.asset",
                ObjectId::from_u128(id),
                Header::default(),
                values,
            )
            .map(drop)
        })
        .wait()
        .unwrap();
}

fn count(path: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

/// A package holding four assets: `kept` (referenced by object 1), `kept_large` (chunked,
/// referenced by object 3), `dropped` (referenced by object 2, which is then deleted) and
/// `unused` (chunked, never referenced). Returns the package, the document and the hashes.
fn setup(dir: &TempDir) -> (Package, Document, [AssetHash; 4]) {
    let mut package = Package::create(
        &dir.path("compact.arc"),
        &Metadata::default(),
        &owner("u", "h"),
    )
    .unwrap();
    let kept = package
        .put_asset("text/plain", &mut bytes(1, 3000).as_slice())
        .unwrap();
    let kept_large = package
        .put_asset("model/x", &mut bytes(2, 9 * 1024 * 1024).as_slice())
        .unwrap();
    let dropped = package
        .put_asset("text/plain", &mut bytes(3, 4000).as_slice())
        .unwrap();
    let unused = package
        .put_asset("model/x", &mut bytes(4, 5 * 1024 * 1024).as_slice())
        .unwrap();
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    create_holder(&document, holder(1, Some(kept), vec![]));
    create_holder(&document, holder(2, None, vec![dropped]));
    create_holder(&document, holder(3, Some(kept_large), vec![]));
    package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    document
        .submit(|txn| txn.delete(ObjectId::from_u128(2)))
        .wait()
        .unwrap();
    package
        .save_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();
    (package, document, [kept, kept_large, dropped, unused])
}

#[test]
fn compaction_reopens_equal_keeps_referenced_assets_and_reports_the_real_size() {
    let dir = TempDir::new("compaction reopen");
    let (mut package, document, [kept, kept_large, dropped, unused]) = setup(&dir);
    let path = package.path().to_path_buf();
    let before = objects(&document.snapshot());
    assert_eq!(count(&path, "arc_asset"), 4);
    let chunks_before = count(&path, "arc_chunk");

    let compaction = package
        .compact_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();

    // The never-referenced asset and its chunks are gone. `dropped` is no longer referenced by an
    // object, but the retained create record of its object still names it, so it stays.
    assert_eq!(count(&path, "arc_asset"), 3);
    assert!(count(&path, "arc_chunk") < chunks_before);
    assert_eq!(compaction.assets_removed, 1);
    assert_eq!(
        compaction.chunks_removed,
        u64::try_from(chunks_before - count(&path, "arc_chunk")).unwrap()
    );
    assert!(package.asset_bytes(&unused).is_err());
    assert_eq!(package.asset_bytes(&dropped).unwrap(), bytes(3, 4000));
    assert_eq!(package.asset_bytes(&kept).unwrap(), bytes(1, 3000));
    assert_eq!(
        package.asset_bytes(&kept_large).unwrap(),
        bytes(2, 9 * 1024 * 1024)
    );

    // The report is the real difference of the file sizes.
    let actual_after = std::fs::metadata(&path).unwrap().len();
    assert_eq!(compaction.file_bytes_after, actual_after);
    assert!(compaction.reclaimed_bytes > 0);
    assert_eq!(
        compaction.reclaimed_bytes,
        compaction.file_bytes_before - compaction.file_bytes_after
    );

    // The compacted package reopens to the same objects.
    let reopened = Package::open(&path, Mode::ReadOnly, &Limits::DEFAULT).unwrap();
    let loaded = reopened
        .open_document(
            registry(),
            Arc::new(Evaluators::new()),
            &LoadOptions::default(),
            DocumentConfig::default(),
        )
        .unwrap();
    assert_eq!(objects(&loaded.snapshot()), before);
}

#[test]
fn compaction_removes_journal_records_beyond_the_retention() {
    let dir = TempDir::new("compaction retention");
    let (package, document, _) = setup(&dir);
    let path = package.path().to_path_buf();
    let records = count(&path, "arc_journal");
    assert!(records > 0);
    drop(package);
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO arc_meta (key, value) VALUES ('history_retention', 'none')",
            [],
        )
        .unwrap();
    }
    let mut package = Package::open(&path, Mode::Edit(&owner("u", "h")), &Limits::DEFAULT).unwrap();

    let compaction = package
        .compact_document(&document, &[], None, &mut |_| Ok(()))
        .unwrap();

    assert_eq!(count(&path, "arc_journal"), 0);
    assert_eq!(
        compaction.journal_records_removed,
        u64::try_from(records).unwrap()
    );
    // The document itself is unchanged by the rewrite.
    let loaded = package.load(registry(), &LoadOptions::default()).unwrap();
    assert_eq!(objects(&loaded.snapshot), objects(&document.snapshot()));
}
