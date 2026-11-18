//! The document executor (CORE-DOC-016) and fault rollback (CORE-DOC-018).
//!
//! Each document runs its transactions on one dedicated thread, strictly in submission order.
//! A job is a closure that receives a fresh [`Txn`] on the latest snapshot and returns a value;
//! the executor then commits the transaction ([`crate::commit::commit`]) and sends the outcome
//! to the [`Pending`] handle the submitter holds. Because the thread runs one job at a time,
//! exactly one transaction is open per document. Submitting never waits for a job: the queue
//! is unbounded and the caller decides whether to wait on the handle, poll it or drop it.
//!
//! A job that returns an error, or unwinds, or whose commit fails, leaves the published
//! snapshot as it was (the very same snapshot): the transaction is dropped. An unwind is caught
//! on the executor thread and reported to the submitter as `doc.executor.fault` with the panic
//! message as the detail `panic`, and to the diagnostics sink; the thread then runs the next
//! job. Read-only work needs no executor: take a [`crate::Snapshot`] from
//! [`crate::Document::snapshot`] on any thread.
//!
//! Closing the document stops the thread after the running job; jobs still queued, and jobs
//! submitted afterwards, receive `doc.executor.stopped`.
//!
//! Every published transaction is appended to the document's [`crate::Journal`] by the commit's
//! journal step and then reported to its [`crate::UndoManager`] with the job's coalesce key
//! ([`SubmitOptions`]); a transaction made while an undo group is open carries that group's
//! identifier. Both live in one lock of the document (`History`), which is a leaf: nothing else
//! is locked while it is held, and the job itself runs without it. The journal append and the
//! report cannot fail for a record this process just produced; if one does (a defect, or a
//! record beyond the encoding limits), the transaction stays published, a `doc.executor.fault`
//! diagnostic (detail `step` = `history`, the failure as its source) is delivered and returned,
//! and the history is cut there: after a failed append the journal and both undo stacks start
//! over empty, after a failed report the undo stacks do.
//!
//! WebAssembly in the browser has no threads by default. On `wasm32` the executor runs each job
//! inline in `submit`, on the calling thread, before `submit` returns; the API, the order and
//! the rollback are the same. A job submitted from inside a running job is queued and runs when
//! the running job has finished.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::Duration;

use arc_base::id::IdGenerator;
use arc_base::{Error, Limits, Result, SessionId, TransactionId};

use crate::commit::{CommitHooks, Committed, JournalSink, commit, fault, fault_diagnostic};
use crate::diagnostic::{Diagnostic, DiagnosticSink};
use crate::document::{History, Shared};
use crate::error::codes;
use crate::snapshot::Snapshot;
use crate::txn::{Txn, TxnKind, TxnLimits, TxnMeta};
use crate::undo::UndoManager;

/// A committed job's result.
#[derive(Debug)]
pub struct Completed<T> {
    /// What the job returned.
    pub value: T,
    /// The snapshot published by the commit, or the unchanged latest snapshot when the job made
    /// no operation.
    pub snapshot: Snapshot,
    /// The published transaction; `None` when the job made no operation.
    pub txn_id: Option<TransactionId>,
    /// The validator warnings, and faults of steps after publication.
    pub diagnostics: Vec<Diagnostic>,
}

/// The handle of a submitted job: wait for its outcome, poll it, or drop it.
#[derive(Debug)]
pub struct Pending<T> {
    receiver: mpsc::Receiver<Result<Completed<T>>>,
}

impl<T> Pending<T> {
    /// Waits for the outcome.
    pub fn wait(self) -> Result<Completed<T>> {
        self.receiver.recv().unwrap_or_else(|_| Err(stopped()))
    }

