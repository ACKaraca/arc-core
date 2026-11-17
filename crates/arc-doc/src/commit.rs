//! Commit (CORE-DOC-015): validation in the order of the document model, publication, and the
//! steps that follow it.
//!
//! [`commit`] validates a finished [`Txn`] in four stages: (1) the field schema check of every
//! created, modified or recreated object; (2) the relationship rules ([`Txn::apply_delete_rules`],
//! whose cascades and clears become operations of the transaction), the read-only flag of
//! objects and the reference check ([`crate::integrity::check_references`]); (3) the type
//! validators of the touched objects; (4) the document validators. The first error of any stage
//! discards the transaction and is returned; nothing is published. Warnings become
//! [`Diagnostic`]s. Then the candidate is published ([`SnapshotCell::publish`]) and, in this
//! order, the journal sink records the transaction, the dependency graph applies the commit's
//! change set, the diagnostics sink receives the warnings and the `committed` event is emitted.
//! A step after publication that unwinds does not undo the publication: it is reported as a
//! `doc.executor.fault` diagnostic and the remaining steps run.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arc_base::{Error, Limits, Result, TransactionId};

use crate::diagnostic::{Diagnostic, DiagnosticSink, Origin};
use crate::error::codes;
use crate::events::{Changes, CommittedEvent, Event, EventBus};
use crate::graph::{ChangeSet, Graph};
use crate::integrity;
use crate::op::Op;
use crate::schema::Severity;
use crate::snapshot::{Snapshot, SnapshotCell};
use crate::txn::{Txn, TxnMeta, TxnParts};
use crate::validate::{self, DocumentValidator};

/// A published transaction, as the journal receives it.
#[derive(Debug)]
pub struct Committed {
    /// The metadata of the journal record.
    pub meta: TxnMeta,
    /// Every operation in order, including those the deletion rules added: applied to `before`
    /// they give `after`.
    pub ops: Vec<Op>,
    /// The snapshot the transaction applied to.
    pub before: Snapshot,
    /// The published snapshot.
    pub after: Snapshot,
}

/// Receives every published transaction, on the document executor, right after publication.
/// The journal implements it. It must not fail: whatever it cannot keep, it reports itself.
pub trait JournalSink: Send + Sync {
    /// Records one published transaction.
    fn record(&self, committed: &Committed);
}

/// Where a commit publishes and whom it informs.
#[derive(Clone, Copy)]
pub struct CommitHooks<'a> {
    /// The cell to publish in.
    pub cell: &'a SnapshotCell,
    /// The limits of the deletion rules (`record_count` bounds cascades).
    pub limits: &'a Limits,
    /// The document validators, in order.
    pub validators: &'a [DocumentValidator],
    /// The dependency graph to apply the change set to.
    pub graph: Option<&'a Graph>,
    /// The journal.
    pub journal: Option<&'a dyn JournalSink>,
    /// The receiver of warnings and faults.
    pub diagnostics: Option<&'a dyn DiagnosticSink>,
    /// The event bus for the `committed` event.
    pub events: Option<&'a EventBus>,
}

impl<'a> CommitHooks<'a> {
    /// Hooks that publish in `cell` and inform nobody.
    #[must_use]
    pub fn new(cell: &'a SnapshotCell, limits: &'a Limits) -> Self {
        Self {
            cell,
            limits,
            validators: &[],
            graph: None,
            journal: None,
            diagnostics: None,
            events: None,
        }
    }
}

/// What a successful commit produced.
#[derive(Debug)]
pub struct CommitOutcome {
    /// The published snapshot, or the unchanged base of a transaction without operations.
    pub snapshot: Snapshot,
    /// The published transaction; `None` when a transaction without operations published
    /// nothing.
    pub txn_id: Option<TransactionId>,
    /// The warnings of the validators, then any fault of a step after publication.
    pub diagnostics: Vec<Diagnostic>,
}

