//! Chained schema migration on load (CORE-DOC-037): stored fixtures of versions 1.0 and 2.1 of a
//! type at version 3.0 load as the expected 3.0 values in memory, the side table keeps the
//! original rows, and objects that cannot be migrated are quarantined.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::{
    Bound, DiagnosticKind, FieldDecl, Header, HeldKind, LoadOptions, Migration, Object, Schema,
    SchemaVersion, StoredObject, StoredType, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
    load, payload,
};

const FIXTURE_V1: &str = include_str!("fixtures/load_wall_v1_0.hex");
const FIXTURE_V2: &str = include_str!("fixtures/load_wall_v2_1.hex");

fn v1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("length_mm", ValueType::F64),
            FieldDecl::new("label", ValueType::String),
        ],
    )
}

fn v2() -> Schema {
    Schema::new(
        SchemaVersion::new(2, 1),
        vec![
            FieldDecl::new("length", ValueType::F64),
            FieldDecl::new("label", ValueType::String),
            FieldDecl::new("layer", ValueType::I64),
            FieldDecl::new("note", ValueType::String)
                .optional()
                .since_minor(1),
        ],
    )
}

fn v3() -> Schema {
    Schema::new(
        SchemaVersion::new(3, 0),
        vec![
            FieldDecl::new("length", ValueType::F64),
            FieldDecl::new("name", ValueType::String),
            FieldDecl::new("layer", ValueType::I64).with_bound(Bound::Int { min: 0, max: 100 }),
            FieldDecl::new("note", ValueType::String).optional(),
            FieldDecl::new("double_length", ValueType::F64),
        ],
    )
}

/// 1 → 2: millimeters to meters, layer 0. A label "fail" makes the migration fail.
fn one_to_two(values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    let [Some(Value::F64(mm)), label] = <[_; 2]>::try_from(values).unwrap() else {
        return Err(Error::new(codes::VALUE_TYPE_MISMATCH));
    };
    if label == Some(Value::String("fail".into())) {
        return Err(Error::new(codes::VALUE_OUT_OF_RANGE));
    }
    Ok(vec![
        Some(Value::F64(mm / 1000.0)),
        label,
        Some(Value::I64(0)),
        None,
    ])
}

/// 2 → 3: adds the doubled length. A label "invalid" gives a layer outside its bound.
fn two_to_three(values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    let [Some(Value::F64(length)), label, layer, note] = <[_; 4]>::try_from(values).unwrap() else {
        return Err(Error::new(codes::VALUE_TYPE_MISMATCH));
    };
    let layer = if label == Some(Value::String("invalid".into())) {
        Some(Value::I64(-1))
    } else {
        layer
    };
    Ok(vec![
        Some(Value::F64(length)),
        label,
        layer,
        note,
        Some(Value::F64(length * 2.0)),
    ])
}

fn registry() -> Arc<TypeRegistry> {
    let mut decl = TypeDecl::new(TypeKey::new("test.wall").unwrap(), v3());
    decl.migrations = vec![
        Migration {
            source: v1(),
            migrate: one_to_two,
        },
        Migration {
            source: v2(),
            migrate: two_to_three,
        },
    ];
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    Arc::new(registry)
}

fn types() -> Vec<StoredType> {
    [(1, 0), (2, 1), (2, 3)]
        .into_iter()
        .map(|(major, minor)| StoredType {
            type_key: "test.wall".into(),
            major,
            minor,
            descriptor: String::new(),
        })
        .collect()
}

