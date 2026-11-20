//! The handle of an open document: the latest snapshot, the dependency graph, the executor, the
//! event bus, the document validators, the journal and undo history, the journal hook, the
//! diagnostics list and the regeneration scheduler.
//!
//! Storage and the package are not part of this handle; the recovery journal of the package
//! attaches as a journal hook ([`Document::set_journal`]).
//!
//! Locks: readers take none (the snapshot cell is lock-free). The executor reads the hooks
//! (a read lock, released at once by cloning them) and then takes the history lock (journal and
//! undo manager) only for the journal step of a commit, the report to the undo manager and the
//! preparation of an undo, redo or rollback; it holds no other lock of the document meanwhile,
//! and the graph and the scheduler never take it. The history lock is therefore a leaf, and no
//! lock order between it, the snapshot cell, the graph and the scheduler arises.

use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockWriteGuard};
use std::time::Duration;

use arc_base::id::{IdGenerator, Identifier};
use arc_base::{Limits, Result, SessionId, TransactionId};

use crate::commit::JournalSink;
use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::diagnostics::{
    DEFAULT_DIAGNOSTICS_CAPACITY, DiagnosticsList, DiagnosticsSnapshot, Emitter,
};
use crate::events::{Event, EventBus, Subscription};
use crate::executor::{
    Completed, Core, DryRun, Executor, Open, Pending, Report, SubmitOptions, lock, task_on,
};
use crate::graph::{Evaluators, Graph, NodeKey, NodeView};
use crate::history::HistoryQuery;
use crate::journal::Journal;
use crate::load::{LoadCounts, Loaded, LoadedRows, StoredRecord};
use crate::record::TxnRecord;
use crate::registry::TypeRegistry;
use crate::scheduler::{Priority, Scheduler, default_workers};
use crate::snapshot::{Snapshot, SnapshotCell};
use crate::txn::{Txn, TxnKind, TxnLimits};
#[cfg(feature = "package")]
use crate::undo::prepare;
use crate::undo::{UndoLimits, UndoManager};
use crate::validate::DocumentValidator;

/// The settings of a [`Document`].
#[derive(Debug)]
pub struct DocumentConfig {
    /// The generator of transaction identifiers. Applications pass a generator on their clock
    /// and entropy; the default is seeded, so identifiers repeat between documents.
    pub ids: IdGenerator,
    /// The session recorded in every transaction.
    pub session: SessionId,
    /// The limits of each transaction.
    pub txn_limits: TxnLimits,
    /// The limits of the deletion rules at commit (`record_count` bounds cascades).
    pub limits: Limits,
    /// The most regeneration jobs of this document the process-wide worker pool runs at once
    /// (the pool grows to it, within its bound); with none, regeneration runs when pumped
    /// ([`Scheduler::pump`]). Ignored on `wasm32`, which has none.
    pub workers: usize,
    /// The most diagnostics the diagnostics list holds.
    pub diagnostics_capacity: usize,
    /// The bounds of the undo stack.
    pub undo_limits: UndoLimits,
}

impl Default for DocumentConfig {
    /// A seeded generator (seed 0) and the session it generates first; default limits;
    /// [`default_workers`] workers; [`DEFAULT_DIAGNOSTICS_CAPACITY`] diagnostics.
    fn default() -> Self {
        let mut ids = IdGenerator::seeded(0);
        let session = ids.generate();
        Self {
            ids,
            session,
            txn_limits: TxnLimits::default(),
            limits: Limits::DEFAULT,
            workers: default_workers(),
            diagnostics_capacity: DEFAULT_DIAGNOSTICS_CAPACITY,
            undo_limits: UndoLimits::default(),
        }
    }
}

/// The hooks a commit reads; replaced as a whole by the setters of [`Document`].
#[derive(Clone)]
pub(crate) struct Hooks {
    pub(crate) validators: Arc<[DocumentValidator]>,
    pub(crate) journal: Option<Arc<dyn JournalSink>>,
    pub(crate) diagnostics: Option<Arc<dyn DiagnosticSink>>,
}