/// Validates `txn`, publishes it and informs the hooks.
///
/// A transaction without operations publishes nothing and returns its base. A transaction on a
/// read-only document fails with `doc.document.read_only`. Otherwise fails with the first error
/// of the validation stages, or `doc.snapshot.stale` when the published snapshot is no longer
/// the transaction's base; in every failure the published snapshot is untouched and the
/// transaction is dropped. Costs time proportional to the touched objects, the references they
/// hold and their referrers, plus what document validators read.
pub fn commit(mut txn: Txn, hooks: &CommitHooks<'_>) -> Result<CommitOutcome> {
    if txn.ops().is_empty() {
        return Ok(CommitOutcome {
            snapshot: txn.base().clone(),
            txn_id: None,
            diagnostics: Vec::new(),
        });
    }
    if let Some(reason) = txn.base().read_only().first() {
        return Err(reason.error());
    }
    validate::check_fields(txn.snapshot(), txn.touched())?;
    txn.apply_delete_rules(hooks.limits)?;
    let TxnParts { set, ops, meta, .. } = txn.into_parts();
    let (base, candidate, touched) = (set.base(), set.current(), set.touched());
    validate::check_read_only_objects(base, candidate, touched)?;
    integrity::check_references(&set)?;
    let mut diagnostics = Vec::new();
    validate::run_type_validators(candidate, touched, &mut diagnostics)?;
    validate::run_document_validators(
        hooks.validators,
        base,
        candidate,
        touched,
        &mut diagnostics,
    )?;
    hooks.cell.publish(&set)?;

    let after = candidate.clone();
    let mut faults = Vec::new();
    let changes = Changes::of_commit(base, candidate, touched);
    let graph_changes = hooks
        .graph
        .is_some()
        .then(|| guarded(&mut faults, || ChangeSet::from_commit(&set, &ops)))
        .flatten();
    let committed = Committed {
        meta,
        ops,
        before: base.clone(),
        after: after.clone(),
    };
    if let Some(journal) = hooks.journal {
        guarded(&mut faults, || journal.record(&committed));
    }
    if let (Some(graph), Some(changes)) = (hooks.graph, graph_changes) {
        guarded(&mut faults, || graph.apply(&changes, after.clone()));
    }
    diagnostics.extend(faults);
    if let Some(sink) = hooks.diagnostics.filter(|_| !diagnostics.is_empty()) {
        let mut faults = Vec::new();
        guarded(&mut faults, || sink.deliver(&diagnostics));
        diagnostics.extend(faults);
    }
    if let Some(events) = hooks.events {
        let event = Event::Committed(CommittedEvent {
            revision: after.revision(),
            txn_id: committed.meta.id,
            kind: committed.meta.kind,
            changes: std::sync::Arc::new(changes),
            commits: 1,
            snapshot: after.clone(),
        });
        let mut faults = Vec::new();
        guarded(&mut faults, || events.emit(&event));
        diagnostics.extend(faults);
    }
    Ok(CommitOutcome {
        snapshot: after,
        txn_id: Some(committed.meta.id),
        diagnostics,
    })
}

/// Runs `step`, turning an unwind into a fault diagnostic in `faults`.
fn guarded<T>(faults: &mut Vec<Diagnostic>, step: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(step)) {
        Ok(value) => Some(value),
        Err(payload) => {
            faults.push(fault_diagnostic(fault(payload.as_ref())));
            None
        }
    }
}

/// The `doc.executor.fault` error of an unwind, with its message as the detail `panic`.
pub(crate) fn fault(payload: &(dyn std::any::Any + Send)) -> Error {
    let message = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    Error::new(codes::EXECUTOR_FAULT).with_detail("panic", message)
}

/// The error diagnostic of a fault.
pub(crate) fn fault_diagnostic(error: Error) -> Diagnostic {
    Diagnostic::new(Severity::Error, error, Vec::new(), Origin::Executor)
}
