//! The diagnostics list of a document (CORE-DOC-035): load findings of preserved and
//! quarantined objects, validator warnings and failed nodes, with their lifetimes and change
//! events; an extra sink still receives what commits deliver.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{
    CollectingSink, DiagnosticKind, Document, DocumentConfig, Event, FieldDecl, FieldPath, Finding,
    Header, LoadOptions, Origin, Priority, Schema, SchemaVersion, Severity, StoredObject,
    StoredType, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, load,
};

const WAIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

/// 13 is a warning.
fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    match values.first() {
        Some(Some(Value::I64(13))) => vec![Finding {
            severity: Severity::Warning,
            error: Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "n"),
        }],
        _ => Vec::new(),
    }
}

fn schema() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    )
}

fn registry() -> Arc<TypeRegistry> {
    let mut decl = TypeDecl::new(TypeKey::new("test.item").unwrap(), schema());
    decl.validators.push(validator);
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn half() -> NodeKey {
    NodeKey::object(id(1), DerivedKey::new("test.half").unwrap())
}

/// Halves `n`; odd values fail.
fn evaluators() -> Arc<Evaluators> {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(
            TypeKey::new("test.item").unwrap(),
            DerivedKey::new("test.half").unwrap(),
            |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                match ctx.field(ctx.object_id().unwrap(), "n")? {
                    Some(Value::I64(n)) if n % 2 == 0 => Ok(Arc::new(Number(n / 2))),
                    _ => Err(Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "n")),
                }
            },
        )
        .unwrap();
    Arc::new(evaluators)
}

fn row(n: u128, value: i64) -> StoredObject {
    let object = arc_doc::Object::new(
        id(n),
        SchemaVersion::new(1, 0),
        Header::default(),
        vec![Some(Value::I64(value))],
    );
    StoredObject::of(
        &TypeKey::new("test.item").unwrap(),
        &schema(),
        &object,
        &Limits::DEFAULT,
    )
    .unwrap()
}

/// Item 1 (n = 13: a load warning), a preserved object 10 and a quarantined item 11.
fn loaded_document() -> Document {
    let types: Vec<StoredType> = ["test.item", "other.thing"]
        .into_iter()
        .map(|key| StoredType {
            type_key: key.into(),
            major: 1,
            minor: 0,
            descriptor: String::new(),
        })
        .collect();
    let raw = |n: u128, key: &str, payload: Vec<u8>| StoredObject {
        id: n.to_be_bytes().to_vec(),
        type_key: key.into(),
        major: 1,
        minor: 0,
        payload,
        payload_hash: vec![7; 32],
        last_txn: vec![9; 16],
        ..StoredObject::default()
    };
    let rows = vec![
        row(1, 13),
        raw(10, "other.thing", vec![1, 2, 3]),
        raw(11, "test.item", vec![0xFF, 0xFF]),
    ];
    let loaded = load(registry(), &types, rows, &LoadOptions::default()).unwrap();
    let config = DocumentConfig {
        workers: 1,
        ..DocumentConfig::default()
    };
    Document::from_loaded_with(loaded, evaluators(), config).unwrap()
}

fn origins(doc: &Document) -> Vec<Origin> {
    doc.diagnostics_snapshot()
        .diagnostics
        .iter()
        .map(|d| d.origin.clone())
        .collect()
}

#[test]
fn load_findings_live_while_their_objects_are_held() {
    let doc = loaded_document();
    assert_eq!(
        origins(&doc),
        [
            Origin::Load(DiagnosticKind::Warning),
            Origin::Load(DiagnosticKind::Preserved),
            Origin::Load(DiagnosticKind::Quarantined),
        ]
    );
    let quarantined = doc.diagnostics().for_object(id(11));
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].severity, Severity::Error);
    // Committing item 1 ends its load warning; its validator reports again what still holds.
    doc.submit(|txn| txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(13))))
        .wait()
        .unwrap();
    assert_eq!(
        origins(&doc),
        [
            Origin::Load(DiagnosticKind::Preserved),
            Origin::Load(DiagnosticKind::Quarantined),
            Origin::Validator,
        ]
    );
    doc.submit(|txn| txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(4))))
        .wait()
        .unwrap();
    assert_eq!(doc.diagnostics().len(), 2, "the held objects stay listed");
}

#[test]
fn failed_nodes_are_listed_and_cleared_on_success_with_events() {
    let doc = loaded_document();
    let sink = Arc::new(CollectingSink::new(8));
    doc.set_diagnostics(sink.clone());
    let sub = doc.subscribe();
    let before = doc.diagnostics().version();
    doc.regenerate([half()], Priority::Interactive);
    assert!(doc.wait_idle(WAIT));
    let failed = doc.diagnostics().for_origin(&Origin::Node(half()));
    assert_eq!(failed.len(), 1, "13 is odd");
    assert_eq!(failed[0].code(), "doc.value.out_of_range");
    assert_eq!(failed[0].objects, [id(1)]);
    assert!(doc.diagnostics().version() > before);

    doc.submit(|txn| txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(13))))
        .wait()
        .unwrap();
    assert_eq!(
        sink.take().0.len(),
        1,
        "the extra sink got the validator warning"
    );

    doc.submit(|txn| txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(8))))
        .wait()
        .unwrap();
    assert!(doc.wait_idle(WAIT));
    assert!(
        doc.diagnostics()
            .for_origin(&Origin::Node(half()))
            .is_empty()
    );
    assert!(doc.diagnostics().for_object(id(1)).is_empty());

    let mut added = 0;
    let mut removed = 0;
    let mut failed_nodes = 0;
    while let Some(event) = sub.try_recv() {
        match event {
            Event::DiagnosticsChanged(change) => {
                added += change.added.len();
                removed += change.removed.len();
            }
            Event::Regenerated(event) => {
                if let arc_doc::events::Ids::Listed(set) = event.failed {
                    failed_nodes += set.len();
                }
            }
            _ => {}
        }
    }
    // From three load diagnostics to the two of the held objects: everything the events added
    // (failures, the validator warning) was removed again, plus the load warning of item 1.
    assert!(failed_nodes >= 1);
    assert!(added >= 2);
    assert_eq!(removed, added + 1);
    assert_eq!(doc.diagnostics().len(), 2);
}