/// The journal and the undo manager of a document, behind one lock.
pub(crate) struct History {
    pub(crate) journal: Journal,
    pub(crate) undo: UndoManager,
    /// The limits the journal's records encode within.
    pub(crate) limits: Limits,
}

impl History {
    /// Cuts the history after a failed journal append: the journal and both undo stacks start
    /// over empty, since later records could not name their parent in it.
    pub(crate) fn cut(&mut self) {
        self.journal = Journal::new(self.limits);
        self.cut_undo();
    }

    /// Cuts the undo history: both stacks start over empty.
    pub(crate) fn cut_undo(&mut self) {
        self.undo = UndoManager::new(self.undo.limits());
    }
}

/// The state the document handle and its executor share.
pub(crate) struct Shared {
    pub(crate) cell: SnapshotCell,
    pub(crate) graph: Graph,
    pub(crate) events: EventBus,
    pub(crate) hooks: RwLock<Hooks>,
    pub(crate) history: Mutex<History>,
}

impl Shared {
    pub(crate) fn new(snapshot: Snapshot, evaluators: Arc<Evaluators>) -> Self {
        Self {
            graph: Graph::new(evaluators, snapshot.clone()),
            cell: SnapshotCell::new(snapshot),
            events: EventBus::new(),
            hooks: RwLock::new(Hooks {
                validators: Arc::from(Vec::new()),
                journal: None,
                diagnostics: None,
            }),
            history: Mutex::new(History {
                journal: Journal::default(),
                undo: UndoManager::default(),
                limits: Limits::DEFAULT,
            }),
        }
    }
}

/// The diagnostics sink of a document: the diagnostics list, then the sink set with
/// [`Document::set_diagnostics`].
struct Sinks {
    list: Arc<DiagnosticsList>,
    extra: Option<Arc<dyn DiagnosticSink>>,
}

impl DiagnosticSink for Sinks {
    fn deliver(&self, diagnostics: &[Diagnostic]) {
        self.list.deliver(diagnostics);
        if let Some(extra) = &self.extra {
            extra.deliver(diagnostics);
        }
    }
}

/// An open document.
///
/// Readers call [`Document::snapshot`] on any thread, without the executor and without locks.
/// Changes are jobs submitted with [`Document::submit`], which run one at a time, in submission
/// order, on the document's executor ([`crate::executor`]). The handle is `Send + Sync`; share it
/// in an `Arc` between submitting threads. Dropping it, or [`Document::close`], stops the
/// executor after the running job and the regeneration scheduler.
pub struct Document {
    shared: Arc<Shared>,
    executor: Executor,
    scheduler: Scheduler,
    diagnostics: Arc<DiagnosticsList>,
    load_diagnostics: Vec<Diagnostic>,
    /// The load this document came from, without its snapshot and diagnostics: what a save
    /// needs beyond the snapshot. Empty for a document never loaded.
    loaded: Loaded,
    /// Where the document was last loaded from or saved to (see `crate::package`).
    #[cfg(feature = "package")]
    pub(crate) saved: Mutex<Option<crate::package::Baseline>>,
    /// The journal records the package holds as far as the document knows (loaded, then as
    /// each save retained them), and the revision of the last save: the journal's records of
    /// later revisions are not stored yet.
    pub(crate) stored: Mutex<(Vec<StoredRecord>, u64)>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("snapshot", &self.snapshot())
            .finish_non_exhaustive()
    }
}

impl Document {
    /// An empty document of `registry`, with the default configuration. Fails with
    /// `doc.executor.stopped` when the executor thread cannot be started.
    pub fn new(registry: Arc<TypeRegistry>, evaluators: Arc<Evaluators>) -> Result<Self> {
        Self::open(
            Snapshot::empty(registry),
            evaluators,
            DocumentConfig::default(),
        )
    }

    /// A document on `snapshot` with `config`.
    pub fn open(
        snapshot: Snapshot,
        evaluators: Arc<Evaluators>,
        config: DocumentConfig,
    ) -> Result<Self> {
        Self::open_after(snapshot, evaluators, config, None)
    }

