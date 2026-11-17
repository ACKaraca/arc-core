//! The rules and failures of transactions (CORE-DOC-013) and savepoints (CORE-DOC-014).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{ObjectId, SessionId, TransactionId};
use arc_doc::{
    FieldDecl, FieldPath, Flags, Header, Op, Properties, Schema, SchemaVersion, Snapshot,
    SnapshotCell, Txn, TxnKind, TxnLimits, TxnMeta, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType, WorkingSet,
};

fn registry() -> Arc<TypeRegistry> {
    let record = ValueType::Record(vec![
        FieldDecl::new("a", ValueType::I64),
        FieldDecl::new("b", ValueType::String).optional(),
    ]);
    let schema = Schema::new(
        SchemaVersion::new(1, 2),
        vec![
            FieldDecl::new("n", ValueType::I64),
            FieldDecl::new("tags", ValueType::Map(Box::new(ValueType::I64))).optional(),
            FieldDecl::new("items", ValueType::List(Box::new(ValueType::I64))),
            FieldDecl::new("rec", record).optional(),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.a").unwrap(), schema))
        .unwrap();
    let other = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new("test.b").unwrap(), other))
        .unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn values(n: i64) -> Vec<Option<Value>> {
    vec![
        Some(Value::I64(n)),
        None,
        Some(Value::List(vec![Value::I64(1), Value::I64(2)])),
        None,
    ]
}

fn meta() -> TxnMeta {
    TxnMeta::new(TransactionId::from_u128(7), SessionId::from_u128(9))
}

fn open(base: &Snapshot) -> Txn {
    Txn::new(base, meta(), TxnLimits::default())
}

fn n_of(txn: &Txn, object: ObjectId) -> Option<i64> {
    match txn.get(object).ok()?.values.first()? {
        Some(Value::I64(n)) => Some(*n),
        _ => None,
    }
}

#[test]
fn reads_inside_see_the_changes_and_other_readers_see_none_until_publication() {
    let cell = SnapshotCell::new(Snapshot::empty(registry()));
    let before = cell.load();
    let mut txn = open(&before);
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    txn.create("test.a", id(2), Header::default(), values(2))
        .unwrap();
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(10)))
        .unwrap();
    txn.set_properties(
        id(1),
        Properties {
            name: Some("first".into()),
            partition: Some("level 1".into()),
            flags: Flags {
                hidden_from_ui: true,
                ..Flags::default()
            },
        },
    )
    .unwrap();
    txn.relink(id(2), Some(id(1))).unwrap();
    txn.delete(id(2)).unwrap();
    let during = cell.load();

    assert_eq!(n_of(&txn, id(1)), Some(10));
    assert_eq!(
        txn.get(id(1)).unwrap().header.name.as_deref(),
        Some("first")
    );
    assert_eq!(txn.get(id(2)).unwrap_err().code(), "doc.object.not_found");
    assert_eq!(txn.snapshot().iter().count(), 1);
    assert_eq!(
        txn.resolve(id(1)).unwrap(),
        txn.snapshot().resolve(id(1)).unwrap()
    );
    for reader in [&before, &during, &cell.load()] {
        assert!(reader.is_empty());
    }
    assert_eq!(txn.ops().len(), 6);

    let parts = txn.into_parts();
    assert_eq!(parts.meta, meta());
    cell.publish(&parts.set).unwrap();
    assert_eq!(cell.load().len(), 1);
    assert!(before.is_empty() && during.is_empty());
}

