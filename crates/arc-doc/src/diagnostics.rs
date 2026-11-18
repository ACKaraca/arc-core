//! The document diagnostics list (CORE-DOC-035): one bounded, versioned list per document of the
//! diagnostics of its four sources, with change events.
//!
//! - **Delivered** diagnostics (validator warnings of a commit, executor faults, evaluator
//!   panics) arrive through [`DiagnosticSink::deliver`]. One about objects lives until a later
//!   commit touches any of its objects (created, modified, deleted or recreated); the validators
//!   of that commit report again what still holds. One about no object lives until the next
//!   commit.
//! - **Node** diagnostics mirror the failed, blocked and cyclic nodes of the dependency graph:
//!   added when a node enters such a state, replaced when its state changes, removed when the
//!   node becomes clean, dirty or running, or is removed ([`DiagnosticsList::sync_nodes`]).
//! - **Load** diagnostics (preserved, quarantined, unaddressable, warnings, dangling references)
//!   come from the load result. One about an object lives while the object is held, that is
//!   until a commit touches it (a preserved or quarantined object can only be deleted); one
//!   about no object lives as long as the document.
//! - **Link** diagnostics report unresolved links to other documents. External links are a
//!   later phase: [`DiagnosticsList::set_link`] and [`DiagnosticsList::clear_link`] exist and
//!   nothing calls them yet.
//!
//! The list keeps insertion order. It holds at most its capacity of diagnostics: adding one
//! more drops the least severe, and among equals the oldest, diagnostic (possibly the new one),
//! and counts it ([`DiagnosticsSnapshot::dropped`]). Every change increments the version and
//! emits one `diagnostics_changed` event with the added and removed diagnostics; dropped
//! diagnostics count as removed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use arc_base::{Error, ObjectId};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Origin};
use crate::error::codes;
use crate::events::{DiagnosticsChanged, Event};
use crate::graph::{ChangeSet, Graph, NodeKey, NodeState, Subject};
use crate::schema::Severity;

/// The capacity of a list made by [`DiagnosticsList::default`].
pub const DEFAULT_DIAGNOSTICS_CAPACITY: usize = 10_000;

/// Receives the change events of a list.
pub type Emitter = Arc<dyn Fn(&Event) + Send + Sync>;

/// Which source a diagnostic of the list comes from; it decides its lifetime.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// Delivered through the [`DiagnosticSink`]: validators and faults.
    Delivered,
    /// The state of a node.
    Node(NodeKey),
    /// The load result.
    Load,
    /// An unresolved link, by the link object.
    Link(ObjectId),
}

/// An immutable copy of the list, cheap to clone and to hand to the user interface.
#[derive(Debug, Clone, Default)]
pub struct DiagnosticsSnapshot {
    /// The version of the list it copies.
    pub version: u64,
    /// The diagnostics, in insertion order.
    pub diagnostics: Arc<[Diagnostic]>,
    /// The number of diagnostics dropped by the capacity since the list was made.
    pub dropped: u64,
}

struct Entry {
    source: Source,
    diagnostic: Diagnostic,
}

#[derive(Default)]
struct State {
    next: u64,
    version: u64,
    dropped: u64,
    entries: BTreeMap<u64, Entry>,
    /// Eviction order: least severe first, then oldest.
    rank: BTreeSet<(u8, u64)>,
    by_object: HashMap<ObjectId, BTreeSet<u64>>,
    by_node: HashMap<NodeKey, u64>,
    snapshot: Option<DiagnosticsSnapshot>,
}

/// Higher for more severe.
fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Warning => 0,
        Severity::Error => 1,
    }
}

/// The added and removed diagnostics of one change.
#[derive(Default)]
struct Delta {
    added: Vec<Diagnostic>,
    removed: Vec<Diagnostic>,
}

impl State {
    fn insert(
        &mut self,
        capacity: usize,
        source: Source,
        diagnostic: Diagnostic,
        delta: &mut Delta,
    ) {
        let seq = self.next;
        self.next = self.next.saturating_add(1);
        let rank = (severity_rank(diagnostic.severity), seq);
        if self.entries.len() >= capacity {
            let lowest = self.rank.first().copied();
            match lowest {
                Some(lowest) if lowest < rank => {
                    if let Some(entry) = self.remove(lowest.1) {
                        delta.removed.push(entry.diagnostic);
                    }
                }
                _ => {
                    // The new diagnostic is the least severe and oldest: it is dropped.
                    self.dropped = self.dropped.saturating_add(1);
                    return;
                }
            }
            self.dropped = self.dropped.saturating_add(1);
        }
        for id in &diagnostic.objects {
            self.by_object.entry(*id).or_default().insert(seq);
        }
        if let Source::Node(key) = &source {
            self.by_node.insert(key.clone(), seq);
        }
        self.rank.insert(rank);
        delta.added.push(diagnostic.clone());
        self.entries.insert(seq, Entry { source, diagnostic });
    }

