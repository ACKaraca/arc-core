//! The document executor (CORE-DOC-016) and fault rollback (CORE-DOC-018): the guard test of
//! `docs/testing.md`, rollback of failing jobs, strict order across concurrent submitters,
//! submission without blocking, and shutdown.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use arc_base::id::Identifier;
use arc_base::{Category, Error, ObjectId};
use arc_doc::error::codes;
use arc_doc::graph::Evaluators;
use arc_doc::{
    Bound, CollectingSink, Document, FieldDecl, FieldPath, Header, Origin, Schema, SchemaVersion,
    Txn, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

fn document() -> Document {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int {
            min: 0,
            max: 1_000_000,
        })],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.item").unwrap(), schema))
        .unwrap();
    Document::new(Arc::new(registry), Arc::new(Evaluators::new())).unwrap()
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn create(txn: &mut Txn, n: u128, value: i64) -> arc_base::Result<()> {
    txn.create(
        "test.item",
        id(n),
        Header::default(),
        vec![Some(Value::I64(value))],
    )
    .map(drop)
}

fn set(txn: &mut Txn, n: u128, value: i64) -> arc_base::Result<()> {
    txn.modify(id(n), FieldPath::new("n"), Some(Value::I64(value)))
}

fn value(doc: &Document, n: u128) -> Option<i64> {
    let snapshot = doc.snapshot();
    let object = snapshot.get(snapshot.resolve(id(n)).ok()?).ok()?;
    match object.values.first() {
        Some(Some(Value::I64(v))) => Some(*v),
        _ => None,
    }
}

#[test]
fn guard_a_job_that_unwinds_leaves_the_last_snapshot_and_the_executor_runs_on() {
    let doc = document();
    let sink = Arc::new(CollectingSink::new(10));
    doc.set_diagnostics(sink.clone());
    doc.submit(|txn| create(txn, 1, 1)).wait().unwrap();
    let before = doc.snapshot();

    let error = doc
        .submit(|txn| -> arc_base::Result<()> {
            set(txn, 1, 2)?;
            create(txn, 2, 2)?;
            panic!("guard test fault");
        })
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.executor.fault");
    assert_eq!(error.category(), Category::Internal);
    assert_eq!(
        error.details().get("panic"),
        Some(&arc_base::error::Value::Text("guard test fault".into()))
    );
    assert!(doc.snapshot().ptr_eq(&before));
    let (diagnostics, _) = sink.take();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code(), "doc.executor.fault");
    assert_eq!(diagnostics[0].origin, Origin::Executor);

    let next = doc.submit(|txn| set(txn, 1, 3)).wait().unwrap();
    assert_eq!(next.snapshot.revision(), 2);
    assert_eq!(value(&doc, 1), Some(3));
    assert_eq!(value(&doc, 2), None);
}

#[test]
fn a_job_error_or_a_failed_commit_leaves_no_partial_change() {
    let doc = document();
    doc.submit(|txn| create(txn, 1, 1)).wait().unwrap();
    let before = doc.snapshot();
    let error = doc
        .submit(|txn| -> arc_base::Result<()> {
            set(txn, 1, 5)?;
            Err(Error::new(codes::PATH_INVALID))
        })
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.path.invalid");
    assert!(doc.snapshot().ptr_eq(&before));
    let error = doc.submit(|txn| set(txn, 1, 2_000_000)).wait().unwrap_err();
    assert_eq!(error.code(), "doc.value.out_of_range");
    assert!(doc.snapshot().ptr_eq(&before));
    assert_eq!(value(&doc, 1), Some(1));
}

