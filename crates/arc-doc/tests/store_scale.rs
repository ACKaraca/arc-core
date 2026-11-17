//! Snapshot and single-object write costs at one million objects of one type, against generous
//! bounds; the budgets are 50 µs per snapshot and 2 ms per single-object commit. Ignored by
//! default because filling the store takes several seconds in debug builds; run with
//! `cargo nextest run -p arc-doc --release --run-ignored only store_scale`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stderr,
    reason = "the ignored scale test reports its measurements"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::{
    FieldDecl, Header, Object, Schema, SchemaVersion, Snapshot, SnapshotCell, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType, WorkingSet,
};

const OBJECTS: u128 = 1_000_000;
const ROUNDS: u32 = 1_000;

fn object(n: u128) -> Object {
    Object::new(
        ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(0))],
    )
}

#[test]
#[ignore = "fills a store with one million objects"]
fn snapshot_and_single_write_stay_within_bounds_at_one_million_objects() {
    let mut registry = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new("test.a").unwrap(), schema))
        .unwrap();
    let mut set = WorkingSet::new(&Snapshot::empty(Arc::new(registry)));
    for n in 0..OBJECTS {
        set.create("test.a", object(n)).unwrap();
    }
    let cell = SnapshotCell::new(set.current().clone());
    drop(set);

    let start = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(cell.load());
    }
    let snapshot = start.elapsed() / ROUNDS;

    let mut worst = Duration::ZERO;
    let start = Instant::now();
    for round in 0..ROUNDS {
        let begin = Instant::now();
        let base = cell.load();
        let id = object(u128::from(round) * 997 % OBJECTS).id;
        let mut set = WorkingSet::new(&base);
        let handle = base.resolve(id).unwrap();
        set.replace_values(handle, vec![Some(Value::I64(round.into()))])
            .unwrap();
        cell.publish(set.current().clone()).unwrap();
        worst = worst.max(begin.elapsed());
    }
    let write = start.elapsed() / ROUNDS;

    let start = Instant::now();
    for round in 0..ROUNDS {
        let mut set = WorkingSet::new(&cell.load());
        set.create("test.a", object(OBJECTS + u128::from(round)))
            .unwrap();
        cell.publish(set.current().clone()).unwrap();
    }
    let create = start.elapsed() / ROUNDS;

    eprintln!(
        "snapshot {snapshot:?}, replace and publish {write:?} (worst {worst:?}), \
         create and publish {create:?}"
    );
    assert_eq!(
        cell.load().len(),
        usize::try_from(OBJECTS).unwrap() + usize::try_from(ROUNDS).unwrap()
    );
    assert!(snapshot < Duration::from_micros(500), "{snapshot:?}");
    assert!(write < Duration::from_millis(20), "{write:?}");
    assert!(create < Duration::from_millis(20), "{create:?}");
}
