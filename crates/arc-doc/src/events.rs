//! Document events (CORE-DOC-036): the six event kinds of the document model, delivered to
//! subscribers through bounded queues that coalesce for slow subscribers.
//!
//! The [`EventBus`] never blocks the emitter and never holds more than a bounded number of
//! events per subscriber: a subscriber's queue holds at most its capacity of events; when it is
//! full, further events go to an overflow of at most one event per kind, where an event merges
//! into the pending one of its kind ([`Event::merge`]). Once the overflow holds an event, every
//! later event joins the overflow too, so a subscriber receives events in emission order: the
//! queue first, then the overflow in the order its kinds first overflowed. A subscriber that is
//! dropped is removed at the next emission. Handlers receive [`Snapshot`] values, which are
//! immutable: they read the document and issue commands to change it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use arc_base::{ObjectId, TransactionId};

use crate::diagnostic::Diagnostic;
use crate::graph::NodeKey;
use crate::schema::TypeKey;
use crate::snapshot::{Change, Snapshot};
use crate::txn::TxnKind;

/// The capacity of a subscription made by [`EventBus::subscribe`].
pub const DEFAULT_CAPACITY: usize = 64;

/// The most identifiers (objects, nodes or conflict records) an event lists; a merge that
/// would list more turns the set into "everything" ([`Ids::All`], [`Changes::All`]).
pub const MAX_COALESCED_IDS: usize = 65_536;

/// The most added plus removed diagnostics a [`DiagnosticsChanged`] event lists; a merge that
/// would list more drops the lists and sets [`DiagnosticsChanged::overflow`].
pub const MAX_COALESCED_DIAGNOSTICS: usize = 1_024;

/// A set of identifiers, or "everything" when it grew beyond [`MAX_COALESCED_IDS`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ids<T: Ord> {
    /// These identifiers.
    Listed(BTreeSet<T>),
    /// Any identifier may be concerned: re-read what the subscriber needs.
    All,
}

impl<T: Ord> Default for Ids<T> {
    fn default() -> Self {
        Self::Listed(BTreeSet::new())
    }
}

impl<T: Ord> Ids<T> {
    /// The set of `ids`, or `All` beyond [`MAX_COALESCED_IDS`].
    #[must_use]
    pub fn of(ids: impl IntoIterator<Item = T>) -> Self {
        let set: BTreeSet<T> = ids
            .into_iter()
            .take(MAX_COALESCED_IDS.saturating_add(1))
            .collect();
        Self::bounded(set)
    }

    fn bounded(set: BTreeSet<T>) -> Self {
        if set.len() > MAX_COALESCED_IDS {
            Self::All
        } else {
            Self::Listed(set)
        }
    }

    /// Adds every identifier of `other`.
    pub fn merge(&mut self, other: Self) {
        match (std::mem::replace(self, Self::All), other) {
            (Self::Listed(mut a), Self::Listed(b)) => {
                a.extend(b);
                *self = Self::bounded(a);
            }
            _ => *self = Self::All,
        }
    }
}

/// The identifiers of one type changed by commits, by change kind ([`Change`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeGroups {
    /// Created objects.
    pub created: BTreeSet<ObjectId>,
    /// Modified objects.
    pub modified: BTreeSet<ObjectId>,
    /// Deleted objects.
    pub deleted: BTreeSet<ObjectId>,
    /// Objects deleted and created again (the type is the new one).
    pub recreated: BTreeSet<ObjectId>,
}

impl ChangeGroups {
    fn len(&self) -> usize {
        self.created.len() + self.modified.len() + self.deleted.len() + self.recreated.len()
    }

    fn group(&mut self, change: Change) -> &mut BTreeSet<ObjectId> {
        match change {
            Change::Created => &mut self.created,
            Change::Modified => &mut self.modified,
            Change::Deleted => &mut self.deleted,
            Change::Recreated => &mut self.recreated,
        }
    }
}

/// The objects committed transactions changed, grouped by type key and change kind; or
/// "everything" when more than [`MAX_COALESCED_IDS`] would be listed. After coalescing, an
/// object may be listed under several kinds (created, then modified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changes {
    /// The changed objects by type key. A deleted object is listed under the type it had.
    Listed(BTreeMap<TypeKey, ChangeGroups>),
    /// Any object may have changed.
    All,
}

