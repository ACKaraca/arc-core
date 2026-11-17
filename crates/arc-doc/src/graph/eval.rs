//! Evaluation: jobs, the evaluation context that records reads, inline demand of other nodes and
//! cycle detection.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::sync::Arc;

use arc_base::{Cancel, Category, Error, ObjectId, Result};

use super::{Applied, Evaluator, Graph, Input, NodeKey, Settled, State, Subject, stale};
use crate::error::codes;
use crate::reverse_index::Referrers;
use crate::schema::{TypeKey, Value};
use crate::snapshot::{Header, Object, Snapshot};

/// One evaluation of a node on one snapshot, from [`Graph::begin`]. It is `Send`: a scheduler
/// may run [`Job::evaluate`] on any thread and hand the outcome to [`Graph::finish`].
pub struct Job {
    graph: Graph,
    pub(super) key: NodeKey,
    pub(super) snapshot: Snapshot,
    pub(super) epoch: u64,
    evaluator: Evaluator,
    cancel: Cancel,
    /// The node dependencies to verify before evaluating, with the stamps they had; `None`
    /// when an object input changed or the node has no outcome yet.
    verify: Option<Vec<(NodeKey, u64)>>,
    pub(super) done: bool,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("key", &self.key)
            .field("revision", &self.snapshot.revision())
            .finish_non_exhaustive()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        if !self.done {
            self.graph.lock().release(&self.key, self.epoch);
        }
    }
}

impl Job {
    pub(super) fn new(
        graph: Graph,
        key: NodeKey,
        snapshot: Snapshot,
        epoch: u64,
        evaluator: Evaluator,
        cancel: Cancel,
        verify: Option<Vec<(NodeKey, u64)>>,
    ) -> Self {
        Self {
            graph,
            key,
            snapshot,
            epoch,
            evaluator,
            cancel,
            verify,
            done: false,
        }
    }

    /// The node.
    #[must_use]
    pub fn key(&self) -> &NodeKey {
        &self.key
    }

    /// The revision of the snapshot the job reads.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.snapshot.revision()
    }

    /// The cancellation token of the job, shared with the nodes it demands. An evaluator
    /// observes it at its yield points; a cancelled outcome is discarded.
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// Runs the job without holding any graph lock.
    ///
    /// When the node is dirty only because nodes it depends on are, those nodes are brought up
    /// to date first; if none of them changed its result, the evaluator does not run and the
    /// outcome is a verification. Otherwise the evaluator runs. Nodes it demands are evaluated
    /// inline on this thread, on the same snapshot.
    #[must_use]
    pub fn evaluate(&self) -> Outcome {
        self.run(None)
    }

    fn run(&self, parent: Option<&Frame<'_>>) -> Outcome {
        let frame = Frame {
            job: self,
            parent,
            depth: parent.map_or(1, |parent| parent.depth.saturating_add(1)),
            reads: RefCell::new(BTreeSet::new()),
            nodes: RefCell::new(BTreeMap::new()),
            cycle: RefCell::new(None),
            blocked: RefCell::new(BTreeSet::new()),
            stale: Cell::new(false),
        };
        if let Some(deps) = &self.verify
            && deps
                .iter()
                .all(|(key, stamp)| frame.settle(key) == Some(*stamp))
        {
            return Outcome(Kind::Verified);
        }
        let result = (self.evaluator)(&EvalContext { frame: &frame });
        Outcome(Kind::Evaluated {
            result,
            inputs: frame.reads.into_inner(),
            nodes: frame.nodes.into_inner(),
            cycle: frame.cycle.into_inner(),
            blocked: frame.blocked.into_inner(),
            stale: frame.stale.get(),
        })
    }
}

/// The outcome of [`Job::evaluate`], for [`Graph::finish`].
pub struct Outcome(Kind);

enum Kind {
    Verified,
    Evaluated {
        result: Result<super::DerivedValue>,
        inputs: BTreeSet<Input>,
        nodes: BTreeMap<NodeKey, NodeRead>,
        cycle: Option<Arc<[NodeKey]>>,
        blocked: BTreeSet<NodeKey>,
        stale: bool,
    },
}

impl fmt::Debug for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Kind::Verified => f.write_str("Outcome::Verified"),
            Kind::Evaluated { result, .. } => f
                .debug_struct("Outcome::Evaluated")
                .field("ok", &result.is_ok())
                .finish_non_exhaustive(),
        }
    }
}