#[test]
fn concurrent_submitters_run_in_order_without_interleaving() {
    const THREADS: u128 = 8;
    const JOBS: i64 = 100;
    let doc = Arc::new(document());
    doc.submit(|txn| (0..THREADS).try_for_each(|t| create(txn, t + 1, 0)))
        .wait()
        .unwrap();
    let running = Arc::new(AtomicBool::new(false));
    let order = Arc::new(Mutex::new(Vec::new()));
    let submitters: Vec<_> = (0..THREADS)
        .map(|t| {
            let (doc, running, order) = (doc.clone(), running.clone(), order.clone());
            thread::spawn(move || {
                let pending: Vec<_> = (0..JOBS)
                    .map(|seq| {
                        let (running, order) = (running.clone(), order.clone());
                        doc.submit(move |txn| {
                            assert!(!running.swap(true, Ordering::SeqCst), "jobs interleave");
                            // Each job reads the value its predecessor of this thread wrote.
                            let n = t + 1;
                            let current = match txn.get(id(n))?.values.first() {
                                Some(Some(Value::I64(v))) => *v,
                                _ => -1,
                            };
                            assert_eq!(current, seq);
                            set(txn, n, seq + 1)?;
                            order.lock().unwrap().push((t, seq));
                            running.store(false, Ordering::SeqCst);
                            Ok(())
                        })
                    })
                    .collect();
                for pending in pending {
                    pending.wait().unwrap();
                }
            })
        })
        .collect();
    for submitter in submitters {
        submitter.join().unwrap();
    }
    let order = order.lock().unwrap();
    assert_eq!(order.len(), usize::try_from(THREADS * 100).unwrap());
    for t in 0..THREADS {
        let seqs: Vec<i64> = order
            .iter()
            .filter(|(x, _)| *x == t)
            .map(|(_, s)| *s)
            .collect();
        assert_eq!(seqs, (0..JOBS).collect::<Vec<_>>());
        assert_eq!(value(&doc, t + 1), Some(JOBS));
    }
    assert_eq!(
        doc.snapshot().revision(),
        1 + u64::try_from(THREADS).unwrap() * u64::try_from(JOBS).unwrap()
    );
}

#[test]
fn submitting_does_not_wait_for_a_running_job() {
    let doc = document();
    let (release, gate) = mpsc::channel::<()>();
    let blocked = doc.submit(move |txn| {
        gate.recv().unwrap();
        create(txn, 1, 1)
    });
    let start = Instant::now();
    let queued = doc.submit(|txn| set(txn, 1, 2));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(blocked.try_take().is_none());
    assert!(queued.wait_timeout(Duration::from_millis(20)).is_none());
    release.send(()).unwrap();
    blocked.wait().unwrap();
    assert_eq!(queued.wait().unwrap().snapshot.revision(), 2);
}

#[test]
fn closing_stops_after_the_running_job_and_answers_queued_jobs() {
    let doc = Arc::new(document());
    let (release, gate) = mpsc::channel::<()>();
    let (started_tx, started) = mpsc::channel::<()>();
    let running = doc.submit(move |txn| {
        started_tx.send(()).unwrap();
        gate.recv().unwrap();
        create(txn, 1, 1)
    });
    let queued = doc.submit(|txn| create(txn, 2, 2));
    started.recv().unwrap();
    let closer = {
        let doc = doc.clone();
        thread::spawn(move || doc.close())
    };
    thread::sleep(Duration::from_millis(20));
    release.send(()).unwrap();
    closer.join().unwrap();
    assert_eq!(running.wait().unwrap().snapshot.revision(), 1);
    assert_eq!(queued.wait().unwrap_err().code(), "doc.executor.stopped");
    let late = doc.submit(|txn| create(txn, 3, 3));
    assert_eq!(late.wait().unwrap_err().code(), "doc.executor.stopped");
    assert_eq!(value(&doc, 1), Some(1));
    assert_eq!(value(&doc, 2), None);
}

#[test]
fn transactions_chain_their_parents_and_skip_empty_jobs() {
    let doc = document();
    let first = doc.submit(|txn| create(txn, 1, 1)).wait().unwrap();
    let empty = doc.submit(|_| Ok(7)).wait().unwrap();
    assert_eq!(empty.value, 7);
    assert!(empty.txn_id.is_none());
    assert!(empty.snapshot.ptr_eq(&first.snapshot));
    let parent = doc
        .submit(|txn| Ok(txn.meta().parent))
        .wait()
        .unwrap()
        .value;
    assert_eq!(parent, first.txn_id);
}
