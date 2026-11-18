//! The regeneration scheduler (CORE-DOC-031) and the cancellation of running jobs
//! (CORE-DOC-032).
//!
//! A [`Scheduler`] evaluates the dirty nodes of one document's [`Graph`] on a fixed pool of
//! worker threads, highest [`Priority`] first and first in, first out within a priority.
//! Independent nodes run in parallel, one job per node: the scheduler never runs two jobs of
//! one node at once, and a node demanded from inside a job is evaluated inline by that job
//! ([`crate::graph::EvalContext::node`]), so no worker waits for another.
//!
//! The scheduler observes the graph ([`Graph::set_observer`]): after each applied change set it
//! cancels the token of every running job whose node, or a node it depends on, recorded an
//! input the change set changed ([`Graph::inputs_changed_since`]), and queues the nodes that
//! became dirty. A cancelled evaluator stops at its next yield point; its outcome is discarded
//! and the node is queued again ahead of its priority. Correctness never depends on
//! cancellation: [`Graph::finish`] applies an outcome only when none of its inputs changed after
//! the revision it read, so an evaluator that ignores its token only wastes time.
//!
//! After each job and each change set the scheduler drains the graph's transitions: it emits
//! one `regenerated` event for the nodes that settled since the previous drain, and brings the
//! node diagnostics of its [`DiagnosticsList`] in line. It therefore owns
//! [`Graph::drain_transitions`] of its graph.
//!
//! WebAssembly in the browser has no threads by default: on `wasm32` the pool has no worker,
//! jobs run on the calling thread when [`Scheduler::pump`] or [`Scheduler::wait_idle`] is
//! called, and [`Scheduler::ensure`] evaluates inline as everywhere.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use arc_base::{Cancel, Error, ObjectId, Result};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Origin};
use crate::diagnostics::{DiagnosticsList, Emitter};
use crate::error::codes;
use crate::events::{Event, Ids, RegeneratedEvent};
use crate::graph::{ChangeSet, Finish, Graph, NodeKey, NodeView, StateKind, Subject};
use crate::schema::Severity;

/// How many times [`Scheduler::ensure`] evaluates a node whose results keep being superseded
/// before it fails with `doc.graph.stale`.
const ATTEMPTS: usize = 16;

/// The priority classes of the document model, highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// A caller waits for the node ([`Scheduler::ensure`]).
    Blocking,
    /// The node is visible in the active viewport or sheet.
    Interactive,
    /// The node is visible in another open view.
    Visible,
    /// Everything else that is dirty, so saves and exports find clean data. The default.
    Background,
}

impl Priority {
    const ALL: [Self; 4] = [
        Self::Blocking,
        Self::Interactive,
        Self::Visible,
        Self::Background,
    ];

    fn index(self) -> usize {
        match self {
            Self::Blocking => 0,
            Self::Interactive => 1,
            Self::Visible => 2,
            Self::Background => 3,
        }
    }
}

/// A predicate that assigns its priority to the nodes it accepts ([`Scheduler::set_rule`]). It
/// runs under the scheduler's lock: it must be quick and must not call the scheduler.
pub type Rule = Arc<dyn Fn(&NodeKey) -> bool + Send + Sync>;

/// The default number of workers: the logical cores minus one, at least one; none on `wasm32`.
#[must_use]
pub fn default_workers() -> usize {
    if cfg!(target_arch = "wasm32") {
        0
    } else {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get().saturating_sub(1))
            .max(1)
    }
}

struct Running {
    /// The job's token, once it began.
    cancel: Option<Cancel>,
    /// The epoch up to which change sets were checked against the node's recorded inputs.
    checked: u64,
}

#[derive(Default)]
struct State {
    /// One queue per priority; an entry is valid only while `queued` maps its key to that
    /// priority, so re-prioritizing leaves a stale entry behind that `pop` skips.
    queues: [VecDeque<NodeKey>; 4],
    queued: HashMap<NodeKey, Priority>,
    running: HashMap<NodeKey, Running>,
    pinned: HashMap<NodeKey, Priority>,
    rules: [Option<Rule>; 4],
    /// Nodes that could not begin or whose evaluator unwound: left alone until the next
    /// change set.
    skipped: HashSet<NodeKey>,
    /// The graph's dirty nodes must be compared with the queue once it is empty.
    scan: bool,
    stopped: bool,
}

