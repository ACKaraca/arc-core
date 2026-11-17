//! The handle of an open document: the latest snapshot, the dependency graph, the executor, the
//! event bus, the document validators, the journal hook and the diagnostics sink.
//!
//! Storage and the package are not part of this handle yet.

use std::sync::{Arc, PoisonError, RwLock, RwLockWriteGuard};

use arc_base::id::IdGenerator;
use arc_base::{Limits, Result, SessionId};

use crate::commit::JournalSink;
use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::events::{EventBus, Subscription};
use crate::executor::{Core, Executor, Pending, task};
use crate::graph::{Evaluators, Graph};
use crate::load::{Loaded, LoadedRows};
use crate::registry::TypeRegistry;
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
}

impl Default for DocumentConfig {
    /// A seeded generator (seed 0) and the session it generates first; default limits.
    fn default() -> Self {
        let mut ids = IdGenerator::seeded(0);
        let session = ids.generate();
        Self {
            ids,
            session,
            txn_limits: TxnLimits::default(),
            limits: Limits::DEFAULT,
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

/// An open document.
///
/// Readers call [`Document::snapshot`] on any thread, without the executor and without locks.
/// Changes are jobs submitted with [`Document::submit`], which run one at a time, in submission
/// order, on the document's executor ([`crate::executor`]). The handle is `Send + Sync`; share it
/// in an `Arc` between submitting threads. Dropping it, or [`Document::close`], stops the
/// executor after the running job.
pub struct Document {
    shared: Arc<Shared>,
    executor: Executor,
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

    /// Sets the sink of validator warnings and faults of every later job.
    pub fn set_diagnostics(&self, sink: Arc<dyn DiagnosticSink>) {
        self.hooks().diagnostics = Some(sink);
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
    /// queued and later jobs receive `doc.executor.stopped`. Closing twice does nothing more.
    pub fn close(&self) {
        self.executor.close();
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.executor.close();
    }
}
