//! The dependency graph (CORE-DOC-029, CORE-DOC-030, CORE-DOC-034 and the stale-result rule of
//! CORE-DOC-032).
//!
//! A node is a derived key computed for an object or a graph-local identifier ([`NodeKey`]).
//! Products register a pure evaluator per derived key and object type in an [`Evaluators`]
//! value. An evaluator reads everything through its [`EvalContext`], which records every read as
//! a dependency: object existence, header, aspects, referrer sets and other nodes. A committed
//! [`ChangeSet`] marks the readers of the changed inputs dirty and, transitively, their
//! dependents; a dirty node is recomputed when demanded, and only when one of its recorded
//! inputs actually changed (early cutoff). An evaluation runs on one snapshot without holding
//! any lock ([`Graph::begin`], [`Job::evaluate`], [`Graph::finish`]); its result is applied only
//! if none of its inputs changed after the revision it read.
//!
//! Derived results live only in the graph: they are never journaled and are not part of the
//! snapshot.

mod change;
mod eval;

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arc_base::{Cancel, Error, ObjectId, Result};

pub use change::{ChangeSet, Input};
pub use eval::{EvalContext, Job, Outcome};

use crate::error::codes;
use crate::schema::{TypeKey, is_key};
use crate::snapshot::Snapshot;

/// The default limit of nested demands on one thread ([`Graph::with_depth_limit`]).
pub const DEFAULT_DEPTH_LIMIT: usize = 256;

/// How many times [`Graph::evaluate_now`] evaluates a node whose results keep being superseded
/// by concurrent commits before it gives up with `doc.graph.stale`.
const ATTEMPTS: usize = 16;

/// The name of a kind of derived data, in the key grammar of type keys, for example
/// `core.geometry.solid`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DerivedKey(Arc<str>);

impl DerivedKey {
    /// Validates `key`; a malformed key fails with `doc.graph.malformed_key`.
    pub fn new(key: &str) -> Result<Self> {
        if is_key(key) {
            Ok(Self(Arc::from(key)))
        } else {
            Err(Error::new(codes::GRAPH_MALFORMED_KEY).with_arg("key", key))
        }
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DerivedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a node is computed for: an object, or a graph-local identifier chosen by the product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subject {
    /// An object of the document.
    Object(ObjectId),
    /// A graph-local identifier.
    Local(u64),
}

/// A node of the dependency graph: a derived key of a subject.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey {
    /// The object or graph-local identifier.
    pub subject: Subject,
    /// The derived key.
    pub key: DerivedKey,
}

impl NodeKey {
    /// The node `key` of the object `id`.
    #[must_use]
    pub fn object(id: ObjectId, key: DerivedKey) -> Self {
        Self {
            subject: Subject::Object(id),
            key,
        }
    }

    /// The node `key` of the graph-local identifier `id`.
    #[must_use]
    pub fn local(id: u64, key: DerivedKey) -> Self {
        Self {
            subject: Subject::Local(id),
            key,
        }
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.subject {
            Subject::Object(id) => write!(f, "{id}/{}", self.key),
            Subject::Local(id) => write!(f, "local:{id}/{}", self.key),
        }
    }
}

/// An immutable derived result. Its content hash decides early cutoff: a recomputed result with
/// the hash of the previous one leaves the dependents of its node untouched.
///
/// The hash must be a function of the content only. Distinct contents are assumed not to
/// collide in 128 bits; [`content_hash`] gives that for accidental collisions (about `n² / 2¹²⁹`
/// for `n` results), not for contents chosen to collide, which at worst leave a dependent
/// showing a result of the previous content.
pub trait Derived: Any + Send + Sync + fmt::Debug {
    /// The 128-bit hash of the content.
    fn content_hash(&self) -> u128;
}

impl dyn Derived {
    /// The result as a `T`, if it is one.
    #[must_use]
    pub fn downcast_ref<T: Derived>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }
}

/// A shared derived result.
pub type DerivedValue = Arc<dyn Derived>;

/// A 128-bit hash of `value` for [`Derived::content_hash`]: two SipHash-1-3 hashes with fixed
/// keys and distinct prefixes. It is the same within one process run, which is all the graph
/// needs: results are never stored.
#[must_use]
pub fn content_hash<T: Hash + ?Sized>(value: &T) -> u128 {
    let half = |prefix: u8| {
        let mut hasher = DefaultHasher::new();
        prefix.hash(&mut hasher);
        value.hash(&mut hasher);
        hasher.finish()
    };
    (u128::from(half(0)) << 64) | u128::from(half(1))
}

