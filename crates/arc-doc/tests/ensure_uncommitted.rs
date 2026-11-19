//! Synchronous ensure (CORE-DOC-033): a command computes a derived node from its uncommitted
//! state, and the result is discarded when the transaction rolls back, so the document's node
//! keeps the value of the committed state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code builds valid literal fixtures"
)]

use std::sync::{Arc, Mutex};

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId, Result};
use arc_doc::ensure::ensure;
use arc_doc::error::codes;
use arc_doc::graph::{Derived, DerivedKey, DerivedValue, Evaluators, NodeKey, content_hash};
use arc_doc::{
    Document, FieldDecl, FieldPath, Header, Schema, SchemaVersion, Txn, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType,
};

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

fn num(value: &DerivedValue) -> i64 {
    value.downcast_ref::<Number>().unwrap().0
}

fn ok(n: i64) -> Result<DerivedValue> {
    Ok(Arc::new(Number(n)))
}

fn cell() -> TypeKey {
    TypeKey::new("test.cell").unwrap()
}

fn object() -> ObjectId {
    ObjectId::from_u128(1)
}

fn double() -> NodeKey {
    NodeKey::object(object(), DerivedKey::new("test.double").unwrap())
}

fn evaluators() -> Arc<Evaluators> {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(cell(), DerivedKey::new("test.double").unwrap(), |ctx| {
            let me = ctx.object_id().unwrap();
            match ctx.field(me, "v") {
                Ok(Some(Value::I64(n))) => ok(n * 2),
                _ => Err(Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "v")),
            }
        })
        .unwrap();
    Arc::new(evaluators)
}

fn document() -> Document {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("v", ValueType::I64)],
    );
    let mut registry = TypeRegistry::new();
    registry.register(TypeDecl::new(cell(), schema)).unwrap();
    let document = Document::new(Arc::new(registry), evaluators()).unwrap();
    document
        .submit(|txn: &mut Txn| {
            txn.create(
                "test.cell",
                object(),
                Header::default(),
                vec![Some(Value::I64(5))],
            )
            .map(drop)
        })
        .wait()
        .unwrap();
    document
}

fn committed_value(document: &Document) -> i64 {
    let view = document.ensure(&double()).unwrap();
    num(view.result.as_ref().unwrap())
}

#[test]
fn ensure_computes_from_the_uncommitted_state_and_rollback_restores_the_committed_node() {
    let document = document();
    assert_eq!(committed_value(&document), 10);

    let seen = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&seen);
    let evaluators = evaluators();
    let outcome = document
        .submit(move |txn| -> Result<()> {
            txn.modify(object(), FieldPath::new("v"), Some(Value::I64(7)))?;
            let views = ensure(txn, evaluators, &[double()])?;
            *captured.lock().unwrap() = Some(num(views[0].result.as_ref().unwrap()));
            // Fails the transaction after the ensure, so it rolls back.
            Err(Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "v"))
        })
        .wait();

    assert!(outcome.is_err());
    assert_eq!(
        *seen.lock().unwrap(),
        Some(14),
        "computed from the uncommitted 7"
    );
    assert_eq!(
        committed_value(&document),
        10,
        "the node reflects the committed 5 again"
    );
}

#[test]
fn a_committed_ensure_matches_the_node_the_document_computes_afterwards() {
    let document = document();
    let evaluators = evaluators();
    let seen = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&seen);
    document
        .submit(move |txn| {
            txn.modify(object(), FieldPath::new("v"), Some(Value::I64(9)))?;
            let views = ensure(txn, evaluators, &[double()])?;
            *captured.lock().unwrap() = Some(num(views[0].result.as_ref().unwrap()));
            Ok(())
        })
        .wait()
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(18));
    assert_eq!(committed_value(&document), 18);
}

#[test]
fn ensure_fails_for_a_key_no_evaluator_computes() {
    let document = document();
    let evaluators = evaluators();
    let unknown = NodeKey::object(object(), DerivedKey::new("test.none").unwrap());
    let error = document
        .submit(move |txn| ensure(txn, evaluators, &[unknown]).map(drop))
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), codes::GRAPH_UNKNOWN_KEY.code);
}
