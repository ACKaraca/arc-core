//! The background save (CORE-DOC-045): a save of one snapshot on a storage thread, with progress
//! and cooperative cancellation, while the document keeps committing. The snapshot is immutable,
//! so the saved state is exactly the state it holds whatever is committed meanwhile.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use arc_base::{Error, Limits, ObjectId, Result};

use super::incremental::{Saved, Strategy};
use super::preview::Preview;
use super::write::{Save, SaveProgress};
use super::{Metadata, Package};
use crate::diagnostic::Diagnostic;
use crate::error::codes;
use crate::load::{Loaded, StoredRecord};
use crate::snapshot::Snapshot;

/// What a [`BackgroundSave`] saves: the owned form of [`Save`] and the [`Strategy`].
#[derive(Debug)]
pub struct SaveRequest {
    /// The state to save.
    pub snapshot: Snapshot,
    /// The load the snapshot descends from, if any.
    pub loaded: Option<Arc<Loaded>>,
    /// The objects changed since load and the bytes of the transaction that last changed each.
    pub touched: BTreeMap<ObjectId, Vec<u8>>,
    /// The metadata; the save writes the next `save_counter`.
    pub metadata: Metadata,
    /// The previews the application rendered.
    pub previews: Vec<Preview>,
    /// The journal records of the saved state ([`Save::journal`]).
    pub journal: Vec<StoredRecord>,
    /// The document's diagnostics ([`Save::diagnostics`]).
    pub diagnostics: Vec<Diagnostic>,
    /// The limits payloads are encoded under.
    pub limits: Limits,
    /// How to save.
    pub strategy: Strategy,
}

/// A finished background save: the package, handed back, and the result of the save.
#[derive(Debug)]
pub struct Finished {
    /// The package, with its new metadata after a successful save in place.
    pub package: Package,
    /// What the save wrote, or why it failed; a failed or cancelled save left the file as it
    /// was.
    pub result: Result<Saved>,
}

/// A save running on a storage thread of its own. It owns the package while it runs, so a
/// package runs one save at a time and nothing else writes to it meanwhile; [`join`] hands it
/// back. Dropping the handle without joining lets the save finish and drops the package
/// afterwards, which releases its edit lock.
///
/// [`join`]: BackgroundSave::join
#[derive(Debug)]
pub struct BackgroundSave {
    cancel: Arc<AtomicBool>,
    slot: Arc<Mutex<Option<Package>>>,
    thread: Option<JoinHandle<Result<Saved>>>,
    early: Option<Error>,
}

impl BackgroundSave {
    /// Starts saving `request` into `package` on a new storage thread. `progress` is called on
    /// that thread before each step and every [`super::ROWS_PER_TICK`] rows. When the thread
    /// cannot start, the save fails with `doc.package.io_failed` and [`join`] hands the package
    /// back at once.
    ///
    /// [`join`]: BackgroundSave::join
    pub fn start(
        package: Package,
        request: SaveRequest,
        mut progress: impl FnMut(SaveProgress) + Send + 'static,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(Mutex::new(Some(package)));
        let (thread_cancel, thread_slot) = (Arc::clone(&cancel), Arc::clone(&slot));
        let spawned = std::thread::Builder::new()
            .name("arc-doc-save".into())
            .spawn(move || {
                let taken = thread_slot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                let Some(mut package) = taken else {
                    return Err(Error::new(codes::PACKAGE_SAVE_PANICKED));
                };
                let result = catch_unwind(AssertUnwindSafe(|| {
                    run(&mut package, &request, &thread_cancel, &mut progress)
                }))
                .unwrap_or_else(|_| Err(Error::new(codes::PACKAGE_SAVE_PANICKED)));
                *thread_slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(package);
                result
            });
        let (thread, early) = match spawned {
            Ok(thread) => (Some(thread), None),
            Err(error) => (
                None,
                Some(
                    Error::new(codes::PACKAGE_IO_FAILED)
                        .with_arg("path", "storage thread")
                        .with_source(error),
                ),
            ),
        };
        Self {
            cancel,
            slot,
            thread,
            early,
        }
    }

    /// Asks the save to stop. It stops at its next step or row report before the final commit
    /// or rename and fails with `base.cancelled`, leaving the file as it was; a save already
    /// past that point completes.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// True when the save has ended and [`BackgroundSave::join`] returns without waiting.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Waits for the save to end and hands back the package with the result. Fails with
    /// `doc.package.save_panicked` only when the storage thread was lost with the package,
    /// which then was dropped and released its edit lock.
    pub fn join(mut self) -> Result<Finished> {
        let result = match (self.thread.take(), self.early.take()) {
            (Some(thread), _) => thread
                .join()
                .unwrap_or_else(|_| Err(Error::new(codes::PACKAGE_SAVE_PANICKED))),
            (None, Some(error)) => Err(error),
            (None, None) => Err(Error::new(codes::PACKAGE_SAVE_PANICKED)),
        };
        let package = self
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| Error::new(codes::PACKAGE_SAVE_PANICKED))?;
        Ok(Finished { package, result })
    }
}

/// The save on the storage thread.
fn run(
    package: &mut Package,
    request: &SaveRequest,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(SaveProgress),
) -> Result<Saved> {
    let save = Save {
        snapshot: &request.snapshot,
        loaded: request.loaded.as_deref(),
        touched: &request.touched,
        metadata: &request.metadata,
        previews: &request.previews,
        journal: &request.journal,
        diagnostics: &request.diagnostics,
        limits: request.limits,
    };
    let cancelled = || {
        if cancel.load(Ordering::SeqCst) {
            Err(Error::new(arc_base::error::codes::CANCELLED))
        } else {
            Ok(())
        }
    };
    package.save(&save, &request.strategy, &mut |event| {
        cancelled()?;
        progress(event);
        // A cancellation requested while the callback ran stops the step it announced.
        cancelled()
    })
}