/// A pure evaluator: computes a node's result from what it reads through the context.
pub type Evaluator = Arc<dyn Fn(&EvalContext<'_>) -> Result<DerivedValue> + Send + Sync>;

#[derive(Default)]
struct Registered {
    types: BTreeMap<TypeKey, Evaluator>,
    local: Option<Evaluator>,
}

/// The evaluators of a document, by derived key: per object type for object nodes, and one per
/// key for graph-local nodes. Like the type registry it is an explicit value, built once and
/// shared immutably.
#[derive(Default)]
pub struct Evaluators {
    keys: BTreeMap<DerivedKey, Registered>,
}

impl fmt::Debug for Evaluators {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.keys.keys()).finish()
    }
}

impl Evaluators {
    /// No evaluators.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the evaluator of `key` for objects of `type_key`. A second evaluator for the
    /// same pair fails with `doc.graph.duplicate_key` and changes nothing.
    pub fn register_object(
        &mut self,
        type_key: TypeKey,
        key: DerivedKey,
        evaluator: impl Fn(&EvalContext<'_>) -> Result<DerivedValue> + Send + Sync + 'static,
    ) -> Result<()> {
        let entry = self.keys.entry(key.clone()).or_default();
        if entry.types.contains_key(&type_key) {
            return Err(duplicate(&key).with_arg("type", type_key.as_str()));
        }
        entry.types.insert(type_key, Arc::new(evaluator));
        Ok(())
    }

    /// Registers the evaluator of `key` for graph-local nodes. A second one fails with
    /// `doc.graph.duplicate_key` and changes nothing.
    pub fn register_local(
        &mut self,
        key: DerivedKey,
        evaluator: impl Fn(&EvalContext<'_>) -> Result<DerivedValue> + Send + Sync + 'static,
    ) -> Result<()> {
        let entry = self.keys.entry(key.clone()).or_default();
        if entry.local.is_some() {
            return Err(duplicate(&key));
        }
        entry.local = Some(Arc::new(evaluator));
        Ok(())
    }

    fn get(&self, key: &DerivedKey, type_key: Option<&str>) -> Option<&Evaluator> {
        let entry = self.keys.get(key)?;
        match type_key {
            Some(type_key) => entry.types.get(type_key),
            None => entry.local.as_ref(),
        }
    }
}

fn duplicate(key: &DerivedKey) -> Error {
    Error::new(codes::GRAPH_DUPLICATE_KEY).with_arg("key", key.as_str())
}

/// The state of a node without its details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateKind {
    /// The result is valid for the current inputs.
    Clean,
    /// An input may have changed; the node waits for evaluation.
    Dirty,
    /// Dirty, with an evaluation in progress.
    Running,
    /// The evaluator returned an error.
    Failed,
    /// An input node is failed, cyclic or blocked.
    Blocked,
    /// The node takes part in a dependency cycle.
    Cyclic,
}

/// The state of a node.
#[derive(Debug, Clone)]
pub enum NodeState {
    /// The result is valid for the current inputs.
    Clean,
    /// An input may have changed; the node waits for evaluation.
    Dirty,
    /// Dirty, with an evaluation in progress.
    Running,
    /// The evaluator returned this error.
    Failed(Arc<Error>),
    /// Input nodes failed or are cyclic: the failed and cyclic nodes upstream, in key order.
    Blocked(Arc<[NodeKey]>),
    /// The node takes part in this dependency cycle, listed from the node where it was found.
    Cyclic(Arc<[NodeKey]>),
}

impl NodeState {
    /// The kind of the state.
    #[must_use]
    pub fn kind(&self) -> StateKind {
        match self {
            Self::Clean => StateKind::Clean,
            Self::Dirty => StateKind::Dirty,
            Self::Running => StateKind::Running,
            Self::Failed(_) => StateKind::Failed,
            Self::Blocked(_) => StateKind::Blocked,
            Self::Cyclic(_) => StateKind::Cyclic,
        }
    }
}

/// A node as it is now.
#[derive(Debug, Clone)]
pub struct NodeView {
    /// The node.
    pub key: NodeKey,
    /// The state.
    pub state: NodeState,
    /// The last good result, kept while the node is dirty, failed, blocked or cyclic.
    pub result: Option<DerivedValue>,
    /// The revision of the snapshot the last good result was computed from.
    pub revision: Option<u64>,
}

/// A change of a node's state since the last [`Graph::drain_transitions`]: the state before
/// and after, `None` for a node that did not exist or was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The node.
    pub key: NodeKey,
    /// The state at the previous drain.
    pub before: Option<StateKind>,
    /// The state now.
    pub after: Option<StateKind>,
}

