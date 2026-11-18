//! The process-wide worker pool (CORE-DOC-031): documents share a bounded set of threads, both
//! make progress, and stopping one document never waits for another's work.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, Subject, content_hash,
};
use arc_doc::scheduler::Priority;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry, pool_threads};

const WAIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Number(u64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

fn key() -> DerivedKey {
    DerivedKey::new("test.pool").unwrap()
}

fn local(n: u64) -> NodeKey {
    NodeKey::local(n, key())
}

/// A document whose node 0 waits until `gate` opens (ignoring cancellation) and whose other
/// nodes count their runs in `count` after a short sleep.
fn document(gate: Arc<AtomicBool>, count: Arc<AtomicUsize>) -> Document {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_local(
            key(),
            move |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let Subject::Local(n) = ctx.key().subject else {
                    panic!("local nodes only");
                };
                if n == 0 {
                    let start = Instant::now();
                    while !gate.load(Ordering::SeqCst) && start.elapsed() < WAIT {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                    count.fetch_add(1, Ordering::SeqCst);
                }
                Ok(Arc::new(Number(n)))
            },
        )
        .unwrap();
    let config = DocumentConfig {
        workers: 2,
        ..DocumentConfig::default()
    };
    Document::open(
        Snapshot::empty(Arc::new(TypeRegistry::new())),
        Arc::new(evaluators),
        config,
    )
    .unwrap()
}

fn wait_until(done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() > WAIT {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn documents_share_a_bounded_pool_and_one_stopping_never_stalls_another() {
    let (gate_a, count_a) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let (gate_b, count_b) = (
        Arc::new(AtomicBool::new(true)),
        Arc::new(AtomicUsize::new(0)),
    );
    let a = document(gate_a.clone(), count_a.clone());
    let b = document(gate_b, count_b.clone());
    let cap = std::thread::available_parallelism()
        .map_or(2, |n| n.get() - 1)
        .max(2);
    assert!(
        pool_threads() >= 2 && pool_threads() <= cap,
        "{}",
        pool_threads()
    );

    // A holds one of its two job slots with a job that waits; both documents still progress.
    a.regenerate([local(0)], Priority::Interactive);
    a.regenerate((1..=200).map(local), Priority::Background);
    b.regenerate((1..=200).map(local), Priority::Background);
    assert!(
        wait_until(|| count_a.load(Ordering::SeqCst) >= 20 && count_b.load(Ordering::SeqCst) >= 20),
        "both progress: {} and {}",
        count_a.load(Ordering::SeqCst),
        count_b.load(Ordering::SeqCst)
    );
    let threads = pool_threads();

    // Dropping A waits for A's own job only, on another thread; B keeps running meanwhile and
    // finishes.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        drop(a);
        sender.send(()).unwrap();
    });
    assert!(b.wait_idle(WAIT), "B finishes while A is being dropped");
    assert_eq!(count_b.load(Ordering::SeqCst), 200);
    assert!(receiver.try_recv().is_err(), "A waits for its running job");
    // Dropping B does not wait for A's job either.
    let (sender_b, receiver_b) = mpsc::channel();
    std::thread::spawn(move || {
        drop(b);
        sender_b.send(()).unwrap();
    });
    assert!(receiver_b.recv_timeout(WAIT).is_ok(), "dropping B returned");
    gate_a.store(true, Ordering::SeqCst);
    assert!(receiver.recv_timeout(WAIT).is_ok(), "dropping A returned");
    assert_eq!(
        pool_threads(),
        threads,
        "the pool started no thread per document"
    );
}
