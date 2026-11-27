//! Running commands for the protocol: mutual exclusion per document, the change summary of a
//! run and preview runs (CORE-CMD-006).
//!
//! A preview is a dry run of the document ([`Invoker::dry_run`]): the command runs on the
//! executor against the current snapshot and is validated like a commit, then discarded. The
//! document, its journal and its undo state are untouched and no event is emitted. Real runs
//! still serialize per document ([`DocState::run`]): the macro recorder reads the revision a
//! run starts on, and the selection a run leaves is read back from the run's own record, both
//! of which another session's commit in between would disturb.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use arc_base::{Cancel, Error, ObjectId, TransactionId};
use arc_doc::Document;
use serde_json::{Map, Value, json};

use super::encode::{Summary, diagnostic_json};
use super::events::Hub;
use super::{Answer, Failure, Server};
use crate::command_line::{CommandLine, LineStep, Submitted, Target as LineTarget};
use crate::context::Session;
use crate::error::codes;
use crate::history::{self, Direction};
use crate::invoke::{Invocation, Outcome};
use crate::line;
use crate::prompt::{Accept, Prompt};

/// What the server shares between sessions about one document.
#[derive(Debug, Default)]
pub(super) struct DocState {
    /// Held for the whole of every run.
    run: Mutex<()>,
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

/// The kind names of the inputs a prompt accepts, as `command.prompt` reports them.
fn kind(accept: &Accept) -> &'static str {
    match accept {
        Accept::Point { .. } => "point",
        Accept::Distance { .. } => "distance",
        Accept::Angle { .. } => "angle",
        Accept::Pick { .. } => "pick",
        Accept::Selection { .. } => "selection",
        Accept::Text { .. } => "text",
        Accept::Number { .. } => "number",
        Accept::Expression => "expression",
        Accept::Keyword { .. } => "keyword",
    }
}

/// A prompt as `command.prompt` answers it.
fn prompt_json(prompt: &Prompt) -> Value {
    json!({
        "key": prompt.key,
        "messageId": prompt.message.id,
        "arguments": prompt.message.args,
        "accepts": prompt.accepts.iter().map(kind).collect::<Vec<_>>(),
        "hasDefault": prompt.default.is_some(),
    })
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
            return self.preview(document, session, invocation);
        }
        let outcome = self.invoke(document, session, invocation, true)?;
        Ok(Self::settle(hub, doc_id, document, session, &outcome))
    }

    /// The answer of a run that committed: the selection it left goes to `hub`, and the result
    /// names the change.
    fn settle(
        hub: Option<&Hub>,
        doc_id: &str,
        document: &Document,
        session: &Session,
        outcome: &Outcome,
    ) -> Value {
        let after = outcome
            .selection
            .clone()
            .or_else(|| selection_after(document, outcome.txn_id));
        let limit = usize::try_from(session.limits.array_length).unwrap_or(usize::MAX);
        if let (Some(hub), Some(ids)) = (hub, after) {
            hub.selection_after(doc_id, &document.snapshot(), ids, limit);
        }
        let extra = Extra {
            summary: Summary::of_transaction(document, outcome.txn_id),
            base_revision: None,
            revision: outcome.revision,
        };
        run_result(outcome, &extra)
    }

    /// Answers `command.prompt`: `text` goes to the command line this session keeps for the
    /// document ([`CommandLine::submit`]), so it answers the prompt that is asked
    /// (`PromptSession::answer_text`), or, with none asked, starts the command it names. A
    /// command that completes runs through [`Self::invoke`] like `command.run`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the worker's state of one request"
    )]
    pub(super) fn prompt_job(
        &self,
        hub: Option<&Hub>,
        lines: &mut BTreeMap<String, CommandLine>,
        target: &Target<'_>,
        session: &Session,
        text: &str,
        seed: u64,
        cancel: &Cancel,
    ) -> Answer {
        let Target {
            id: doc_id,
            document,
            state,
        } = *target;
        let _run = Self::lock_run(state);
        if cancel.is_cancelled() {
            return Err(Error::new(codes::CANCELLED).into());
        }
        let line = lines.entry(doc_id.to_owned()).or_default();
        let localized = BTreeMap::new();
        if line.active().is_none()
            && let Some(parsed) = line::parse_line(self.registry, &localized, text)?
        {
            // A command that is not exposed on this surface is not started.
            self.command(&parsed.command)?;
        }
        let selection =
            hub.map_or_else(Vec::new, |h| h.selection_ids(doc_id, &document.snapshot()));
        let step_target = LineTarget {
            registry: self.registry,
            document,
            session,
            selection: &selection,
            localized: &localized,
            seed,
        };
        let mut ran = None;
        let mut extra = Map::new();
        let mut runner = |mut invocation: Invocation| -> arc_base::Result<Outcome> {
            invocation.cancel = cancel.clone();
            match self.invoke(document, session, invocation, true) {
                Ok(outcome) => {
                    ran = Some(Self::settle(hub, doc_id, document, session, &outcome));
                    Ok(outcome)
                }
                Err(failure) => {
                    let (error, more) = *failure.0;
                    extra = more;
                    Err(error)
                }
            }
        };
        let submitted = line.drive(&step_target, LineStep::Submit(text), &mut runner);
        let submitted = submitted.map_err(|error| Failure(Box::new((error, extra))))?;
        Ok(match submitted {
            Submitted::Idle => json!({"state": "idle"}),
            Submitted::Cancelled => json!({"state": "cancelled"}),
            Submitted::Prompt(prompt) => json!({"state": "prompt", "prompt": prompt_json(&prompt)}),
            Submitted::Rejected { error, prompt } => json!({
                "state": "rejected",
                "error": super::errors::error_object(&error, Map::new())["data"],
                "prompt": prompt_json(&prompt),
            }),
            Submitted::Ran(_) => {
                let mut result = ran.unwrap_or_else(|| json!({}));
                if let Some(map) = result.as_object_mut() {
                    map.insert("state".into(), json!("ran"));
                }
                result
            }
        })
    }

    /// Steps the undo or redo stack of the document for `session`, after the requests queued
    /// before it. A cancelled request does not step. A step changes the document, which a
    /// macro cannot express, so a recording of the session is marked as not faithful.
    pub(super) fn step_job(
        &self,
        target: &Target<'_>,
        session: &Session,
        direction: Direction,
        cancel: &Cancel,
        pending: &super::Pending,
        key: &str,
    ) -> Answer {
        let _run = Self::lock_run(target.state);
        {
            // From here on the step runs to its end: `command.cancel` answers `cancelled: false`.
            let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
            if cancel.is_cancelled() {
                return Err(Error::new(codes::CANCELLED).into());
            }
            if let Some(entry) = pending.get_mut(key) {
                *entry = None;
            }
        }
        let stepped = history::step(target.document, session, direction)
            .map_err(|error| self.failed(error))?;
        if let Some(recorder) = self.recorder {
            recorder.mark_unrecordable();
        }
        Ok(json!({
            "command": stepped.command,
            "label": stepped.label,
            "revision": stepped.revision,
        }))
    }

    /// Runs `invocation` as a dry run and answers with the would-be change summary. The
    /// document is not changed; `revision` is the revision the command ran on.
    fn preview(&self, document: &Document, session: &Session, invocation: Invocation) -> Answer {
        let preview = self.dry_run(document, session, invocation)?;
        let extra = Extra {
            summary: Summary::of_ops(&preview.ops),
            base_revision: Some(preview.outcome.revision),
            revision: preview.outcome.revision,
        };
        Ok(run_result(&preview.outcome, &extra))
    }
}
