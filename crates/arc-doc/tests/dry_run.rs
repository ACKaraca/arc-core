//! The dry run (`Document::dry_run`): it validates a job's transaction exactly as a commit does
//! (every stage, with the same error codes) and then leaves no trace: the same snapshot, no
//! revision, journal, undo or redo change, no event and no diagnostic.

#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Category, Error, ErrorCode, ObjectId, Result};
use arc_doc::graph::Evaluators;
use arc_doc::{
    Bound, Cardinality, CollectingSink, Document, FieldDecl, Finding, Header, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Snapshot, Strength, Txn,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

const THIRTEEN: ErrorCode = ErrorCode {
    code: "test.item.thirteen",
    category: Category::InvalidInput,
    message_id: "test-item-thirteen",
};
const REJECTED: ErrorCode = ErrorCode {
    code: "test.document.rejected",
    category: Category::InvalidInput,
    message_id: "test-document-rejected",
};

fn id(slot: u128) -> ObjectId {
    ObjectId::from_u128(slot)
}

fn n_of(values: &[Option<Value>]) -> Option<i64> {
    match values.first() {
        Some(Some(Value::I64(n))) => Some(*n),
        _ => None,
    }
}

fn type_validator(values: &[Option<Value>]) -> Vec<Finding> {
    let severity = match n_of(values) {
        Some(13) => Severity::Error,
        Some(7) => Severity::Warning,
        _ => return Vec::new(),
    };
    vec![Finding {
        severity,
        error: Error::new(THIRTEEN),
    }]
}

fn document() -> Document {
    let link = ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new("test.item").unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    });
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 100 }),
            FieldDecl::new("link", link).optional(),
        ],
    );
    let mut decl = TypeDecl::new(TypeKey::new("test.item").unwrap(), schema);
    decl.validators.push(type_validator);
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    let doc = Document::new(Arc::new(registry), Arc::new(Evaluators::new())).unwrap();
    // Rejects a transaction that leaves a touched object with `n` 42.
    doc.add_validator(Arc::new(
        |_: &Snapshot, candidate: &Snapshot, touched: &_| {
            let touched: &std::collections::BTreeMap<ObjectId, arc_doc::Change> = touched;
            let rejected = touched.keys().any(|id| {
                candidate
                    .resolve(*id)
                    .and_then(|handle| candidate.get(handle))
                    .is_ok_and(|object| n_of(&object.values) == Some(42))
            });
            if rejected {
                vec![Finding {
                    severity: Severity::Error,
                    error: Error::new(REJECTED),
                }]
            } else {
                Vec::new()
            }
        },
    ));
    doc
}

fn create(txn: &mut Txn, slot: u128, n: i64, link: Option<u128>) -> Result<()> {
    let values = vec![Some(Value::I64(n)), link.map(|t| Value::Reference(id(t)))];
    txn.create("test.item", id(slot), Header::default(), values)
        .map(drop)
}

/// Everything a dry run must leave alone.
fn state(doc: &Document) -> (Snapshot, u64, usize, usize, usize) {
    let snapshot = doc.snapshot();
    let revision = snapshot.revision();
    let (undo, redo) = doc.with_undo(|u| (u.undo_steps().count(), u.redo_steps().count()));
    (
        snapshot,
        revision,
        doc.with_journal(|j| j.len()),
        undo,
        redo,
    )
}

fn assert_untouched(before: &(Snapshot, u64, usize, usize, usize), doc: &Document) {
    let after = state(doc);
    assert!(before.0.ptr_eq(&after.0), "the snapshot is the same one");
    assert_eq!(
        (before.1, before.2, before.3, before.4),
        (after.1, after.2, after.3, after.4)
    );
}

/// A document with objects 1 and 2 (2 links 1), one undone step on the redo stack.
fn prepared() -> Document {
    let doc = document();
    doc.submit(|txn| {
        create(txn, 1, 1, None)?;
        create(txn, 2, 2, Some(1))
    })
    .wait()
    .unwrap();
    doc.submit(|txn| create(txn, 3, 3, None)).wait().unwrap();
    assert!(doc.undo().wait().unwrap().value);
    doc
}

type Job = fn(&mut Txn) -> Result<()>;

