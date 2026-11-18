//! Prioritized scheduling (CORE-DOC-031): dirty nodes run on the worker pool by priority
//! (blocking, interactive, visible, background), first in first out within a priority, in
//! parallel where independent; `ensure` overtakes queued work; settled nodes are announced in
//! `regenerated` events.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use arc_doc::events::Ids;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, NodeState, Subject,
    content_hash,
};
use arc_doc::scheduler::{Priority, Rule};
use arc_doc::{Document, DocumentConfig, Event, Snapshot, TypeRegistry};

/// Generous bound for waits that only end early on success.
const WAIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Number(u64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

/// A list of values with a condition variable, to wait for entries instead of sleeping.
#[derive(Default)]
struct Log {
    entries: Mutex<Vec<u64>>,
    changed: Condvar,
}

impl Log {
    fn push(&self, n: u64) {
        self.entries.lock().unwrap().push(n);
        self.changed.notify_all();
    }

    fn get(&self) -> Vec<u64> {
        self.entries.lock().unwrap().clone()
    }

    fn wait_for(&self, done: impl Fn(&[u64]) -> bool) -> bool {
        let entries = self.entries.lock().unwrap();
        let (entries, _) = self
            .changed
            .wait_timeout_while(entries, WAIT, |e| !done(e))
            .unwrap();
        done(&entries)
    }
}

/// A gate the evaluator of the node `GATE` waits at.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Gate {
    fn wait(&self) {
        let open = self.open.lock().unwrap();
        drop(
            self.changed
                .wait_timeout_while(open, WAIT, |o| !*o)
                .unwrap(),
        );
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }
}

const GATE: u64 = 0;
/// The nodes 200 and 201 run only when both run at once.
const MEET: [u64; 2] = [200, 201];

struct Fixture {
    doc: Document,
    log: Arc<Log>,
    gate: Arc<Gate>,
    met: Arc<Log>,
}

fn key() -> DerivedKey {
    DerivedKey::new("test.record").unwrap()
}

fn local(n: u64) -> NodeKey {
    NodeKey::local(n, key())
}

fn fixture(workers: usize) -> Fixture {
    let log = Arc::new(Log::default());
    let gate = Arc::new(Gate::default());
    let arrived = Arc::new(Log::default());
    let met = Arc::new(Log::default());
    let mut evaluators = Evaluators::new();
    let (l, g, a, m) = (log.clone(), gate.clone(), arrived.clone(), met.clone());
    evaluators
        .register_local(
            key(),
            move |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let Subject::Local(n) = ctx.key().subject else {
                    panic!("local nodes only");
                };
                l.push(n);
                if n == GATE {
                    g.wait();
                }
                if MEET.contains(&n) {
                    a.push(n);
                    if a.wait_for(|e| e.len() >= MEET.len()) {
                        m.push(n);
                    }
                }
                Ok(Arc::new(Number(n)))
            },
        )
        .unwrap();
    let config = DocumentConfig {
        workers,
        ..DocumentConfig::default()
    };
    let doc = Document::open(
        Snapshot::empty(Arc::new(TypeRegistry::new())),
        Arc::new(evaluators),
        config,
    )
    .unwrap();
    Fixture {
        doc,
        log,
        gate,
        met,
    }
}

fn visible_rule() -> Rule {
    Arc::new(|key: &NodeKey| matches!(key.subject, Subject::Local(n) if (100..200).contains(&n)))
}

/// Requests background nodes 1 and 2, interactive (pinned) nodes 10 and 11 and visible (by
/// rule) nodes 100 and 101, interleaved.
fn request_mixed(f: &Fixture) {
    let scheduler = f.doc.scheduler();
    scheduler.set_rule(Priority::Visible, Some(visible_rule()));
    scheduler.request([local(1), local(100)]);
    f.doc.regenerate([local(10)], Priority::Interactive);
    scheduler.request([local(2), local(101)]);
    f.doc.regenerate([local(11)], Priority::Interactive);
}

const MIXED_ORDER: [u64; 6] = [10, 11, 100, 101, 1, 2];