/// What [`Graph::finish`] did with a job's outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// The outcome was applied: the node is settled (clean, failed, blocked or cyclic).
    Applied,
    /// The outcome was discarded: an input changed after the job's revision, the job was
    /// cancelled, the node was removed, or another job settled it first. The node keeps its
    /// state.
    Stale,
}

/// The settled outcome of a node.
#[derive(Debug, Clone)]
enum Settled {
    Ok(u128),
    Failed(Arc<Error>),
    Blocked(Arc<[NodeKey]>),
    Cyclic(Arc<[NodeKey]>),
}

impl Settled {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ok(a), Self::Ok(b)) => a == b,
            (Self::Failed(a), Self::Failed(b)) => {
                a.code() == b.code() && a.arguments() == b.arguments()
            }
            (Self::Blocked(a), Self::Blocked(b)) | (Self::Cyclic(a), Self::Cyclic(b)) => a == b,
            _ => false,
        }
    }
}

struct Node {
    /// The node may be out of date. Every dependent of a dirty node is dirty.
    dirty: bool,
    /// An object input recorded by the last evaluation changed since.
    inputs_changed: bool,
    /// Jobs begun and not finished.
    jobs: u32,
    settled: Option<Settled>,
    /// Changes whenever `settled` changes; dependents compare it.
    stamp: u64,
    result: Option<DerivedValue>,
    revision: Option<u64>,
    inputs: Vec<Input>,
    deps: Vec<(NodeKey, u64)>,
    dependents: HashSet<NodeKey>,
}

impl Node {
    fn new() -> Self {
        Self {
            dirty: true,
            inputs_changed: false,
            jobs: 0,
            settled: None,
            stamp: 0,
            result: None,
            revision: None,
            inputs: Vec::new(),
            deps: Vec::new(),
            dependents: HashSet::new(),
        }
    }

    fn kind(&self) -> StateKind {
        match (&self.settled, self.dirty) {
            (_, true) | (None, _) if self.jobs > 0 => StateKind::Running,
            (_, true) | (None, _) => StateKind::Dirty,
            (Some(Settled::Ok(_)), false) => StateKind::Clean,
            (Some(Settled::Failed(_)), false) => StateKind::Failed,
            (Some(Settled::Blocked(_)), false) => StateKind::Blocked,
            (Some(Settled::Cyclic(_)), false) => StateKind::Cyclic,
        }
    }

    fn view(&self, key: &NodeKey) -> NodeView {
        let state = match (self.kind(), &self.settled) {
            (StateKind::Clean, _) => NodeState::Clean,
            (StateKind::Running, _) => NodeState::Running,
            (StateKind::Failed, Some(Settled::Failed(error))) => NodeState::Failed(error.clone()),
            (StateKind::Blocked, Some(Settled::Blocked(up))) => NodeState::Blocked(up.clone()),
            (StateKind::Cyclic, Some(Settled::Cyclic(cycle))) => NodeState::Cyclic(cycle.clone()),
            _ => NodeState::Dirty,
        };
        NodeView {
            key: key.clone(),
            state,
            result: self.result.clone(),
            revision: self.revision,
        }
    }
}

/// The readers of the inputs of one object.
#[derive(Default)]
struct Readers {
    exists: HashSet<NodeKey>,
    header: HashSet<NodeKey>,
    referrers: HashSet<NodeKey>,
    aspects: HashMap<Arc<str>, HashSet<NodeKey>>,
}

impl Readers {
    fn is_empty(&self) -> bool {
        self.exists.is_empty()
            && self.header.is_empty()
            && self.referrers.is_empty()
            && self.aspects.is_empty()
    }
}

/// A settled node as a dependent observes it.
#[derive(Clone)]
struct Observed {
    stamp: u64,
    settled: Settled,
    result: Option<DerivedValue>,
}

struct State {
    snapshot: Snapshot,
    /// Counts applied change sets; jobs compare it, not the snapshot revision.
    epoch: u64,
    stamps: u64,
    nodes: HashMap<NodeKey, Node>,
    by_object: HashMap<ObjectId, HashSet<NodeKey>>,
    readers: HashMap<ObjectId, Readers>,
    dirty: BTreeSet<NodeKey>,
    problems: BTreeSet<NodeKey>,
    /// The change sets applied since the oldest running job began, by epoch.
    log: VecDeque<(u64, Arc<ChangeSet>)>,
    /// Running jobs by the epoch they began at.
    running: BTreeMap<u64, usize>,
    transitions: BTreeMap<NodeKey, (Option<StateKind>, Option<StateKind>)>,
}

impl State {
    fn kind(&self, key: &NodeKey) -> Option<StateKind> {
        self.nodes.get(key).map(Node::kind)
    }