impl Changes {
    /// The changes of one commit: each touched identifier under its type key in the candidate,
    /// or in the base for a deleted one. Costs time proportional to the touched identifiers.
    #[must_use]
    pub fn of_commit(
        base: &Snapshot,
        candidate: &Snapshot,
        touched: &BTreeMap<ObjectId, Change>,
    ) -> Self {
        if touched.len() > MAX_COALESCED_IDS {
            return Self::All;
        }
        let mut by_type: BTreeMap<TypeKey, ChangeGroups> = BTreeMap::new();
        for (&id, &change) in touched {
            let from = if change == Change::Deleted {
                base
            } else {
                candidate
            };
            let key = from
                .resolve(id)
                .ok()
                .and_then(|handle| from.type_key(handle));
            if let Some(key) = key {
                by_type
                    .entry(key.clone())
                    .or_default()
                    .group(change)
                    .insert(id);
            }
        }
        Self::Listed(by_type)
    }

    /// The number of listed identifiers; `None` for `All`.
    #[must_use]
    pub fn count(&self) -> Option<usize> {
        match self {
            Self::Listed(by_type) => Some(by_type.values().map(ChangeGroups::len).sum()),
            Self::All => None,
        }
    }

    /// Adds the changes of `other`.
    pub fn merge(&mut self, other: &Self) {
        let (Self::Listed(mine), Self::Listed(theirs)) = (&mut *self, other) else {
            *self = Self::All;
            return;
        };
        for (key, groups) in theirs {
            let into = mine.entry(key.clone()).or_default();
            into.created.extend(&groups.created);
            into.modified.extend(&groups.modified);
            into.deleted.extend(&groups.deleted);
            into.recreated.extend(&groups.recreated);
        }
        if self.count().is_some_and(|n| n > MAX_COALESCED_IDS) {
            *self = Self::All;
        }
    }
}

/// A transaction was committed (or, coalesced, several were).
#[derive(Debug, Clone)]
pub struct CommittedEvent {
    /// The revision published; the latest one when coalesced.
    pub revision: u64,
    /// The transaction; the latest one when coalesced.
    pub txn_id: TransactionId,
    /// Its kind; the latest one's when coalesced.
    pub kind: TxnKind,
    /// The changed objects; the union when coalesced.
    pub changes: Arc<Changes>,
    /// The number of commits this event covers: 1, or more when coalesced.
    pub commits: u64,
    /// The published snapshot of `revision`, to read from.
    pub snapshot: Snapshot,
}

/// Nodes of the dependency graph settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegeneratedEvent {
    /// The revision the results belong to; the latest one when coalesced.
    pub revision: u64,
    /// Nodes that became clean.
    pub clean: Ids<NodeKey>,
    /// Nodes that became failed, blocked or cyclic.
    pub failed: Ids<NodeKey>,
}

/// Diagnostics were added to or removed from the document diagnostics list.
#[derive(Debug, Clone)]
pub struct DiagnosticsChanged {
    /// Added diagnostics, in order.
    pub added: Vec<Diagnostic>,
    /// Removed diagnostics, in order.
    pub removed: Vec<Diagnostic>,
    /// True when coalescing dropped the lists: re-read the whole diagnostics list.
    pub overflow: bool,
}

/// The document was saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEvent {
    /// Where it was saved.
    pub path: String,
    /// The package format version written.
    pub format_version: u32,
    /// The revision saved.
    pub revision: u64,
}

/// The state of a link to another document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkState {
    /// The linked document is loaded and current.
    Loaded,
    /// The linked document is not loaded, by its load mode.
    NotLoaded,
    /// The linked document is missing or cannot be loaded.
    Unresolved,
    /// The linked document changed on disk since it was loaded.
    Outdated,
}

/// Links changed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStateChanged {
    /// The new state of each link; the latest state per link when coalesced. The number of
    /// entries is bounded by the link objects of the document.
    pub states: BTreeMap<ObjectId, LinkState>,
}

/// Conflict records were recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRecorded {
    /// The conflict records.
    pub conflicts: Ids<ObjectId>,
}

/// The kind of an [`Event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EventKind {
    /// [`Event::Committed`].
    Committed,
    /// [`Event::Regenerated`].
    Regenerated,
    /// [`Event::DiagnosticsChanged`].
    DiagnosticsChanged,
    /// [`Event::Saved`].
    Saved,
    /// [`Event::LinkStateChanged`].
    LinkStateChanged,
    /// [`Event::ConflictRecorded`].
    ConflictRecorded,
}

/// A document event.
#[derive(Debug, Clone)]
pub enum Event {
    /// A transaction was committed.
    Committed(CommittedEvent),
    /// Nodes settled.
    Regenerated(RegeneratedEvent),
    /// The diagnostics list changed.
    DiagnosticsChanged(DiagnosticsChanged),
    /// The document was saved.
    Saved(SavedEvent),
    /// Links changed state.
    LinkStateChanged(LinkStateChanged),
    /// Conflict records were recorded.
    ConflictRecorded(ConflictRecorded),
}