/// A node an evaluation read: the stamp it had, and whether the read must still hold at
/// finish (not for members of a cycle, which were being evaluated when read).
#[derive(Clone, Copy)]
pub(super) struct NodeRead {
    stamp: u64,
    check: bool,
}

impl Outcome {
    /// What to apply to the job's node, or `None` when the outcome is stale or cancelled.
    pub(super) fn into_settled(self, state: &State, job: &Job) -> Option<Applied> {
        let node = state.nodes.get(&job.key)?;
        let current = |key: &NodeKey, stamp: u64| {
            state
                .nodes
                .get(key)
                .is_some_and(|node| !node.dirty && node.stamp == stamp)
        };
        match self.0 {
            Kind::Verified => {
                let valid = !node.inputs_changed
                    && node.deps.iter().all(|(key, stamp)| current(key, *stamp));
                Some(Applied {
                    settled: node.settled.clone()?,
                    result: None,
                    inputs: Vec::new(),
                    deps: Vec::new(),
                    verified: true,
                })
                .filter(|_| valid)
            }
            Kind::Evaluated {
                result,
                inputs,
                nodes,
                cycle,
                blocked,
                stale,
            } => {
                let cancelled = result
                    .as_ref()
                    .is_err_and(|error| error.category() == Category::Cancelled);
                let changed = state
                    .log
                    .iter()
                    .filter(|(epoch, _)| *epoch > job.epoch)
                    .any(|(_, changes)| inputs.iter().any(|input| changes.contains(input)));
                let superseded = nodes
                    .iter()
                    .any(|(key, read)| read.check && !current(key, read.stamp));
                if stale || (cancelled && cycle.is_none()) || changed || superseded {
                    return None;
                }
                let (settled, result) = match (cycle, result) {
                    (Some(members), _) => (Settled::Cyclic(members), None),
                    (None, _) if !blocked.is_empty() => {
                        (Settled::Blocked(blocked.into_iter().collect()), None)
                    }
                    (None, Ok(value)) => (Settled::Ok(value.content_hash()), Some(value)),
                    (None, Err(error)) => (Settled::Failed(Arc::new(error)), None),
                };
                Some(Applied {
                    settled,
                    result,
                    inputs: inputs.into_iter().collect(),
                    deps: nodes
                        .into_iter()
                        .map(|(key, read)| (key, read.stamp))
                        .collect(),
                    verified: false,
                })
            }
        }
    }
}

/// One evaluation on the stack of a thread.
struct Frame<'a> {
    job: &'a Job,
    parent: Option<&'a Frame<'a>>,
    /// 1 for the outermost frame.
    depth: usize,
    reads: RefCell<BTreeSet<Input>>,
    nodes: RefCell<BTreeMap<NodeKey, NodeRead>>,
    cycle: RefCell<Option<Arc<[NodeKey]>>>,
    blocked: RefCell<BTreeSet<NodeKey>>,
    stale: Cell<bool>,
}

