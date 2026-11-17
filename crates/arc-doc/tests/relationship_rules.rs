//! Relationship rules at commit (CORE-DOC-008, CORE-DOC-009): deletion rules, the reference
//! check, cardinality, owning references and the on-copy plan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::relationship::copy_references;
use arc_doc::{
    Cardinality, FieldDecl, FieldPath, Header, Object, OnCopy, OnDelete, ReferenceField,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Snapshot, Strength, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType, WorkingSet, apply_delete_rules, check_references,
};

fn key(text: &str) -> TypeKey {
    TypeKey::new(text).unwrap()
}

fn spec(
    targets: &[&str],
    strength: Strength,
    on_delete: OnDelete,
    on_copy: OnCopy,
    cardinality: Cardinality,
) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: targets.iter().map(|t| key(t)).collect(),
        rule: RelationshipRule {
            strength,
            on_delete,
            on_copy,
            cardinality,
        },
    })
}

fn registry() -> Arc<TypeRegistry> {
    use Cardinality::{Many, One, Optional};
    use Strength::{Associative, Informational, Owning};
    let node: &[&str] = &["test.node"];
    let list = |t| ValueType::List(Box::new(t));
    let fields = vec![
        FieldDecl::new(
            "parent",
            spec(
                node,
                Associative,
                OnDelete::Cascade,
                OnCopy::ShareTarget,
                Optional,
            ),
        )
        .optional(),
        FieldDecl::new(
            "link",
            spec(&[], Informational, OnDelete::Clear, OnCopy::Clear, Optional),
        )
        .optional(),
        FieldDecl::new(
            "pin",
            spec(
                node,
                Associative,
                OnDelete::Block,
                OnCopy::ShareTarget,
                Optional,
            ),
        )
        .optional(),
        FieldDecl::new(
            "items",
            list(spec(
                &[],
                Associative,
                OnDelete::Clear,
                OnCopy::ShareTarget,
                Many { max: 3 },
            )),
        ),
        FieldDecl::new(
            "owns",
            list(spec(
                &[],
                Owning,
                OnDelete::Clear,
                OnCopy::CopyTarget,
                Many { max: 8 },
            )),
        ),
    ];
    let req = vec![
        FieldDecl::new(
            "target",
            spec(
                &["test.leaf"],
                Associative,
                OnDelete::Block,
                OnCopy::ShareTarget,
                One,
            ),
        ),
        FieldDecl::new(
            "nested",
            ValueType::Record(vec![
                FieldDecl::new(
                    "r",
                    spec(
                        &[],
                        Associative,
                        OnDelete::Clear,
                        OnCopy::ShareTarget,
                        Optional,
                    ),
                )
                .optional(),
            ]),
        )
        .optional(),
    ];
    let leaf = vec![FieldDecl::new("n", ValueType::I64).optional()];
    let mut registry = TypeRegistry::new();
    for (name, fields) in [
        ("test.node", fields),
        ("test.req", req),
        ("test.leaf", leaf),
    ] {
        registry
            .register(TypeDecl::new(
                key(name),
                Schema::new(SchemaVersion::new(1, 0), fields),
            ))
            .unwrap();
    }
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

#[derive(Default, Clone)]
struct N {
    parent: Option<u128>,
    link: Option<u128>,
    pin: Option<u128>,
    items: Vec<u128>,
    owns: Vec<u128>,
    owner: Option<u128>,
}

fn reference(n: Option<u128>) -> Option<Value> {
    n.map(|n| Value::Reference(id(n)))
}

fn refs(list: &[u128]) -> Option<Value> {
    Some(Value::List(
        list.iter().map(|&n| Value::Reference(id(n))).collect(),
    ))
}

fn node(n: u128, spec: N) -> Object {
    let header = Header {
        owner: spec.owner.map(id),
        ..Header::default()
    };
    Object::new(
        id(n),
        SchemaVersion::new(1, 0),
        header,
        vec![
            reference(spec.parent),
            reference(spec.link),
            reference(spec.pin),
            refs(&spec.items),
            refs(&spec.owns),
        ],
    )
}

fn leaf(n: u128) -> Object {
    Object::new(
        id(n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![None],
    )
}

/// A committed snapshot of the given nodes.
fn base(nodes: Vec<(u128, N)>) -> Snapshot {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    for (n, spec) in nodes {
        set.create("test.node", node(n, spec)).unwrap();
    }
    check_references(&set).unwrap();
    set.current().clone()
}

fn delete(set: &mut WorkingSet, n: u128) {
    let handle = set.current().resolve(id(n)).unwrap();
    set.delete(handle).unwrap();
}

fn live(snapshot: &Snapshot) -> Vec<u128> {
    snapshot.iter().map(|(_, o)| o.id.as_u128()).collect()
}

fn detail(error: &arc_base::Error, name: &str) -> String {
    match error
        .details()
        .get(name)
        .or_else(|| error.arguments().get(name))
    {
        Some(arc_base::error::Value::Text(text)) => text.clone(),
        other => format!("{other:?}"),
    }
}

/// The observable state of a working set.
fn state(set: &WorkingSet) -> (Vec<Arc<Object>>, Vec<(ObjectId, arc_doc::Change)>, usize) {
    (
        set.current().iter().map(|(_, o)| Arc::clone(o)).collect(),
        set.touched().iter().map(|(k, v)| (*k, *v)).collect(),
        set.current().reference_count(),
    )
}

#[test]
fn references_to_missing_or_wrongly_typed_objects_dangle() {
    let snapshot = base(vec![(1, N::default())]);
    let cases: Vec<(&str, Object, &str, u128)> = vec![
        (
            "test.node",
            node(
                2,
                N {
                    link: Some(99),
                    ..N::default()
                },
            ),
            "link",
            99,
        ),
        (
            "test.node",
            node(
                2,
                N {
                    items: vec![1, 98],
                    ..N::default()
                },
            ),
            "items[1]",
            98,
        ),
        (
            "test.node",
            node(
                2,
                N {
                    owner: Some(97),
                    ..N::default()
                },
            ),
            "(owner)",
            97,
        ),
    ];
    for (type_key, object, field, target) in cases {
        let mut set = WorkingSet::new(&snapshot);
        set.create(type_key, object).unwrap();
        let error = check_references(&set).unwrap_err();
        assert_eq!(error.code(), "doc.reference.dangling");
        assert_eq!(detail(&error, "field"), field);
        assert_eq!(detail(&error, "source"), id(2).to_string());
        assert_eq!(detail(&error, "target"), id(target).to_string());
    }
    // `pin` allows nodes only; `link` allows any type.
    let mut set = WorkingSet::new(&snapshot);
    set.create("test.leaf", leaf(3)).unwrap();
    let pinned = N {
        pin: Some(3),
        ..N::default()
    };
    set.create("test.node", node(2, pinned)).unwrap();
    assert_eq!(
        check_references(&set).unwrap_err().code(),
        "doc.reference.dangling"
    );
    let mut set = WorkingSet::new(&snapshot);
    set.create("test.leaf", leaf(3)).unwrap();
    let linked = N {
        link: Some(3),
        owner: Some(1),
        ..N::default()
    };
    set.create("test.node", node(2, linked)).unwrap();
    check_references(&set).unwrap();
}

#[test]
fn a_delete_without_its_rules_applied_dangles() {
    let snapshot = base(vec![
        (1, N::default()),
        (
            2,
            N {
                link: Some(1),
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let before = state(&set);
    let error = check_references(&set).unwrap_err();
    assert_eq!(error.code(), "doc.reference.dangling");
    assert_eq!(detail(&error, "source"), id(2).to_string());
    assert_eq!(state(&set), before, "a failed check changes nothing");
    apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    check_references(&set).unwrap();
}

#[test]
fn a_recreated_identifier_must_keep_a_type_its_referrers_allow() {
    let snapshot = base(vec![
        (1, N::default()),
        (
            2,
            N {
                pin: Some(1),
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    set.create("test.leaf", leaf(1)).unwrap();
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert!(report.is_empty(), "a recreated object is not deleted");
    let error = check_references(&set).unwrap_err();
    assert_eq!(error.code(), "doc.reference.dangling");
    assert_eq!(detail(&error, "field"), "pin");
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    set.create("test.node", node(1, N::default())).unwrap();
    check_references(&set).unwrap();
}

#[test]
fn cascade_deletes_sources_and_header_owners_take_their_objects() {
    let snapshot = base(vec![
        (1, N::default()),
        (
            2,
            N {
                parent: Some(1),
                ..N::default()
            },
        ),
        (
            3,
            N {
                parent: Some(2),
                ..N::default()
            },
        ),
        (
            4,
            N {
                owner: Some(3),
                ..N::default()
            },
        ),
        (5, N::default()),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    let deleted: Vec<u128> = report
        .deleted
        .iter()
        .map(|d| d.before.id.as_u128())
        .collect();
    assert_eq!(deleted, [2, 3, 4]);
    assert_eq!(report.deleted[0].type_key.as_str(), "test.node");
    assert!(report.cleared.is_empty());
    assert_eq!(live(set.current()), [5]);
    check_references(&set).unwrap();
    // Applying the rules again finds nothing to do.
    assert!(
        apply_delete_rules(&mut set, &Limits::DEFAULT)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clear_empties_optional_fields_and_removes_list_elements() {
    let snapshot = base(vec![
        (1, N::default()),
        (5, N::default()),
        (
            2,
            N {
                link: Some(1),
                items: vec![1, 5, 1],
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert!(report.deleted.is_empty());
    assert_eq!(report.cleared.len(), 1);
    let cleared = &report.cleared[0];
    assert_eq!(cleared.before.id, id(2));
    assert_eq!(
        cleared.fields,
        [FieldPath::new("link"), FieldPath::new("items")]
    );
    let current = set.current();
    let after = current.get(current.resolve(id(2)).unwrap()).unwrap();
    assert_eq!(after.values[1], None);
    assert_eq!(after.values[3], refs(&[5]));
    assert_eq!(current.referrers(id(1)).count(), 0);
    let fields: Vec<_> = current.referrers(id(5)).map(|r| r.field.clone()).collect();
    assert_eq!(
        fields,
        [ReferenceField::Field(FieldPath::new("items").index(0))]
    );
    check_references(&set).unwrap();
}

#[test]
fn clear_reaches_references_in_nested_records() {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    set.create("test.leaf", leaf(10)).unwrap();
    set.create("test.leaf", leaf(11)).unwrap();
    let req = Object::new(
        id(12),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![
            Some(Value::Reference(id(10))),
            Some(Value::Record(vec![Some(Value::Reference(id(11)))])),
        ],
    );
    set.create("test.req", req).unwrap();
    check_references(&set).unwrap();
    let field = set
        .current()
        .referrers(id(11))
        .next()
        .unwrap()
        .field
        .clone();
    assert_eq!(field.to_string(), "nested.r");
    let mut set = WorkingSet::new(set.current());
    delete(&mut set, 11);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert_eq!(report.cleared[0].fields, [FieldPath::new("nested")]);
    check_references(&set).unwrap();
    // The required `target` blocks the deletion of its leaf.
    delete(&mut set, 10);
    let error = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.reference.delete_blocked");
    assert_eq!(detail(&error, "field"), "target");
}

#[test]
fn block_refuses_the_delete_and_leaves_the_working_set_unchanged() {
    let snapshot = base(vec![
        (1, N::default()),
        (
            2,
            N {
                pin: Some(1),
                ..N::default()
            },
        ),
        (
            3,
            N {
                parent: Some(1),
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let before = state(&set);
    let error = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.reference.delete_blocked");
    assert_eq!(detail(&error, "field"), "pin");
    assert_eq!(detail(&error, "source"), id(2).to_string());
    assert_eq!(detail(&error, "target"), id(1).to_string());
    assert_eq!(state(&set), before);
    // The caller's own savepoints survive.
    let savepoint = set.savepoint();
    assert!(apply_delete_rules(&mut set, &Limits::DEFAULT).is_err());
    set.rollback_to(&savepoint).unwrap();
}

#[test]
fn a_block_from_an_object_the_cascade_deletes_does_not_refuse() {
    // Deleting 1 cascades to 2, then 3; 4 pins 2 but is owned by 3, so it goes too.
    let snapshot = base(vec![
        (1, N::default()),
        (
            2,
            N {
                parent: Some(1),
                ..N::default()
            },
        ),
        (
            3,
            N {
                parent: Some(2),
                ..N::default()
            },
        ),
        (
            4,
            N {
                pin: Some(2),
                owner: Some(3),
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert_eq!(report.deleted.len(), 3);
    assert!(set.current().is_empty());
    check_references(&set).unwrap();
}

#[test]
fn owning_references_take_their_targets_and_cycles_terminate() {
    let snapshot = base(vec![
        (
            1,
            N {
                owns: vec![2, 3],
                ..N::default()
            },
        ),
        (
            2,
            N {
                owns: vec![1],
                ..N::default()
            },
        ),
        (3, N::default()),
        (
            4,
            N {
                items: vec![3],
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 1);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert_eq!(report.deleted.len(), 2);
    assert_eq!(live(set.current()), [4]);
    // 4 lost its reference to the owned 3.
    assert_eq!(report.cleared.len(), 1);
    check_references(&set).unwrap();

    // A cycle of cascading parents.
    let snapshot = base(vec![
        (
            1,
            N {
                parent: Some(3),
                ..N::default()
            },
        ),
        (
            2,
            N {
                parent: Some(1),
                ..N::default()
            },
        ),
        (
            3,
            N {
                parent: Some(2),
                ..N::default()
            },
        ),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    delete(&mut set, 2);
    assert_eq!(
        apply_delete_rules(&mut set, &Limits::DEFAULT)
            .unwrap()
            .deleted
            .len(),
        2
    );
    assert!(set.current().is_empty());
}

#[test]
fn an_owned_target_handed_to_another_owner_survives() {
    let snapshot = base(vec![
        (
            1,
            N {
                owns: vec![3],
                ..N::default()
            },
        ),
        (2, N::default()),
        (3, N::default()),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    let one = set.current().resolve(id(1)).unwrap();
    set.replace_values(one, node(1, N::default()).values)
        .unwrap();
    let two = set.current().resolve(id(2)).unwrap();
    let owner = N {
        owns: vec![3],
        ..N::default()
    };
    set.replace_values(two, node(2, owner).values).unwrap();
    delete(&mut set, 1);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert!(report.is_empty());
    assert_eq!(live(set.current()), [2, 3]);
    check_references(&set).unwrap();
}

#[test]
fn an_object_has_at_most_one_owning_reference() {
    let snapshot = base(vec![
        (
            1,
            N {
                owns: vec![3],
                ..N::default()
            },
        ),
        (3, N::default()),
    ]);
    let mut set = WorkingSet::new(&snapshot);
    let other = N {
        owns: vec![3],
        ..N::default()
    };
    set.create("test.node", node(2, other)).unwrap();
    let error = check_references(&set).unwrap_err();
    assert_eq!(error.code(), "doc.reference.owner_conflict");
    assert_eq!(detail(&error, "owner"), id(1).to_string());
    let mut set = WorkingSet::new(&snapshot);
    let handle = set.current().resolve(id(1)).unwrap();
    let twice = N {
        owns: vec![3, 3],
        ..N::default()
    };
    set.replace_values(handle, node(1, twice).values).unwrap();
    assert_eq!(
        check_references(&set).unwrap_err().code(),
        "doc.reference.owner_conflict"
    );
}

#[test]
fn cardinality_violations_are_rejected() {
    let snapshot = base(vec![(1, N::default())]);
    let mut set = WorkingSet::new(&snapshot);
    let crowded = N {
        items: vec![1, 1, 1, 1],
        ..N::default()
    };
    set.create("test.node", node(2, crowded)).unwrap();
    let error = check_references(&set).unwrap_err();
    assert_eq!(error.code(), "doc.reference.cardinality");
    assert_eq!(detail(&error, "field"), "items");
    let mut set = WorkingSet::new(&snapshot);
    let missing = Object::new(
        id(2),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![None, None],
    );
    set.create("test.req", missing).unwrap();
    let error = check_references(&set).unwrap_err();
    assert_eq!(error.code(), "doc.reference.cardinality");
    assert_eq!(detail(&error, "field"), "target");
}

/// A chain in which deleting object 0 cascades to every other one.
fn chain(length: u128) -> Snapshot {
    let mut set = WorkingSet::new(&Snapshot::empty(registry()));
    for n in 0..length {
        let parent = N {
            parent: n.checked_sub(1),
            ..N::default()
        };
        set.create("test.node", node(n, parent)).unwrap();
    }
    set.current().clone()
}

#[test]
fn a_cascade_of_100_000_objects_runs_iteratively() {
    let mut set = WorkingSet::new(&chain(100_000));
    delete(&mut set, 0);
    let report = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap();
    assert_eq!(report.deleted.len(), 99_999);
    assert!(set.current().is_empty());
    assert_eq!(set.current().reference_count(), 0);
    check_references(&set).unwrap();
}

#[test]
fn the_cascade_limit_fails_cleanly() {
    let mut set = WorkingSet::new(&chain(1_000));
    delete(&mut set, 0);
    let before = state(&set);
    let limit = |record_count| Limits {
        record_count,
        ..Limits::DEFAULT
    };
    let error = apply_delete_rules(&mut set, &limit(998)).unwrap_err();
    assert_eq!(error.code(), "doc.reference.cascade_limit");
    assert_eq!(state(&set), before, "a failed cascade changes nothing");
    let report = apply_delete_rules(&mut set, &limit(999)).unwrap();
    assert_eq!(report.deleted.len(), 999);
}

#[test]
fn copies_follow_the_on_copy_rules() {
    let snapshot = base(vec![
        (2, N::default()),
        (3, N::default()),
        (4, N::default()),
        (5, N::default()),
        (
            1,
            N {
                owns: vec![2],
                link: Some(3),
                items: vec![3, 4],
                parent: Some(5),
                owner: Some(5),
                ..N::default()
            },
        ),
    ]);
    let mapping = BTreeMap::from([(id(1), id(101))]);
    let plan = copy_references(&snapshot, &mapping).unwrap();
    assert_eq!(plan.copy_targets, [id(2)]);
    let copy = &plan.objects[0];
    assert_eq!((copy.source, copy.copy), (id(1), id(101)));
    assert_eq!(copy.header.owner, Some(id(5)));
    let expected = N {
        owns: vec![2],
        items: vec![3, 4],
        parent: Some(5),
        ..N::default()
    };
    assert_eq!(copy.values, node(1, expected).values, "link is cleared");

    let mapping = BTreeMap::from([(id(1), id(101)), (id(2), id(102)), (id(5), id(105))]);
    let plan = copy_references(&snapshot, &mapping).unwrap();
    assert!(plan.copy_targets.is_empty());
    let copy = &plan.objects[0];
    assert_eq!(copy.header.owner, Some(id(105)));
    let expected = N {
        owns: vec![102],
        items: vec![3, 4],
        parent: Some(105),
        ..N::default()
    };
    assert_eq!(copy.values, node(1, expected).values);
    assert_eq!(plan.objects.len(), 3);

    let missing = BTreeMap::from([(id(77), id(177))]);
    assert_eq!(
        copy_references(&snapshot, &missing).unwrap_err().code(),
        "doc.object.not_found"
    );
}