fn hex(text: &str) -> Vec<u8> {
    let text = text.trim();
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn row(id: u128, major: i64, minor: i64, payload: Vec<u8>) -> StoredObject {
    StoredObject {
        id: id.to_be_bytes().to_vec(),
        type_key: "test.wall".into(),
        major,
        minor,
        name: Some(format!("wall {id}")),
        payload,
        last_txn: vec![7; 16],
        ..StoredObject::default()
    }
}

fn v1_payload(mm: f64, label: &str) -> Vec<u8> {
    let values = [Some(Value::F64(mm)), Some(Value::String(label.into()))];
    payload::encode(&v1(), &values, &Limits::DEFAULT)
        .unwrap()
        .bytes
}

fn expected(length: f64, name: &str, layer: i64, note: Option<&str>) -> Vec<Option<Value>> {
    vec![
        Some(Value::F64(length)),
        Some(Value::String(name.into())),
        Some(Value::I64(layer)),
        note.map(|note| Value::String(note.into())),
        Some(Value::F64(length * 2.0)),
    ]
}

fn values_of(snapshot: &arc_doc::Snapshot, id: u128) -> Vec<Option<Value>> {
    let handle = snapshot.resolve(ObjectId::from_u128(id)).unwrap();
    snapshot.get(handle).unwrap().values.clone()
}

#[test]
fn the_fixtures_are_the_encodings_of_the_source_schemas() {
    assert_eq!(hex(FIXTURE_V1), v1_payload(2500.0, "wall"));
    let values = [
        Some(Value::F64(2.5)),
        Some(Value::String("wall".into())),
        Some(Value::I64(3)),
        Some(Value::String("n".into())),
    ];
    let encoded = payload::encode(&v2(), &values, &Limits::DEFAULT).unwrap();
    assert_eq!(hex(FIXTURE_V2), encoded.bytes);
}

#[test]
fn a_version_1_fixture_migrates_through_both_steps() {
    let stored = row(1, 1, 0, hex(FIXTURE_V1));
    let loaded = load(
        registry(),
        &types(),
        [stored.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    let snapshot = &loaded.snapshot;
    assert_eq!(values_of(snapshot, 1), expected(2.5, "wall", 0, None));
    let object = snapshot
        .get(snapshot.resolve(ObjectId::from_u128(1)).unwrap())
        .unwrap();
    assert_eq!(object.version, SchemaVersion::new(3, 0));
    assert_eq!(object.header.name.as_deref(), Some("wall 1"));
    assert_eq!(loaded.counts.migrated, 1);
    assert!(loaded.diagnostics.is_empty());
    assert!(
        snapshot.read_only().is_empty(),
        "migration does not make a document read-only"
    );
    // In memory only: the row is not changed, and a save re-encodes the object at 3.0, so a
    // writable document keeps no original row; the stored `last_txn` is kept.
    assert!(loaded.rows.get(ObjectId::from_u128(1)).is_none());
    assert_eq!(
        loaded.rows.last_txn(ObjectId::from_u128(1)),
        Some(stored.last_txn.as_slice())
    );
}

#[test]
fn a_version_2_fixture_migrates_through_the_last_step() {
    let stored = row(2, 2, 1, hex(FIXTURE_V2));
    let loaded = load(
        registry(),
        &types(),
        [stored.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        values_of(&loaded.snapshot, 2),
        expected(2.5, "wall", 3, Some("n"))
    );
    assert!(loaded.rows.is_empty());
}

#[test]
fn an_older_minor_of_an_older_major_takes_the_defaults_before_migrating() {
    // Version 2.0 holds no note: it decodes as absent, then migrates.
    let values = [
        Some(Value::F64(1.0)),
        Some(Value::String("w".into())),
        Some(Value::I64(7)),
    ];
    let fields = v2().fields_at(0).to_vec();
    let bytes = payload::encode(
        &Schema::new(SchemaVersion::new(2, 0), fields),
        &values,
        &Limits::DEFAULT,
    )
    .unwrap()
    .bytes;
    let mut types = types();
    types.push(StoredType {
        type_key: "test.wall".into(),
        major: 2,
        minor: 0,
        descriptor: String::new(),
    });
    let loaded = load(
        registry(),
        &types,
        [row(3, 2, 0, bytes)],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(values_of(&loaded.snapshot, 3), expected(1.0, "w", 7, None));
}

#[test]
fn a_read_only_document_keeps_the_original_rows_of_migrated_objects() {
    // A declared newer minor version makes the document read-only; a copy of it writes the
    // stored rows, so the side table keeps them.
    let mut types = types();
    types.push(StoredType {
        type_key: "test.wall".into(),
        major: 3,
        minor: 1,
        descriptor: String::new(),
    });
    let stored = [row(1, 1, 0, hex(FIXTURE_V1)), row(2, 2, 1, hex(FIXTURE_V2))];
    let loaded = load(registry(), &types, stored.clone(), &LoadOptions::default()).unwrap();
    assert_eq!(loaded.snapshot.read_only().len(), 1);
    assert_eq!(loaded.rows.len(), 2);
    for (n, stored) in (1..).zip(&stored) {
        let original = loaded.rows.get(ObjectId::from_u128(n)).unwrap();
        assert_eq!(&original.stored, stored);
        assert!(original.trailing.is_empty());
    }
    assert_eq!(
        loaded.rows.get(ObjectId::from_u128(1)).unwrap().written,
        SchemaVersion::new(1, 0)
    );
    assert_eq!(loaded.types, types, "the type rows are kept as read");
}

fn quarantined(stored: StoredObject) -> Error {
    let loaded = load(
        registry(),
        &types(),
        [stored.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(loaded.snapshot.len(), 0);
    assert_eq!(loaded.counts.quarantined, 1);
    let (_, held) = loaded.snapshot.held().next().unwrap();
    assert_eq!(held.kind(), HeldKind::Quarantined);
    assert_eq!(held.row(), &stored, "written back unchanged");
    let diagnostic = loaded.diagnostics.into_iter().next().unwrap();
    assert_eq!(diagnostic.kind, DiagnosticKind::Quarantined);
    assert_eq!(diagnostic.object, Some(held.object().id));
    assert_eq!(diagnostic.error.code(), held.reason().code());
    diagnostic.error
}

#[test]
fn a_failing_migration_quarantines_the_object() {
    let error = quarantined(row(4, 1, 0, v1_payload(1.0, "fail")));
    assert_eq!(error.code(), "doc.migration.failed");
    assert_eq!(error.details()["reason"], "migration failed".into());
    assert_eq!(error.details()["step"], 1_i64.into());
}

#[test]
fn a_migration_result_that_fails_validation_quarantines_the_object() {
    let error = quarantined(row(5, 1, 0, v1_payload(1.0, "invalid")));
    assert_eq!(error.code(), "doc.migration.failed");
    assert_eq!(error.details()["reason"], "invalid result".into());
    assert_eq!(error.details()["step"], 2_i64.into());
}

#[test]
fn trailing_bytes_of_a_newer_minor_of_an_old_major_are_not_migrated() {
    // Version 2.3 is newer than the migration source 2.1: two more bytes follow.
    let mut bytes = hex(FIXTURE_V2);
    bytes.extend([0x02, 0x00]);
    let error = quarantined(row(6, 2, 3, bytes));
    assert_eq!(error.code(), "doc.migration.failed");
    assert_eq!(error.details()["reason"], "trailing bytes".into());
}

#[test]
fn a_damaged_old_payload_is_quarantined_with_the_decoding_code() {
    let error = quarantined(row(7, 1, 0, vec![0xFF]));
    assert_eq!(error.code(), "doc.payload.invalid");
}

#[test]
fn loading_builds_the_objects_a_working_set_would() {
    let loaded = load(
        registry(),
        &types(),
        [row(1, 1, 0, hex(FIXTURE_V1))],
        &LoadOptions::default(),
    )
    .unwrap();
    let object = loaded.snapshot.iter().next().unwrap().1;
    let header = Header {
        name: Some("wall 1".into()),
        ..Header::default()
    };
    let expected = Object::new(
        ObjectId::from_u128(1),
        SchemaVersion::new(3, 0),
        header,
        expected(2.5, "wall", 0, None),
    );
    assert_eq!(**object, expected);
}