    /// The outcome if it is ready.
    #[must_use]
    pub fn try_take(&self) -> Option<Result<Completed<T>>> {
        match self.receiver.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(stopped())),
        }
    }

    /// The outcome, waiting at most `timeout`.
    #[must_use]
    pub fn wait_timeout(&self, timeout: Duration) -> Option<Result<Completed<T>>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(outcome) => Some(outcome),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => Some(Err(stopped())),
        }
    }
}

/// The `doc.executor.stopped` error.
pub(crate) fn stopped() -> Error {
    Error::new(codes::EXECUTOR_STOPPED)
}

/// A queued job with its reply channel; `None` tells it the executor stopped.
pub(crate) type Task = Box<dyn FnOnce(Option<&mut Core>) + Send>;

/// Wraps `job` into a task and returns the handle of its outcome.
#[cfg(test)]
pub(crate) fn task<T, F>(job: F) -> (Task, Pending<T>)
where
    T: Send + 'static,
    F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
{
    task_on(move |core| core.run(SubmitOptions::default(), job))
}

/// Wraps `work`, which runs on the executor with its [`Core`], into a task and returns the
/// handle of its outcome.
pub(crate) fn task_on<T, F>(work: F) -> (Task, Pending<T>)
where
    T: Send + 'static,
    F: FnOnce(&mut Core) -> Result<Completed<T>> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let task: Task = Box::new(move |core: Option<&mut Core>| {
        let outcome = match core {
            Some(core) => work(core),
            None => Err(stopped()),
        };
        // The submitter may have dropped its handle; the outcome is then not needed.
        drop(sender.send(outcome));
    });
    (task, Pending { receiver })
}

/// How a job's transaction is reported to the undo manager.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct SubmitOptions {
    /// The coalesce key of the transaction: within a gesture, consecutive transactions with the
    /// same key form one undo step ([`crate::undo::UndoManager::committed`]).
    pub coalesce: Option<String>,
}

impl SubmitOptions {
    /// Options with the coalesce key `key`.
    #[must_use]
    pub fn coalesce(key: impl Into<String>) -> Self {
        Self {
            coalesce: Some(key.into()),
        }
    }
}

/// What a committed transaction reports to the undo manager.
pub(crate) enum Report {
    /// Report it with this coalesce key.
    Undo(Option<String>),
    /// Report nothing: a replayed transaction of crash recovery.
    #[cfg_attr(
        not(feature = "package"),
        expect(dead_code, reason = "only crash recovery replays")
    )]
    Skip,
}

/// Opens the transaction of a [`Core::transact`] on the history, the base snapshot, the
/// prepared metadata and the limits; `None` when there is nothing to do.
pub(crate) type Open<'a> =
    Box<dyn FnOnce(&mut History, &Snapshot, TxnMeta, TxnLimits) -> Result<Option<Txn>> + 'a>;

/// The journal hook of the executor: appends each published transaction to the document's
/// journal, then informs the sink set with [`crate::Document::set_journal`].
struct Recorder<'a> {
    history: &'a Mutex<History>,
    extra: Option<&'a dyn JournalSink>,
    failure: Mutex<Option<Error>>,
}

impl JournalSink for Recorder<'_> {
    fn record(&self, committed: &Committed) {
        let appended = {
            let mut history = lock(self.history);
            let appended = history
                .journal
                .append(
                    committed.meta.clone(),
                    committed.ops.clone(),
                    &committed.before,
                    &committed.after,
                )
                .map(drop);
            if appended.is_err() {
                history.cut();
            }
            appended
        };
        if let Err(error) = appended {
            *lock(&self.failure) = Some(error);
        }
        if let Some(extra) = self.extra {
            extra.record(committed);
        }
    }
}

/// Locks `mutex`, ignoring poisoning: the guarded state is replaced as a whole or not at all.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the executor thread owns: the document state and the transaction identity.
pub(crate) struct Core {
    pub(crate) shared: Arc<Shared>,
    pub(crate) ids: IdGenerator,
    pub(crate) session: SessionId,
    pub(crate) last: Option<TransactionId>,
    pub(crate) txn_limits: TxnLimits,
    pub(crate) limits: Limits,
}