    fn remove(&mut self, seq: u64) -> Option<Entry> {
        let entry = self.entries.remove(&seq)?;
        self.rank
            .remove(&(severity_rank(entry.diagnostic.severity), seq));
        for id in &entry.diagnostic.objects {
            if let Some(set) = self.by_object.get_mut(id) {
                set.remove(&seq);
                if set.is_empty() {
                    self.by_object.remove(id);
                }
            }
        }
        if let Source::Node(key) = &entry.source
            && self.by_node.get(key) == Some(&seq)
        {
            self.by_node.remove(key);
        }
        Some(entry)
    }

    fn remove_where(&mut self, seqs: Vec<u64>, delta: &mut Delta) {
        for seq in seqs {
            if let Some(entry) = self.remove(seq) {
                delta.removed.push(entry.diagnostic);
            }
        }
    }
}

/// The diagnostics list of a document. It is `Send + Sync`; one mutex guards it and events are
/// emitted under it, so they arrive in the order of the versions.
pub struct DiagnosticsList {
    capacity: usize,
    emitter: Option<Emitter>,
    state: Mutex<State>,
}

impl fmt::Debug for DiagnosticsList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("DiagnosticsList")
            .field("version", &state.version)
            .field("len", &state.entries.len())
            .field("dropped", &state.dropped)
            .finish_non_exhaustive()
    }
}

impl Default for DiagnosticsList {
    fn default() -> Self {
        Self::new(DEFAULT_DIAGNOSTICS_CAPACITY, None)
    }
}