    /// [`Document::open`] whose first transaction names `last` as its parent.
    fn open_after(
        snapshot: Snapshot,
        evaluators: Arc<Evaluators>,
        config: DocumentConfig,
        last: Option<TransactionId>,
    ) -> Result<Self> {
        let revision = snapshot.revision();
        let shared = Arc::new(Shared::new(snapshot, evaluators));
        *lock(&shared.history) = History {
            journal: Journal::new(config.limits),
            undo: UndoManager::new(config.undo_limits),
            limits: config.limits,
        };
        let weak = Arc::downgrade(&shared);
        let emitter: Emitter = Arc::new(move |event: &Event| {
            if let Some(shared) = weak.upgrade() {
                shared.events.emit(event);
            }
        });
        let diagnostics = Arc::new(DiagnosticsList::new(
            config.diagnostics_capacity,
            Some(Arc::clone(&emitter)),
        ));
        shared
            .hooks
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .diagnostics = Some(Arc::new(Sinks {
            list: Arc::clone(&diagnostics),
            extra: None,
        }));
        let scheduler = Scheduler::start(
            shared.graph.clone(),
            config.workers,
            Some(Arc::clone(&diagnostics)),
            Some(emitter),
        )?;
        let base = Loaded {
            snapshot: Snapshot::empty(Arc::clone(shared.cell.load().registry())),
            diagnostics: Vec::new(),
            types: Vec::new(),
            rows: LoadedRows::default(),
            counts: LoadCounts::default(),
        };
        let executor = Executor::start(Core {
            shared: Arc::clone(&shared),
            ids: config.ids,
            session: config.session,
            last,
            txn_limits: config.txn_limits,
            limits: config.limits,
        })?;
        Ok(Self {
            shared,
            executor,
            scheduler,
            diagnostics,
            load_diagnostics: Vec::new(),
            loaded: base,
            #[cfg(feature = "package")]
            saved: Mutex::new(None),
            stored: Mutex::new((Vec::new(), revision)),
        })
    }

    /// The document of a load result, with the default configuration: its snapshot (read-only
    /// when it lists reasons), its diagnostics and its original rows.
    pub fn from_loaded(loaded: Loaded, evaluators: Arc<Evaluators>) -> Result<Self> {
        Self::from_loaded_with(loaded, evaluators, DocumentConfig::default())
    }

    /// The document of a load result with `config`.
    pub fn from_loaded_with(
        loaded: Loaded,
        evaluators: Arc<Evaluators>,
        config: DocumentConfig,
    ) -> Result<Self> {
        let Loaded {
            snapshot,
            diagnostics,
            types,
            rows,
            counts,
        } = loaded;
        // The first transaction of the session is applied on the last stored one.
        let last = rows
            .journal()
            .last()
            .and_then(|record| <[u8; 16]>::try_from(record.txn_id.as_slice()).ok())
            .map(|bytes| TransactionId::from_u128(u128::from_be_bytes(bytes)))
            .filter(|id| id.as_u128() != 0);
        let mut document = Self::open_after(snapshot, evaluators, config, last)?;
        lock(&document.stored).0 = rows.journal().to_vec();
        document.load_diagnostics = diagnostics.into_iter().map(Diagnostic::from).collect();
        document
            .diagnostics
            .add_load(document.load_diagnostics.iter().cloned());
        document.loaded.types = types;
        document.loaded.rows = rows;
        document.loaded.counts = counts;
        Ok(document)
    }

