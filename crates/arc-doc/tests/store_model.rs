//! Model-based test of typed stores, the identifier index and snapshots: random creates,
//! replacements and deletes across several types, with snapshots published and retained at
//! random points, each compared against a reference map of its own state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::{
    FieldDecl, Handle, Header, Object, Schema, SchemaVersion, Snapshot, SnapshotCell, TypeDecl,
    TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};
use proptest::prelude::*;

const KEYS: [&str; 3] = ["test.a", "test.b", "test.c"];

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for key in KEYS {
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("n", ValueType::I64)],
        );
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema))
            .unwrap();
    }
    Arc::new(registry)
}

#[derive(Debug, Clone)]
enum Op {
    Create { kind: usize, n: i64 },
    Replace { pick: usize, n: i64 },
    Rename { pick: usize, name: u8 },
    Delete { pick: usize },
    Publish,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..KEYS.len(), any::<i64>()).prop_map(|(kind, n)| Op::Create { kind, n }),
        2 => (any::<usize>(), any::<i64>()).prop_map(|(pick, n)| Op::Replace { pick, n }),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(pick, name)| Op::Rename { pick, name }),
        2 => any::<usize>().prop_map(|pick| Op::Delete { pick }),
        1 => Just(Op::Publish),
    ]
}

/// The reference state of one object: type index, value and name.
type Model = BTreeMap<ObjectId, (usize, i64, Option<String>)>;

fn check(snapshot: &Snapshot, model: &Model, created: &[ObjectId], stale: &[Handle]) {
    assert_eq!(snapshot.len(), model.len());
    let mut seen = Vec::new();
    for (handle, object) in snapshot.iter() {
        let (kind, n, name) = model.get(&object.id).expect("an object of the model");
        assert_eq!(snapshot.type_key(handle).unwrap().as_str(), KEYS[*kind]);
        assert_eq!(object.values, vec![Some(Value::I64(*n))]);
        assert_eq!(&object.header.name, name);
        assert_eq!(snapshot.resolve(object.id).unwrap(), handle);
        assert!(Arc::ptr_eq(snapshot.get(handle).unwrap(), object));
        seen.push(handle);
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "handle order"
    );
    for (index, key) in KEYS.iter().enumerate() {
        let count = model.values().filter(|entry| entry.0 == index).count();
        assert_eq!(snapshot.iter_type(key).unwrap().count(), count);
    }
    for id in created.iter().filter(|id| !model.contains_key(id)) {
        assert_eq!(
            snapshot.resolve(*id).unwrap_err().code(),
            "doc.object.not_found"
        );
    }
    for handle in stale {
        assert_eq!(
            snapshot.get(*handle).unwrap_err().code(),
            "doc.object.not_found"
        );
    }
}

proptest! {
    #[test]
    fn retained_snapshots_keep_their_own_state(ops in prop::collection::vec(op(), 1..200)) {
        let cell = SnapshotCell::new(Snapshot::empty(registry()));
        let mut set = WorkingSet::new(&cell.load());
        let mut model = Model::new();
        let mut retained = vec![(cell.load(), model.clone(), Vec::new())];
        let mut created = Vec::new();
        let mut handles = BTreeMap::new();
        let mut stale = Vec::new();
        let mut next = 1_u128;
        for op in ops {
            match op {
                Op::Create { kind, n } => {
                    // Identifiers close together, as one generator produces them.
                    let id = ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + next);
                    next += 1;
                    let object = Object::new(
                        id,
                        SchemaVersion::new(1, 0),
                        Header::default(),
                        vec![Some(Value::I64(n))],
                    );
                    handles.insert(id, set.create(KEYS[kind], object).unwrap());
                    model.insert(id, (kind, n, None));
                    created.push(id);
                }
                Op::Replace { pick, n } if !model.is_empty() => {
                    let id = *model.keys().nth(pick % model.len()).unwrap();
                    let handle = set.current().resolve(id).unwrap();
                    prop_assert_eq!(handle, handles[&id]);
                    set.replace_values(handle, vec![Some(Value::I64(n))]).unwrap();
                    model.get_mut(&id).unwrap().1 = n;
                }
                Op::Rename { pick, name } if !model.is_empty() => {
                    let id = *model.keys().nth(pick % model.len()).unwrap();
                    let name = format!("n{name}");
                    let header = Header { name: Some(name.clone()), ..Header::default() };
                    set.replace_header(handles[&id], header).unwrap();
                    model.get_mut(&id).unwrap().2 = Some(name);
                }
                Op::Delete { pick } if !model.is_empty() => {
                    let id = *model.keys().nth(pick % model.len()).unwrap();
                    let old = set.delete(handles[&id]).unwrap();
                    prop_assert_eq!(old.id, id);
                    model.remove(&id);
                    stale.push(handles[&id]);
                }
                Op::Publish => {
                    cell.publish(&set).unwrap();
                    retained.push((cell.load(), model.clone(), stale.clone()));
                    set = WorkingSet::new(&cell.load());
                }
                _ => {}
            }
        }
        retained.push((set.current().clone(), model, stale));
        for (revision, (snapshot, model, stale)) in retained.iter().enumerate() {
            prop_assert_eq!(snapshot.revision(), u64::try_from(revision).unwrap());
            check(snapshot, model, &created, stale);
        }
    }
}