impl State {
    fn priority_of(&self, key: &NodeKey) -> Priority {
        if let Some(priority) = self.pinned.get(key) {
            return *priority;
        }
        Priority::ALL
            .into_iter()
            .find(|priority| {
                self.rules
                    .get(priority.index())
                    .and_then(Option::as_ref)
                    .is_some_and(|rule| rule(key))
            })
            .unwrap_or(Priority::Background)
    }

    /// Queues `key` at `priority` unless it runs or is queued at that priority or higher.
    fn enqueue(&mut self, key: NodeKey, priority: Priority, front: bool) {
        if self.stopped || self.running.contains_key(&key) {
            return;
        }
        if self
            .queued
            .get(&key)
            .is_some_and(|queued| *queued <= priority)
        {
            return;
        }
        self.queued.insert(key.clone(), priority);
        if let Some(queue) = self.queues.get_mut(priority.index()) {
            if front {
                queue.push_front(key);
            } else {
                queue.push_back(key);
            }
        }
        let entries: usize = self.queues.iter().map(VecDeque::len).sum();
        if entries > self.queued.len().saturating_mul(2).saturating_add(64) {
            self.compact();
        }
    }

    /// Drops the stale entries, keeping the order of the valid ones.
    fn compact(&mut self) {
        for priority in Priority::ALL {
            let queued = &self.queued;
            if let Some(queue) = self.queues.get_mut(priority.index()) {
                let mut seen = HashSet::new();
                queue.retain(|key| queued.get(key) == Some(&priority) && seen.insert(key.clone()));
            }
        }
    }

    fn pop(&mut self) -> Option<(NodeKey, Priority)> {
        for priority in Priority::ALL {
            while let Some(key) = self
                .queues
                .get_mut(priority.index())
                .and_then(VecDeque::pop_front)
            {
                if self.queued.get(&key) == Some(&priority) {
                    self.queued.remove(&key);
                    return Some((key, priority));
                }
            }
        }
        None
    }

    fn idle(&self) -> bool {
        self.queued.is_empty() && self.running.is_empty() && !self.scan
    }
}

