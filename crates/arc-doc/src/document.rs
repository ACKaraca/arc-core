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

use arc_base::id::IdGenerator;
use arc_base::{Limits, Result, SessionId};

use crate::commit::JournalSink;
use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::diagnostics::{
    DEFAULT_DIAGNOSTICS_CAPACITY, DiagnosticsList, DiagnosticsSnapshot, Emitter,
};
use crate::events::{Event, EventBus, Subscription};
use crate::executor::{
    Completed, Core, Executor, Open, Pending, Report, SubmitOptions, lock, task_on,
};
use crate::graph::{Evaluators, Graph, NodeKey, NodeView};
use crate::journal::Journal;
use crate::load::{Loaded, LoadedRows};
#[cfg(feature = "package")]
use crate::record::TxnRecord;
use crate::registry::TypeRegistry;
use crate::scheduler::{Priority, Scheduler, default_workers};
use crate::snapshot::{Snapshot, SnapshotCell};
use crate::txn::{Txn, TxnLimits};
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
    /// The worker threads of the regeneration scheduler; with none, regeneration runs when
    /// pumped ([`Scheduler::pump`]). Ignored on `wasm32`, which has none.
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
    rows: LoadedRows,
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
        let executor = Executor::start(Core {
            shared: Arc::clone(&shared),
            ids: config.ids,
            session: config.session,
            last: None,
            txn_limits: config.txn_limits,
            limits: config.limits,
        })?;
        Ok(Self {
            shared,
            executor,
            scheduler,
            diagnostics,
            load_diagnostics: Vec::new(),
            rows: LoadedRows::default(),
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
        let mut document = Self::open(loaded.snapshot, evaluators, config)?;
        document.load_diagnostics = loaded
            .diagnostics
            .into_iter()
            .map(Diagnostic::from)
            .collect();
        document
            .diagnostics
            .add_load(document.load_diagnostics.iter().cloned());
        document.rows = loaded.rows;
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

    /// The original rows of the load this document came from, which a save needs.
    #[must_use]
    pub fn loaded_rows(&self) -> &LoadedRows {
        &self.rows
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
