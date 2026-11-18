//! Running commands for the protocol: mutual exclusion per document, the change summary of a
//! run and preview runs (CORE-CMD-006).
//!
//! A preview opens an undo group on the document, runs the command in it and rolls the group
//! back with [`Document::fail_group`]. The document's objects are identical afterwards, the undo
//! stack is as before, and the transactions of the run and of the rollback stay in the journal;
//! the revision advances by the two. Because an open group takes in every transaction committed
//! meanwhile, all runs through one server serialize per document ([`DocState::run`]).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use arc_base::{Error, ObjectId, TransactionId};
use arc_doc::Document;
use serde_json::{Value, json};

use super::encode::{Summary, diagnostic_json};
use super::events::Hub;
use super::{Answer, LogLevel, Server};
use crate::context::Session;
use crate::error::codes;
use crate::invoke::{Invocation, Outcome};
use crate::spec::UndoBehavior;

/// The identifier of the undo group a preview runs in.
const PREVIEW_GROUP: &str = "arc.preview";

/// How many preview transactions a document remembers, to keep them out of change events.
const HIDDEN: usize = 256;

/// What the server shares between sessions about one document.
#[derive(Debug, Default)]
pub(super) struct DocState {
    /// Held for the whole of every run, so a preview's group never takes in another run.
    run: Mutex<()>,
    /// The previews in flight.
    previews: AtomicUsize,
    /// The transactions of finished previews (the run and its rollback), newest last.
    hidden: Mutex<VecDeque<TransactionId>>,
}

impl DocState {
    /// True while a preview runs: its change events are not yet known.
    pub(super) fn previewing(&self) -> bool {
        self.previews.load(Ordering::SeqCst) > 0
    }

    /// True when `txn` belongs to a finished preview.
    pub(super) fn is_hidden(&self, txn: TransactionId) -> bool {
        self.hidden
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&txn)
    }

    fn hide(&self, txns: impl IntoIterator<Item = TransactionId>) {
        let mut hidden = self.hidden.lock().unwrap_or_else(PoisonError::into_inner);
        hidden.extend(txns);
        while hidden.len() > HIDDEN {
            hidden.pop_front();
        }
    }
}

/// Counts a preview in flight until dropped.
struct InFlight<'a>(&'a DocState);

impl<'a> InFlight<'a> {
    fn new(state: &'a DocState) -> Self {
        state.previews.fetch_add(1, Ordering::SeqCst);
        Self(state)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.previews.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a run answers with besides the outcome.
struct Extra {
    summary: Summary,
    /// For a preview: the revision the command ran on.
    base_revision: Option<u64>,
    revision: u64,
}

fn run_result(outcome: &Outcome, extra: &Extra) -> Value {
    let diagnostics: Vec<Value> = outcome.diagnostics.iter().map(diagnostic_json).collect();
    let mut result = json!({
        "command": outcome.command,
        "revision": extra.revision,
        "value": outcome.value,
        "diagnostics": diagnostics,
        "preview": extra.base_revision.is_some(),
        "summary": extra.summary.json(),
    });
    if let (Some(base), Some(map)) = (extra.base_revision, result.as_object_mut()) {
        map.insert("baseRevision".into(), json!(base));
    }
    result
}

/// The selection a transaction recorded to restore after it, as identifiers.
fn selection_after(document: &Document, txn: Option<TransactionId>) -> Option<Vec<ObjectId>> {
    let text = document.with_journal(|journal| {
        journal
            .get(txn?)
            .and_then(|entry| entry.record().selection_after.clone())
    })?;
    let ids: Vec<String> = serde_json::from_str(&text).ok()?;
    Some(ids.iter().filter_map(|id| id.parse().ok()).collect())
}

/// A document a run is aimed at.
pub(super) struct Target<'a> {
    pub(super) id: &'a str,
    pub(super) document: &'a Document,
    pub(super) state: &'a DocState,
}

impl Server<'_> {
    /// Locks the run lock of a document.
    fn lock_run(state: &DocState) -> MutexGuard<'_, ()> {
        state.run.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `invocation` on `document`, as a preview when `preview` is set. `hub` is the
    /// calling session's state: it supplies the selection the command sees and takes the
    /// selection the command leaves.
    pub(super) fn run_job(
        &self,
        hub: Option<&Hub>,
        target: &Target<'_>,
        session: &Session,
        mut invocation: Invocation,
        preview: bool,
    ) -> Answer {
        let Target {
            id: doc_id,
            document,
            state,
        } = *target;
        let _run = Self::lock_run(state);
        if invocation.cancel.is_cancelled() {
            return Err(Error::new(codes::CANCELLED).into());
        }
        if let Some(hub) = hub {
            invocation.selection = hub.selection_ids(doc_id, &document.snapshot());
        }
        if preview {
            return self.preview(document, state, session, invocation);
        }
        let outcome = self.invoke(document, session, invocation, true)?;
        if let (Some(hub), Some(ids)) = (hub, selection_after(document, outcome.txn_id)) {
            hub.selection_after(doc_id, &document.snapshot(), ids);
        }
        let extra = Extra {
            summary: Summary::of_transaction(document, outcome.txn_id),
            base_revision: None,
            revision: outcome.revision,
        };
        Ok(run_result(&outcome, &extra))
    }

    fn preview(
        &self,
        document: &Document,
        state: &DocState,
        session: &Session,
        invocation: Invocation,
    ) -> Answer {
        let spec = self
            .registry
            .resolve(&invocation.command, &std::collections::BTreeMap::new())?
            .spec;
        if spec.undo() == UndoBehavior::UndoControl {
            return Err(Error::new(codes::PREVIEW_UNSUPPORTED)
                .with_arg("command", spec.id())
                .into());
        }
        let _in_flight = InFlight::new(state);
        let base_revision = document.snapshot().revision();
        document.begin_group(PREVIEW_GROUP).wait()?;
        let ran = self.invoke(document, session, invocation, false);
        let rolled = document.fail_group().wait();
        let revision = document.snapshot().revision();
        let outcome = match rolled {
            Ok(done) => {
                let rollback = done.value.then(|| {
                    document
                        .with_journal(|journal| journal.last().map(|entry| entry.record().txn_id))
                });
                let run = ran.as_ref().ok().and_then(|outcome| outcome.txn_id);
                state.hide(run.into_iter().chain(rollback.flatten()));
                ran?
            }
            Err(error) => {
                // Close the group so that later commands do not join it; the previewed
                // changes then stay as an undoable step.
                let _ = document.end_group().wait();
                self.log
                    .log(LogLevel::Error, "preview.rollback", error.code());
                return Err(Error::new(codes::PREVIEW_ROLLBACK_FAILED)
                    .with_detail("rollback", error.code())
                    .into());
            }
        };
        let extra = Extra {
            summary: Summary::of_transaction(document, outcome.txn_id),
            base_revision: Some(base_revision),
            revision,
        };
        Ok(run_result(&outcome, &extra))
    }
}
