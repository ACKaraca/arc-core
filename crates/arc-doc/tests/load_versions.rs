//! The version policy of loading (CORE-DOC-038): older and newer minor versions, refusal of a
//! newer major version, and the read-only view of it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::cell::Cell;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::{
    FieldDecl, Header, HeldKind, LoadOptions, NewerMajor, ReadOnlyReason, Schema, SchemaVersion,
    StoredObject, StoredType, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet, load,
    payload,
};

/// `test.item` at version `1.minor`: field `a`, then `b` from 1.1 and `c` from 1.2.
fn schema(minor: u32) -> Schema {
    let fields = vec![
        FieldDecl::new("a", ValueType::I64),
        FieldDecl::new("b", ValueType::String)
            .with_default(Value::String("default".into()))
            .since_minor(1),
        FieldDecl::new("c", ValueType::I64)
            .optional()
            .since_minor(2),
    ];
    let fields = fields
        .into_iter()
        .filter(|field| field.since_minor <= minor)
        .collect();
    Schema::new(SchemaVersion::new(1, minor), fields)
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for key in ["test.item", "test.other"] {
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema(1)))
            .unwrap();
    }
    Arc::new(registry)
}

fn declared(key: &str, major: i64, minor: i64) -> StoredType {
    StoredType {
        type_key: key.into(),
        major,
        minor,
        descriptor: String::new(),
    }
}

fn row(key: &str, id: u128, minor: u32, values: &[Option<Value>]) -> StoredObject {
    let object = arc_doc::Object::new(
        ObjectId::from_u128(id),
        SchemaVersion::new(1, minor),
        Header::default(),
        values.to_vec(),
    );
    StoredObject::of(
        &TypeKey::new(key).unwrap(),
        &schema(minor),
        &object,
        &Limits::DEFAULT,
    )
    .unwrap()
}

fn values(snapshot: &arc_doc::Snapshot, id: u128) -> Vec<Option<Value>> {
    let handle = snapshot.resolve(ObjectId::from_u128(id)).unwrap();
    snapshot.get(handle).unwrap().values.clone()
}

