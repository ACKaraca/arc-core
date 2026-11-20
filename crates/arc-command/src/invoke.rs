//! Guarded execution (CORE-CMD-003, CORE-CMD-004, CORE-CMD-005, CORE-CMD-007, CORE-CMD-008).
//!
//! [`Invoker::invoke`] is the one entry point that runs a command. In order it resolves the
//! name, checks the exposure for the session's surface (`cmd.not_exposed`), the permission
//! classes (`cmd.permission_denied`), the parameter limits and schema
//! (`cmd.params.limit_exceeded`, `cmd.params.invalid`) and the enabled predicate
//! (`cmd.disabled`), then runs the command:
//!
//! - an `undoable` command runs as a job of [`Document::submit`]: the predicate is evaluated on
//!   the job's base snapshot, `run` gets the transaction, and the executor commits. Any error,
//!   a cancellation (`cmd.cancelled`) or an escaped panic (`cmd.fault`) drops the transaction,
//!   so the published snapshot stays the very same;
//! - a `not_undoable` or `undo_control` command runs on the latest snapshot without a
//!   transaction, under its own unwind boundary.
//!
//! Every panic on the way, in parameter decoding, the predicate, `run` or result encoding, is
//! contained and reported as `cmd.fault` with the panic message in the detail `panic`.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use arc_base::{Cancel, Category, Error, ObjectId, Result, TransactionId};
use arc_doc::{Diagnostic, Document, Op, Snapshot, Txn};
use serde_json::{Value, json};

use crate::context::{Context, ProgressSink, Session};
use crate::error::codes;
use crate::registry::{NameFlags, Registry};
use crate::schema;
use crate::selection;
use crate::settings::Writes;
use crate::spec::{CommandSpec, UndoBehavior};

/// One call of a command.
#[derive(Clone)]
pub struct Invocation {
    /// The command identifier, an alias, a localized name or a name a product's resolver owns
    /// (see [`Registry::resolve`]).
    pub command: String,
    /// The parameters, a JSON object.
    pub params: Value,
    /// The selection the command sees.
    pub selection: Vec<ObjectId>,
    /// The seed of the context's identifiers and random numbers.
    pub seed: u64,
    /// The identifier of the caller's open undo group, written to the transaction.
    pub group: Option<String>,
    /// The cancellation token; cancelling rolls the command back.
    pub cancel: Cancel,
    /// Where progress goes.
    pub progress: Option<Arc<dyn ProgressSink>>,
    /// The revision the document must be at when the command starts; otherwise the invocation
    /// fails with `cmd.revision.mismatch` without running.
    pub expected_revision: Option<u64>,
    /// The call makes the document's starting content (what a product's new-document command
    /// creates). Its transaction commits as bookkeeping (journal kind `merge`): it is recorded, so
    /// the document saves and recovers with it, but it is no undo step, so undo never removes
    /// the starting content. Unlike `Context::commit_silently` it may change anything, and is
    /// accepted only on a document at revision 0, where no step exists whose inverse it could
    /// break; otherwise the call fails with `cmd.silent_scope`.
    pub initial: bool,
}

impl std::fmt::Debug for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Invocation")
            .field("command", &self.command)
            .field("params", &self.params)
            .field("seed", &self.seed)
            .finish_non_exhaustive()
    }
}

impl Invocation {
    /// A call of `command` with `params`, no selection, seed 0, no group, a fresh token, no
    /// progress sink and no expected revision.
    #[must_use]
    pub fn new(command: impl Into<String>, params: Value) -> Self {
        Self {
            command: command.into(),
            params,
            selection: Vec::new(),
            seed: 0,
            group: None,
            cancel: Cancel::new(),
            progress: None,
            expected_revision: None,
            initial: false,
        }
    }
}

