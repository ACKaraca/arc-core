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
//! WebAssembly in the browser has no threads by default. On `wasm32` the executor runs each job
//! inline in `submit`, on the calling thread, before `submit` returns; the API, the order and
//! the rollback are the same. A job submitted from inside a running job is queued and runs when
//! the running job has finished.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, PoisonError, mpsc};
use std::time::Duration;

use arc_base::id::IdGenerator;
use arc_base::{Error, Limits, Result, SessionId, TransactionId};

use crate::commit::{CommitHooks, commit, fault, fault_diagnostic};
use crate::diagnostic::Diagnostic;
use crate::document::Shared;
use crate::error::codes;
use crate::snapshot::Snapshot;
use crate::txn::{Txn, TxnLimits, TxnMeta};

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
pub(crate) fn task<T, F>(job: F) -> (Task, Pending<T>)
where
    T: Send + 'static,
    F: FnOnce(&mut Txn) -> Result<T> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let task: Task = Box::new(move |core: Option<&mut Core>| {
        let outcome = match core {
            Some(core) => core.run(job),
            None => Err(stopped()),
        };
        // The submitter may have dropped its handle; the outcome is then not needed.
        drop(sender.send(outcome));
    });
    (task, Pending { receiver })
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
    fn run<T>(&mut self, job: impl FnOnce(&mut Txn) -> Result<T>) -> Result<Completed<T>> {
        let base = self.shared.cell.load();
        let mut meta = TxnMeta::new(self.ids.generate(), self.session);
        meta.parent = self.last;
        let txn = Txn::new(&base, meta, self.txn_limits);
        let hooks = self
            .shared
            .hooks
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let commit_hooks = CommitHooks {
            cell: &self.shared.cell,
            limits: &self.limits,
            validators: &hooks.validators,
            graph: Some(&self.shared.graph),
            journal: hooks.journal.as_deref(),
            diagnostics: hooks.diagnostics.as_deref(),
            events: Some(&self.shared.events),
        };
        let body = catch_unwind(AssertUnwindSafe(move || {
            let mut txn = txn;
            let value = job(&mut txn)?;
            commit(txn, &commit_hooks).map(|outcome| (value, outcome))
        }));
        match body {
            Ok(Ok((value, outcome))) => {
                if outcome.txn_id.is_some() {
                    self.last = outcome.txn_id;
                }
                Ok(Completed {
                    value,
                    snapshot: outcome.snapshot,
                    txn_id: outcome.txn_id,
                    diagnostics: outcome.diagnostics,
                })
            }
            Ok(Err(error)) => Err(error),
            Err(payload) => {
                if let Some(sink) = hooks.diagnostics.as_deref() {
                    let diagnostic = fault_diagnostic(fault(payload.as_ref()));
                    // A sink that unwinds as well is ignored: the submitter gets the fault.
                    drop(catch_unwind(AssertUnwindSafe(|| {
                        sink.deliver(&[diagnostic])
                    })));
                }
                Err(fault(payload.as_ref()))
            }
        }
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
