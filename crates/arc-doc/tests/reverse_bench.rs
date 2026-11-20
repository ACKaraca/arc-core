//! Commit, undo and redo of 10,000 objects in documents of 10,000 and 100,000 objects, every
//! object owned by one root (one reverse-index entry each). Ignored by default; run it in a
//! release build and read the minimum of the rounds:
//! `cargo test --release -p arc-doc --test reverse_bench -- --ignored --nocapture`.

#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "a measurement that reports its numbers"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::graph::Evaluators;
use arc_doc::{
    Document, DocumentConfig, FieldDecl, FieldPath, Header, Schema, SchemaVersion, Snapshot,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

const BATCH: u128 = 10_000;
const ROOT: u128 = 1 << 40;

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn document(size: u128) -> Document {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.item").unwrap(), schema))
        .unwrap();
    let mut config = DocumentConfig::default();
    config.txn_limits.memory_bytes = None;
    let doc = Document::open(
        Snapshot::empty(Arc::new(registry)),
        Arc::new(Evaluators::new()),
        config,
    )
    .unwrap();
    create(&doc, 1, size);
    doc
}

fn create(doc: &Document, first: u128, count: u128) {
    doc.submit(move |txn| {
        if first == 1 {
            txn.create(
                "test.item",
                id(ROOT),
                Header::default(),
                vec![Some(Value::I64(0))],
            )?;
        }
        for n in first..first + count {
            let header = Header {
                owner: Some(id(ROOT)),
                ..Header::default()
            };
            txn.create("test.item", id(n), header, vec![Some(Value::I64(0))])?;
        }
        Ok(())
    })
    .wait()
    .unwrap();
}

fn time(times: &mut BTreeMap<&'static str, Duration>, name: &'static str, run: impl FnOnce()) {
    let start = Instant::now();
    run();
    let elapsed = start.elapsed();
    let best = times.entry(name).or_insert(elapsed);
    *best = (*best).min(elapsed);
}

#[test]
#[ignore = "release-mode measurement"]
fn commit_undo_redo_of_ten_thousand_objects() {
    // `ARC_BENCH_ROUNDS` and `ARC_BENCH_SIZE` narrow a run, for a profiler.
    let rounds = std::env::var("ARC_BENCH_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let sizes = match std::env::var("ARC_BENCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        Some(size) => vec![size],
        None => vec![10_000_u128, 100_000],
    };
    for size in sizes {
        let mut best = BTreeMap::new();
        for _ in 0..rounds {
            let doc = document(size);
            let undo = |doc: &Document| assert!(doc.undo().wait().unwrap().value);
            let redo = |doc: &Document| assert!(doc.redo().wait().unwrap().value);
            time(&mut best, "1 modify", || {
                doc.submit(|txn| {
                    for n in 1..=BATCH {
                        txn.modify(id(n), FieldPath::new("n"), Some(Value::I64(1)))?;
                    }
                    Ok(())
                })
                .wait()
                .unwrap();
            });
            time(&mut best, "2 modify undo", || undo(&doc));
            time(&mut best, "3 modify redo", || redo(&doc));
            time(&mut best, "4 create", || create(&doc, size + 1, BATCH));
            time(&mut best, "5 create undo", || undo(&doc));
            time(&mut best, "6 create redo", || redo(&doc));
            time(&mut best, "7 delete", || {
                doc.submit(|txn| {
                    for n in 1..=BATCH {
                        txn.delete(id(n))?;
                    }
                    Ok(())
                })
                .wait()
                .unwrap();
            });
            time(&mut best, "8 delete undo", || undo(&doc));
            time(&mut best, "9 delete redo", || redo(&doc));
        }
        for (name, elapsed) in best {
            println!("{size:>7} objects, {name:<14} min of {rounds}: {elapsed:?}");
        }
    }
}