#[test]
fn operations_record_identifiers_paths_and_both_values() {
    let mut txn = open(&Snapshot::empty(registry()));
    txn.create_copy(id(9), "test.a", id(1), Header::default(), values(1))
        .unwrap();
    let path = FieldPath::new("n");
    txn.modify(id(1), path.clone(), Some(Value::I64(2)))
        .unwrap();
    txn.modify(id(1), path.clone(), Some(Value::I64(3)))
        .unwrap();
    txn.relink(id(1), Some(id(5))).unwrap();
    txn.delete(id(1)).unwrap();
    let ops = txn.ops();
    let Op::Create {
        type_key,
        object,
        copied_from,
    } = &ops[0]
    else {
        panic!("a create");
    };
    assert_eq!(type_key.as_str(), "test.a");
    assert_eq!(object.version, SchemaVersion::new(1, 2));
    assert_eq!(*copied_from, Some(id(9)));
    // Consecutive changes of one field are kept, each with its own before value.
    for (op, (before, after)) in ops[1..3].iter().zip([(1, 2), (2, 3)]) {
        assert_eq!(
            *op,
            Op::Modify {
                id: id(1),
                version: SchemaVersion::new(1, 2),
                path: path.clone(),
                before: Some(Value::I64(before)),
                after: Some(Value::I64(after)),
            }
        );
    }
    assert_eq!(
        ops[3],
        Op::Relink {
            id: id(1),
            before: None,
            after: Some(id(5))
        }
    );
    let Op::Delete { object, .. } = &ops[4] else {
        panic!("a delete");
    };
    assert_eq!(object.values[0], Some(Value::I64(3)));
    assert_eq!(object.header.owner, Some(id(5)));
    assert!(ops.iter().all(|op| op.id() == id(1)));
    // The inverse of a create drops its lineage; the inverse of an inverse is the operation.
    assert!(matches!(
        ops[0].inverse().inverse(),
        Op::Create {
            copied_from: None,
            ..
        }
    ));
    assert_eq!(ops[1].inverse().inverse(), ops[1]);
    assert!(txn.touched().is_empty(), "created and deleted is no change");
}

#[test]
fn nested_paths_set_record_fields_list_elements_and_map_entries() {
    let mut txn = open(&Snapshot::empty(registry()));
    let mut start = values(0);
    start[1] = Some(Value::Map(BTreeMap::from([("x".into(), Value::I64(1))])));
    start[3] = Some(Value::Record(vec![Some(Value::I64(5)), None]));
    txn.create("test.a", id(1), Header::default(), start)
        .unwrap();
    let cases = [
        (FieldPath::new("tags").key("y"), Some(Value::I64(2)), None),
        (FieldPath::new("tags").key("x"), None, Some(Value::I64(1))),
        (
            FieldPath::new("items").index(1),
            Some(Value::I64(7)),
            Some(Value::I64(2)),
        ),
        (
            FieldPath::new("rec").field("a"),
            Some(Value::I64(6)),
            Some(Value::I64(5)),
        ),
        (
            FieldPath::new("rec").field("b"),
            Some(Value::String("s".into())),
            None,
        ),
    ];
    for (path, after, before) in cases {
        txn.modify(id(1), path.clone(), after.clone()).unwrap();
        let Some(Op::Modify {
            before: recorded,
            after: written,
            ..
        }) = txn.ops().last()
        else {
            panic!("a modify");
        };
        assert_eq!((recorded, written), (&before, &after), "{path}");
    }
    let values = &txn.get(id(1)).unwrap().values;
    assert_eq!(
        values[1],
        Some(Value::Map(BTreeMap::from([("y".into(), Value::I64(2))])))
    );
    assert_eq!(
        values[2],
        Some(Value::List(vec![Value::I64(1), Value::I64(7)]))
    );
    assert_eq!(
        values[3],
        Some(Value::Record(vec![
            Some(Value::I64(6)),
            Some(Value::String("s".into()))
        ]))
    );
}

#[test]
fn a_failed_create_changes_nothing() {
    let mut txn = open(&Snapshot::empty(registry()));
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    let failures = [
        (
            txn.create("test.zzz", id(2), Header::default(), values(1)),
            "doc.type.unknown",
        ),
        (
            txn.create(
                "test.b",
                id(1),
                Header::default(),
                vec![Some(Value::I64(0))],
            ),
            "doc.object.duplicate_id",
        ),
        (
            txn.create(
                "test.a",
                id(2),
                Header::default(),
                vec![Some(Value::I64(0))],
            ),
            "doc.value.type_mismatch",
        ),
    ];
    for (result, code) in failures {
        assert_eq!(result.unwrap_err().code(), code);
    }
    assert_eq!(txn.ops().len(), 1);
    assert_eq!(txn.snapshot().len(), 1);
}

