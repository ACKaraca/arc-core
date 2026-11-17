//! Preserved and quarantined objects (CORE-DOC-039): held beside the typed stores, read-only,
//! targets of references, written back unchanged; unusable rows, dangling references and limits.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::{
    Cardinality, DiagnosticKind, FieldDecl, Finding, Header, HeldKind, LoadOptions, Loaded, OnCopy,
    OnDelete, ReferenceField, ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity,
    SnapshotCell, StoredObject, StoredType, Strength, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType, WorkingSet, apply_delete_rules, check_references, load,
};

fn reference(targets: &[&str]) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: targets
            .iter()
            .map(|key| TypeKey::new(*key).unwrap())
            .collect(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    })
}

/// Negative numbers are errors, 13 is a warning.
fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    match values.last() {
        Some(Some(Value::I64(n))) if *n < 0 => vec![Finding {
            severity: Severity::Error,
            error: Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "n"),
        }],
        Some(Some(Value::I64(13))) => vec![Finding {
            severity: Severity::Warning,
            error: Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "n"),
        }],
        _ => Vec::new(),
    }
}

fn node_schema() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("thing", reference(&["test.thing"])).optional(),
            FieldDecl::new("any", reference(&[])).optional(),
            FieldDecl::new("n", ValueType::I64),
        ],
    )
}

fn thing_schema() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    )
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let mut node = TypeDecl::new(TypeKey::new("test.node").unwrap(), node_schema());
    node.validators.push(validator);
    registry.register(node).unwrap();
    registry
        .register(TypeDecl::new(
            TypeKey::new("test.thing").unwrap(),
            thing_schema(),
        ))
        .unwrap();
    Arc::new(registry)
}

fn types() -> Vec<StoredType> {
    ["test.node", "test.thing", "other.thing"]
        .into_iter()
        .map(|key| StoredType {
            type_key: key.into(),
            major: 1,
            minor: 0,
            descriptor: String::new(),
        })
        .collect()
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn node(n: u128, thing: Option<u128>, any: Option<u128>, value: i64) -> StoredObject {
    let object = arc_doc::Object::new(
        id(n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![
            thing.map(|t| Value::Reference(id(t))),
            any.map(|t| Value::Reference(id(t))),
            Some(Value::I64(value)),
        ],
    );
    let key = TypeKey::new("test.node").unwrap();
    StoredObject::of(&key, &node_schema(), &object, &Limits::DEFAULT).unwrap()
}

fn raw(n: u128, key: &str, payload: Vec<u8>) -> StoredObject {
    StoredObject {
        id: n.to_be_bytes().to_vec(),
        type_key: key.into(),
        major: 1,
        minor: 0,
        payload,
        payload_hash: vec![7; 32],
        last_txn: vec![9; 16],
        ..StoredObject::default()
    }
}

fn load_rows(rows: Vec<StoredObject>) -> Loaded {
    load(registry(), &types(), rows, &LoadOptions::default()).unwrap()
}

fn kinds(loaded: &Loaded) -> Vec<(DiagnosticKind, &'static str)> {
    loaded
        .diagnostics
        .iter()
        .map(|diagnostic| (diagnostic.kind, diagnostic.error.code()))
        .collect()
}

/// Node 1 refers to a preserved object 10 (owned by node 1) and a quarantined thing 11.
fn held_document() -> (Vec<StoredObject>, Loaded) {
    let mut preserved = raw(10, "other.thing", vec![1, 2, 3]);
    preserved.owner = Some(1_u128.to_be_bytes().to_vec());
    preserved.name = Some("kept".into());
    let rows = vec![
        node(1, Some(11), Some(10), 0),
        preserved,
        raw(11, "test.thing", vec![0xFF, 0xFF]),
    ];
    let loaded = load_rows(rows.clone());
    (rows, loaded)
}

#[test]
fn unregistered_and_invalid_objects_are_held_and_written_back_unchanged() {
    let (rows, loaded) = held_document();
    let snapshot = &loaded.snapshot;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot.held_len(), 2);
    assert_eq!(
        kinds(&loaded),
        [
            (DiagnosticKind::Preserved, "doc.type.unknown"),
            (DiagnosticKind::Quarantined, "doc.payload.invalid"),
        ],
        "no reference to a held object dangles"
    );
    let held: Vec<_> = snapshot.held().collect();
    assert_eq!(held[0].1.kind(), HeldKind::Preserved);
    assert_eq!(held[1].1.kind(), HeldKind::Quarantined);
    assert_eq!(held[0].1.row(), &rows[1]);
    assert_eq!(held[1].1.row(), &rows[2]);
    assert_eq!(held[0].1.object().header.name.as_deref(), Some("kept"));
    assert!(held[0].1.object().values.is_empty());
    for (handle, object) in &held {
        assert_eq!(snapshot.resolve(object.object().id).unwrap(), *handle);
        assert_eq!(snapshot.get(*handle).unwrap(), object.object());
        assert_eq!(snapshot.type_key(*handle), object.type_key());
        assert!(snapshot.held_object(*handle).is_some());
    }
    assert_eq!(
        snapshot.iter().count(),
        1,
        "held objects are not in the typed stores"
    );
    // The owner of a preserved object is indexed; its fields are not decoded.
    let referrers: Vec<_> = snapshot.referrers(id(1)).cloned().collect();
    assert_eq!(referrers.len(), 1);
    assert_eq!(
        (referrers[0].source, &referrers[0].field),
        (id(10), &ReferenceField::Owner)
    );
    assert_eq!(snapshot.referrers(id(10)).count(), 1);
    assert_eq!(snapshot.referrers(id(11)).count(), 1);
}

#[test]
fn held_objects_cannot_change_and_survive_working_sets() {
    let (_, loaded) = held_document();
    let cell = SnapshotCell::new(loaded.snapshot.clone());
    let base = cell.load();
    let preserved = base.resolve(id(10)).unwrap();
    let mut set = WorkingSet::new(&base);
    let savepoint = set.savepoint();
    for error in [
        set.replace_header(preserved, Header::default())
            .unwrap_err(),
        set.replace_values(preserved, Vec::new()).unwrap_err(),
        set.delete(preserved).unwrap_err(),
    ] {
        assert_eq!(error.code(), "doc.object.not_modifiable");
        assert_eq!(error.arguments()["id"], id(10).to_string().into());
    }
    let object = arc_doc::Object::new(
        id(10),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(0))],
    );
    assert_eq!(
        set.create("test.thing", object).unwrap_err().code(),
        "doc.object.duplicate_id"
    );
    assert!(set.touched().is_empty());

    // A touched object referring to held objects passes the reference check: the quarantined
    // thing keeps its type key, so the typed field accepts it.
    let node = base.resolve(id(1)).unwrap();
    let values = base.get(node).unwrap().values.clone();
    set.replace_values(node, values).unwrap();
    check_references(&set).unwrap();
    set.rollback_to(&savepoint).unwrap();
    assert!(set.current().resolve(id(11)).is_ok());
    cell.publish(&set).unwrap();
    assert_eq!(cell.load().held_len(), 2);

    // Deleting the owner of a preserved object would cascade to it, which is refused.
    let mut set = WorkingSet::new(&cell.load());
    set.delete(node).unwrap();
    let error = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.object.not_modifiable");
}