#[test]
fn an_older_minor_version_takes_the_defaults_of_appended_fields() {
    let stored = row("test.item", 1, 0, &[Some(Value::I64(5))]);
    let loaded = load(
        registry(),
        &[declared("test.item", 1, 0)],
        [stored.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(
        values(&loaded.snapshot, 1),
        [Some(Value::I64(5)), Some(Value::String("default".into()))]
    );
    assert_eq!(loaded.counts.older_minor, 1);
    assert!(loaded.snapshot.read_only().is_empty());
    assert_eq!(
        loaded.rows.get(ObjectId::from_u128(1)).unwrap().stored,
        stored
    );
}

#[test]
fn the_current_version_needs_no_original_row() {
    let stored = row(
        "test.item",
        1,
        1,
        &[Some(Value::I64(5)), Some(Value::String("x".into()))],
    );
    let loaded = load(
        registry(),
        &[declared("test.item", 1, 1)],
        [stored],
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(loaded.rows.is_empty());
    assert_eq!(loaded.counts.objects, 1);
}

#[test]
fn a_newer_minor_version_keeps_its_trailing_bytes_and_opens_read_only() {
    let stored = row(
        "test.item",
        1,
        2,
        &[
            Some(Value::I64(5)),
            Some(Value::String("x".into())),
            Some(Value::I64(9)),
        ],
    );
    let types = [declared("test.item", 1, 2), declared("test.other", 1, 1)];
    let loaded = load(
        registry(),
        &types,
        [stored.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    let snapshot = &loaded.snapshot;
    assert_eq!(
        values(snapshot, 1),
        [Some(Value::I64(5)), Some(Value::String("x".into()))]
    );
    assert_eq!(loaded.counts.newer_minor, 1);
    let original = loaded.rows.get(ObjectId::from_u128(1)).unwrap();
    assert_eq!(original.stored, stored);
    // Re-encoding the known fields and appending the trailing bytes gives the stored bytes.
    let mut bytes = payload::encode(&schema(1), &values(snapshot, 1), &Limits::DEFAULT)
        .unwrap()
        .bytes;
    bytes.extend_from_slice(&original.trailing);
    assert_eq!(bytes, stored.payload);
    assert_eq!(original.trailing, [1, 18], "an optional i64 9");

    let reasons = snapshot.read_only();
    assert_eq!(
        reasons,
        [ReadOnlyReason::NewerMinor {
            type_key: TypeKey::new("test.item").unwrap(),
            written: SchemaVersion::new(1, 2),
            current: SchemaVersion::new(1, 1),
        }]
    );
    let error = reasons[0].error();
    assert_eq!(error.code(), "doc.document.read_only");
    assert_eq!(error.arguments()["type"], "test.item".into());
    assert_eq!(error.arguments()["version"], "1.2".into());

    // Every write of a working set on the read-only document fails and changes nothing.
    let mut set = WorkingSet::new(snapshot);
    let handle = snapshot.resolve(ObjectId::from_u128(1)).unwrap();
    let object = arc_doc::Object::new(
        ObjectId::from_u128(2),
        SchemaVersion::new(1, 1),
        Header::default(),
        vec![Some(Value::I64(1)), None],
    );
    for error in [
        set.create("test.other", object).unwrap_err(),
        set.replace_header(handle, Header::default()).unwrap_err(),
        set.replace_values(handle, Vec::new()).unwrap_err(),
        set.delete(handle).unwrap_err(),
    ] {
        assert_eq!(error.code(), "doc.document.read_only");
    }
    assert!(set.touched().is_empty());
    assert_eq!(
        set.current().read_only(),
        reasons,
        "derived snapshots stay read-only"
    );
}

/// The rows of a document with a newer major version of `test.item`, counting the rows read.
fn newer_major_rows(read: &Cell<usize>) -> impl Iterator<Item = StoredObject> + '_ {
    let mut newer = row(
        "test.item",
        1,
        1,
        &[Some(Value::I64(1)), Some(Value::String("x".into()))],
    );
    newer.major = 2;
    newer.minor = 0;
    let known = row(
        "test.other",
        2,
        1,
        &[Some(Value::I64(2)), Some(Value::String("y".into()))],
    );
    [newer, known]
        .into_iter()
        .inspect(move |_| read.set(read.get() + 1))
}

#[test]
fn a_newer_major_version_is_refused_before_any_row_is_read() {
    let types = [declared("test.item", 2, 0), declared("test.other", 1, 1)];
    let read = Cell::new(0);
    let error = load(
        registry(),
        &types,
        newer_major_rows(&read),
        &LoadOptions::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "doc.load.newer_major");
    assert_eq!(error.arguments()["type"], "test.item".into());
    assert_eq!(error.arguments()["version"], "2.0".into());
    assert_eq!(error.arguments()["current"], "1.1".into());
    assert_eq!(read.get(), 0, "the input is not touched");
}

#[test]
fn a_major_version_beyond_32_bits_is_refused_too() {
    let types = [declared("test.item", i64::MAX, 0)];
    let error = load(registry(), &types, [], &LoadOptions::default()).unwrap_err();
    assert_eq!(error.code(), "doc.load.newer_major");
}

#[test]
fn the_read_only_view_preserves_newer_major_objects() {
    let types = [declared("test.item", 2, 0), declared("test.other", 1, 1)];
    let read = Cell::new(0);
    let options = LoadOptions {
        newer_major: NewerMajor::ReadOnlyView,
        ..LoadOptions::default()
    };
    let rows: Vec<StoredObject> = newer_major_rows(&read).collect();
    let loaded = load(registry(), &types, rows.clone(), &options).unwrap();
    let snapshot = &loaded.snapshot;
    assert_eq!(snapshot.len(), 1, "the object of a known version is shown");
    assert_eq!(
        values(snapshot, 2),
        [Some(Value::I64(2)), Some(Value::String("y".into()))]
    );
    let (handle, held) = snapshot.held().next().unwrap();
    assert_eq!(held.kind(), HeldKind::Preserved);
    assert_eq!(held.reason().code(), "doc.load.newer_major");
    assert_eq!(held.row(), &rows[0]);
    assert_eq!(snapshot.resolve(ObjectId::from_u128(1)).unwrap(), handle);
    assert_eq!(snapshot.type_key(handle).unwrap().as_str(), "test.item");
    assert!(matches!(
        snapshot.read_only(),
        [ReadOnlyReason::NewerMajor { written, .. }] if *written == SchemaVersion::new(2, 0)
    ));
}