impl DiagnosticsList {
    /// An empty list of at most `capacity` diagnostics (at least one) that reports its changes
    /// to `emitter`.
    #[must_use]
    pub fn new(capacity: usize, emitter: Option<Emitter>) -> Self {
        Self {
            capacity: capacity.max(1),
            emitter,
            state: Mutex::new(State::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `change` and, when it changed the list, increments the version and emits the event.
    fn change(&self, change: impl FnOnce(&mut State, &mut Delta)) {
        let mut state = self.lock();
        let mut delta = Delta::default();
        let dropped = state.dropped;
        change(&mut state, &mut delta);
        if delta.added.is_empty() && delta.removed.is_empty() {
            if state.dropped != dropped {
                state.snapshot = None;
            }
            return;
        }
        state.version = state.version.saturating_add(1);
        state.snapshot = None;
        if let Some(emitter) = &self.emitter {
            emitter(&Event::DiagnosticsChanged(DiagnosticsChanged {
                added: delta.added,
                removed: delta.removed,
                overflow: false,
            }));
        }
    }

    /// The version: 0 for a list that never changed, incremented by every change.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// The number of diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    /// True when the list holds no diagnostic.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The list now. Repeated calls without a change in between share one allocation.
    #[must_use]
    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        let mut state = self.lock();
        if let Some(snapshot) = &state.snapshot {
            return snapshot.clone();
        }
        let snapshot = DiagnosticsSnapshot {
            version: state.version,
            diagnostics: state
                .entries
                .values()
                .map(|entry| entry.diagnostic.clone())
                .collect(),
            dropped: state.dropped,
        };
        state.snapshot = Some(snapshot.clone());
        snapshot
    }

    /// The diagnostics that name `id`, in list order.
    #[must_use]
    pub fn for_object(&self, id: ObjectId) -> Vec<Diagnostic> {
        let state = self.lock();
        state
            .by_object
            .get(&id)
            .into_iter()
            .flatten()
            .filter_map(|seq| state.entries.get(seq))
            .map(|entry| entry.diagnostic.clone())
            .collect()
    }

    /// The diagnostics of `origin`, in list order.
    #[must_use]
    pub fn for_origin(&self, origin: &Origin) -> Vec<Diagnostic> {
        let state = self.lock();
        if let Origin::Node(key) = origin {
            return state
                .by_node
                .get(key)
                .and_then(|seq| state.entries.get(seq))
                .map(|entry| entry.diagnostic.clone())
                .into_iter()
                .collect();
        }
        state
            .entries
            .values()
            .filter(|entry| entry.diagnostic.origin == *origin)
            .map(|entry| entry.diagnostic.clone())
            .collect()
    }

    /// The diagnostics of `source`, in list order.
    #[must_use]
    pub fn for_source(&self, source: &Source) -> Vec<Diagnostic> {
        self.lock()
            .entries
            .values()
            .filter(|entry| entry.source == *source)
            .map(|entry| entry.diagnostic.clone())
            .collect()
    }

    /// Adds the diagnostics of a load, in order.
    pub fn add_load(&self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        let capacity = self.capacity;
        self.change(|state, delta| {
            for diagnostic in diagnostics {
                state.insert(capacity, Source::Load, diagnostic, delta);
            }
        });
    }

    /// Replaces the diagnostics of the link `link` with `diagnostics` (none clears them).
    pub fn set_link(&self, link: ObjectId, diagnostics: Vec<Diagnostic>) {
        let capacity = self.capacity;
        self.change(|state, delta| {
            let old = seqs(state, |source| *source == Source::Link(link));
            state.remove_where(old, delta);
            for diagnostic in diagnostics {
                state.insert(capacity, Source::Link(link), diagnostic, delta);
            }
        });
    }

    /// Removes the diagnostics of the link `link`.
    pub fn clear_link(&self, link: ObjectId) {
        self.set_link(link, Vec::new());
    }

    /// Applies the lifetime rules of a commit: removes delivered diagnostics without objects,
    /// and delivered and load diagnostics that name an object `changes` touches. Costs time
    /// proportional to the touched objects and the diagnostics removed, plus the delivered
    /// diagnostics without objects.
    pub fn on_commit(&self, changes: &ChangeSet) {
        self.change(|state, delta| {
            let mut doomed: BTreeSet<u64> = BTreeSet::new();
            for id in changes.changed_objects() {
                if let Some(set) = state.by_object.get(&id) {
                    doomed.extend(set.iter().copied().filter(|seq| {
                        state.entries.get(seq).is_some_and(|entry| {
                            matches!(entry.source, Source::Delivered | Source::Load)
                        })
                    }));
                }
            }
            doomed.extend(state.entries.iter().filter_map(|(seq, entry)| {
                (entry.source == Source::Delivered && entry.diagnostic.objects.is_empty())
                    .then_some(*seq)
            }));
            state.remove_where(doomed.into_iter().collect(), delta);
        });
    }

    /// Brings the node diagnostics of `keys` in line with the nodes' current states in `graph`:
    /// a failed, blocked or cyclic node has exactly one diagnostic describing its state; any
    /// other node, or a removed one, has none.
    pub fn sync_nodes<'a>(&self, graph: &Graph, keys: impl IntoIterator<Item = &'a NodeKey>) {
        let wanted: Vec<(NodeKey, Option<Diagnostic>)> = keys
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .map(|key| (key.clone(), node_diagnostic(graph, key)))
            .collect();
        let capacity = self.capacity;
        self.change(|state, delta| {
            let mut wanted = wanted;
            wanted.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, diagnostic) in wanted {
                let current = state.by_node.get(&key).copied();
                let same = match (current.and_then(|seq| state.entries.get(&seq)), &diagnostic) {
                    (Some(entry), Some(new)) => same(&entry.diagnostic, new),
                    (None, None) => true,
                    _ => false,
                };
                if same {
                    continue;
                }
                if let Some(seq) = current {
                    state.remove_where(vec![seq], delta);
                }
                if let Some(diagnostic) = diagnostic {
                    state.insert(capacity, Source::Node(key), diagnostic, delta);
                }
            }
        });
    }
}

fn seqs(state: &State, keep: impl Fn(&Source) -> bool) -> Vec<u64> {
    state
        .entries
        .iter()
        .filter(|(_, entry)| keep(&entry.source))
        .map(|(seq, _)| *seq)
        .collect()
}

fn same(a: &Diagnostic, b: &Diagnostic) -> bool {
    a.severity == b.severity
        && a.code() == b.code()
        && a.error.arguments() == b.error.arguments()
        && a.error.details() == b.error.details()
        && a.objects == b.objects
        && a.origin == b.origin
}

fn object_of(key: &NodeKey) -> Option<ObjectId> {
    match key.subject {
        Subject::Object(id) => Some(id),
        Subject::Local(_) => None,
    }
}

