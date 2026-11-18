//! References of preserved objects (CORE-DOC-039): a preserved object of an unregistered type
//! has its field references decoded with the descriptor its document stores, so they enter the
//! reverse index, are checked for dangling targets at load, and the deletion rules of that
//! descriptor apply to their targets without the held payload ever changing.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::graph::Evaluators;
use arc_doc::{
    Cardinality, DiagnosticKind, Document, FieldDecl, FieldPath, Header, HeldKind, LoadOptions,
    Object, OnCopy, OnDelete, ReferenceField, ReferenceSpec, RelationshipRule, Schema,
    SchemaVersion, StoredObject, StoredType, Strength, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType, descriptor, load,
};

const TARGET: &str = "h.target";
const HOLDER: &str = "h.holder";

fn reference(on_delete: OnDelete) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new(TARGET).unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    })
}

fn target_schema() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    )
}

fn holder_schema() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("block", reference(OnDelete::Block)).optional(),
            FieldDecl::new("cascade", reference(OnDelete::Cascade)).optional(),
            FieldDecl::new("clear", reference(OnDelete::Clear)).optional(),
        ],
    )
}

fn registry(with_holder: bool) -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(
            TypeKey::new(TARGET).unwrap(),
            target_schema(),
        ))
        .unwrap();
    if with_holder {
        registry
            .register(TypeDecl::new(
                TypeKey::new(HOLDER).unwrap(),
                holder_schema(),
            ))
            .unwrap();
    }
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

/// The stored type rows, with the descriptors the full registry generates.
fn types() -> Vec<StoredType> {
    let full = registry(true);
    [TARGET, HOLDER]
        .into_iter()
        .map(|key| StoredType {
            type_key: key.into(),
            major: 1,
            minor: 0,
            descriptor: descriptor(full.get(key).unwrap(), SchemaVersion::new(1, 0)).unwrap(),
        })
        .collect()
}

fn row(key: &str, schema: &Schema, n: u128, values: Vec<Option<Value>>) -> StoredObject {
    let object = Object::new(id(n), SchemaVersion::new(1, 0), Header::default(), values);
    let mut row = StoredObject::of(
        &TypeKey::new(key).unwrap(),
        schema,
        &object,
        &Limits::DEFAULT,
    )
    .unwrap();
    row.last_txn = vec![1; 16];
    row
}

fn rows(holder_refs: [Option<u128>; 3]) -> Vec<StoredObject> {
    let mut rows: Vec<StoredObject> = (1..=3)
        .map(|n| row(TARGET, &target_schema(), n, vec![Some(Value::I64(0))]))
        .collect();
    let values = holder_refs
        .iter()
        .map(|r| r.map(|t| Value::Reference(id(t))))
        .collect();
    rows.push(row(HOLDER, &holder_schema(), 10, values));
    rows
}

fn document(holder_refs: [Option<u128>; 3]) -> Document {
    let loaded = load(
        registry(false),
        &types(),
        rows(holder_refs),
        &LoadOptions::default(),
    )
    .unwrap();
    Document::from_loaded(loaded, Arc::new(Evaluators::new())).unwrap()
}

fn delete(document: &Document, n: u128) -> arc_base::Result<()> {
    document
        .submit(move |txn| txn.delete(id(n)))
        .wait()
        .map(drop)
}

#[test]
fn decoded_references_of_a_preserved_object_enter_the_reverse_index() {
    let loaded = load(
        registry(false),
        &types(),
        rows([Some(1), Some(2), Some(3)]),
        &LoadOptions::default(),
    )
    .unwrap();
    let snapshot = &loaded.snapshot;
    let (_, held) = snapshot.held().next().unwrap();
    assert_eq!(held.kind(), HeldKind::Preserved);
    let fields: Vec<String> = held
        .references()
        .iter()
        .map(|r| r.field.to_string())
        .collect();
    assert_eq!(fields, ["block", "cascade", "clear"]);
    for (target, field) in [(1, "block"), (2, "cascade"), (3, "clear")] {
        let referrers: Vec<_> = snapshot.referrers(id(target)).collect();
        assert_eq!(referrers.len(), 1);
        assert_eq!(referrers[0].source, id(10));
        assert_eq!(
            referrers[0].field,
            ReferenceField::Field(FieldPath::new(field))
        );
    }
    // Only the dangling-free document loads without a dangling finding.
    assert!(
        loaded
            .diagnostics
            .iter()
            .all(|d| d.kind != DiagnosticKind::Dangling)
    );
    let dangling = load(
        registry(false),
        &types(),
        rows([Some(99), None, None]),
        &LoadOptions::default(),
    )
    .unwrap();
    assert!(
        dangling
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::Dangling && d.object == Some(id(10)))
    );
}

#[test]
fn deleting_a_target_follows_the_rule_of_the_stored_descriptor() {
    let doc = document([Some(1), Some(2), Some(3)]);
    let before = doc.snapshot();
    let row = before.held().next().unwrap().1.row().clone();
    let blocked = delete(&doc, 1).unwrap_err();
    assert_eq!(blocked.code(), codes::REFERENCE_DELETE_BLOCKED.code);
    // Cascade would delete the held object, clear would rewrite its payload: both are refused.
    for target in [2, 3] {
        let refused = delete(&doc, target).unwrap_err();
        assert_eq!(
            refused.code(),
            codes::OBJECT_NOT_MODIFIABLE.code,
            "{target}"
        );
    }
    let after = doc.snapshot();
    assert_eq!(after.revision(), before.revision());
    assert_eq!(after.held().next().unwrap().1.row(), &row);
    // A target the held object does not refer to deletes normally.
    let free = document([None, None, None]);
    delete(&free, 1).unwrap();
}

#[test]
fn an_undecodable_descriptor_leaves_only_the_owner() {
    let mut types = types();
    types[1].descriptor = "{}".into();
    let loaded = load(
        registry(false),
        &types,
        rows([Some(1), None, None]),
        &LoadOptions::default(),
    )
    .unwrap();
    let (_, held) = loaded.snapshot.held().next().unwrap();
    assert!(held.references().is_empty());
    assert_eq!(loaded.snapshot.referrers(id(1)).count(), 0);
}