impl Core {
    /// Runs `job` in a transaction on the latest snapshot and commits it, containing unwinds.
    pub(crate) fn run<T>(
        &mut self,
        options: SubmitOptions,
        job: impl FnOnce(&mut Txn) -> Result<T>,
    ) -> Result<Completed<T>> {
        let open: Open<'_> =
            Box::new(|_, base, meta, limits| Ok(Some(Txn::new(base, meta, limits))));
        let completed = self.transact(Report::Undo(options.coalesce), false, open, job)?;
        completed.ok_or_else(|| Error::new(codes::EXECUTOR_FAULT))
    }

    /// The outcome of work that published nothing.
    pub(crate) fn nothing<T>(&self, value: T) -> Completed<T> {
        Completed {
            value,
            snapshot: self.shared.cell.load(),
            txn_id: None,
            diagnostics: Vec::new(),
        }
    }

    /// Opens a transaction with `open` (under the history lock), runs `job` on it (without the
    /// lock), commits it and reports it to the undo manager as `report` says, containing
    /// unwinds. `None` when `open` had nothing to do. With `restore`, the undo manager is put
    /// back as it was before `open` when the transaction fails.
    pub(crate) fn transact<T>(
        &mut self,
        report: Report,
        restore: bool,
        open: Open<'_>,
        job: impl FnOnce(&mut Txn) -> Result<T>,
    ) -> Result<Option<Completed<T>>> {
        let base = self.shared.cell.load();
        let mut meta = TxnMeta::new(self.ids.generate(), self.session);
        meta.parent = self.last;
        let hooks = self
            .shared
            .hooks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let recorder = Recorder {
            history: &self.shared.history,
            extra: hooks.journal.as_deref(),
            failure: Mutex::new(None),
        };
        let commit_hooks = CommitHooks {
            cell: &self.shared.cell,
            limits: &self.limits,
            validators: &hooks.validators,
            graph: Some(&self.shared.graph),
            journal: Some(&recorder),
            diagnostics: hooks.diagnostics.as_deref(),
            events: Some(&self.shared.events),
        };
        let mut saved = None;
        let txn_limits = self.txn_limits;
        let history = &self.shared.history;
        let body = catch_unwind(AssertUnwindSafe(|| {
            let opened = {
                let mut history = lock(history);
                meta.group = history.undo.group().map(str::to_owned);
                if restore {
                    saved = Some(history.undo.clone());
                }
                let group = meta.group.clone();
                open(&mut history, &base, meta, txn_limits).map(|txn| (txn, group))
            };
            let (Some(mut txn), group) = opened? else {
                return Ok(None);
            };
            let value = job(&mut txn)?;
            same_group(&mut txn, group)?;
            commit(txn, &commit_hooks).map(|outcome| Some((value, outcome)))
        }));
        let fault_of = |payload: &(dyn std::any::Any + Send)| {
            if let Some(sink) = hooks.diagnostics.as_deref() {
                let diagnostic = fault_diagnostic(fault(payload));
                // A sink that unwinds as well is ignored: the submitter gets the fault.
                drop(catch_unwind(AssertUnwindSafe(|| {
                    sink.deliver(&[diagnostic])
                })));
            }
            fault(payload)
        };
        let outcome = match body {
            Ok(Ok(Some(outcome))) => outcome,
            Ok(Ok(None)) => return Ok(None),
            Ok(Err(error)) => {
                self.restore(saved);
                return Err(error);
            }
            Err(payload) => {
                self.restore(saved);
                return Err(fault_of(payload.as_ref()));
            }
        };
        let (value, mut outcome) = outcome;
        match outcome.txn_id {
            Some(id) => {
                self.last = Some(id);
                let failure = lock(&recorder.failure).take();
                let diagnostics = hooks.diagnostics.as_deref();
                if let Some(fault) = self.report(id, failure, report, diagnostics) {
                    outcome.diagnostics.push(fault);
                }
            }
            None => self.restore(saved),
        }
        Ok(Some(Completed {
            value,
            snapshot: outcome.snapshot,
            txn_id: outcome.txn_id,
            diagnostics: outcome.diagnostics,
        }))
    }