/// What a successful invocation returns.
#[derive(Debug)]
pub struct Outcome {
    /// The identifier of the command that ran.
    pub command: String,
    /// The result, valid against the command's result schema.
    pub value: Value,
    /// The revision of the snapshot published by the command, or read by it.
    pub revision: u64,
    /// The committed transaction; `None` when nothing changed or no transaction was opened.
    pub txn_id: Option<TransactionId>,
    /// The coalesce key to report with the transaction to the undo manager.
    pub coalesce: Option<String>,
    /// The commit's diagnostics, and a deprecation warning when an alias was deprecated.
    pub diagnostics: Vec<Diagnostic>,
    /// The selection the command set with [`Context::set_selection`], now the session's current
    /// selection; `None` when it set none, or for a dry run.
    pub selection: Option<Vec<ObjectId>>,
}

/// What a dry run returns: the outcome as a run on the same snapshot would give it, and the
/// operations the run would have committed.
#[derive(Debug)]
pub struct Preview {
    /// The outcome; `revision` is the revision the command ran on, `txn_id` is `None`.
    pub outcome: Outcome,
    /// The operations of the would-be transaction, deletion-rule cascades included; empty for a
    /// command that is not `undoable` or changed nothing.
    pub ops: Vec<Op>,
}

/// Runs commands of a registry.
#[derive(Debug, Clone, Copy)]
pub struct Invoker<'r> {
    registry: &'r Registry,
}

/// The `cmd.fault` error of a panic payload.
fn fault(payload: &(dyn Any + Send)) -> Error {
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Error::new(codes::FAULT).with_detail("panic", message)
}

/// Maps the errors of the executor and of cancellation to this crate's codes.
fn map_error(error: Error) -> Error {
    match error.code() {
        "doc.executor.fault" => {
            let panic = error.details().get("panic").cloned();
            let mapped = Error::new(codes::FAULT);
            match panic {
                Some(panic) => mapped.with_detail("panic", panic),
                None => mapped,
            }
        }
        _ if error.category() == Category::Cancelled => Error::new(codes::CANCELLED),
        _ => error,
    }
}

/// `cmd.revision.mismatch` unless `snapshot` is at the expected revision.
fn expect_revision(expected: Option<u64>, snapshot: &Snapshot) -> Result<()> {
    match expected {
        Some(expected) if expected != snapshot.revision() => {
            let as_int = |r: u64| i64::try_from(r).unwrap_or(i64::MAX);
            Err(Error::new(codes::REVISION_MISMATCH)
                .with_arg("expected", as_int(expected))
                .with_detail("expected", as_int(expected))
                .with_detail("current", as_int(snapshot.revision())))
        }
        _ => Ok(()),
    }
}

fn disabled(spec: &CommandSpec, probe: &crate::context::Probe<'_>) -> Result<()> {
    spec.enabled(probe).map_err(|reason| {
        reason.arguments.into_iter().fold(
            Error::new(codes::DISABLED)
                .with_arg("command", spec.id())
                .with_detail("reason", reason.message_id),
            |error, (name, value)| error.with_detail(name, value),
        )
    })
}

impl<'r> Invoker<'r> {
    /// An invoker of `registry`'s commands.
    #[must_use]
    pub fn new(registry: &'r Registry) -> Self {
        Self { registry }
    }

    /// Runs `invocation` for `session` on `document` and waits for the outcome. The checks and
    /// execution are described in the [module documentation](self).
    ///
    /// # Errors
    ///
    /// `cmd.unknown`, `cmd.not_exposed`, `cmd.permission_denied`,
    /// `cmd.params.limit_exceeded`, `cmd.params.invalid`, `cmd.disabled`, `cmd.cancelled`,
    /// `cmd.fault`, `cmd.result.invalid`, the command's own errors, and the commit's errors.
    pub fn invoke(
        &self,
        document: &Document,
        session: &Session,
        invocation: Invocation,
    ) -> Result<Outcome> {
        catch_unwind(AssertUnwindSafe(|| {
            self.guarded(document, session, invocation, false)
        }))
        .unwrap_or_else(|payload| Err(fault(payload.as_ref())))
        .map(|preview| preview.outcome)
    }