    /// The latest published snapshot, in constant time, on any thread. Read-only queries use it
    /// directly; they never go through the executor.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.shared.cell.load()
    }

    /// The type registry.
    #[must_use]
    pub fn registry(&self) -> Arc<TypeRegistry> {
        Arc::clone(self.snapshot().registry())
    }

    /// True when the document is read-only ([`Snapshot::read_only`]); every commit with
    /// operations then fails with `doc.document.read_only`.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        !self.snapshot().read_only().is_empty()
    }

    /// The dependency graph, moved to each published snapshot by the commit.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.shared.graph
    }

    /// The event bus, for subscribing and for the units that emit events other than
    /// `committed`.
    #[must_use]
    pub fn events(&self) -> &EventBus {
        &self.shared.events
    }

    /// Subscribes to the document's events with the default capacity.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.shared.events.subscribe()
    }

    /// Queues `job`, which runs on the executor with a new transaction on the latest snapshot;
    /// the executor then commits the transaction. Returns at once with the handle of the
    /// outcome: the job's value with the published snapshot, or the error of the job, of its
    /// commit, `doc.executor.fault` when it unwound, or `doc.executor.stopped` when the
    /// document was closed first. In every failure the published snapshot stays the same.
    ///
    /// The transaction is appended to the journal and reported to the undo manager without a
    /// coalesce key; see [`Document::submit_with`].
    pub fn submit<T, F>(&self, job: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
    {
        self.submit_with(SubmitOptions::default(), job)
    }

    /// [`Document::submit`] with `options`: the coalesce key the transaction is reported to the
    /// undo manager with. While an undo group is open, the transaction carries that group's
    /// identifier (set when the job leaves it empty); a job that names another group fails with
    /// `doc.undo.sequence_invalid` before its commit.
    pub fn submit_with<T, F>(&self, options: SubmitOptions, job: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
    {
        self.on_executor(move |core| core.run(options, job))
    }

    /// Runs `job` like [`Document::submit`] and validates its transaction exactly as a commit
    /// does (field schemas, deletion and relationship rules, references, type and document
    /// validators), then discards it. The value is the job's result with the operations and
    /// changed identifiers the commit would have produced; the diagnostics are the validators'
    /// warnings, which are returned and not delivered. Nothing else happens: no publication,
    /// revision change, journal record, undo or redo effect, event or graph update, and the
    /// snapshot is the very same afterwards. The job runs on the executor between two commits,
    /// so it sees the snapshot the next commit would apply to. Fails with the error of the job
    /// or of the validation, `doc.executor.fault` when it unwound, or `doc.executor.stopped`.
    pub fn dry_run<T, F>(&self, job: F) -> Pending<DryRun<T>>
    where
        T: Send + 'static,
        F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
    {
        self.on_executor(move |core| core.dry_run(job))
    }

    fn on_executor<T, F>(&self, work: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Core) -> Result<Completed<T>> + Send + 'static,
    {
        let (task, pending) = task_on(work);
        self.executor.submit(task);
        pending
    }

    /// Runs `step` on the undo manager in submission order, as a job that commits nothing.
    fn undo_control<T, F>(&self, step: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut UndoManager) -> Result<T> + Send + 'static,
    {
        self.on_executor(move |core| {
            let value = step(&mut lock(&core.shared.history).undo)?;
            Ok(core.nothing(value))
        })
    }

    /// Runs an undo-manager transaction opened by `open` as a job; the value is true when it
    /// committed one.
    fn undo_job<F>(&self, restore: bool, open: F) -> Pending<bool>
    where
        F: FnOnce(&mut History, &Snapshot, crate::txn::TxnMeta, TxnLimits) -> Result<Option<Txn>>
            + Send
            + 'static,
    {
        self.on_executor(move |core| {
            let open: Open<'_> = Box::new(open);
            let done = core.transact(Report::Undo(None), restore, open, |_| Ok(true))?;
            Ok(done.unwrap_or_else(|| core.nothing(false)))
        })
    }

    /// Undoes the top undo step as a job: the undo manager prepares the transaction on the
    /// job's base snapshot ([`UndoManager::undo`]), it commits like any transaction with kind
    /// `undo_of`, and only then do the stacks change. The value is false when there was nothing
    /// to undo. Fails with the codes of [`UndoManager::undo`] (inside an open group,
    /// `doc.undo.sequence_invalid`) or of the commit; the stacks stay as they were then.
    pub fn undo(&self) -> Pending<bool> {
        self.undo_job(false, |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            undo.undo(journal, base, meta, limits)
        })
    }

    /// Redoes the top redo step as a job, like [`Document::undo`] with kind `redo_of`.
    pub fn redo(&self) -> Pending<bool> {
        self.undo_job(false, |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            undo.redo(journal, base, meta, limits)
        })
    }

    /// Undoes the top `count` undo steps in one transaction ([`UndoManager::undo_many`]): all
    /// or nothing, one commit, one notification. The batch is one transaction, so it is subject
    /// to the transaction limits: a very large batch fails atomically with the limit error where
    /// single steps would succeed. Fails with `doc.undo.sequence_invalid` for a
    /// `count` of zero or above the undo stack, or inside an open group.
    pub fn undo_many(&self, count: usize) -> Pending<bool> {
        self.undo_job(true, move |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            let txn = undo.undo_many(count, journal, base, meta, limits)?;
            undo.expect_depth(count);
            Ok(Some(txn))
        })
    }

    /// Redoes the top `count` redo steps in one transaction ([`UndoManager::redo_many`]); the
    /// redo forms one undo step. Fails like [`Document::undo_many`].
    pub fn redo_many(&self, count: usize) -> Pending<bool> {
        self.undo_job(true, move |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            let txn = undo.redo_many(count, journal, base, meta, limits)?;
            undo.expect_depth(count);
            Ok(Some(txn))
        })
    }

    /// Sets the undo mark `name` at the top of the undo stack in submission order
    /// ([`UndoManager::set_mark`]).
    pub fn set_mark(&self, name: impl Into<String>) -> Pending<()> {
        let name = name.into();
        self.undo_control(move |undo| undo.set_mark(&name))
    }

    /// Removes the undo mark `name` in submission order; the value is true when it existed.
    pub fn clear_mark(&self, name: impl Into<String>) -> Pending<bool> {
        let name = name.into();
        self.undo_control(move |undo| Ok(undo.clear_mark(&name)))
    }

    /// Undoes every step above the undo mark `name` in one transaction; the value is false when
    /// the stack is already at the mark. Fails with `doc.undo.sequence_invalid` when the mark is
    /// not set or no longer valid, or inside an open group.
    pub fn undo_to_mark(&self, name: impl Into<String>) -> Pending<bool> {
        let name = name.into();
        self.undo_job(true, move |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            match undo.steps_to_mark(&name)? {
                0 => Ok(None),
                count => {
                    let txn = undo.undo_many(count, journal, base, meta, limits)?;
                    undo.expect_depth(count);
                    Ok(Some(txn))
                }
            }
        })
    }

    /// The names of the valid undo marks, in the order they were set.
    #[must_use]
    pub fn marks(&self) -> Vec<String> {
        lock(&self.shared.history)
            .undo
            .marks()
            .map(str::to_owned)
            .collect()
    }

    /// Up to `limit` steps of the undo stack, the one [`Document::undo`] undoes first.
    #[must_use]
    pub fn undo_step_list(&self, limit: usize) -> Vec<StepSummary> {
        let history = lock(&self.shared.history);
        summarize(&history.journal, history.undo.undo_steps().rev(), limit)
    }

    /// Up to `limit` steps of the redo stack, the one [`Document::redo`] redoes first.
    #[must_use]
    pub fn redo_step_list(&self, limit: usize) -> Vec<StepSummary> {
        let history = lock(&self.shared.history);
        summarize(&history.journal, history.undo.redo_steps().rev(), limit)
    }

    /// Re-creates, with their original identifiers, the objects deleted by the newest ordinary
    /// transaction (`normal` or `link_reload`, among the last [`MAX_RESTORE_SCAN`] journal
    /// records) that deleted anything, without undoing later transactions. Only that record is
    /// considered: when all its deleted objects are live again (the erase was undone, or already
    /// restored) the value is zero and an older erase is never touched; when only some are live,
    /// the rest are restored. It is a normal commit with `command` and `label` as its opaque
    /// texts: undoable like any step, and it clears the redo stack. The value is the number of objects restored, zero when there is nothing to
    /// restore. The references and owner relations the erase cleared in surviving objects are not
    /// restored. Fails, changing nothing, with the codes of the commit when the objects cannot
    /// return consistently (for example `doc.reference.dangling` when one references an object
    /// deleted since) or `doc.undo.state_mismatch` when a type's schema version changed.
    pub fn restore_erased(&self, command: String, label: String) -> Pending<usize> {
        self.on_executor(move |core| {
            let restored = std::cell::Cell::new(0);
            let open: Open<'_> = Box::new(|history, base, mut meta, limits| {
                meta.command = command;
                meta.label = label;
                let Some(ops) = last_erased(&history.journal, base) else {
                    return Ok(None);
                };
                restored.set(ops.len());
                crate::undo::prepare(base, meta, limits, TxnKind::Normal, &ops).map(Some)
            });
            let done = core.transact(Report::Undo(None), false, open, |_| Ok(()))?;
            Ok(match done {
                Some(done) => Completed {
                    value: restored.get(),
                    snapshot: done.snapshot,
                    txn_id: done.txn_id,
                    diagnostics: done.diagnostics,
                },
                None => core.nothing(0),
            })
        })
    }

    /// Opens an undo group `id` in submission order ([`UndoManager::begin_group`]): the
    /// transactions of later jobs carry the outermost group's identifier and form one step when
    /// the outermost group ends.
    pub fn begin_group(&self, id: impl Into<String>) -> Pending<()> {
        let id = id.into();
        self.undo_control(move |undo| {
            undo.begin_group(id);
            Ok(())
        })
    }

    /// Closes the innermost undo group ([`UndoManager::end_group`]); fails with
    /// `doc.undo.sequence_invalid` when none is open.
    pub fn end_group(&self) -> Pending<()> {
        self.undo_control(UndoManager::end_group)
    }

    /// Closes the innermost undo group because its owner failed and commits the rollback of
    /// its members ([`UndoManager::fail_group`]); the value is true when there was something to
    /// roll back. When the rollback fails to commit, the group stays open and the stacks as
    /// they were.
    pub fn fail_group(&self) -> Pending<bool> {
        self.undo_job(true, |history, base, meta, limits| {
            let History { journal, undo, .. } = history;
            let before = undo.group().map(str::to_owned);
            let txn = undo.fail_group(journal, base, meta, limits)?;
            // The rollback carries the outermost group the members carried.
            Ok(txn.map(|mut txn| {
                txn.meta_mut().group = before;
                txn
            }))
        })
    }

    /// Starts a gesture in submission order ([`UndoManager::begin_gesture`]).
    pub fn begin_gesture(&self) -> Pending<()> {
        self.undo_control(|undo| {
            undo.begin_gesture();
            Ok(())
        })
    }

    /// Ends the gesture in submission order ([`UndoManager::end_gesture`]).
    pub fn end_gesture(&self) -> Pending<()> {
        self.undo_control(|undo| {
            undo.end_gesture();
            Ok(())
        })
    }

    /// True when [`Document::undo`] would undo a step now: the undo stack is not empty and no
    /// group is open. Jobs still queued may change that.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        let history = lock(&self.shared.history);
        history.undo.group().is_none() && history.undo.undo_steps().next_back().is_some()
    }

    /// True when [`Document::redo`] would redo a step now.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        let history = lock(&self.shared.history);
        history.undo.group().is_none() && history.undo.redo_steps().next_back().is_some()
    }

    /// The label of the step [`Document::undo`] would undo (the label of its last transaction,
    /// opaque), for the user interface.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        let history = lock(&self.shared.history);
        let id = *history
            .undo
            .undo_steps()
            .next_back()?
            .transactions()
            .last()?;
        Some(history.journal.get(id)?.record().label.clone())
    }

    /// The label of the step [`Document::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        let history = lock(&self.shared.history);
        let id = *history
            .undo
            .redo_steps()
            .next_back()?
            .transactions()
            .last()?;
        Some(history.journal.get(id)?.record().label.clone())
    }

    /// Calls `read` with the document's journal, under the history lock: `read` must not wait
    /// for a job of this document.
    pub fn with_journal<R>(&self, read: impl FnOnce(&Journal) -> R) -> R {
        read(&lock(&self.shared.history).journal)
    }

    /// Calls `read` with the document's undo manager, under the history lock: `read` must not
    /// wait for a job of this document.
    pub fn with_undo<R>(&self, read: impl FnOnce(&UndoManager) -> R) -> R {
        read(&lock(&self.shared.history).undo)
    }

    /// Commits the operations of `record` again as a job, keeping its identifier, parent, kind
    /// and metadata: each operation is first applied strictly to the base
    /// (`doc.undo.state_mismatch` otherwise), then the transaction is validated and committed
    /// like any other and appended to the journal. It is not reported to the undo manager.
    #[cfg(feature = "package")]
    pub(crate) fn replay(&self, record: TxnRecord) -> Pending<()> {
        self.on_executor(move |core| {
            let open: Open<'_> = Box::new(move |_, base, _, limits| {
                let kind = record.kind;
                prepare(base, record.meta(), limits, kind, &record.ops).map(Some)
            });
            let done = core.transact(Report::Skip, false, open, |_| Ok(()))?;
            Ok(done.unwrap_or_else(|| core.nothing(())))
        })
    }

    fn hooks(&self) -> RwLockWriteGuard<'_, Hooks> {
        self.shared
            .hooks
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a document validator; it runs at every later commit, after those added before.
    pub fn add_validator(&self, validator: DocumentValidator) {
        let mut hooks = self.hooks();
        let mut validators = hooks.validators.to_vec();
        validators.push(validator);
        hooks.validators = Arc::from(validators);
    }

    /// Sets a sink that records every later commit after the document's own journal, such as
    /// the recovery journal of the package.
    pub fn set_journal(&self, journal: Arc<dyn JournalSink>) {
        self.hooks().journal = Some(journal);
    }

    /// Sets a sink that receives the validator warnings and faults of every later job, after
    /// the document's diagnostics list.
    pub fn set_diagnostics(&self, sink: Arc<dyn DiagnosticSink>) {
        self.hooks().diagnostics = Some(Arc::new(Sinks {
            list: Arc::clone(&self.diagnostics),
            extra: Some(sink),
        }));
    }

    /// The document diagnostics list (CORE-DOC-035): validator warnings, faults, failed,
    /// blocked and cyclic nodes, load findings and unresolved links.
    #[must_use]
    pub fn diagnostics(&self) -> &Arc<DiagnosticsList> {
        &self.diagnostics
    }

    /// A copy of the diagnostics list for display ([`DiagnosticsList::snapshot`]).
    #[must_use]
    pub fn diagnostics_snapshot(&self) -> DiagnosticsSnapshot {
        self.diagnostics.snapshot()
    }

    /// The regeneration scheduler (CORE-DOC-031, CORE-DOC-032).
    #[must_use]
    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    /// Requests regeneration of `nodes` at `priority` ([`Scheduler::prioritize`]).
    pub fn regenerate(&self, nodes: impl IntoIterator<Item = NodeKey>, priority: Priority) {
        self.scheduler.prioritize(nodes, priority);
    }

    /// Brings `node` to a settled state now, ahead of queued work ([`Scheduler::ensure`]).
    pub fn ensure(&self, node: &NodeKey) -> Result<NodeView> {
        self.scheduler.ensure(node)
    }

    /// Waits at most `timeout` until regeneration is idle ([`Scheduler::wait_idle`]); a save
    /// calls it to find clean data.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.scheduler.wait_idle(timeout)
    }

    /// The diagnostics of the load this document came from, in load order.
    #[must_use]
    pub fn load_diagnostics(&self) -> &[Diagnostic] {
        &self.load_diagnostics
    }

    /// The journal records the package holds as far as this document knows, in `seq` order:
    /// those it was loaded with, then those each save retained (CORE-DOC-040). They are kept
    /// for inspection, history and compaction; undo and redo reach only the transactions of
    /// this session ([`Document::with_journal`]).
    #[must_use]
    pub fn stored_journal(&self) -> Vec<StoredRecord> {
        lock(&self.stored).0.clone()
    }

    /// The records that match `query`, in commit order: the stored records that decode against
    /// the document's registry (a record that does not decode is skipped), then the records of
    /// this session's journal; a record held by both is listed once (CORE-DOC-025).
    #[must_use]
    pub fn history(&self, query: &HistoryQuery) -> Vec<TxnRecord> {
        let registry = self.registry();
        let limits = lock(&self.shared.history).limits;
        let stored = self.stored_journal();
        self.with_journal(|journal| {
            stored
                .iter()
                .filter_map(|row| TxnRecord::decode(&row.record, &registry, &limits).ok())
                .filter(|record| journal.get(record.txn_id).is_none())
                .chain(journal.iter().map(|entry| entry.record().clone()))
                .filter(|record| query.matches(record))
                .collect()
        })
    }

    /// The original rows of the load this document came from, which a save needs.
    #[must_use]
    pub fn loaded_rows(&self) -> &LoadedRows {
        &self.loaded.rows
    }

    /// The load this document came from without its snapshot and diagnostics (type rows, side
    /// table and counts), as a save takes it.
    #[cfg(feature = "package")]
    pub(crate) fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    /// Calls `read` on the executor, between two commits, with the latest snapshot and the
    /// journal that leads to it, and returns its value. Must not be called from a job of this
    /// document. Fails with `doc.executor.stopped` when the document is closed.
    #[cfg(feature = "package")]
    pub(crate) fn read_state<T, F>(&self, read: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Snapshot, &Journal) -> T + Send + 'static,
    {
        self.on_executor(move |core| {
            let snapshot = core.shared.cell.load();
            let value = read(&snapshot, &lock(&core.shared.history).journal);
            Ok(core.nothing(value))
        })
        .wait()
        .map(|completed| completed.value)
    }

    /// Stops the executor after the running job and waits for it (unless called from a job);
    /// queued and later jobs receive `doc.executor.stopped`. Then stops the regeneration
    /// scheduler ([`Scheduler::shutdown`]). Closing twice does nothing more.
    pub fn close(&self) {
        self.executor.close();
        self.scheduler.shutdown();
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.close();
    }
}