#[test]
fn a_failed_operation_changes_nothing() {
    let mut txn = open(&Snapshot::empty(registry()));
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    let state = txn.snapshot().clone();
    let modifications = [
        (
            id(2),
            FieldPath::new("n"),
            Some(Value::I64(1)),
            "doc.object.not_found",
        ),
        (id(1), FieldPath::new("missing"), None, "doc.path.invalid"),
        (
            id(1),
            FieldPath::new("n").index(0),
            None,
            "doc.path.invalid",
        ),
        (
            id(1),
            FieldPath::new("items").index(2),
            Some(Value::I64(1)),
            "doc.path.invalid",
        ),
        (
            id(1),
            FieldPath::new("items").index(0),
            None,
            "doc.path.invalid",
        ),
        (
            id(1),
            FieldPath::new("tags").key("a"),
            Some(Value::I64(1)),
            "doc.path.invalid",
        ),
        (
            id(1),
            FieldPath::new("rec").field("a"),
            Some(Value::I64(1)),
            "doc.path.invalid",
        ),
        (
            id(1),
            FieldPath::new("n"),
            Some(Value::Bool(true)),
            "doc.value.type_mismatch",
        ),
    ];
    for (object, path, value, code) in modifications {
        let error = txn.modify(object, path.clone(), value).unwrap_err();
        assert_eq!(error.code(), code, "{path}");
    }
    assert_eq!(
        txn.delete(id(2)).unwrap_err().code(),
        "doc.object.not_found"
    );
    assert_eq!(
        txn.relink(id(2), None).unwrap_err().code(),
        "doc.object.not_found"
    );
    assert_eq!(
        txn.set_properties(id(2), Properties::default())
            .unwrap_err()
            .code(),
        "doc.object.not_found"
    );
    assert_eq!(txn.ops().len(), 1);
    assert_eq!(
        txn.get(id(1)).unwrap(),
        state.get(state.resolve(id(1)).unwrap()).unwrap()
    );
    assert_eq!(txn.snapshot().len(), 1);
    // A required field may be cleared inside the transaction; commit validation rejects it.
    txn.modify(id(1), FieldPath::new("n"), None).unwrap();
}

#[test]
fn savepoints_roll_back_in_order_and_keep_the_transaction_open() {
    let mut txn = open(&Snapshot::empty(registry()));
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    let outer = txn.savepoint();
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(2)))
        .unwrap();
    let inner = txn.savepoint();
    txn.create(
        "test.b",
        id(2),
        Header::default(),
        vec![Some(Value::I64(0))],
    )
    .unwrap();
    txn.delete(id(1)).unwrap();
    assert_eq!(txn.ops().len(), 4);

    txn.rollback_to(&inner).unwrap();
    assert_eq!(txn.ops().len(), 2);
    assert_eq!(n_of(&txn, id(1)), Some(2));
    assert!(txn.get(id(2)).is_err());
    txn.rollback_to(&outer).unwrap();
    assert_eq!(txn.ops().len(), 1);
    assert_eq!(n_of(&txn, id(1)), Some(1));
    // The later savepoint is invalid now, the earlier one stays valid, and the transaction
    // takes more operations.
    assert_eq!(
        txn.rollback_to(&inner).unwrap_err().code(),
        "doc.savepoint.invalid"
    );
    assert_eq!(
        txn.release(&inner).unwrap_err().code(),
        "doc.savepoint.invalid"
    );
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(3)))
        .unwrap();
    txn.rollback_to(&outer).unwrap();
    assert_eq!(txn.ops().len(), 1);

    let foreign = open(&Snapshot::empty(registry())).savepoint();
    assert_eq!(
        txn.rollback_to(&foreign).unwrap_err().code(),
        "doc.savepoint.invalid"
    );
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(4)))
        .unwrap();
    txn.release(&outer).unwrap();
    assert_eq!(
        txn.rollback_to(&outer).unwrap_err().code(),
        "doc.savepoint.invalid"
    );
    assert_eq!(
        n_of(&txn, id(1)),
        Some(4),
        "a failed rollback changes nothing"
    );
    assert_eq!(txn.ops().len(), 2);
}

