//! Cancellation and stale rejection (CORE-DOC-032): a commit that changes the inputs of a
//! running job cancels its token and the node is evaluated again; a result read from a
//! superseded revision is never applied, whether or not the evaluator reaches a yield point;
//! shutdown cancels running jobs and joins the workers.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Finish, NodeKey, NodeState,
    content_hash,
};
use arc_doc::{
    Document, DocumentConfig, FieldDecl, FieldPath, Header, Priority, Schema, SchemaVersion,
    Snapshot, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

const WAIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

fn registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("a", ValueType::I64).in_aspect("geometry")],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.cell").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn sum_key() -> DerivedKey {
    DerivedKey::new("test.sum").unwrap()
}

fn sum(n: u128) -> NodeKey {
    NodeKey::object(id(n), sum_key())
}

fn field(ctx: &EvalContext<'_>, object: ObjectId) -> i64 {
    match ctx.field(object, "a") {
        Ok(Some(Value::I64(n))) => *n,
        _ => 0,
    }
}

fn open(evaluators: Evaluators, workers: usize) -> Document {
    let config = DocumentConfig {
        workers,
        ..DocumentConfig::default()
    };
    Document::open(Snapshot::empty(registry()), Arc::new(evaluators), config).unwrap()
}

fn create(doc: &Document, n: u128, a: i64) {
    doc.submit(move |txn| {
        txn.create(
            "test.cell",
            id(n),
            Header::default(),
            vec![Some(Value::I64(a))],
        )
        .map(drop)
    })
    .wait()
    .unwrap();
}

fn set(doc: &Document, n: u128, a: i64) {
    doc.submit(move |txn| txn.modify(id(n), FieldPath::new("a"), Some(Value::I64(a))))
        .wait()
        .unwrap();
}

fn value(doc: &Document, key: &NodeKey) -> i64 {
    let view = doc.graph().node(key).unwrap();
    assert!(matches!(view.state, NodeState::Clean), "{view:?}");
    view.result.unwrap().downcast_ref::<Number>().unwrap().0
}

// How the evaluator of the single-cell tests behaves; by default (mode 0) it returns at once.
/// Reaches a yield point every millisecond until the gate opens.
const COOPERATE: u8 = 1;
/// Waits for the gate without any yield point.
const IGNORE: u8 = 2;

#[derive(Default)]
struct Control {
    mode: AtomicU8,
    open: Mutex<bool>,
    changed: Condvar,
    /// The value each run read, and whether its token was cancelled when it ended.
    runs: Mutex<Vec<(i64, bool)>>,
    started: Mutex<Vec<i64>>,
    cancelled: AtomicUsize,
}

impl Control {
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.changed.notify_all();
    }

    fn wait_open(&self, timeout: Duration) -> bool {
        let open = self.open.lock().unwrap();
        *self
            .changed
            .wait_timeout_while(open, timeout, |o| !*o)
            .unwrap()
            .0
    }

    fn wait_started(&self, a: i64) -> bool {
        let started = self.started.lock().unwrap();
        let (started, _) = self
            .changed
            .wait_timeout_while(started, WAIT, |s| !s.contains(&a))
            .unwrap();
        started.contains(&a)
    }

    fn wait_cancelled(&self) -> bool {
        let started = self.started.lock().unwrap();
        let (_started, _) = self
            .changed
            .wait_timeout_while(started, WAIT, |_| {
                self.cancelled.load(Ordering::SeqCst) == 0
            })
            .unwrap();
        self.cancelled.load(Ordering::SeqCst) > 0
    }
}

fn controlled(control: &Arc<Control>) -> Evaluators {
    let c = control.clone();
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(
            TypeKey::new("test.cell").unwrap(),
            sum_key(),
            move |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let a = field(ctx, ctx.object_id().unwrap());
                c.started.lock().unwrap().push(a);
                c.changed.notify_all();
                match c.mode.load(Ordering::SeqCst) {
                    COOPERATE => {
                        while !c.wait_open(Duration::from_millis(1)) {
                            if let Err(error) = ctx.yield_point() {
                                c.cancelled.fetch_add(1, Ordering::SeqCst);
                                c.changed.notify_all();
                                return Err(error);
                            }
                        }
                    }
                    IGNORE => {
                        c.wait_open(WAIT);
                    }
                    _ => {}
                }
                c.runs
                    .lock()
                    .unwrap()
                    .push((a, ctx.cancel().is_cancelled()));
                Ok(Arc::new(Number(a)))
            },
        )
        .unwrap();
    evaluators
}

/// A clean node of cell 1 with a = 1, then a run blocked on a = 2 in `mode`, then a = 3.
fn superseded_run(mode: u8) -> (Document, Arc<Control>) {
    let control = Arc::new(Control::default());
    let doc = open(controlled(&control), 2);
    create(&doc, 1, 1);
    doc.regenerate([sum(1)], Priority::Interactive);
    assert!(doc.wait_idle(WAIT));
    assert_eq!(value(&doc, &sum(1)), 1);
    control.mode.store(mode, Ordering::SeqCst);
    set(&doc, 1, 2);
    assert!(
        control.wait_started(2),
        "the node runs again after the commit"
    );
    set(&doc, 1, 3);
    (doc, control)
}

#[test]
fn a_cooperating_evaluator_is_cancelled_and_the_node_rescheduled() {
    let (doc, control) = superseded_run(COOPERATE);
    assert!(
        control.wait_cancelled(),
        "the commit cancelled the running job"
    );
    control.open();
    assert!(doc.wait_idle(WAIT));
    assert_eq!(value(&doc, &sum(1)), 3);
    let runs = control.runs.lock().unwrap().clone();
    assert!(
        runs.iter().all(|(a, _)| *a != 2),
        "the run on a = 2 ended at its yield point: {runs:?}"
    );
}