    /// Updates the dirty and problem sets and the transitions after `key` changed from `before`.
    fn note(&mut self, key: &NodeKey, before: Option<StateKind>) {
        let after = self.kind(key);
        match after {
            Some(StateKind::Dirty | StateKind::Running) => {
                self.dirty.insert(key.clone());
                self.problems.remove(key);
            }
            Some(StateKind::Failed | StateKind::Blocked | StateKind::Cyclic) => {
                self.dirty.remove(key);
                self.problems.insert(key.clone());
            }
            Some(StateKind::Clean) | None => {
                self.dirty.remove(key);
                self.problems.remove(key);
            }
        }
        if before != after {
            let entry = self
                .transitions
                .entry(key.clone())
                .or_insert((before, after));
            entry.1 = after;
            if entry.0 == entry.1 {
                self.transitions.remove(key);
            }
        }
    }

    fn observe(&self, key: &NodeKey) -> Option<Observed> {
        let node = self.nodes.get(key)?;
        if node.dirty {
            return None;
        }
        Some(Observed {
            stamp: node.stamp,
            settled: node.settled.clone()?,
            result: node.result.clone(),
        })
    }

    /// Marks `starts` dirty (flagging changed object inputs when `inputs_changed`) and every
    /// dependent of a node that becomes dirty, with an explicit work list.
    fn mark_dirty(&mut self, starts: Vec<NodeKey>, inputs_changed: bool) {
        let mut work = starts;
        if inputs_changed {
            for key in &work {
                if let Some(node) = self.nodes.get_mut(key) {
                    node.inputs_changed = true;
                }
            }
        }
        while let Some(key) = work.pop() {
            let before = self.kind(&key);
            let Some(node) = self.nodes.get_mut(&key) else {
                continue;
            };
            if node.dirty {
                continue;
            }
            node.dirty = true;
            work.extend(node.dependents.iter().cloned());
            self.note(&key, before);
        }
    }

    fn readers_of(&mut self, id: ObjectId) -> &mut Readers {
        self.readers.entry(id).or_default()
    }

    fn register(&mut self, key: &NodeKey, input: &Input) {
        let set = match input {
            Input::Exists(id) => &mut self.readers_of(*id).exists,
            Input::Header(id) => &mut self.readers_of(*id).header,
            Input::Referrers(id) => &mut self.readers_of(*id).referrers,
            Input::Aspect(id, aspect) => self
                .readers_of(*id)
                .aspects
                .entry(aspect.clone())
                .or_default(),
        };
        set.insert(key.clone());
    }

    fn unregister(&mut self, key: &NodeKey, input: &Input) {
        let (Input::Exists(id) | Input::Header(id) | Input::Referrers(id) | Input::Aspect(id, _)) =
            input;
        let Some(readers) = self.readers.get_mut(id) else {
            return;
        };
        match input {
            Input::Exists(_) => readers.exists.remove(key),
            Input::Header(_) => readers.header.remove(key),
            Input::Referrers(_) => readers.referrers.remove(key),
            Input::Aspect(_, aspect) => {
                let removed = readers
                    .aspects
                    .get_mut(aspect)
                    .is_some_and(|set| set.remove(key));
                if readers.aspects.get(aspect).is_some_and(HashSet::is_empty) {
                    readers.aspects.remove(aspect);
                }
                removed
            }
        };
        if readers.is_empty() {
            self.readers.remove(id);
        }
    }

    /// Replaces the recorded dependencies of `key`.
    fn set_deps(&mut self, key: &NodeKey, inputs: Vec<Input>, deps: Vec<(NodeKey, u64)>) {
        let Some(node) = self.nodes.get_mut(key) else {
            return;
        };
        let old_inputs = std::mem::replace(&mut node.inputs, inputs.clone());
        let old_deps = std::mem::replace(&mut node.deps, deps.clone());
        for input in &old_inputs {
            self.unregister(key, input);
        }
        for (dep, _) in &old_deps {
            if let Some(node) = self.nodes.get_mut(dep) {
                node.dependents.remove(key);
            }
        }
        for input in &inputs {
            self.register(key, input);
        }
        for (dep, _) in &deps {
            if let Some(node) = self.nodes.get_mut(dep) {
                node.dependents.insert(key.clone());
            }
        }
    }

    /// Removes the node `key` and marks its dependents dirty.
    fn remove(&mut self, key: &NodeKey) {
        let before = self.kind(key);
        self.set_deps(key, Vec::new(), Vec::new());
        let Some(node) = self.nodes.remove(key) else {
            return;
        };
        if let Subject::Object(id) = key.subject
            && let Some(set) = self.by_object.get_mut(&id)
        {
            set.remove(key);
            if set.is_empty() {
                self.by_object.remove(&id);
            }
        }
        self.note(key, before);
        self.mark_dirty(node.dependents.into_iter().collect(), false);
    }