impl Frame<'_> {
    fn on_stack(&self, key: &NodeKey) -> bool {
        let mut frame = Some(self);
        while let Some(current) = frame {
            if current.job.key == *key {
                return true;
            }
            frame = current.parent;
        }
        false
    }

    /// Brings `key` to a settled state and returns it, evaluating it inline on the job's
    /// snapshot when needed; `Ok(None)` when its outcome was stale.
    fn ensure(&self, key: &NodeKey) -> Result<Option<super::Observed>> {
        let graph = &self.job.graph;
        let job = {
            let mut state = graph.lock();
            if let Some(observed) = state.observe(key) {
                return Ok(Some(observed));
            }
            graph.begin_locked(
                &mut state,
                key,
                self.job.snapshot.clone(),
                self.job.epoch,
                self.job.cancel.clone(),
            )?
        };
        let outcome = job.run(Some(self));
        graph.finish(job, outcome);
        Ok(graph.lock().observe(key))
    }

    /// The stamp of `key` once settled, for verification; `None` when it cannot be settled
    /// here (on the stack, too deep, stale or failing to begin).
    fn settle(&self, key: &NodeKey) -> Option<u64> {
        if self.on_stack(key) || self.depth >= self.job.graph.0.depth_limit {
            return None;
        }
        self.ensure(key)
            .ok()
            .flatten()
            .map(|observed| observed.stamp)
    }

    fn read(&self, input: Input) {
        self.reads.borrow_mut().insert(input);
    }

    fn read_node(&self, key: &NodeKey, read: NodeRead) {
        let mut nodes = self.nodes.borrow_mut();
        match nodes.get(key) {
            Some(earlier) if earlier.check && read.check && earlier.stamp != read.stamp => {
                self.stale.set(true);
            }
            Some(_) => {}
            None => {
                nodes.insert(key.clone(), read);
            }
        }
    }

    /// Marks every frame from this one up to the frame of `key` as members of the cycle and
    /// returns its members, from the frame of `key` down to this one.
    fn close_cycle(&self, key: &NodeKey) -> Arc<[NodeKey]> {
        let mut path = Vec::new();
        let mut frame = Some(self);
        while let Some(current) = frame {
            path.push(current);
            if current.job.key == *key {
                break;
            }
            frame = current.parent;
        }
        let members: Arc<[NodeKey]> = path
            .iter()
            .rev()
            .map(|frame| frame.job.key.clone())
            .collect();
        for frame in path {
            frame
                .cycle
                .borrow_mut()
                .get_or_insert_with(|| members.clone());
        }
        members
    }
}

/// What an evaluator reads the document and other nodes through. Every read is recorded as a
/// dependency of the node being evaluated: reads of an object's existence or type, header and
/// field aspects, of the referrers of an object, and of other nodes. A read of an object that
/// is not live is a dependency on its existence.
///
/// Reads see the snapshot of the job's revision. An evaluator calls
/// [`EvalContext::yield_point`] at least every 50 ms of computation.
pub struct EvalContext<'a> {
    frame: &'a Frame<'a>,
}

impl fmt::Debug for EvalContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvalContext")
            .field("node", &self.frame.job.key)
            .finish_non_exhaustive()
    }
}

impl<'a> EvalContext<'a> {
    /// The node being evaluated.
    #[must_use]
    pub fn key(&self) -> &NodeKey {
        &self.frame.job.key
    }

    /// The object of the node, for an object node.
    #[must_use]
    pub fn object_id(&self) -> Option<ObjectId> {
        match self.frame.job.key.subject {
            Subject::Object(id) => Some(id),
            Subject::Local(_) => None,
        }
    }

