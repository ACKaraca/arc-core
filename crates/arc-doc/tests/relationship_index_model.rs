//! Model-based test of the reverse reference index (CORE-DOC-010): random creates, value and
//! owner replacements, deletes, re-creations of one identifier, savepoints and rollbacks, with
//! every intermediate snapshot retained and compared at the end with an index rebuilt from
//! scratch from that snapshot's objects.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::relationship::references;
use arc_doc::{
    Cardinality, FieldDecl, Header, Object, OnCopy, OnDelete, ReferenceField, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Snapshot, Strength, TypeDecl, TypeKey, TypeRegistry,
    Value, ValueType, WorkingSet,
};
use proptest::prelude::*;

const POOL: u128 = 12;

fn reference(cardinality: Cardinality) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: Vec::new(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality,
        },
    })
}

fn registry() -> Arc<TypeRegistry> {
    let many = || ValueType::List(Box::new(reference(Cardinality::Many { max: 4 })));
    let record = |name: &str| {
        ValueType::Record(vec![
            FieldDecl::new(name, reference(Cardinality::Optional)).optional(),
        ])
    };
    let mut registry = TypeRegistry::new();
    let m = vec![
        FieldDecl::new("a", reference(Cardinality::Optional)).optional(),
        FieldDecl::new("l", many()),
        FieldDecl::new("r", record("x")).optional(),
    ];
    let o = vec![FieldDecl::new("b", ValueType::List(Box::new(record("y"))))];
    for (key, fields) in [("test.m", m), ("test.o", o)] {
        registry
            .register(TypeDecl::new(
                TypeKey::new(key).unwrap(),
                Schema::new(SchemaVersion::new(1, 0), fields),
            ))
            .unwrap();
    }
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

/// Values of `test.m` (`other` false) or `test.o` from target numbers.
fn values(other: bool, targets: &[u128]) -> Vec<Option<Value>> {
    let refs = |list: &[u128]| list.iter().map(|&n| Value::Reference(id(n))).collect();
    let record = |n: Option<&u128>| Value::Record(vec![n.map(|&n| Value::Reference(id(n)))]);
    if other {
        let items = targets.chunks(1).map(|c| record(c.first())).collect();
        return vec![Some(Value::List(items))];
    }
    vec![
        targets.first().map(|&n| Value::Reference(id(n))),
        Some(Value::List(refs(targets.get(1..).unwrap_or_default()))),
        targets.get(4).map(|n| record(Some(n))),
    ]
}

#[derive(Debug, Clone)]
enum Op {
    Create {
        n: u128,
        other: bool,
        targets: Vec<u128>,
        owner: Option<u128>,
    },
    Replace {
        n: u128,
        targets: Vec<u128>,
    },
    Relink {
        n: u128,
        owner: Option<u128>,
    },
    Delete {
        n: u128,
    },
    Savepoint,
    Rollback {
        pick: usize,
    },
    Publish,
}

fn targets() -> impl Strategy<Value = Vec<u128>> {
    prop::collection::vec(0..POOL, 0..6)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..POOL, any::<bool>(), targets(), prop::option::of(0..POOL))
            .prop_map(|(n, other, targets, owner)| Op::Create { n, other, targets, owner }),
        3 => (0..POOL, targets()).prop_map(|(n, targets)| Op::Replace { n, targets }),
        1 => (0..POOL, prop::option::of(0..POOL)).prop_map(|(n, owner)| Op::Relink { n, owner }),
        2 => (0..POOL).prop_map(|n| Op::Delete { n }),
        1 => Just(Op::Savepoint),
        1 => any::<usize>().prop_map(|pick| Op::Rollback { pick }),
        1 => Just(Op::Publish),
    ]
}

/// Checks the index of `snapshot` against one rebuilt from its objects.
fn check(snapshot: &Snapshot) {
    let mut rebuilt: BTreeMap<ObjectId, BTreeSet<(ObjectId, ReferenceField)>> = BTreeMap::new();
    let mut total = 0;
    for (handle, object) in snapshot.iter() {
        let key = snapshot.type_key(handle).unwrap();
        for reference in references(snapshot.registry(), key.as_str(), object) {
            total += 1;
            rebuilt
                .entry(reference.target)
                .or_default()
                .insert((object.id, reference.field));
        }
    }
    assert_eq!(snapshot.reference_count(), total);
    for n in 0..POOL {
        let found: Vec<_> = snapshot
            .referrers(id(n))
            .map(|r| (r.source, r.field.clone()))
            .collect();
        let expected: Vec<_> = rebuilt
            .get(&id(n))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        assert_eq!(found, expected, "referrers of {n}");
    }
}

fn run(ops: Vec<Op>) {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    let mut savepoints = Vec::new();
    let mut retained = vec![set.current().clone()];
    for op in ops {
        let current = set.current().clone();
        let live = |n: u128| current.resolve(id(n)).ok();
        match op {
            Op::Create {
                n,
                other,
                targets,
                owner,
            } => {
                let header = Header {
                    owner: owner.map(id),
                    ..Header::default()
                };
                let object = Object::new(
                    id(n),
                    SchemaVersion::new(1, 0),
                    header,
                    values(other, &targets),
                );
                let key = if other { "test.o" } else { "test.m" };
                // Fails when the identifier is live; a deleted one is created again.
                let _ = set.create(key, object);
            }
            Op::Replace { n, targets } => {
                if let Some(handle) = live(n) {
                    let other = current.type_key(handle).unwrap().as_str() == "test.o";
                    set.replace_values(handle, values(other, &targets)).unwrap();
                }
            }
            Op::Relink { n, owner } => {
                if let Some(handle) = live(n) {
                    let mut header = current.get(handle).unwrap().header.clone();
                    header.owner = owner.map(id);
                    set.replace_header(handle, header).unwrap();
                }
            }
            Op::Delete { n } => {
                if let Some(handle) = live(n) {
                    set.delete(handle).unwrap();
                }
            }
            Op::Savepoint => savepoints.push(set.savepoint()),
            Op::Rollback { pick } => {
                if !savepoints.is_empty() {
                    let at = pick % savepoints.len();
                    set.rollback_to(&savepoints[at]).unwrap();
                    savepoints.truncate(at + 1);
                }
            }
            Op::Publish => {
                set = WorkingSet::new(set.current());
                savepoints.clear();
            }
        }
        retained.push(set.current().clone());
    }
    for snapshot in &retained {
        check(snapshot);
    }
}

proptest! {
    #[test]
    fn the_reverse_index_matches_a_rebuild_in_every_retained_snapshot(
        ops in prop::collection::vec(op(), 0..80),
    ) {
        run(ops);
    }
}

#[test]
fn delete_then_create_of_one_identifier_moves_its_references() {
    use Op::{Create, Delete, Publish, Rollback, Savepoint};
    run(vec![
        Create {
            n: 1,
            other: false,
            targets: vec![2, 3, 3, 4, 5],
            owner: Some(6),
        },
        Publish,
        Savepoint,
        Delete { n: 1 },
        Create {
            n: 1,
            other: true,
            targets: vec![7, 2],
            owner: None,
        },
        Rollback { pick: 0 },
        Delete { n: 1 },
        Create {
            n: 1,
            other: false,
            targets: vec![],
            owner: Some(1),
        },
    ]);
}