/// The most journal records [`Document::restore_erased`] searches.
pub const MAX_RESTORE_SCAN: usize = 10_000;

/// One step of the undo or redo stack, as the lists describe it. The texts are the opaque
/// command and label of the step's last transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSummary {
    /// The last transaction of the step.
    pub last: arc_base::TransactionId,
    /// The number of transactions in the step.
    pub transactions: usize,
    /// The command text of the last transaction.
    pub command: String,
    /// The label text of the last transaction.
    pub label: String,
}

fn summarize<'a>(
    journal: &Journal,
    steps: impl Iterator<Item = &'a crate::undo::Step>,
    limit: usize,
) -> Vec<StepSummary> {
    steps
        .take(limit)
        .filter_map(|step| {
            let last = *step.transactions().last()?;
            let record = journal.get(last)?.record();
            Some(StepSummary {
                last,
                transactions: step.transactions().len(),
                command: record.command.clone(),
                label: record.label.clone(),
            })
        })
        .collect()
}

/// The operations that re-create the objects the newest ordinary record that deleted anything
/// deleted and that are not live, last deletion first so owners return before their children.
fn last_erased(journal: &Journal, base: &Snapshot) -> Option<Vec<crate::op::Op>> {
    let record = journal
        .iter()
        .rev()
        .take(MAX_RESTORE_SCAN)
        .map(crate::journal::JournalEntry::record)
        .filter(|record| matches!(record.kind, TxnKind::Normal | TxnKind::LinkReload))
        .find(|record| {
            record
                .ops
                .iter()
                .any(|op| matches!(op, crate::op::Op::Delete { .. }))
        })?;
    let ops: Vec<_> = record
        .ops
        .iter()
        .rev()
        .filter(|op| matches!(op, crate::op::Op::Delete { .. }))
        .filter(|op| base.resolve(op.id()).is_err())
        .map(crate::op::Op::inverse)
        .collect();
    (!ops.is_empty()).then_some(ops)
}