    /// Runs `invocation` through the same resolution, exposure, permission, parameter,
    /// revision and enabled checks as [`Invoker::invoke`], and an `undoable` command in a
    /// dry run of the document ([`Document::dry_run`]) that validates like a commit and
    /// discards the result: the document, its journal and its undo state stay as they were.
    /// The identifiers the command generates are those a real run on the same revision
    /// generates.
    ///
    /// # Errors
    ///
    /// Those of [`Invoker::invoke`], and `cmd.preview.unsupported` for an `undo_control`
    /// command.
    pub fn dry_run(
        &self,
        document: &Document,
        session: &Session,
        invocation: Invocation,
    ) -> Result<Preview> {
        catch_unwind(AssertUnwindSafe(|| {
            self.guarded(document, session, invocation, true)
        }))
        .unwrap_or_else(|payload| Err(fault(payload.as_ref())))
    }

    fn guarded(
        &self,
        document: &Document,
        session: &Session,
        mut invocation: Invocation,
        dry: bool,
    ) -> Result<Preview> {
        let resolved = self
            .registry
            .resolve(&invocation.command, &std::collections::BTreeMap::new())?;
        let mut diagnostics: Vec<Diagnostic> = resolved
            .deprecation(&invocation.command)
            .into_iter()
            .collect();
        let spec = resolved.spec;
        let flags = resolved.flags;
        if !spec.exposure().contains(&session.surface) {
            return Err(Error::new(codes::NOT_EXPOSED).with_arg("command", spec.id()));
        }
        if let Some(missing) = spec
            .permissions()
            .iter()
            .find(|class| !session.permissions.contains(class))
        {
            let name = serde_json::to_value(missing)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::new(codes::PERMISSION_DENIED)
                .with_arg("command", spec.id())
                .with_detail("permission", name));
        }
        // Keywords of identifier lists (`selection`, `previous`, `last`) stand for the session's
        // identifiers, so they are expanded before the schema check sees the parameters.
        selection::expand(
            spec.params_schema(),
            &mut invocation.params,
            session,
            &document.snapshot(),
        )?;
        let violations = spec.validate(&invocation.params, &session.limits)?;
        if !violations.is_empty() {
            return Err(schema::invalid(&violations));
        }
        if dry && spec.undo() == UndoBehavior::UndoControl {
            return Err(Error::new(codes::PREVIEW_UNSUPPORTED).with_arg("command", spec.id()));
        }
        let coalesce = spec.coalesce().map(str::to_owned);
        let command = spec.id().to_owned();
        let (value, revision, txn_id, more, ops, requests) = match spec.undo() {
            UndoBehavior::Undoable => run_in_txn(document, session, invocation, spec, flags, dry)?,
            UndoBehavior::NotUndoable | UndoBehavior::UndoControl => {
                let snapshot = document.snapshot();
                let (value, requests) =
                    run_on_snapshot(&snapshot, session, &invocation, &spec, flags)?;
                (
                    value,
                    snapshot.revision(),
                    None,
                    Vec::new(),
                    Vec::new(),
                    requests,
                )
            }
        };
        // The selection and the settings are session state: applied after the command succeeded,
        // never journaled, and not at all on a dry run.
        let (requested, writes) = requests;
        let selection = if dry { None } else { requested };
        if !dry {
            session.settings.apply(&writes)?;
        }
        if let Some(ids) = &selection {
            session.record_selection(ids, &document.snapshot())?;
        }
        diagnostics.extend(more);
        Ok(Preview {
            outcome: Outcome {
                command,
                value,
                revision,
                txn_id,
                coalesce,
                diagnostics,
                selection,
            },
            ops,
        })
    }
}

