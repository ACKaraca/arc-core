//! The commit budget of `docs/testing.md`: a single-object edit commits in 2 ms at one million
//! objects. Ignored by default; run it in a release build:
//! `cargo test --release -p arc-doc --test commit_scale -- --ignored --nocapture`.

#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "a measurement that reports its numbers"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::graph::Evaluators;
use arc_doc::{
    Document, DocumentConfig, FieldDecl, FieldPath, Header, Schema, SchemaVersion, Snapshot,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

const OBJECTS: u128 = 1_000_000;
const ROUNDS: usize = 10_000;

#[test]
#[ignore = "release-mode measurement"]
fn single_object_commit_at_one_million_objects() {
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
    let start = Instant::now();
    doc.submit(|txn| {
        for n in 1..=OBJECTS {
            let values = vec![Some(Value::I64(0))];
            txn.create(
                "test.item",
                ObjectId::from_u128(n),
                Header::default(),
                values,
            )?;
        }
        Ok(())
    })
    .wait()
    .unwrap();
    println!("filled {OBJECTS} objects in {:?}", start.elapsed());
    assert_eq!(doc.snapshot().len(), 1_000_000);

    let mut times = Vec::with_capacity(ROUNDS);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for round in 0..ROUNDS {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let target = ObjectId::from_u128(u128::from(state) % OBJECTS + 1);
        let value = i64::try_from(round).unwrap();
        let start = Instant::now();
        doc.submit(move |txn| txn.modify(target, FieldPath::new("n"), Some(Value::I64(value))))
            .wait()
            .unwrap();
        times.push(start.elapsed());
    }
    times.sort();
    let mean = times.iter().sum::<Duration>() / u32::try_from(ROUNDS).unwrap();
    let p99 = times[ROUNDS * 99 / 100];
    let max = times[ROUNDS - 1];
    println!("submit to result over {ROUNDS} rounds: mean {mean:?}, p99 {p99:?}, max {max:?}");
    assert!(
        p99 < Duration::from_millis(2),
        "p99 {p99:?} exceeds the 2 ms budget"
    );
}