    fn release_epoch(&mut self, epoch: u64) {
        if let Some(count) = self.running.get_mut(&epoch) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.running.remove(&epoch);
            }
        }
        match self.running.keys().next() {
            Some(&oldest) => {
                while self.log.front().is_some_and(|(at, _)| *at <= oldest) {
                    self.log.pop_front();
                }
            }
            None => self.log.clear(),
        }
    }

    /// Ends one job of `key` without applying anything.
    fn release(&mut self, key: &NodeKey, epoch: u64) {
        self.release_epoch(epoch);
        let before = self.kind(key);
        if let Some(node) = self.nodes.get_mut(key) {
            node.jobs = node.jobs.saturating_sub(1);
        }
        self.note(key, before);
    }

    fn next_stamp(&mut self) -> u64 {
        self.stamps = self.stamps.saturating_add(1);
        self.stamps
    }
}

/// Called after each [`Graph::apply`], outside the graph's lock, with the applied change set.
pub type ApplyObserver = Arc<dyn Fn(&ChangeSet) + Send + Sync>;

/// The most nodes [`Graph::inputs_changed_since`] visits; beyond it the answer is `false`.
pub const SUPERSEDE_WALK_LIMIT: usize = 4_096;

struct Shared {
    evaluators: Arc<Evaluators>,
    depth_limit: usize,
    state: Mutex<State>,
    observer: Mutex<Option<ApplyObserver>>,
}

/// The dependency graph of one document: nodes, their recorded dependencies, states and
/// results.
///
/// It is `Send + Sync`; clones share one graph. One mutex guards its state; it is never held
/// while an evaluator runs.
#[derive(Clone)]
pub struct Graph(Arc<Shared>);

impl fmt::Debug for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("Graph")
            .field("revision", &state.snapshot.revision())
            .field("nodes", &state.nodes.len())
            .field("dirty", &state.dirty.len())
            .finish_non_exhaustive()
    }
}

impl Graph {
    /// An empty graph on `snapshot` with the evaluators of the document.
    #[must_use]
    pub fn new(evaluators: Arc<Evaluators>, snapshot: Snapshot) -> Self {
        Self::with_depth_limit(evaluators, snapshot, DEFAULT_DEPTH_LIMIT)
    }