#[test]
fn a_non_cooperating_evaluator_runs_to_the_end_and_its_result_is_discarded() {
    let (doc, control) = superseded_run(IGNORE);
    let sub = doc.subscribe();
    control.open();
    assert!(doc.wait_idle(WAIT));
    assert_eq!(value(&doc, &sum(1)), 3);
    let runs = control.runs.lock().unwrap().clone();
    assert!(
        runs.contains(&(2, true)),
        "the run on a = 2 finished with its token cancelled: {runs:?}"
    );
    assert_eq!(runs.last(), Some(&(3, false)));
    // Only the result of a = 3 was applied: one node became clean once.
    let mut cleaned = 0;
    while let Some(event) = sub.try_recv() {
        if let arc_doc::Event::Regenerated(event) = event {
            cleaned += match event.clean {
                arc_doc::events::Ids::Listed(set) => set.len(),
                arc_doc::events::Ids::All => panic!("all"),
            };
        }
    }
    assert_eq!(cleaned, 1);
}

#[test]
fn a_lone_job_superseded_by_a_commit_is_stale_for_the_scheduler() {
    // Regression: `Graph::finish` must check the outcome before the job releases its change
    // sets, also when no other job runs.
    let control = Arc::new(Control::default());
    let doc = open(controlled(&control), 0);
    create(&doc, 1, 1);
    let job = doc.graph().begin(&sum(1)).unwrap();
    set(&doc, 1, 2);
    let outcome = job.evaluate();
    assert_eq!(doc.graph().finish(job, outcome), Finish::Stale);
    assert_eq!(doc.ensure(&sum(1)).unwrap().revision, Some(2));
}

#[test]
fn shutdown_cancels_running_jobs_and_joins_the_workers() {
    let control = Arc::new(Control::default());
    control.mode.store(COOPERATE, Ordering::SeqCst);
    let doc = open(controlled(&control), 2);
    create(&doc, 1, 1);
    doc.regenerate([sum(1)], Priority::Background);
    assert!(control.wait_started(1));
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        drop(doc);
        sender.send(()).unwrap();
    });
    assert!(
        receiver.recv_timeout(WAIT).is_ok(),
        "dropping the document returned"
    );
    assert_eq!(control.cancelled.load(Ordering::SeqCst), 1);
}

/// A tiny deterministic generator for the stress test.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const CELLS: u128 = 12;

/// The sum of cell `n` is its value plus the sum of cell `n - 1`, read inline. Odd cells reach
/// yield points while they spin for a pseudo-random time; even cells never do.
fn chain(stop: &Arc<AtomicBool>) -> Evaluators {
    let stop = stop.clone();
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(
            TypeKey::new("test.cell").unwrap(),
            sum_key(),
            move |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let object = ctx.object_id().unwrap();
                let n = object.as_u128();
                let a = field(ctx, object);
                let below = if n > 1 {
                    ctx.node(&sum(n - 1))?
                        .downcast_ref::<Number>()
                        .map_or(0, |v| v.0)
                } else {
                    0
                };
                let spins = content_hash(&(n, a)) % 200;
                for _ in 0..spins {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if n % 2 == 1 {
                        ctx.yield_point()?;
                    }
                    std::thread::yield_now();
                }
                Ok(Arc::new(Number(a.wrapping_add(below))))
            },
        )
        .unwrap();
    evaluators
}

fn stress(seed: u64) {
    let stop = Arc::new(AtomicBool::new(false));
    let doc = open(chain(&stop), 3);
    let mut values = vec![0_i64; CELLS as usize + 1];
    for n in 1..=CELLS {
        create(&doc, n, 0);
    }
    doc.regenerate((1..=CELLS).map(sum), Priority::Background);
    doc.regenerate([sum(CELLS / 2)], Priority::Interactive);
    let mut rng = Rng(seed);
    for round in 0..300 {
        let n = u128::from(rng.below(u64::try_from(CELLS).unwrap())) + 1;
        let a = i64::try_from(rng.below(1_000)).unwrap();
        set(&doc, n, a);
        values[n as usize] = a;
        if round % 50 == 49 {
            // A caller waiting for a node gets the result of the current revision.
            let key = sum(u128::from(rng.below(u64::try_from(CELLS).unwrap())) + 1);
            let view = doc.ensure(&key).unwrap();
            let k = match key.subject {
                arc_doc::graph::Subject::Object(object) => object.as_u128() as usize,
                arc_doc::graph::Subject::Local(_) => panic!("object nodes only"),
            };
            let expected: i64 = values[1..=k].iter().sum();
            assert_eq!(
                view.result.unwrap().downcast_ref::<Number>().unwrap().0,
                expected
            );
        }
    }
    assert!(doc.wait_idle(WAIT), "regeneration settles");
    assert!(doc.graph().non_clean().next().is_none());
    let mut total = 0;
    for n in 1..=CELLS {
        total += values[n as usize];
        assert_eq!(value(&doc, &sum(n)), total, "cell {n}");
    }
    assert!(doc.diagnostics().is_empty());
    stop.store(true, Ordering::Relaxed);
}

#[test]
fn hundreds_of_commits_during_evaluation_settle_on_the_final_snapshot() {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for seed in 1..=3 {
            stress(seed);
        }
        sender.send(()).unwrap();
    });
    assert!(
        receiver.recv_timeout(Duration::from_secs(300)).is_ok(),
        "the stress test finished without deadlock"
    );
}