impl Event {
    /// The kind.
    #[must_use]
    pub fn kind(&self) -> EventKind {
        match self {
            Self::Committed(_) => EventKind::Committed,
            Self::Regenerated(_) => EventKind::Regenerated,
            Self::DiagnosticsChanged(_) => EventKind::DiagnosticsChanged,
            Self::Saved(_) => EventKind::Saved,
            Self::LinkStateChanged(_) => EventKind::LinkStateChanged,
            Self::ConflictRecorded(_) => EventKind::ConflictRecorded,
        }
    }

    /// Merges the later event `newer` of the same kind into this one: the latest revision,
    /// transaction, snapshot, save or link state, and the union of what changed, bounded by
    /// [`MAX_COALESCED_IDS`] and [`MAX_COALESCED_DIAGNOSTICS`]. An event of another kind is
    /// handed back.
    pub fn merge(&mut self, newer: Self) -> Result<(), Self> {
        match (self, newer) {
            (Self::Committed(old), Self::Committed(new)) => {
                Arc::make_mut(&mut old.changes).merge(&new.changes);
                old.revision = new.revision;
                old.txn_id = new.txn_id;
                old.kind = new.kind;
                old.snapshot = new.snapshot;
                old.commits = old.commits.saturating_add(new.commits);
            }
            (Self::Regenerated(old), Self::Regenerated(new)) => {
                old.revision = new.revision;
                old.clean.merge(new.clean);
                old.failed.merge(new.failed);
            }
            (Self::DiagnosticsChanged(old), Self::DiagnosticsChanged(new)) => {
                let total =
                    old.added.len() + old.removed.len() + new.added.len() + new.removed.len();
                if old.overflow || new.overflow || total > MAX_COALESCED_DIAGNOSTICS {
                    *old = DiagnosticsChanged {
                        added: Vec::new(),
                        removed: Vec::new(),
                        overflow: true,
                    };
                } else {
                    old.added.extend(new.added);
                    old.removed.extend(new.removed);
                }
            }
            (Self::Saved(old), Self::Saved(new)) => *old = new,
            (Self::LinkStateChanged(old), Self::LinkStateChanged(new)) => {
                old.states.extend(new.states);
            }
            (Self::ConflictRecorded(old), Self::ConflictRecorded(new)) => {
                old.conflicts.merge(new.conflicts);
            }
            (_, newer) => return Err(newer),
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Pending {
    queue: VecDeque<Event>,
    /// At most one event per kind, in the order the kinds first overflowed.
    overflow: Vec<Event>,
    closed: bool,
}

#[derive(Debug)]
struct Queue {
    capacity: usize,
    pending: Mutex<Pending>,
    ready: Condvar,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, event: Event) {
        let mut pending = self.lock();
        if pending.overflow.is_empty() && pending.queue.len() < self.capacity {
            pending.queue.push_back(event);
        } else {
            let kind = event.kind();
            match pending.overflow.iter_mut().find(|e| e.kind() == kind) {
                Some(slot) => {
                    // Same kind by construction, so the merge always succeeds.
                    if let Err(event) = slot.merge(event) {
                        *slot = event;
                    }
                }
                None => pending.overflow.push(event),
            }
        }
        drop(pending);
        self.ready.notify_one();
    }
}

fn pop(pending: &mut Pending) -> Option<Event> {
    pending
        .queue
        .pop_front()
        .or_else(|| (!pending.overflow.is_empty()).then(|| pending.overflow.remove(0)))
}

/// A subscription to the events of a document. Dropping it unsubscribes.
#[derive(Debug)]
pub struct Subscription {
    queue: Arc<Queue>,
}

impl Subscription {
    /// The next event, waiting for one; `None` once the bus is gone and every event was read.
    #[must_use]
    pub fn recv(&self) -> Option<Event> {
        let mut pending = self.queue.lock();
        loop {
            if let Some(event) = pop(&mut pending) {
                return Some(event);
            }
            if pending.closed {
                return None;
            }
            pending = self
                .queue
                .ready
                .wait(pending)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// The next event, waiting at most `timeout`.
    #[must_use]
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        let pending = self.queue.lock();
        let (mut pending, _) = self
            .queue
            .ready
            .wait_timeout_while(pending, timeout, |p| {
                p.queue.is_empty() && p.overflow.is_empty() && !p.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        pop(&mut pending)
    }

    /// The next event if one is waiting.
    #[must_use]
    pub fn try_recv(&self) -> Option<Event> {
        pop(&mut self.queue.lock())
    }
}

/// The subscribers of one document's events.
#[derive(Debug, Default)]
pub struct EventBus {
    subscribers: Mutex<Vec<Weak<Queue>>>,
}

impl EventBus {
    /// A bus without subscribers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Weak<Queue>>> {
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Subscribes with a queue of [`DEFAULT_CAPACITY`] events.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.subscribe_with_capacity(DEFAULT_CAPACITY)
    }

    /// Subscribes with a queue of `capacity` events (at least one) before coalescing starts.
    #[must_use]
    pub fn subscribe_with_capacity(&self, capacity: usize) -> Subscription {
        let queue = Arc::new(Queue {
            capacity: capacity.max(1),
            pending: Mutex::new(Pending::default()),
            ready: Condvar::new(),
        });
        self.lock().push(Arc::downgrade(&queue));
        Subscription { queue }
    }

    /// The number of live subscriptions.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.lock()
            .iter()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    /// Delivers `event` to every subscriber without blocking on any of them, and removes
    /// subscriptions that were dropped.
    pub fn emit(&self, event: &Event) {
        let mut subscribers = self.lock();
        subscribers.retain(|weak| match weak.upgrade() {
            Some(queue) => {
                queue.push(event.clone());
                true
            }
            None => false,
        });
    }
}

impl Drop for EventBus {
    fn drop(&mut self) {
        for queue in self.lock().iter().filter_map(Weak::upgrade) {
            queue.lock().closed = true;
            queue.ready.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;

    fn saved(revision: u64) -> Event {
        Event::Saved(SavedEvent {
            path: "a".into(),
            format_version: 1,
            revision,
        })
    }

    fn conflicts(ids: impl IntoIterator<Item = u128>) -> Event {
        Event::ConflictRecorded(ConflictRecorded {
            conflicts: Ids::of(ids.into_iter().map(ObjectId::from_u128)),
        })
    }

    #[test]
    fn full_queue_coalesces_by_kind_in_order() {
        let bus = EventBus::new();
        let sub = bus.subscribe_with_capacity(1);
        bus.emit(&saved(1));
        bus.emit(&conflicts([1]));
        bus.emit(&saved(2));
        bus.emit(&conflicts([2]));
        bus.emit(&saved(3));
        assert!(matches!(
            sub.try_recv(),
            Some(Event::Saved(SavedEvent { revision: 1, .. }))
        ));
        match sub.try_recv() {
            Some(Event::ConflictRecorded(c)) => {
                assert_eq!(c.conflicts, Ids::of([1, 2].map(ObjectId::from_u128)))
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            sub.try_recv(),
            Some(Event::Saved(SavedEvent { revision: 3, .. }))
        ));
        assert!(sub.try_recv().is_none());
    }

    #[test]
    fn ids_beyond_the_bound_become_all() {
        let mut ids = Ids::of(0..MAX_COALESCED_IDS as u128);
        assert!(matches!(ids, Ids::Listed(_)));
        ids.merge(Ids::of([u128::MAX]));
        assert_eq!(ids, Ids::All);
        assert_eq!(Ids::of(0..=MAX_COALESCED_IDS as u128), Ids::All);
    }

    #[test]
    fn diagnostics_overflow_drops_lists() {
        use crate::diagnostic::Origin;
        use crate::schema::Severity;
        let d = Diagnostic::new(
            Severity::Warning,
            arc_base::Error::new(crate::error::codes::VALUE_MISSING),
            vec![],
            Origin::Validator,
        );
        let event = |n| {
            Event::DiagnosticsChanged(DiagnosticsChanged {
                added: vec![d.clone(); n],
                removed: vec![],
                overflow: false,
            })
        };
        let mut a = event(MAX_COALESCED_DIAGNOSTICS);
        assert!(a.merge(event(1)).is_ok());
        assert!(matches!(
            a,
            Event::DiagnosticsChanged(DiagnosticsChanged { overflow: true, .. })
        ));
        assert!(a.merge(saved(1)).is_err());
    }

    #[test]
    fn dropped_subscriber_is_removed_and_closing_ends_recv() {
        let bus = EventBus::new();
        let kept = bus.subscribe();
        drop(bus.subscribe());
        bus.emit(&saved(1));
        assert_eq!(bus.subscribers(), 1);
        assert_eq!(bus.lock().len(), 1);
        drop(bus);
        assert!(kept.recv().is_some());
        assert!(kept.recv().is_none());
    }
}