struct Inner {
    graph: Graph,
    workers: usize,
    state: Mutex<State>,
    /// Wakes workers: work was queued, a scan is due, or the scheduler stopped.
    work: Condvar,
    /// Wakes waiters: a job ended or the queue ran dry.
    done: Condvar,
    /// Serializes draining transitions, so events and diagnostics follow the drain order.
    settle: Mutex<()>,
    diagnostics: Option<Arc<DiagnosticsList>>,
    emitter: Option<Emitter>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next job, marked running; waits for one when `wait`. `None` once stopped, or when
    /// there is nothing to do and `wait` is false.
    fn next_job(&self, wait: bool) -> Option<(NodeKey, Priority)> {
        let mut state = self.lock();
        loop {
            if state.stopped {
                return None;
            }
            if let Some((key, priority)) = state.pop() {
                state.running.insert(
                    key.clone(),
                    Running {
                        cancel: None,
                        checked: 0,
                    },
                );
                return Some((key, priority));
            }
            if state.scan {
                state.scan = false;
                for key in self.graph.dirty_keys() {
                    if !state.skipped.contains(&key) {
                        let priority = state.priority_of(&key);
                        state.enqueue(key, priority, false);
                    }
                }
                if state.queued.is_empty() {
                    self.done.notify_all();
                }
                continue;
            }
            if !wait {
                return None;
            }
            state = self
                .work
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Runs one job of `key`, which `next_job` or `ensure` marked running. A stale outcome is
    /// queued again when `requeue`.
    fn run(&self, key: &NodeKey, priority: Priority, requeue: bool) -> Result<Finish> {
        let job = match self.graph.begin(key) {
            Ok(job) => job,
            Err(error) => {
                let mut state = self.lock();
                state.running.remove(key);
                state.skipped.insert(key.clone());
                drop(state);
                self.done.notify_all();
                return Err(error);
            }
        };
        {
            let mut state = self.lock();
            let stopped = state.stopped;
            if let Some(running) = state.running.get_mut(key) {
                running.cancel = Some(job.cancel().clone());
                // Change sets applied between `begin` and now were not checked for this job.
                let (changed, now) = self.graph.inputs_changed_since(key, job.epoch());
                running.checked = now;
                if changed || stopped {
                    job.cancel().cancel();
                }
            }
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| job.evaluate()));
        let result = match outcome {
            Ok(outcome) => Ok(self.graph.finish(job, outcome)),
            Err(payload) => {
                drop(job);
                if let Some(list) = &self.diagnostics {
                    let objects = match key.subject {
                        Subject::Object(id) => vec![id],
                        Subject::Local(_) => Vec::<ObjectId>::new(),
                    };
                    list.deliver(&[Diagnostic::new(
                        Severity::Error,
                        panicked(key, payload.as_ref()),
                        objects,
                        Origin::Node(key.clone()),
                    )]);
                }
                Err(panicked(key, payload.as_ref()))
            }
        };
        self.settle();
        let mut state = self.lock();
        state.running.remove(key);
        state.scan = true;
        match &result {
            Ok(Finish::Stale) if requeue => {
                let dirty = self
                    .graph
                    .node(key)
                    .is_some_and(|view| view.state.kind() == StateKind::Dirty);
                if dirty {
                    let priority = priority.min(state.priority_of(key));
                    state.enqueue(key.clone(), priority, true);
                }
            }
            Ok(_) => {}
            Err(_) => {
                state.skipped.insert(key.clone());
            }
        }
        drop(state);
        self.work.notify_all();
        self.done.notify_all();
        result
    }

    /// Drains the graph's transitions: queues the nodes that became dirty, updates the node
    /// diagnostics and emits one `regenerated` event for the nodes that settled.
    fn settle(&self) {
        let _order = self.settle.lock().unwrap_or_else(PoisonError::into_inner);
        let transitions = self.graph.drain_transitions();
        if transitions.is_empty() {
            return;
        }
        {
            let mut state = self.lock();
            for transition in &transitions {
                if transition.after == Some(StateKind::Dirty)
                    && !state.skipped.contains(&transition.key)
                {
                    let priority = state.priority_of(&transition.key);
                    state.enqueue(transition.key.clone(), priority, false);
                }
            }
        }
        self.work.notify_all();
        let problem = |kind: Option<StateKind>| {
            matches!(
                kind,
                Some(StateKind::Failed | StateKind::Blocked | StateKind::Cyclic)
            )
        };
        if let Some(list) = &self.diagnostics {
            list.sync_nodes(
                &self.graph,
                transitions
                    .iter()
                    .filter(|t| problem(t.before) || problem(t.after))
                    .map(|t| &t.key),
            );
        }
        let clean: Vec<NodeKey> = transitions
            .iter()
            .filter(|t| t.after == Some(StateKind::Clean))
            .map(|t| t.key.clone())
            .collect();
        let failed: Vec<NodeKey> = transitions
            .iter()
            .filter(|t| problem(t.after))
            .map(|t| t.key.clone())
            .collect();
        if let Some(emit) = self
            .emitter
            .as_ref()
            .filter(|_| !clean.is_empty() || !failed.is_empty())
        {
            emit(&Event::Regenerated(RegeneratedEvent {
                revision: self.graph.revision(),
                clean: Ids::of(clean),
                failed: Ids::of(failed),
            }));
        }
    }

    /// After a change set: ends the diagnostics it outlives, cancels the running jobs it
    /// supersedes and queues the nodes it made dirty.
    fn on_commit(&self, changes: &ChangeSet) {
        if let Some(list) = &self.diagnostics {
            list.on_commit(changes);
        }
        {
            let mut state = self.lock();
            for (key, running) in &mut state.running {
                let Some(cancel) = running.cancel.as_ref().filter(|c| !c.is_cancelled()) else {
                    continue;
                };
                let (changed, now) = self.graph.inputs_changed_since(key, running.checked);
                running.checked = now;
                if changed {
                    cancel.cancel();
                }
            }
            state.skipped.clear();
            state.scan = true;
        }
        self.work.notify_all();
        self.settle();
    }
}

fn panicked(key: &NodeKey, payload: &(dyn std::any::Any + Send)) -> Error {
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Error::new(codes::GRAPH_EVALUATOR_PANICKED)
        .with_arg("node", key.to_string())
        .with_detail("panic", message)
}

fn stopped() -> Error {
    Error::new(codes::SCHEDULER_STOPPED)
}

/// The regeneration scheduler of one document. It is `Send + Sync`. Dropping it, or
/// [`Scheduler::shutdown`], cancels every running job and joins the workers.
pub struct Scheduler {
    inner: Arc<Inner>,
    #[cfg(not(target_arch = "wasm32"))]
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.lock();
        f.debug_struct("Scheduler")
            .field("workers", &self.inner.workers)
            .field("queued", &state.queued.len())
            .field("running", &state.running.len())
            .finish_non_exhaustive()
    }
}