#[test]
fn dangling_references_are_diagnostics_not_failures() {
    // 99 is missing; the typed field `thing` does not allow the preserved `other.thing`.
    let mut owned = node(2, None, None, 0);
    owned.owner = Some(98_u128.to_be_bytes().to_vec());
    let loaded = load_rows(vec![
        node(1, Some(10), Some(99), 0),
        raw(10, "other.thing", Vec::new()),
        owned,
    ]);
    assert_eq!(loaded.snapshot.len(), 2);
    assert_eq!(loaded.counts.dangling, 3);
    let dangling: Vec<_> = loaded
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::Dangling)
        .map(|diagnostic| {
            (
                diagnostic.object,
                diagnostic.error.code(),
                diagnostic.error.arguments()["field"].clone(),
            )
        })
        .collect();
    assert_eq!(
        dangling,
        [
            (Some(id(1)), "doc.reference.dangling", "thing".into()),
            (Some(id(1)), "doc.reference.dangling", "any".into()),
            (Some(id(2)), "doc.reference.dangling", "(owner)".into()),
        ]
    );
}

#[test]
fn rows_without_a_usable_identifier_are_kept_for_write_back() {
    let short = StoredObject {
        id: vec![1, 2, 3],
        ..node(1, None, None, 0)
    };
    let nil = StoredObject {
        id: vec![0; 16],
        ..node(1, None, None, 0)
    };
    let first = node(5, None, None, 1);
    let duplicate = node(5, None, None, 2);
    let loaded = load_rows(vec![short.clone(), first, nil.clone(), duplicate.clone()]);
    assert_eq!(loaded.rows.unaddressable(), [short, nil, duplicate]);
    assert_eq!(loaded.counts.unaddressable, 3);
    assert_eq!(
        loaded.snapshot.len(),
        1,
        "the first row of an identifier wins"
    );
    let handle = loaded.snapshot.resolve(id(5)).unwrap();
    assert_eq!(
        loaded.snapshot.get(handle).unwrap().values[2],
        Some(Value::I64(1))
    );
    assert_eq!(
        kinds(&loaded),
        [
            (DiagnosticKind::Unaddressable, "doc.load.header_invalid"),
            (DiagnosticKind::Unaddressable, "doc.load.header_invalid"),
            (DiagnosticKind::Unaddressable, "doc.object.duplicate_id"),
        ]
    );
    assert_eq!(loaded.diagnostics[2].object, Some(id(5)));
}