    /// An empty graph whose nested demands on one thread are at most `depth_limit` deep.
    #[must_use]
    pub fn with_depth_limit(
        evaluators: Arc<Evaluators>,
        snapshot: Snapshot,
        depth_limit: usize,
    ) -> Self {
        Self(Arc::new(Shared {
            evaluators,
            depth_limit: depth_limit.max(1),
            state: Mutex::new(State {
                snapshot,
                epoch: 0,
                stamps: 0,
                nodes: HashMap::new(),
                by_object: HashMap::new(),
                readers: HashMap::new(),
                dirty: BTreeSet::new(),
                problems: BTreeSet::new(),
                log: VecDeque::new(),
                running: BTreeMap::new(),
                transitions: BTreeMap::new(),
            }),
            observer: Mutex::new(None),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The revision of the snapshot the graph is at.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.lock().snapshot.revision()
    }

    /// The number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().nodes.len()
    }

    /// True when the graph has no node.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Moves the graph to `snapshot`, the snapshot a commit published, with the commit's
    /// `changes` ([`ChangeSet::from_commit`]). The readers of every changed input become dirty,
    /// and so do their dependents, transitively. The nodes of objects created, deleted or
    /// recreated are removed and their dependents become dirty. Costs time proportional to the
    /// changed inputs and the nodes that become dirty.
    ///
    /// The observer set with [`Graph::set_observer`] is then called with `changes`, after the
    /// graph's lock is released.
    pub fn apply(&self, changes: &ChangeSet, snapshot: Snapshot) {
        self.apply_locked(changes, snapshot);
        let observer = self
            .0
            .observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(observer) = observer {
            observer(changes);
        }
    }

    fn apply_locked(&self, changes: &ChangeSet, snapshot: Snapshot) {
        let mut state = self.lock();
        state.snapshot = snapshot;
        state.epoch = state.epoch.saturating_add(1);
        if !state.running.is_empty() {
            let epoch = state.epoch;
            state.log.push_back((epoch, Arc::new(changes.clone())));
        }
        let mut starts = Vec::new();
        let mut removed = Vec::new();
        for (id, parts) in changes.objects() {
            let Some(readers) = state.readers.get(&id) else {
                if parts.is_none() {
                    removed.extend(state.by_object.get(&id).into_iter().flatten().cloned());
                }
                continue;
            };
            match parts {
                None => {
                    starts.extend(readers.exists.iter().cloned());
                    starts.extend(readers.header.iter().cloned());
                    starts.extend(readers.aspects.values().flatten().cloned());
                    removed.extend(state.by_object.get(&id).into_iter().flatten().cloned());
                }
                Some((header, aspects)) => {
                    if header {
                        starts.extend(readers.header.iter().cloned());
                    }
                    for aspect in aspects {
                        starts.extend(readers.aspects.get(aspect).into_iter().flatten().cloned());
                    }
                }
            }
        }
        for target in changes.referrer_targets() {
            if let Some(readers) = state.readers.get(&target) {
                starts.extend(readers.referrers.iter().cloned());
            }
        }
        state.mark_dirty(starts, true);
        for key in removed {
            state.remove(&key);
        }
    }

    /// Starts an evaluation of `key` on the graph's current snapshot: creates the node if
    /// needed and marks it running. The returned job evaluates without any lock
    /// ([`Job::evaluate`]); hand its outcome to [`Graph::finish`]. Dropping a job unfinished
    /// leaves the node dirty.
    ///
    /// Fails with `doc.object.not_found` for an object that is not live and
    /// `doc.graph.unknown_key` when no evaluator computes the key for the object's type.
    pub fn begin(&self, key: &NodeKey) -> Result<Job> {
        let mut state = self.lock();
        let snapshot = state.snapshot.clone();
        let epoch = state.epoch;
        self.begin_locked(&mut state, key, snapshot, epoch, Cancel::new())
    }

    fn begin_locked(
        &self,
        state: &mut State,
        key: &NodeKey,
        snapshot: Snapshot,
        epoch: u64,
        cancel: Cancel,
    ) -> Result<Job> {
        let type_key = match key.subject {
            Subject::Object(id) => {
                let handle = snapshot.resolve(id)?;
                let type_key = snapshot.type_key(handle).ok_or_else(|| {
                    Error::new(codes::OBJECT_NOT_FOUND).with_detail("id", id.to_string())
                })?;
                if !state.nodes.contains_key(key) && state.snapshot.resolve(id).is_err() {
                    // Live in the job's revision only: a node created now would outlive the
                    // object.
                    return Err(stale(key));
                }
                Some(type_key.as_str())
            }
            Subject::Local(_) => None,
        };
        let evaluator = self
            .0
            .evaluators
            .get(&key.key, type_key)
            .ok_or_else(|| {
                let error = Error::new(codes::GRAPH_UNKNOWN_KEY).with_arg("key", key.to_string());
                match type_key {
                    Some(type_key) => error.with_detail("type", type_key),
                    None => error,
                }
            })?
            .clone();
        let before = state.kind(key);
        if before.is_none() {
            state.nodes.insert(key.clone(), Node::new());
            if let Subject::Object(id) = key.subject {
                state.by_object.entry(id).or_default().insert(key.clone());
            }
        }
        let verify = state.nodes.get_mut(key).and_then(|node| {
            node.jobs = node.jobs.saturating_add(1);
            (node.settled.is_some() && !node.inputs_changed).then(|| node.deps.clone())
        });
        *state.running.entry(epoch).or_default() += 1;
        state.note(key, before);
        Ok(Job::new(
            self.clone(),
            key.clone(),
            snapshot,
            epoch,
            evaluator,
            cancel,
            verify,
        ))
    }

    /// Applies the outcome of `job` if it is still valid, under the graph's lock.
    ///
    /// A verified outcome cleans the node when it is still dirty, no object input it recorded
    /// changed and every node it depends on is clean with the result it had. An evaluated
    /// outcome is applied when the node is still dirty, no object input it read changed in a
    /// change set applied after the job began, and every node it read is settled with the
    /// result it read; the node becomes clean, failed, blocked or cyclic, its dependencies are
    /// replaced by the recorded reads, and its result stamp changes only if the outcome
    /// differs from the previous one (early cutoff). Otherwise, and for a cancelled job, the
    /// outcome is discarded and the node stays dirty.
    // The outcome is checked before the job releases its epoch: releasing the last running job
    // clears the change sets the check reads.
    pub fn finish(&self, mut job: Job, outcome: Outcome) -> Finish {
        let mut state = self.lock();
        job.done = true;
        let key = job.key.clone();
        let applied = state
            .nodes
            .get(&key)
            .filter(|node| node.dirty)
            .and_then(|_| outcome.into_settled(&state, &job));
        state.release_epoch(job.epoch);
        let before = state.kind(&key);
        let Some(node) = state.nodes.get_mut(&key) else {
            return Finish::Stale;
        };
        node.jobs = node.jobs.saturating_sub(1);
        let finish = match applied {
            Some(applied) => {
                apply(&mut state, &key, applied, job.snapshot.revision());
                Finish::Applied
            }
            None => Finish::Stale,
        };
        state.note(&key, before);
        finish
    }

    /// The node `key`, if it exists.
    #[must_use]
    pub fn node(&self, key: &NodeKey) -> Option<NodeView> {
        self.lock().nodes.get(key).map(|node| node.view(key))
    }

    /// Every node that is not clean (dirty, running, failed, blocked or cyclic), in key order.
    /// Costs time proportional to those nodes.
    pub fn non_clean(&self) -> impl Iterator<Item = NodeView> + use<> {
        let state = self.lock();
        let mut keys: Vec<&NodeKey> = state.dirty.iter().chain(&state.problems).collect();
        keys.sort();
        let views: Vec<NodeView> = keys
            .into_iter()
            .filter_map(|key| state.nodes.get(key).map(|node| node.view(key)))
            .collect();
        views.into_iter()
    }

    /// The state changes since the previous call, one per node in key order, from the state at
    /// the previous call to the state now; nodes back in their earlier state are not listed.
    /// What accumulates between calls is bounded by the number of nodes that changed.
    pub fn drain_transitions(&self) -> Vec<Transition> {
        std::mem::take(&mut self.lock().transitions)
            .into_iter()
            .map(|(key, (before, after))| Transition { key, before, after })
            .collect()
    }

    /// [`Graph::drain_transitions`] that also returns, under the same lock, the view of every
    /// node that is failed, blocked or cyclic after the drain. A consumer that mirrors those
    /// states reads them at the instant the baseline of the next drain is taken; reading them
    /// later lets a change that cancels out between two drains go unnoticed.
    pub fn drain_transitions_viewed(&self) -> Vec<(Transition, Option<NodeView>)> {
        let mut state = self.lock();
        std::mem::take(&mut state.transitions)
            .into_iter()
            .map(|(key, (before, after))| {
                let view = matches!(
                    after,
                    Some(StateKind::Failed | StateKind::Blocked | StateKind::Cyclic)
                )
                .then(|| state.nodes.get(&key).map(|node| node.view(&key)))
                .flatten();
                (Transition { key, before, after }, view)
            })
            .collect()
    }

    /// Brings `key` to a settled state for the current revision on this thread, evaluating
    /// what it needs, and returns it. Nested demands run inline on this thread.
    ///
    /// Fails like [`Graph::begin`], and with `doc.graph.stale` when concurrent commits
    /// supersede its results 16 times in a row.
    pub fn evaluate_now(&self, key: &NodeKey) -> Result<NodeView> {
        for _ in 0..ATTEMPTS {
            if let Some(view) = self.settled_view(key) {
                return Ok(view);
            }
            let job = self.begin(key)?;
            let outcome = job.evaluate();
            self.finish(job, outcome);
        }
        self.settled_view(key).ok_or_else(|| stale(key))
    }

    /// Evaluates every node that is dirty when called, in key order; nodes that become dirty
    /// meanwhile wait for the next call. Fails like [`Graph::evaluate_now`]; nodes removed
    /// meanwhile are skipped.
    pub fn evaluate_all_dirty(&self) -> Result<()> {
        let keys: Vec<NodeKey> = self.lock().dirty.iter().cloned().collect();
        for key in keys {
            if self.lock().nodes.contains_key(&key) {
                self.evaluate_now(&key)?;
            }
        }
        Ok(())
    }

    /// Sets the observer [`Graph::apply`] calls after each applied change set, or removes it.
    /// The regeneration scheduler and the diagnostics list of a document use it.
    pub fn set_observer(&self, observer: Option<ApplyObserver>) {
        *self
            .0
            .observer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = observer;
    }

    /// The number of change sets applied so far; a [`Job`] records it when it begins
    /// ([`Job::epoch`]).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// The nodes that are dirty without a job running for them, in key order. Costs time
    /// proportional to the dirty and running nodes.
    #[must_use]
    pub fn dirty_keys(&self) -> Vec<NodeKey> {
        let state = self.lock();
        state
            .dirty
            .iter()
            .filter(|key| state.kind(key) == Some(StateKind::Dirty))
            .cloned()
            .collect()
    }

    /// Whether a change set applied after `epoch` changes an input that `key`, or a node it
    /// depends on transitively, recorded in its last applied evaluation; also `true` when `key`
    /// no longer exists. Returns the answer with the current epoch, to pass on the next call.
    ///
    /// Only change sets applied while a job was running are kept, so `epoch` should be that of
    /// a running job ([`Job::epoch`]) or a later one. The walk visits at most
    /// [`SUPERSEDE_WALK_LIMIT`] nodes and answers `false` beyond it; a job's inputs are only
    /// known at finish, so the answer is a hint for cancelling running jobs, never a substitute
    /// for the check of [`Graph::finish`].
    #[must_use]
    pub fn inputs_changed_since(&self, key: &NodeKey, epoch: u64) -> (bool, u64) {
        let state = self.lock();
        let changes: Vec<&Arc<ChangeSet>> = state
            .log
            .iter()
            .filter(|(at, _)| *at > epoch)
            .map(|(_, changes)| changes)
            .collect();
        let now = state.epoch;
        if !state.nodes.contains_key(key) {
            return (true, now);
        }
        if changes.is_empty() {
            return (false, now);
        }
        let mut seen: HashSet<&NodeKey> = HashSet::new();
        let mut work = vec![key];
        while let Some(current) = work.pop() {
            if seen.len() >= SUPERSEDE_WALK_LIMIT {
                break;
            }
            if !seen.insert(current) {
                continue;
            }
            let Some(node) = state.nodes.get(current) else {
                continue;
            };
            if node
                .inputs
                .iter()
                .any(|input| changes.iter().any(|changes| changes.contains(input)))
            {
                return (true, now);
            }
            work.extend(node.deps.iter().map(|(dep, _)| dep));
        }
        (false, now)
    }

    fn settled_view(&self, key: &NodeKey) -> Option<NodeView> {
        let state = self.lock();
        let node = state.nodes.get(key)?;
        (!node.dirty && node.settled.is_some()).then(|| node.view(key))
    }
}

impl Job {
    /// The number of change sets the graph had applied when the job began ([`Graph::epoch`]).
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl ChangeSet {
    /// The objects the change set touches (created, deleted, recreated or modified), in
    /// identifier order.
    pub fn changed_objects(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.objects().map(|(id, _)| id)
    }
}

/// What an applied outcome writes into a node.
struct Applied {
    settled: Settled,
    result: Option<DerivedValue>,
    inputs: Vec<Input>,
    deps: Vec<(NodeKey, u64)>,
    /// A verification: the node keeps its result and dependencies.
    verified: bool,
}

fn apply(state: &mut State, key: &NodeKey, applied: Applied, revision: u64) {
    if !applied.verified {
        state.set_deps(key, applied.inputs, applied.deps);
    }
    let stamp = state.next_stamp();
    let Some(node) = state.nodes.get_mut(key) else {
        return;
    };
    node.dirty = false;
    node.inputs_changed = false;
    if applied.verified {
        return;
    }
    if !node
        .settled
        .as_ref()
        .is_some_and(|settled| settled.same(&applied.settled))
    {
        node.stamp = stamp;
    }
    node.settled = Some(applied.settled);
    if applied.result.is_some() {
        node.result = applied.result;
        node.revision = Some(revision);
    }
}

fn stale(key: &NodeKey) -> Error {
    Error::new(codes::GRAPH_STALE).with_arg("node", key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Number(i64);

    impl Derived for Number {
        fn content_hash(&self) -> u128 {
            content_hash(&self.0)
        }
    }

    #[test]
    fn derived_keys_follow_the_key_grammar() {
        assert!(DerivedKey::new("core.geometry.solid").is_ok());
        assert_eq!(
            DerivedKey::new("Solid").unwrap_err().code(),
            "doc.graph.malformed_key"
        );
    }

    #[test]
    fn results_downcast_and_hash_by_content() {
        let value: DerivedValue = Arc::new(Number(7));
        assert_eq!(value.downcast_ref::<Number>().map(|n| n.0), Some(7));
        assert_eq!(value.content_hash(), Number(7).content_hash());
        assert_ne!(value.content_hash(), Number(8).content_hash());
    }

    #[test]
    fn a_second_evaluator_for_a_key_fails() {
        let key = DerivedKey::new("test.value").unwrap();
        let mut evaluators = Evaluators::new();
        let evaluator = |_: &EvalContext<'_>| -> Result<DerivedValue> { Ok(Arc::new(Number(1))) };
        evaluators.register_local(key.clone(), evaluator).unwrap();
        assert_eq!(
            evaluators
                .register_local(key.clone(), evaluator)
                .unwrap_err()
                .code(),
            "doc.graph.duplicate_key"
        );
        let type_key = TypeKey::new("test.a").unwrap();
        evaluators
            .register_object(type_key.clone(), key.clone(), evaluator)
            .unwrap();
        assert!(
            evaluators
                .register_object(type_key, key, evaluator)
                .is_err()
        );
    }

    #[test]
    fn graphs_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<Graph>();
        check::<Job>();
    }
}