#[test]
fn every_validation_stage_rejects_a_dry_run_with_the_code_of_a_commit() {
    let jobs: [(&str, Job); 6] = [
        ("doc.field.out_of_bounds", |t| create(t, 9, 1000, None)),
        ("test.item.thirteen", |t| create(t, 9, 13, None)),
        ("test.document.rejected", |t| create(t, 9, 42, None)),
        ("doc.reference.dangling", |t| create(t, 9, 1, Some(77))),
        ("doc.object.not_found", |t| t.delete(id(55))),
        ("test.job", |_| {
            Err(Error::new(THIRTEEN).with_detail("x", "y"))
        }),
    ];
    for (name, job) in jobs {
        let (dry, real) = (prepared(), prepared());
        let before = state(&dry);
        let rejected = dry.dry_run(job).wait().unwrap_err();
        let failed = real.submit(job).wait().unwrap_err();
        assert_eq!(rejected.code(), failed.code(), "{name}");
        assert_untouched(&before, &dry);
    }
}

#[test]
fn a_panicking_job_is_a_fault_and_leaves_no_trace() {
    let doc = prepared();
    let before = state(&doc);
    let error = doc
        .dry_run(|_| -> Result<()> { panic!("boom") })
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.executor.fault");
    assert_untouched(&before, &doc);
    assert!(doc.submit(|txn| create(txn, 9, 1, None)).wait().is_ok());
}

#[test]
fn an_accepted_dry_run_reports_the_commit_and_leaves_no_trace() {
    let (dry, real) = (prepared(), prepared());
    let events = dry.subscribe();
    let sink = Arc::new(CollectingSink::new(16));
    dry.set_diagnostics(sink.clone());
    let diagnostics = dry.diagnostics_snapshot().version;
    let before = state(&dry);
    // A new object, a warning, and a deletion whose rule clears the link of object 2.
    let job = |txn: &mut Txn| {
        create(txn, 9, 7, None)?;
        txn.delete(id(1))?;
        Ok("done")
    };
    let done = dry.dry_run(job).wait().unwrap();
    assert_eq!(done.value.value, "done");
    assert!(before.0.ptr_eq(&done.snapshot));
    assert_eq!(done.txn_id, None);
    assert_eq!(done.diagnostics.len(), 1, "the warning is returned");
    assert_untouched(&before, &dry);
    assert!(events.try_recv().is_none(), "no event");
    assert!(sink.take().0.is_empty(), "nothing delivered");
    assert_eq!(dry.diagnostics_snapshot().version, diagnostics);

    // The real commit publishes exactly what the dry run announced.
    let committed = real.submit(job).wait().unwrap();
    assert_eq!(committed.snapshot.revision(), before.1 + 1);
    let journaled = real.with_journal(|j| j.last().unwrap().record().ops.clone());
    assert_eq!(done.value.ops, journaled);
    assert!(
        done.value.ops.len() > 2,
        "the clear of the link is an operation"
    );
    assert!(matches!(
        done.value.changes,
        arc_doc::events::Changes::Listed(_)
    ));
}

#[test]
fn a_dry_run_without_operations_is_empty() {
    let doc = prepared();
    let before = state(&doc);
    let done = doc.dry_run(|_| Ok(5)).wait().unwrap();
    assert_eq!((done.value.value, done.value.ops.len()), (5, 0));
    assert_untouched(&before, &doc);
}

#[test]
fn concurrent_real_commits_are_unaffected_by_dry_runs() {
    let doc = Arc::new(document());
    let committers: Vec<_> = (0..4_u128)
        .map(|t| {
            let doc = Arc::clone(&doc);
            std::thread::spawn(move || {
                for i in 0..25_u128 {
                    let slot = 1 + t * 25 + i;
                    doc.submit(move |txn| create(txn, slot, 1, None))
                        .wait()
                        .unwrap();
                }
            })
        })
        .collect();
    let dryers: Vec<_> = (0..2_u128)
        .map(|t| {
            let doc = Arc::clone(&doc);
            std::thread::spawn(move || {
                for i in 0..50_u128 {
                    let slot = 1000 + t * 50 + i;
                    let done = doc
                        .dry_run(move |txn| create(txn, slot, 1, None))
                        .wait()
                        .unwrap();
                    assert_eq!(done.value.ops.len(), 1);
                }
            })
        })
        .collect();
    for thread in committers.into_iter().chain(dryers) {
        thread.join().unwrap();
    }
    let snapshot = doc.snapshot();
    assert_eq!(snapshot.revision(), 100);
    assert_eq!(snapshot.len(), 100);
    assert_eq!(doc.with_journal(|j| j.len()), 100);
    assert!(snapshot.resolve(id(1000)).is_err());
}