impl Scheduler {
    /// Starts `workers` worker threads on `graph` (none on `wasm32`; with none, jobs run when
    /// pumped) and becomes the graph's observer. Node diagnostics go to `diagnostics`, which
    /// also receives each change set; `regenerated` events go to `emitter`. The graph's dirty
    /// nodes are queued at background priority.
    ///
    /// Fails with `doc.scheduler.stopped` when a thread cannot be started.
    pub fn start(
        graph: Graph,
        workers: usize,
        diagnostics: Option<Arc<DiagnosticsList>>,
        emitter: Option<Emitter>,
    ) -> Result<Self> {
        let workers = if cfg!(target_arch = "wasm32") {
            0
        } else {
            workers
        };
        let inner = Arc::new(Inner {
            graph: graph.clone(),
            workers,
            state: Mutex::new(State {
                scan: true,
                ..State::default()
            }),
            work: Condvar::new(),
            done: Condvar::new(),
            settle: Mutex::new(()),
            diagnostics,
            emitter,
        });
        let scheduler = Self {
            inner: Arc::clone(&inner),
            #[cfg(not(target_arch = "wasm32"))]
            threads: Mutex::new(Vec::new()),
        };
        #[cfg(not(target_arch = "wasm32"))]
        for n in 0..workers {
            let worker = Arc::clone(&inner);
            let handle = std::thread::Builder::new()
                .name(format!("arc-doc-regen-{n}"))
                .spawn(move || {
                    while let Some((key, priority)) = worker.next_job(true) {
                        // Errors are kept in the node's state or the diagnostics list.
                        drop(worker.run(&key, priority, true));
                    }
                })
                .map_err(|error| stopped().with_detail("reason", error.to_string()))?;
            scheduler
                .threads
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(handle);
        }
        let weak: Weak<Inner> = Arc::downgrade(&inner);
        graph.set_observer(Some(Arc::new(move |changes: &ChangeSet| {
            if let Some(inner) = weak.upgrade() {
                inner.on_commit(changes);
            }
        })));
        Ok(scheduler)
    }

