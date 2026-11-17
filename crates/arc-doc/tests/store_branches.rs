//! Publication is tied to the base a working set was built on, and handles stay unique across
//! every branch of a document's history: sibling working sets, discarded working sets and
//! rolled-back savepoints.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::{
    FieldDecl, Header, Object, Schema, SchemaVersion, Snapshot, SnapshotCell, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType, WorkingSet,
};

fn registry(types: usize) -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for index in 0..types {
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("n", ValueType::I64)],
        );
        registry
            .register(TypeDecl::new(
                TypeKey::new(format!("test.t{index}")).unwrap(),
                schema,
            ))
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

#[test]
fn a_set_built_on_an_unpublished_sibling_is_not_published() {
    let cell = SnapshotCell::new(Snapshot::empty(registry(1)));
    let base = cell.load();
    let mut first = WorkingSet::new(&base);
    first.create("test.t0", object(1, 1)).unwrap();
    let mut sibling = WorkingSet::new(&base);
    sibling.create("test.t0", object(2, 2)).unwrap();
    cell.publish(&first).unwrap();
    // Revision 2, the number that follows the published revision, but built on the sibling.
    let on_sibling = WorkingSet::new(sibling.current());
    assert_eq!(on_sibling.current().revision(), 2);
    let error = cell.publish(&on_sibling).unwrap_err();
    assert_eq!(error.code(), "doc.snapshot.stale");
    let published = cell.load();
    assert!(published.resolve(ObjectId::from_u128(1)).is_ok());
    assert!(published.resolve(ObjectId::from_u128(2)).is_err());
}

#[test]
fn a_set_of_another_document_is_not_published() {
    let cell = SnapshotCell::new(Snapshot::empty(registry(1)));
    let other = WorkingSet::new(&Snapshot::empty(registry(2)));
    assert_eq!(other.current().revision(), 1);
    assert_eq!(
        cell.publish(&other).unwrap_err().code(),
        "doc.snapshot.stale"
    );
    assert_eq!(cell.load().revision(), 0);
}

#[test]
fn a_handle_from_a_discarded_set_never_reads_another_object() {
    let mut setup = WorkingSet::new(&Snapshot::empty(registry(1)));
    let gone = setup.create("test.t0", object(1, 1)).unwrap();
    setup.delete(gone).unwrap();
    setup.create("test.t0", object(9, 9)).unwrap();
    let base = setup.current().clone();
    // Two branches of one base each take a slot: a freed one, then a new one.
    let mut aborted = WorkingSet::new(&base);
    let reused = aborted.create("test.t0", object(2, 2)).unwrap();
    let appended = aborted.create("test.t0", object(3, 3)).unwrap();
    let mut retry = WorkingSet::new(&base);
    retry.create("test.t0", object(4, 4)).unwrap();
    retry.create("test.t0", object(5, 5)).unwrap();
    for handle in [gone, reused, appended] {
        assert_eq!(
            retry.current().get(handle).unwrap_err().code(),
            "doc.object.not_found"
        );
    }
}

#[test]
fn a_handle_from_a_rolled_back_savepoint_never_reads_another_object() {
    let mut set = WorkingSet::new(&Snapshot::empty(registry(1)));
    let savepoint = set.savepoint();
    let undone = set.create("test.t0", object(1, 1)).unwrap();
    set.rollback_to(&savepoint).unwrap();
    let again = set.create("test.t0", object(2, 2)).unwrap();
    assert_ne!(undone, again);
    assert_eq!(
        set.current().get(undone).unwrap_err().code(),
        "doc.object.not_found"
    );
}