    /// Reports the published transaction `id` to the undo manager as `report` says, after the
    /// journal step failed with `failure` or not; when either failed, cuts the history and
    /// returns the fault diagnostic, delivered to `sink`.
    fn report(
        &self,
        id: TransactionId,
        failure: Option<Error>,
        report: Report,
        sink: Option<&dyn DiagnosticSink>,
    ) -> Option<Diagnostic> {
        let error = match (failure, report) {
            (Some(error), _) => error,
            (None, Report::Undo(coalesce)) => {
                let mut history = lock(&self.shared.history);
                let History { journal, undo, .. } = &mut *history;
                let error = undo.committed(journal, id, coalesce.as_deref()).err()?;
                history.cut_undo();
                error
            }
            (None, Report::Skip) => return None,
        };
        // The transaction is published; its history is not: undo starts over here.
        let diagnostic = fault_diagnostic(
            Error::new(codes::EXECUTOR_FAULT)
                .with_detail("step", "history")
                .with_source(error),
        );
        if let Some(sink) = sink {
            drop(catch_unwind(AssertUnwindSafe(|| {
                sink.deliver(std::slice::from_ref(&diagnostic));
            })));
        }
        Some(diagnostic)
    }

    fn restore(&self, saved: Option<UndoManager>) {
        if let Some(undo) = saved {
            lock(&self.shared.history).undo = undo;
        }
    }
}

/// Gives a `normal` or `link_reload` transaction made while a group is open that group's
/// identifier, or fails with `doc.undo.sequence_invalid` when the job named another group.
fn same_group(txn: &mut Txn, group: Option<String>) -> Result<()> {
    let meta = txn.meta_mut();
    if !matches!(meta.kind, TxnKind::Normal | TxnKind::LinkReload) {
        return Ok(());
    }
    match (&meta.group, group) {
        (_, None) => Ok(()),
        (None, open) => {
            meta.group = open;
            Ok(())
        }
        (Some(named), Some(open)) if *named == open => Ok(()),
        (Some(_), Some(_)) => Err(Error::new(codes::UNDO_SEQUENCE_INVALID).with_arg(
            "reason",
            "the transaction names another group than the open one",
        )),
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use threaded::Executor;

#[cfg(target_arch = "wasm32")]
pub(crate) use inline::Executor;

#[cfg(not(target_arch = "wasm32"))]
mod threaded {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError, mpsc};
    use std::thread::{self, JoinHandle};

    use arc_base::{Error, Result};

    use super::{Core, Task};
    use crate::error::codes;

    /// One thread that runs tasks in the order they were sent.
    pub(crate) struct Executor {
        sender: Mutex<Option<mpsc::Sender<Task>>>,
        closed: Arc<AtomicBool>,
        thread: Mutex<Option<JoinHandle<()>>>,
    }

    impl Executor {
        /// Starts the thread; fails with `doc.executor.stopped` when it cannot be started.
        pub(crate) fn start(mut core: Core) -> Result<Self> {
            let (sender, receiver) = mpsc::channel::<Task>();
            let closed = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&closed);
            let thread = thread::Builder::new()
                .name("arc-doc-executor".into())
                .spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        if flag.load(Ordering::Acquire) {
                            task(None);
                        } else {
                            task(Some(&mut core));
                        }
                    }
                })
                .map_err(|error| {
                    Error::new(codes::EXECUTOR_STOPPED).with_detail("reason", error.to_string())
                })?;
            Ok(Self {
                sender: Mutex::new(Some(sender)),
                closed,
                thread: Mutex::new(Some(thread)),
            })
        }

        /// Queues `task`; after closing, answers it at once.
        pub(crate) fn submit(&self, task: Task) {
            let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
            match sender.as_ref() {
                Some(sender) => {
                    if let Err(mpsc::SendError(task)) = sender.send(task) {
                        task(None);
                    }
                }
                None => task(None),
            }
        }

        /// Stops the thread after the running job; queued jobs receive `doc.executor.stopped`.
        /// Waits for the thread unless called on it.
        pub(crate) fn close(&self) {
            self.closed.store(true, Ordering::Release);
            drop(
                self.sender
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take(),
            );
            let handle = self
                .thread
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(handle) = handle
                && handle.thread().id() != thread::current().id()
            {
                // Tasks contain their unwinds, so the thread does not end in a panic.
                drop(handle.join());
            }
        }
    }
}

