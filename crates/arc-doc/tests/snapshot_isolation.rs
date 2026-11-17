//! Readers that hold and reload snapshots while a writer publishes always see one whole
//! revision.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions assert on valid fixtures"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::{
    FieldDecl, Header, Object, Schema, SchemaVersion, Snapshot, SnapshotCell, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType, WorkingSet,
};

const BASE: u128 = 2_000;
const STEPS: u64 = 200;

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("revision", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new("test.a").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn object(n: u128, revision: u64) -> Object {
    Object::new(
        ObjectId::from_u128(n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(i64::try_from(revision).unwrap()))],
    )
}

/// The first published revision is 1 and holds `BASE` objects with value 0. Each later revision
/// `r` sets object 0 to `r` and adds one object with value `r`; so revision `r` holds
/// `BASE + r - 1` objects, object 0 carries `r` (or 0 at revision 1), and no value exceeds it.
fn assert_consistent(snapshot: &Snapshot) {
    let revision = snapshot.revision();
    let steps = usize::try_from(revision - 1).unwrap();
    assert_eq!(snapshot.len(), usize::try_from(BASE).unwrap() + steps);
    let latest = if revision == 1 { 0 } else { revision };
    let first = snapshot.resolve(ObjectId::from_u128(0)).unwrap();
    assert_eq!(
        snapshot.get(first).unwrap().values,
        object(0, latest).values
    );
    let mut count = 0;
    for (handle, object) in snapshot.iter() {
        assert_eq!(snapshot.resolve(object.id).unwrap(), handle);
        let Some(Value::I64(value)) = object.values[0] else {
            panic!("an integer value");
        };
        assert!(value <= i64::try_from(latest).unwrap());
        count += 1;
    }
    assert_eq!(count, snapshot.len());
}

#[test]
fn readers_see_whole_revisions_while_a_writer_publishes() {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    set.create("test.a", object(0, 0)).unwrap();
    for n in 1..BASE {
        set.create("test.a", object(1_000_000 + n, 0)).unwrap();
    }
    let cell = Arc::new(SnapshotCell::new(set.current().clone()));
    let done = Arc::new(AtomicBool::new(false));

    let readers: Vec<_> = (0..3)
        .map(|_| {
            let cell = Arc::clone(&cell);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let held = cell.load();
                let mut observed = 0_u32;
                while !done.load(Ordering::Acquire) {
                    assert_consistent(&cell.load());
                    observed += 1;
                }
                assert_consistent(&held);
                observed
            })
        })
        .collect();

    for step in 1..=STEPS {
        let base = cell.load();
        let revision = base.revision() + 1;
        let mut set = WorkingSet::new(&base);
        let first = base.resolve(ObjectId::from_u128(0)).unwrap();
        set.replace_values(first, object(0, revision).values)
            .unwrap();
        set.create("test.a", object(u128::from(step), revision))
            .unwrap();
        cell.publish(set.current().clone()).unwrap();
    }
    done.store(true, Ordering::Release);
    for reader in readers {
        assert!(reader.join().unwrap() > 0);
    }
    assert_eq!(cell.load().revision(), 1 + STEPS);
    assert_consistent(&cell.load());
}
