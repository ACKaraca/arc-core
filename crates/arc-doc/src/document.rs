//! The handle of an open document: the latest snapshot, the dependency graph, the executor, the
//! event bus, the document validators, the journal hook, the diagnostics list and the
//! regeneration scheduler.
//!
//! Storage and the package are not part of this handle yet.

use std::sync::{Arc, PoisonError, RwLock, RwLockWriteGuard};
use std::time::Duration;

use arc_base::id::IdGenerator;
use arc_base::{Limits, Result, SessionId};

use crate::commit::JournalSink;
use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::diagnostics::{
    DEFAULT_DIAGNOSTICS_CAPACITY, DiagnosticsList, DiagnosticsSnapshot, Emitter,
};
use crate::events::{Event, EventBus, Subscription};
use crate::executor::{Core, Executor, Pending, task};
use crate::graph::{Evaluators, Graph, NodeKey, NodeView};
use crate::load::{Loaded, LoadedRows};
use crate::registry::TypeRegistry;
use crate::scheduler::{Priority, Scheduler, default_workers};
use crate::snapshot::{Snapshot, SnapshotCell};
use crate::txn::{Txn, TxnLimits};
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

/// The state the document handle and its executor share.
pub(crate) struct Shared {
    pub(crate) cell: SnapshotCell,
    pub(crate) graph: Graph,
    pub(crate) events: EventBus,
    pub(crate) hooks: RwLock<Hooks>,
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
    pub fn submit<T, F>(&self, job: F) -> Pending<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
    {
        let (task, pending) = task(job);
        self.executor.submit(task);
        pending
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

    /// Sets the journal that records every later commit.
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