    /// The number of worker threads.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.inner.workers
    }

    /// The graph it schedules.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.inner.graph
    }

    /// Gives `keys` the priority `priority` until [`Scheduler::unpin`], and queues those that
    /// are not clean ([`Scheduler::request`]).
    pub fn prioritize(&self, keys: impl IntoIterator<Item = NodeKey>, priority: Priority) {
        let keys: Vec<NodeKey> = keys.into_iter().collect();
        {
            let mut state = self.inner.lock();
            for key in &keys {
                state.pinned.insert(key.clone(), priority);
            }
        }
        self.request(keys);
    }

    /// Queues those of `keys` that are not clean, failed, blocked or cyclic, in order, each at
    /// its priority (pinned, from a rule, or background). A node that does not exist yet is
    /// created by its first evaluation.
    pub fn request(&self, keys: impl IntoIterator<Item = NodeKey>) {
        let pending: Vec<NodeKey> = keys
            .into_iter()
            .filter(|key| {
                self.inner
                    .graph
                    .node(key)
                    .is_none_or(|view| view.state.kind() == StateKind::Dirty)
            })
            .collect();
        let mut state = self.inner.lock();
        for key in pending {
            state.skipped.remove(&key);
            let priority = state.priority_of(&key);
            state.enqueue(key, priority, false);
        }
        drop(state);
        self.inner.work.notify_all();
    }

    /// Removes the priority [`Scheduler::prioritize`] gave `keys`; they fall back to the rules
    /// or to background priority.
    pub fn unpin(&self, keys: impl IntoIterator<Item = NodeKey>) {
        let mut state = self.inner.lock();
        for key in keys {
            state.pinned.remove(&key);
            if state.queued.contains_key(&key) {
                let priority = state.priority_of(&key);
                state.queued.remove(&key);
                state.enqueue(key, priority, false);
            }
        }
    }

    /// Sets the rule of `priority` (interactive or visible; any class is accepted), or removes
    /// it with `None`. A node not pinned by [`Scheduler::prioritize`] takes the highest priority
    /// whose rule accepts it, background otherwise. Queued nodes are re-classified, keeping
    /// their order. Costs time proportional to the queued nodes.
    pub fn set_rule(&self, priority: Priority, rule: Option<Rule>) {
        let mut state = self.inner.lock();
        if let Some(slot) = state.rules.get_mut(priority.index()) {
            *slot = rule;
        }
        let mut order: Vec<NodeKey> = Vec::new();
        for priority in Priority::ALL {
            let entries: Vec<NodeKey> = state
                .queues
                .get_mut(priority.index())
                .map(|queue| queue.drain(..).collect())
                .unwrap_or_default();
            order.extend(
                entries
                    .into_iter()
                    .filter(|key| state.queued.get(key) == Some(&priority)),
            );
        }
        state.queued.clear();
        for key in order {
            let priority = state.priority_of(&key);
            state.enqueue(key, priority, false);
        }
        drop(state);
        self.inner.work.notify_all();
    }

    /// The priority `key` is scheduled with when it is dirty.
    #[must_use]
    pub fn priority_of(&self, key: &NodeKey) -> Priority {
        self.inner.lock().priority_of(key)
    }

    /// Brings `key` to a settled state (clean, failed, blocked or cyclic) for the current
    /// revision and returns it, evaluating it on the calling thread at blocking priority ahead
    /// of all queued work. When a worker runs the node, waits for that job first. Call it
    /// outside evaluators: evaluators read nodes with [`crate::graph::EvalContext::node`].
    ///
    /// Fails like [`Graph::begin`], with `doc.graph.evaluator_panicked` when the evaluator
    /// unwinds, `doc.graph.stale` when commits supersede its results 16 times in a row, and
    /// `doc.scheduler.stopped` after [`Scheduler::shutdown`].
    pub fn ensure(&self, key: &NodeKey) -> Result<NodeView> {
        let inner = &self.inner;
        for _ in 0..ATTEMPTS {
            if let Some(view) = settled(&inner.graph, key) {
                return Ok(view);
            }
            let mut state = inner.lock();
            if state.stopped {
                return Err(stopped());
            }
            if state.running.contains_key(key) {
                if inner.workers > 0 {
                    drop(
                        inner
                            .done
                            .wait_while(state, |s| s.running.contains_key(key) && !s.stopped)
                            .unwrap_or_else(PoisonError::into_inner),
                    );
                    continue;
                }
                // Without workers the running job is on this thread: an evaluator called
                // `ensure`. Evaluate inline like a demand.
                drop(state);
                return inner.graph.evaluate_now(key);
            }
            state.queued.remove(key);
            state.running.insert(
                key.clone(),
                Running {
                    cancel: None,
                    checked: 0,
                },
            );
            drop(state);
            inner.run(key, Priority::Blocking, false)?;
        }
        settled(&inner.graph, key)
            .ok_or_else(|| Error::new(codes::GRAPH_STALE).with_arg("node", key.to_string()))
    }

    /// Runs queued jobs on the calling thread until there is nothing left to do, and returns
    /// how many ran. On `wasm32`, and with no workers, this is how jobs run.
    pub fn pump(&self) -> usize {
        let mut count = 0usize;
        while let Some((key, priority)) = self.inner.next_job(false) {
            // Errors are kept in the node's state or the diagnostics list.
            drop(self.inner.run(&key, priority, true));
            count = count.saturating_add(1);
        }
        count
    }

    /// True when no job is queued or running and every dirty node the scheduler knows about
    /// was looked at. Nodes that could not begin or whose evaluator unwound wait for the next
    /// change set and do not count.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.inner.lock().idle()
    }

    /// Waits until the scheduler is idle, at most `timeout`, and returns whether it is. With no
    /// workers it pumps instead of waiting.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        if self.inner.workers == 0 {
            self.pump();
            return self.is_idle();
        }
        let state = self.inner.lock();
        let (state, _) = self
            .inner
            .done
            .wait_timeout_while(state, timeout, |s| !s.idle() && !s.stopped)
            .unwrap_or_else(PoisonError::into_inner);
        state.idle()
    }

    /// Stops the scheduler: removes it as the graph's observer, cancels every running job,
    /// empties the queue and joins the workers (unless called from one). Calling it twice
    /// does nothing more.
    pub fn shutdown(&self) {
        self.inner.graph.set_observer(None);
        {
            let mut state = self.inner.lock();
            state.stopped = true;
            for running in state.running.values() {
                if let Some(cancel) = &running.cancel {
                    cancel.cancel();
                }
            }
            state.queued.clear();
            for queue in &mut state.queues {
                queue.clear();
            }
        }
        self.inner.work.notify_all();
        self.inner.done.notify_all();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handles: Vec<_> = self
                .threads
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..)
                .collect();
            let current = std::thread::current().id();
            for handle in handles {
                if handle.thread().id() != current {
                    // Jobs contain their unwinds, so a worker does not end in a panic.
                    drop(handle.join());
                }
            }
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn settled(graph: &Graph, key: &NodeKey) -> Option<NodeView> {
    graph
        .node(key)
        .filter(|view| !matches!(view.state.kind(), StateKind::Dirty | StateKind::Running))
}
