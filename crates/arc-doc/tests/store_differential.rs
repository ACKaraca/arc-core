//! Long randomized differential test of typed stores, the identifier index, savepoints and
//! publication against a reference model. A small identifier space makes identifiers be deleted
//! and created again, inside one working set and across working sets, often in another type.
//! Every retained snapshot and savepoint is compared with its own model at the end, and no handle
//! ever issued may read another object. `ARC_DOC_DIFFERENTIAL_OPS` sets the operation count
//! (default 20,000; run 300,000 in a release build for the long run).

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
    Change, FieldDecl, Handle, Header, Object, Savepoint, Schema, SchemaVersion, Snapshot,
    SnapshotCell, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};

const TYPES: usize = 5;
const IDS: u64 = 4_000;

/// Each live identifier's type index and value.
type Model = BTreeMap<u128, (usize, i64)>;

fn key(index: usize) -> String {
    format!("test.t{index}")
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for index in 0..TYPES {
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("n", ValueType::I64)],
        );
        registry
            .register(TypeDecl::new(TypeKey::new(key(index)).unwrap(), schema))
            .unwrap();
    }
    Arc::new(registry)
}

fn object(id: u128, n: i64) -> Object {
    Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(n))],
    )
}

fn check(snapshot: &Snapshot, model: &Model) {
    assert_eq!(snapshot.len(), model.len());
    let mut count = 0;
    for (handle, object) in snapshot.iter() {
        let (kind, n) = model[&object.id.as_u128()];
        assert_eq!(snapshot.type_key(handle).unwrap().as_str(), key(kind));
        assert_eq!(object.values, vec![Some(Value::I64(n))]);
        assert_eq!(snapshot.resolve(object.id).unwrap(), handle);
        count += 1;
    }
    assert_eq!(count, model.len());
    for id in 0..u128::from(IDS) {
        let found = snapshot.resolve(ObjectId::from_u128(id));
        assert_eq!(found.is_ok(), model.contains_key(&id), "id {id}");
    }
}

/// The net change of each identifier between two models.
fn net_changes(base: &Model, current: &Model, recreated: &[u128]) -> BTreeMap<u128, Change> {
    let mut changes = BTreeMap::new();
    for (id, after) in current {
        match base.get(id) {
            None => {
                changes.insert(*id, Change::Created);
            }
            Some(_) if recreated.contains(id) => {
                changes.insert(*id, Change::Recreated);
            }
            Some(before) if before != after => {
                changes.insert(*id, Change::Modified);
            }
            Some(_) => {}
        }
    }
    for id in base.keys().filter(|id| !current.contains_key(id)) {
        changes.insert(*id, Change::Deleted);
    }
    changes
}

struct Mark {
    savepoint: Savepoint,
    model: Model,
    recreated: Vec<u128>,
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one sequential scenario reads best in one place"
)]
fn random_histories_match_the_model_in_every_retained_state() {
    let operations: u64 = std::env::var("ARC_DOC_DIFFERENTIAL_OPS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(20_000);
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let mut set = WorkingSet::new(&cell.load());
    let mut base_model = Model::new();
    let mut model = Model::new();
    // Identifiers of the base deleted and created again in this set and still live.
    let mut recreated: Vec<u128> = Vec::new();
    let mut marks: Vec<Mark> = Vec::new();
    let mut retained: Vec<(Snapshot, Model)> = Vec::new();
    let mut issued: Vec<(Handle, u128)> = Vec::new();
    for _ in 0..operations {
        let id = u128::from(next() % IDS);
        let n = i64::from_le_bytes(next().to_le_bytes());
        match next() % 32 {
            0..=11 => {
                let kind = usize::try_from(next() % 5).unwrap();
                let result = set.create(&key(kind), object(id, n));
                assert_eq!(result.is_ok(), !model.contains_key(&id), "create {id}");
                if let Ok(handle) = result {
                    if base_model.contains_key(&id) {
                        recreated.push(id);
                    }
                    model.insert(id, (kind, n));
                    issued.push((handle, id));
                }
            }
            12..=18 => {
                if let Some((kind, value)) = model.get_mut(&id) {
                    let handle = set.current().resolve(ObjectId::from_u128(id)).unwrap();
                    set.replace_values(handle, vec![Some(Value::I64(n))])
                        .unwrap();
                    *value = n;
                    assert_eq!(set.current().type_key(handle).unwrap().as_str(), key(*kind));
                }
            }
            19..=25 => {
                if model.remove(&id).is_some() {
                    let handle = set.current().resolve(ObjectId::from_u128(id)).unwrap();
                    assert_eq!(set.delete(handle).unwrap().id.as_u128(), id);
                    recreated.retain(|other| *other != id);
                    assert!(set.current().get(handle).is_err());
                }
            }
            26 | 27 => marks.push(Mark {
                savepoint: set.savepoint(),
                model: model.clone(),
                recreated: recreated.clone(),
            }),
            28 if !marks.is_empty() => {
                let at = usize::try_from(next()).unwrap() % marks.len();
                marks.truncate(at + 1);
                let mark = &marks[at];
                set.rollback_to(&mark.savepoint).unwrap();
                model.clone_from(&mark.model);
                recreated.clone_from(&mark.recreated);
                if next() % 2 == 0 {
                    retained.push((set.current().clone(), model.clone()));
                }
            }
            29 if !marks.is_empty() => {
                let at = usize::try_from(next()).unwrap() % marks.len();
                set.release_savepoint(&marks[at].savepoint).unwrap();
                let released = marks.split_off(at);
                assert_eq!(
                    set.rollback_to(&released[0].savepoint).unwrap_err().code(),
                    "doc.savepoint.invalid"
                );
            }
            _ => {
                let expected = net_changes(&base_model, &model, &recreated);
                let touched: BTreeMap<u128, Change> = set
                    .touched()
                    .iter()
                    .map(|(id, change)| (id.as_u128(), *change))
                    .collect();
                assert_eq!(touched, expected);
                // Now and then a sibling built on the same base loses the race.
                let sibling = WorkingSet::new(set.base());
                cell.publish(&set).unwrap();
                assert!(cell.publish(&sibling).is_err());
                if next() % 4 == 0 && retained.len() < 300 {
                    retained.push((cell.load(), model.clone()));
                }
                set = WorkingSet::new(&cell.load());
                base_model.clone_from(&model);
                recreated.clear();
                marks.clear();
            }
        }
    }
    retained.push((set.current().clone(), model));
    for (snapshot, model) in &retained {
        check(snapshot, model);
    }
    for (snapshot, _) in &retained {
        for (handle, id) in &issued {
            if let Ok(object) = snapshot.get(*handle) {
                assert_eq!(object.id.as_u128(), *id, "a handle read another object");
            }
        }
    }
}