#[cfg(any(target_arch = "wasm32", test))]
mod inline {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, PoisonError, TryLockError};

    use arc_base::Result;

    use super::{Core, Task};

    /// Runs tasks on the submitting thread, in submission order.
    pub(crate) struct Executor {
        core: Mutex<Core>,
        queue: Mutex<VecDeque<Task>>,
        closed: AtomicBool,
    }

    impl Executor {
        /// An executor without a thread; never fails (the signature is the threaded one's).
        pub(crate) fn start(core: Core) -> Result<Self> {
            Ok(Self {
                core: Mutex::new(core),
                queue: Mutex::new(VecDeque::new()),
                closed: AtomicBool::new(false),
            })
        }

        fn pop(&self) -> Option<Task> {
            self.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front()
        }

        /// Queues `task` and runs the queue unless a job is already running on this or
        /// another thread, which then runs it.
        pub(crate) fn submit(&self, task: Task) {
            if self.closed.load(Ordering::Acquire) {
                task(None);
                return;
            }
            self.queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(task);
            loop {
                let mut core = match self.core.try_lock() {
                    Ok(core) => core,
                    Err(TryLockError::Poisoned(core)) => core.into_inner(),
                    Err(TryLockError::WouldBlock) => return,
                };
                while let Some(task) = self.pop() {
                    task(Some(&mut core));
                }
                drop(core);
                let empty = self
                    .queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_empty();
                if empty {
                    return;
                }
            }
        }

        /// Answers every queued task with `doc.executor.stopped`.
        pub(crate) fn close(&self) {
            self.closed.store(true, Ordering::Release);
            while let Some(task) = self.pop() {
                task(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_base::id::IdGenerator;
    use arc_base::id::Identifier;
    use arc_base::{Limits, SessionId};

    use super::{Core, inline, task};
    use crate::document::Shared;
    use crate::graph::Evaluators;
    use crate::registry::TypeRegistry;
    use crate::snapshot::Snapshot;
    use crate::txn::TxnLimits;

    #[test]
    fn inline_executor_runs_at_submission_in_order_and_stops() {
        let shared = Arc::new(Shared::new(
            Snapshot::empty(Arc::new(TypeRegistry::new())),
            Arc::new(Evaluators::new()),
        ));
        let core = Core {
            shared,
            ids: IdGenerator::seeded(1),
            session: SessionId::from_u128(1),
            last: None,
            txn_limits: TxnLimits::default(),
            limits: Limits::DEFAULT,
        };
        let executor = inline::Executor::start(core).unwrap();
        let (first, a) = task(|_| Ok(1));
        executor.submit(first);
        assert_eq!(a.try_take().unwrap().unwrap().value, 1);
        let (panicking, b) = task(|_| -> arc_base::Result<()> { panic!("inline fault") });
        executor.submit(panicking);
        assert_eq!(b.wait().unwrap_err().code(), "doc.executor.fault");
        executor.close();
        let (late, c) = task(|_| Ok(2));
        executor.submit(late);
        assert_eq!(c.wait().unwrap_err().code(), "doc.executor.stopped");
    }
}