fn names(keys: &[NodeKey]) -> String {
    keys.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The diagnostic of the node `key` in its current state: an error for a failed node (its
/// evaluator's error) and a cyclic node (`doc.graph.cycle` with the members), a warning for a
/// blocked one (`doc.graph.blocked` naming the upstream nodes). The objects are the node's
/// own, then those of the upstream nodes or cycle members, without repetition.
#[must_use]
pub fn node_diagnostic(graph: &Graph, key: &NodeKey) -> Option<Diagnostic> {
    let view = graph.node(key)?;
    let (severity, error, others) = match view.state {
        NodeState::Failed(error) => (Severity::Error, error, Vec::new()),
        NodeState::Blocked(upstream) => (
            Severity::Warning,
            Arc::new(
                Error::new(codes::GRAPH_BLOCKED)
                    .with_arg("node", key.to_string())
                    .with_detail("upstream", names(&upstream)),
            ),
            upstream.to_vec(),
        ),
        NodeState::Cyclic(members) => (
            Severity::Error,
            Arc::new(
                Error::new(codes::GRAPH_CYCLE)
                    .with_arg("node", key.to_string())
                    .with_arg("members", names(&members)),
            ),
            members.to_vec(),
        ),
        NodeState::Clean | NodeState::Dirty | NodeState::Running => return None,
    };
    let mut objects = Vec::new();
    for id in object_of(key)
        .into_iter()
        .chain(others.iter().filter_map(object_of))
    {
        if !objects.contains(&id) {
            objects.push(id);
        }
    }
    Some(Diagnostic {
        severity,
        error,
        objects,
        origin: Origin::Node(key.clone()),
    })
}

impl DiagnosticSink for DiagnosticsList {
    /// Adds the diagnostics as delivered ones, in order.
    fn deliver(&self, diagnostics: &[Diagnostic]) {
        let capacity = self.capacity;
        self.change(|state, delta| {
            for diagnostic in diagnostics {
                state.insert(capacity, Source::Delivered, diagnostic.clone(), delta);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;

    fn warning(objects: Vec<ObjectId>) -> Diagnostic {
        Diagnostic::new(
            Severity::Warning,
            Error::new(codes::VALUE_MISSING),
            objects,
            Origin::Validator,
        )
    }

    fn error(objects: Vec<ObjectId>) -> Diagnostic {
        Diagnostic::new(
            Severity::Error,
            Error::new(codes::VALUE_MISSING),
            objects,
            Origin::Validator,
        )
    }

    #[test]
    fn capacity_keeps_the_most_severe_and_newest_and_counts_the_dropped() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        let list = DiagnosticsList::new(
            2,
            Some(Arc::new(move |event: &Event| {
                sink.lock().unwrap().push(event.clone());
            })),
        );
        list.deliver(&[warning(vec![]), error(vec![])]);
        list.deliver(&[warning(vec![])]);
        let snapshot = list.snapshot();
        assert_eq!(snapshot.dropped, 1);
        let severities: Vec<Severity> = snapshot.diagnostics.iter().map(|d| d.severity).collect();
        assert_eq!(severities, vec![Severity::Error, Severity::Warning]);
        // An older error outranks a newer warning: the warning arriving now is the one dropped.
        list.deliver(&[error(vec![])]);
        let severities: Vec<Severity> = list
            .snapshot()
            .diagnostics
            .iter()
            .map(|d| d.severity)
            .collect();
        assert_eq!(severities, vec![Severity::Error, Severity::Error]);
        list.deliver(&[warning(vec![])]);
        assert_eq!(list.snapshot().dropped, 3);
        assert_eq!(list.version(), 3, "the dropped arrival changed nothing");
        assert_eq!(events.lock().unwrap().len(), 3);
    }

    #[test]
    fn snapshots_are_shared_until_a_change() {
        let list = DiagnosticsList::default();
        list.add_load([warning(vec![ObjectId::from_u128(1)])]);
        let a = list.snapshot();
        let b = list.snapshot();
        assert!(Arc::ptr_eq(&a.diagnostics, &b.diagnostics));
        list.clear_link(ObjectId::from_u128(9));
        assert_eq!(list.version(), 1, "clearing nothing is no change");
    }

    #[test]
    fn commits_end_delivered_and_load_diagnostics_of_touched_objects() {
        let (a, b) = (ObjectId::from_u128(1), ObjectId::from_u128(2));
        let list = DiagnosticsList::default();
        list.add_load([warning(vec![a]), warning(vec![])]);
        list.deliver(&[warning(vec![b]), warning(vec![])]);
        list.set_link(ObjectId::from_u128(3), vec![warning(vec![a])]);
        let mut changes = ChangeSet::new();
        changes.insert_aspect(a, "geometry");
        list.on_commit(&changes);
        // Load about a and the delivered one without objects are gone.
        assert_eq!(list.len(), 3);
        assert_eq!(list.for_object(a).len(), 1, "the link diagnostic stays");
        assert_eq!(list.for_source(&Source::Load).len(), 1);
        assert_eq!(list.for_object(b).len(), 1);
        list.clear_link(ObjectId::from_u128(3));
        assert!(list.for_object(a).is_empty());
    }
}