#[test]
fn malformed_columns_quarantine_the_object() {
    let good = || node(1, None, None, 0);
    let mut cases: Vec<(StoredObject, &str, Option<&str>)> = Vec::new();
    let mut add = |change: &dyn Fn(&mut StoredObject), code, column| {
        let mut row = good();
        change(&mut row);
        cases.push((row, code, column));
    };
    add(
        &|row| row.type_key = "Not A Key".into(),
        "doc.load.header_invalid",
        Some("type_key"),
    );
    add(
        &|row| row.flags = 8,
        "doc.load.header_invalid",
        Some("flags"),
    );
    add(
        &|row| row.owner = Some(vec![1]),
        "doc.load.header_invalid",
        Some("owner"),
    );
    add(
        &|row| row.major = 0,
        "doc.load.header_invalid",
        Some("schema_major"),
    );
    add(
        &|row| row.minor = -1,
        "doc.load.header_invalid",
        Some("schema_minor"),
    );
    add(
        &|row| row.name = Some("x".repeat(40)),
        "doc.load.header_invalid",
        Some("name"),
    );
    add(
        &|row| row.encoding = 7,
        "doc.payload.encoding_unsupported",
        None,
    );
    add(
        &|row| row.encoding = 1,
        "doc.payload.encoding_unsupported",
        None,
    );
    add(&|row| row.minor = 4, "doc.load.version_undeclared", None);
    add(&|row| row.payload.push(0), "doc.payload.invalid", None);
    let options = LoadOptions {
        limits: Limits {
            string_bytes: 32,
            ..Limits::DEFAULT
        },
        ..LoadOptions::default()
    };
    for (row, code, column) in cases {
        let loaded = load(registry(), &types(), [row.clone()], &options).unwrap();
        let (handle, held) = loaded.snapshot.held().next().unwrap();
        assert_eq!(held.kind(), HeldKind::Quarantined, "{code} {column:?}");
        assert_eq!(held.reason().code(), code, "{column:?}");
        if let Some(column) = column {
            assert_eq!(held.reason().arguments()["column"], column.into());
        }
        assert_eq!(held.row(), &row);
        assert_eq!(loaded.snapshot.resolve(id(1)).unwrap(), handle);
        let malformed_key = row.type_key != "test.node";
        assert_eq!(held.type_key().is_none(), malformed_key);
    }
}

#[test]
fn validator_errors_quarantine_and_warnings_are_reported() {
    let loaded = load_rows(vec![node(1, None, None, -1), node(2, None, None, 13)]);
    assert_eq!(
        kinds(&loaded),
        [
            (DiagnosticKind::Quarantined, "doc.value.out_of_range"),
            (DiagnosticKind::Warning, "doc.value.out_of_range"),
        ]
    );
    assert_eq!(loaded.diagnostics[1].object, Some(id(2)));
    assert_eq!(loaded.snapshot.len(), 1);
    assert_eq!(loaded.counts.warnings, 1);
}

#[test]
fn exceeding_a_document_limit_fails_the_load() {
    let rows = || (1..=3).map(|n| node(n, None, None, 0)).collect::<Vec<_>>();
    let with = |limits: Limits| LoadOptions {
        limits,
        ..LoadOptions::default()
    };
    let few = with(Limits {
        record_count: 2,
        ..Limits::DEFAULT
    });
    let error = load(registry(), &[], rows(), &few).unwrap_err();
    assert_eq!(error.code(), "doc.load.limit_exceeded");
    assert_eq!(error.arguments()["limit"], "record_count".into());
    let error = load(registry(), &types(), Vec::new(), &few).unwrap_err();
    assert_eq!(error.arguments()["limit"], "record_count".into());
    let small = with(Limits {
        memory_bytes: Some(1_000),
        ..Limits::DEFAULT
    });
    let error = load(registry(), &types(), rows(), &small).unwrap_err();
    assert_eq!(error.arguments()["limit"], "memory_bytes".into());
    assert!(load(registry(), &types(), rows(), &with(Limits::DEFAULT)).is_ok());
}