#[test]
fn the_pool_runs_queued_nodes_by_priority_then_in_request_order() {
    let f = fixture(1);
    f.doc.regenerate([local(GATE)], Priority::Background);
    assert!(f.log.wait_for(|e| e == [GATE]), "the worker holds the gate");
    request_mixed(&f);
    assert_eq!(
        f.doc.scheduler().priority_of(&local(100)),
        Priority::Visible
    );
    assert_eq!(
        f.doc.scheduler().priority_of(&local(1)),
        Priority::Background
    );
    f.gate.open();
    assert!(f.doc.wait_idle(WAIT));
    let mut expected = vec![GATE];
    expected.extend(MIXED_ORDER);
    assert_eq!(f.log.get(), expected);
}

#[test]
fn without_workers_pumping_runs_the_same_order() {
    let f = fixture(0);
    request_mixed(&f);
    assert!(f.log.get().is_empty(), "nothing runs until pumped");
    assert!(!f.doc.scheduler().is_idle());
    assert_eq!(f.doc.scheduler().pump(), MIXED_ORDER.len());
    assert_eq!(f.log.get(), MIXED_ORDER);
    assert!(f.doc.scheduler().is_idle());
    // Re-classifying keeps the queue order: without the rule, 100 and 101 become background
    // nodes ahead of 1 and 2, which were queued behind them; unpinning moves 10 to the back.
    let f = fixture(0);
    request_mixed(&f);
    f.doc.scheduler().set_rule(Priority::Visible, None);
    f.doc.scheduler().unpin([local(10)]);
    assert!(f.doc.wait_idle(WAIT));
    assert_eq!(f.log.get(), [11, 100, 101, 1, 2, 10]);
}

#[test]
fn ensure_overtakes_queued_background_work() {
    let f = fixture(1);
    f.doc.regenerate([local(GATE)], Priority::Background);
    assert!(f.log.wait_for(|e| e == [GATE]));
    f.doc.scheduler().request((1..=5).map(local));
    let view = f.doc.ensure(&local(50)).unwrap();
    assert!(matches!(view.state, NodeState::Clean));
    assert_eq!(
        view.result.unwrap().downcast_ref::<Number>().map(|n| n.0),
        Some(50)
    );
    assert_eq!(f.log.get(), [GATE, 50], "no background node ran before it");
    // A settled node is returned without evaluating again.
    f.doc.ensure(&local(50)).unwrap();
    f.gate.open();
    assert!(f.doc.wait_idle(WAIT));
    assert_eq!(f.log.get(), [GATE, 50, 1, 2, 3, 4, 5]);
}

#[test]
fn independent_nodes_run_in_parallel() {
    let f = fixture(2);
    f.doc.scheduler().request(MEET.map(local));
    assert!(f.doc.wait_idle(WAIT));
    let mut met = f.met.get();
    met.sort_unstable();
    assert_eq!(met, MEET, "both evaluations were running at the same time");
}

#[test]
fn settled_nodes_are_announced_in_regenerated_events() {
    let f = fixture(0);
    let sub = f.doc.subscribe();
    f.doc.scheduler().request([local(1), local(2)]);
    assert!(f.doc.wait_idle(WAIT));
    let mut clean = Ids::default();
    while let Some(event) = sub.try_recv() {
        if let Event::Regenerated(event) = event {
            clean.merge(event.clean);
        }
    }
    assert_eq!(clean, Ids::of([local(1), local(2)]));
}

#[test]
fn ensure_reports_errors_and_stops_with_the_document() {
    let f = fixture(1);
    let unknown = NodeKey::local(1, DerivedKey::new("test.unknown").unwrap());
    assert_eq!(
        f.doc.ensure(&unknown).unwrap_err().code(),
        "doc.graph.unknown_key"
    );
    // The node is left alone instead of being retried, so the scheduler becomes idle.
    f.doc.scheduler().request([unknown]);
    assert!(f.doc.wait_idle(WAIT));
    f.doc.close();
    assert_eq!(
        f.doc.ensure(&local(1)).unwrap_err().code(),
        "doc.scheduler.stopped"
    );
}