fn run_on_snapshot(
    snapshot: &Snapshot,
    session: &Session,
    invocation: &Invocation,
    spec: &CommandSpec,
    flags: NameFlags,
) -> Result<(Value, Requests)> {
    let probe = crate::context::Probe {
        snapshot,
        selection: &invocation.selection,
        session,
    };
    expect_revision(invocation.expected_revision, snapshot)?;
    disabled(spec, &probe)?;
    let mut context = Context::new(
        None,
        snapshot,
        session,
        &invocation.selection,
        invocation.seed,
        &invocation.cancel,
        invocation.progress.as_deref(),
    )
    .with_flags(flags);
    let value = spec.call(&mut context, invocation.params.clone());
    let value = value.and_then(|value| context.checkpoint().map(|()| value));
    let requested = context.selection_request.take();
    let writes = std::mem::take(&mut context.settings_writes);
    value
        .map(|value| (value, (requested, writes)))
        .map_err(map_error)
}

/// What a command asks for once it succeeds: the selection and the setting writes.
type Requests = (Option<Vec<ObjectId>>, Writes);

type Ran = (
    Value,
    u64,
    Option<TransactionId>,
    Vec<Diagnostic>,
    Vec<Op>,
    Requests,
);

fn run_in_txn(
    document: &Document,
    session: &Session,
    invocation: Invocation,
    spec: Arc<CommandSpec>,
    flags: NameFlags,
    dry: bool,
) -> Result<Ran> {
    let caller = session.clone();
    let session = session.clone();
    let canonical = serde_json::to_string(&json!({
        "command": spec.id(),
        "params": invocation.params,
    }))
    .unwrap_or_default();
    let label = serde_json::to_string(&json!({ "message_id": spec.label() })).unwrap_or_default();
    let job = move |txn: &mut Txn| {
        let base = txn.base().clone();
        let probe = crate::context::Probe {
            snapshot: &base,
            selection: &invocation.selection,
            session: &session,
        };
        expect_revision(invocation.expected_revision, &base)?;
        disabled(&spec, &probe)?;
        let initial = invocation.initial;
        if initial && base.revision() != 0 {
            return Err(Error::new(codes::SILENT_SCOPE).with_arg("command", spec.id()));
        }
        let meta = txn.meta_mut();
        if initial {
            meta.kind = arc_doc::TxnKind::Merge;
        }
        meta.command = canonical;
        meta.label = label;
        meta.group = invocation.group.clone();
        meta.time_unix_millis = session.clock.now_unix_millis();
        let mut context = Context::new(
            Some(txn),
            &base,
            &session,
            &invocation.selection,
            invocation.seed,
            &invocation.cancel,
            invocation.progress.as_deref(),
        )
        .with_flags(flags);
        let value = spec.call(&mut context, invocation.params)?;
        context.checkpoint()?;
        let selection_after = context.selection_after.take();
        let silent = context.silent;
        let actions = std::mem::take(&mut context.after_commit);
        let requested = context.selection_request.take();
        let writes = std::mem::take(&mut context.settings_writes);
        drop(context);
        // A silent commit only adds rows under keys no operation of the history names, so the
        // inverse of every step on either stack still applies after it.
        let floor = txn.foreign_floor();
        let adds_only = txn.ops().iter().all(|op| {
            matches!(op, Op::ModifyForeign { id, before: None, after: Some(_) } if *id > floor)
        });
        if silent && !adds_only && !initial {
            return Err(Error::new(codes::SILENT_SCOPE).with_arg("command", spec.id()));
        }
        if let Some(ids) = selection_after {
            let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
            txn.meta_mut().selection_after = serde_json::to_string(&ids).ok();
        }
        Ok((value, (requested, writes), actions))
    };
    if dry {
        let done = document.dry_run(job).wait().map_err(map_error)?;
        let (value, requests, _) = done.value.value;
        return Ok((
            value,
            done.snapshot.revision(),
            None,
            done.diagnostics,
            done.value.ops,
            requests,
        ));
    }
    let completed = document.submit(job).wait().map_err(map_error)?;
    let (mut value, requests, actions) = completed.value;
    // The transaction is committed: the actions that were waiting for that run now.
    for action in actions {
        action(&caller, &mut value).map_err(map_error)?;
    }
    Ok((
        value,
        completed.snapshot.revision(),
        completed.txn_id,
        completed.diagnostics,
        Vec::new(),
        requests,
    ))
}