    /// The revision of the snapshot the evaluation reads.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.frame.job.snapshot.revision()
    }

    /// The cancellation token of the evaluation.
    #[must_use]
    pub fn cancel(&self) -> &Cancel {
        &self.frame.job.cancel
    }

    /// Returns `base.cancelled` once the evaluation is cancelled; a cancelled evaluation's
    /// outcome is discarded.
    pub fn yield_point(&self) -> Result<()> {
        self.frame.job.cancel.check()
    }

    fn live(&self, id: ObjectId) -> Option<(&'a TypeKey, &'a Arc<Object>)> {
        let snapshot = &self.frame.job.snapshot;
        let handle = snapshot.resolve(id).ok()?;
        Some((snapshot.type_key(handle)?, snapshot.get(handle).ok()?))
    }

    fn live_or_record(&self, id: ObjectId) -> Result<(&'a TypeKey, &'a Arc<Object>)> {
        self.live(id).ok_or_else(|| {
            self.frame.read(Input::Exists(id));
            Error::new(codes::OBJECT_NOT_FOUND).with_detail("id", id.to_string())
        })
    }

    /// Whether `id` is live. Depends on its existence.
    #[must_use]
    pub fn exists(&self, id: ObjectId) -> bool {
        self.frame.read(Input::Exists(id));
        self.live(id).is_some()
    }

    /// The type key of `id`, if live. Depends on its existence.
    #[must_use]
    pub fn type_key(&self, id: ObjectId) -> Option<&'a TypeKey> {
        self.frame.read(Input::Exists(id));
        self.live(id).map(|(type_key, _)| type_key)
    }

    /// The header of `id`. Depends on its header, or on its existence when it is not live
    /// (`doc.object.not_found`).
    pub fn header(&self, id: ObjectId) -> Result<&'a Header> {
        let (_, object) = self.live_or_record(id)?;
        self.frame.read(Input::Header(id));
        Ok(&object.header)
    }

    /// The value of the object field `field` of `id`, `None` when absent. Depends on the aspect
    /// of the field, or on the object's existence when it is not live (`doc.object.not_found`)
    /// or has no such field (`doc.path.invalid`).
    pub fn field(&self, id: ObjectId, field: &str) -> Result<Option<&'a Value>> {
        let (type_key, object) = self.live_or_record(id)?;
        let position = self
            .frame
            .job
            .snapshot
            .registry()
            .get(type_key.as_str())
            .and_then(|decl| decl.schema(object.version.major))
            .and_then(|schema| {
                schema
                    .fields
                    .iter()
                    .position(|decl| decl.name == field)
                    .map(|position| (position, schema))
            });
        let Some((position, schema)) = position else {
            self.frame.read(Input::Exists(id));
            return Err(Error::new(codes::PATH_INVALID).with_arg("path", field));
        };
        if let Some(decl) = schema.fields.get(position) {
            self.frame
                .read(Input::Aspect(id, Arc::from(decl.aspect.as_str())));
        }
        Ok(object.values.get(position).and_then(Option::as_ref))
    }

    /// The referrers of `target` (source and field), in the order of the reverse index.
    /// Depends on the set of references to `target`, which changes when any object gains or
    /// loses a reference to it.
    pub fn referrers(&self, target: ObjectId) -> Referrers<'a> {
        self.frame.read(Input::Referrers(target));
        self.frame.job.snapshot.referrers(target)
    }

    /// The result of the node `key`, evaluated inline first when it is not settled. Depends
    /// on that node.
    ///
    /// Fails with `doc.graph.cycle` when `key` is being evaluated on this thread (every node
    /// of the cycle becomes cyclic), `doc.graph.blocked` when it is failed, blocked or cyclic
    /// (this node becomes blocked, whatever the evaluator returns),
    /// `doc.graph.depth_exceeded` beyond the graph's depth limit, `doc.graph.stale` when its
    /// evaluation was superseded (this outcome is then discarded), and with the code of
    /// [`Graph::begin`] when it cannot begin (a missing object is a dependency on its
    /// existence).
    pub fn node(&self, key: &NodeKey) -> Result<super::DerivedValue> {
        let frame = self.frame;
        if frame.on_stack(key) {
            let stamp = frame
                .job
                .graph
                .lock()
                .nodes
                .get(key)
                .map_or(0, |node| node.stamp);
            frame.read_node(
                key,
                NodeRead {
                    stamp,
                    check: false,
                },
            );
            let members = frame.close_cycle(key);
            let names: Vec<String> = members.iter().map(ToString::to_string).collect();
            return Err(Error::new(codes::GRAPH_CYCLE)
                .with_arg("node", key.to_string())
                .with_arg("members", names.join(", ")));
        }
        let limit = frame.job.graph.0.depth_limit;
        if frame.depth >= limit {
            return Err(Error::new(codes::GRAPH_DEPTH_EXCEEDED)
                .with_arg("limit", i64::try_from(limit).unwrap_or(i64::MAX)));
        }
        let observed = match frame.ensure(key) {
            Ok(Some(observed)) => observed,
            Ok(None) => {
                frame.stale.set(true);
                return Err(stale(key));
            }
            Err(error) => {
                if error.code() == codes::OBJECT_NOT_FOUND.code
                    && let Subject::Object(id) = key.subject
                {
                    frame.read(Input::Exists(id));
                } else if error.code() == codes::GRAPH_STALE.code {
                    frame.stale.set(true);
                }
                return Err(error);
            }
        };
        frame.read_node(
            key,
            NodeRead {
                stamp: observed.stamp,
                check: true,
            },
        );
        let upstream: Vec<NodeKey> = match &observed.settled {
            Settled::Ok(_) => return observed.result.ok_or_else(|| stale(key)),
            Settled::Failed(_) | Settled::Cyclic(_) => vec![key.clone()],
            Settled::Blocked(roots) => roots.to_vec(),
        };
        let names: HashSet<String> = upstream.iter().map(ToString::to_string).collect();
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        frame.blocked.borrow_mut().extend(upstream);
        Err(Error::new(codes::GRAPH_BLOCKED)
            .with_arg("node", key.to_string())
            .with_detail("upstream", names.join(", ")))
    }
}