#[test]
fn limits_reject_operations_with_a_stable_code_and_leave_the_transaction_usable() {
    let base = Snapshot::empty(registry());
    let limits = TxnLimits {
        operations: 2,
        memory_bytes: None,
    };
    let mut txn = Txn::new(&base, meta(), limits);
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    let savepoint = txn.savepoint();
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(2)))
        .unwrap();
    let error = txn.delete(id(1)).unwrap_err();
    assert_eq!(error.code(), "doc.txn.limit_exceeded");
    assert_eq!(txn.ops().len(), 2);
    assert_eq!(n_of(&txn, id(1)), Some(2));
    txn.rollback_to(&savepoint).unwrap();
    txn.delete(id(1)).unwrap();

    let limits = TxnLimits {
        operations: u64::MAX,
        memory_bytes: Some(4_096),
    };
    let mut txn = Txn::new(&base, meta(), limits);
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    let used = txn.memory_bytes();
    assert!(used > 0);
    let large = Value::List(vec![Value::I64(0); 1_000]);
    let error = txn
        .modify(id(1), FieldPath::new("items"), Some(large))
        .unwrap_err();
    assert_eq!(error.code(), "doc.txn.limit_exceeded");
    assert_eq!(txn.memory_bytes(), used);
    assert_eq!(txn.ops().len(), 1);
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(5)))
        .unwrap();
    assert_eq!(TxnLimits::default().operations, 10_000_000);
}

#[test]
fn operations_apply_only_to_their_before_state() {
    let mut txn = open(&Snapshot::empty(registry()));
    txn.create("test.a", id(1), Header::default(), values(1))
        .unwrap();
    txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(2)))
        .unwrap();
    txn.relink(id(1), Some(id(3))).unwrap();
    txn.set_properties(
        id(1),
        Properties {
            name: Some("x".into()),
            ..Properties::default()
        },
    )
    .unwrap();
    let ops = txn.ops().to_vec();
    let candidate = txn.snapshot().clone();
    let base = txn.base().clone();

    // Each operation after the create refuses the base, which does not hold its before state.
    let mut set = WorkingSet::new(&base);
    for op in &ops[1..] {
        assert_eq!(
            op.apply(&mut set).unwrap_err().code(),
            "doc.object.not_found"
        );
    }
    let mut set = WorkingSet::new(&candidate);
    for op in &ops {
        let code = op.apply(&mut set).unwrap_err().code();
        let expected = if matches!(op, Op::Create { .. }) {
            "doc.object.duplicate_id"
        } else {
            "doc.op.mismatch"
        };
        assert_eq!(code, expected);
    }
    assert!(set.touched().is_empty());
    // A delete of a different object state is refused too.
    let delete = Op::Create {
        type_key: TypeKey::new("test.a").unwrap(),
        object: Arc::new(arc_doc::Object::new(
            id(1),
            SchemaVersion::new(1, 2),
            Header::default(),
            values(1),
        )),
        copied_from: None,
    }
    .inverse();
    assert_eq!(
        delete.apply(&mut set).unwrap_err().code(),
        "doc.op.mismatch"
    );
    // The inverses in reverse order restore the base.
    for op in ops.iter().rev().map(Op::inverse) {
        op.apply(&mut set).unwrap();
    }
    assert!(set.current().is_empty());
}

#[test]
fn metadata_carries_opaque_payloads_to_commit() {
    let mut txn = open(&Snapshot::empty(registry()));
    let meta = txn.meta_mut();
    meta.parent = Some(TransactionId::from_u128(6));
    meta.kind = TxnKind::UndoOf(TransactionId::from_u128(5));
    meta.command = r#"{"id":"test.command","parameters":{}}"#.into();
    meta.selection_after = Some("[]".into());
    let expected = txn.meta().clone();
    txn.create(
        "test.b",
        id(1),
        Header::default(),
        vec![Some(Value::I64(1))],
    )
    .unwrap();
    let parts = txn.into_parts();
    assert_eq!(parts.meta, expected);
    assert_eq!(parts.ops.len(), 1);
    assert_eq!(parts.set.touched().len(), 1);
    open(&Snapshot::empty(registry())).abandon();
}
