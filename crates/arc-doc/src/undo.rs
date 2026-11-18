//! Undo and redo with groups, coalescing and limits (CORE-DOC-020 to CORE-DOC-023).
//!
//! The [`UndoManager`] keeps the undo and redo stacks of a session. A stack entry is a [`Step`]:
//! the identifiers of the journal transactions it contains, oldest first. The manager never
//! commits: [`UndoManager::undo`], [`UndoManager::redo`] and [`UndoManager::fail_group`] return
//! an ordinary [`Txn`] built on the current snapshot, which the caller commits through the same
//! path as any transaction and then reports, like every commit, with
//! [`UndoManager::committed`]. The stacks change only when a commit is reported, so a commit
//! that fails validation changes nothing.
//!
//! - **Undo** of a step applies the inverses of its operations in reverse order, kind
//!   `undo_of(last transaction of the step)`; the step moves to the redo stack.
//! - **Redo** of a step applies its original operations again, kind
//!   `redo_of(last transaction of the step)`; the redo transaction becomes a new undo step. Its
//!   undo is again the inverse of what it applied, so an undo of an undo is a redo.
//! - A reported `normal` or `link_reload` transaction clears the redo stack and forms a new step,
//!   joins the open group's step, or is coalesced into the top step. `merge` and `migration`
//!   transactions leave the stacks unchanged.
//! - **Groups** ([`UndoManager::begin_group`]) nest; only the outermost forms a step. Its
//!   members must carry the outermost group's identifier in their metadata.
//!   [`UndoManager::fail_group`] rolls back the transactions committed since the innermost open
//!   group began, with one transaction of kind `undo_of(last member)` that forms no step.
//! - **Coalescing**: within a gesture ([`UndoManager::begin_gesture`]), consecutive
//!   transactions reported with the same coalesce key form one step; the journal keeps every
//!   record.
//! - **Limits** ([`UndoLimits`]): after each change of the undo stack, its oldest steps are
//!   dropped while it holds more steps than allowed or its estimated memory exceeds the budget.
//!   Journal records stay.
//!
//! Operations apply strictly ([`Op::apply`]): if the current state no longer holds a step's
//! expected values, preparing fails with `doc.undo.state_mismatch`. In phase 0 nothing but the
//! session's own commands writes the document, so that is an internal error; refusing an undo
//! because a merge, link reload or change set changed its fields is CORE-DOC-024.

use std::collections::VecDeque;

use arc_base::{Error, ObjectId, Result, TransactionId};

use crate::error::codes;
use crate::journal::Journal;
use crate::op::Op;
use crate::schema::options_bits_eq;
use crate::snapshot::{Snapshot, WorkingSet};
use crate::txn::{Txn, TxnKind, TxnLimits, TxnMeta};

/// The default number of undo steps.
pub const DEFAULT_UNDO_STEPS: usize = 1_000;

/// The default memory budget of the undo stack, 1 GiB.
pub const DEFAULT_UNDO_MEMORY_BYTES: u64 = 1 << 30;

/// The bounds of the undo stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoLimits {
    /// The most steps.
    pub steps: usize,
    /// The most estimated memory of the steps' records
    /// ([`crate::journal::JournalEntry::memory_bytes`]).
    pub memory_bytes: u64,
}

impl Default for UndoLimits {
    /// [`DEFAULT_UNDO_STEPS`] and [`DEFAULT_UNDO_MEMORY_BYTES`].
    fn default() -> Self {
        Self {
            steps: DEFAULT_UNDO_STEPS,
            memory_bytes: DEFAULT_UNDO_MEMORY_BYTES,
        }
    }
}

/// One undo step: the transactions it contains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    txns: Vec<TransactionId>,
    bytes: u64,
    /// The gesture and coalesce key of a step that later transactions may join.
    coalesce: Option<(u64, String)>,
}

impl Step {
    /// The transactions of the step, oldest first.
    #[must_use]
    pub fn transactions(&self) -> &[TransactionId] {
        &self.txns
    }

    /// The estimated memory of the step's records.
    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        self.bytes
    }

    fn push(&mut self, txn: TransactionId, bytes: u64) {
        self.txns.push(txn);
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

#[derive(Debug, Clone)]
struct Level {
    id: String,
    /// The number of transactions of the open step when the group began.
    start: usize,
}

/// The undo and redo stacks of a session with its open groups and gesture.
#[derive(Debug, Clone, Default)]
pub struct UndoManager {
    limits: UndoLimits,
    undo: VecDeque<Step>,
    undo_bytes: u64,
    redo: Vec<Step>,
    /// The open groups, outermost first, and the step they collect.
    groups: Vec<Level>,
    open: Step,
    gesture: Option<u64>,
    next_gesture: u64,
    /// The last member of a rolled-back group whose rollback has not been reported yet.
    rollback: Option<TransactionId>,
}

impl UndoManager {
    /// An empty manager with `limits`.
    #[must_use]
    pub fn new(limits: UndoLimits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// The limits.
    #[must_use]
    pub fn limits(&self) -> UndoLimits {
        self.limits
    }

    /// Replaces the limits and drops the oldest steps that exceed them.
    pub fn set_limits(&mut self, limits: UndoLimits) {
        self.limits = limits;
        self.trim();
    }

    /// The undo steps, oldest first; the last is undone next.
    pub fn undo_steps(&self) -> impl DoubleEndedIterator<Item = &Step> + '_ {
        self.undo.iter()
    }

    /// The redo steps, oldest first; the last is redone next.
    pub fn redo_steps(&self) -> impl DoubleEndedIterator<Item = &Step> + '_ {
        self.redo.iter()
    }

    /// The estimated memory of the undo steps.
    #[must_use]
    pub fn undo_memory_bytes(&self) -> u64 {
        self.undo_bytes
    }

    /// The identifier of the outermost open group, which the transactions committed while it
    /// is open carry in their metadata.
    #[must_use]
    pub fn group(&self) -> Option<&str> {
        self.groups.first().map(|level| level.id.as_str())
    }

    /// Opens a group with identifier `id`, nested in the open groups.
    pub fn begin_group(&mut self, id: impl Into<String>) {
        if self.groups.is_empty() {
            self.open = Step::default();
        }
        self.groups.push(Level {
            id: id.into(),
            start: self.open.txns.len(),
        });
    }

    /// Closes the innermost open group. Closing the outermost pushes its step, when it holds a
    /// transaction, onto the undo stack. Fails with `doc.undo.sequence_invalid` when no group
    /// is open.
    pub fn end_group(&mut self) -> Result<()> {
        if self.groups.pop().is_none() {
            return Err(sequence("no group is open"));
        }
        if self.groups.is_empty() {
            let step = std::mem::take(&mut self.open);
            if !step.txns.is_empty() {
                self.push_undo(step);
            }
        }
        Ok(())
    }

    /// Closes the innermost open group because its owner failed, and returns the transaction
    /// that rolls back its members, the transactions reported since it began: their inverse
    /// operations in reverse order, kind `undo_of(last member)`, metadata otherwise `meta`.
    /// `None` when it has no member. The members leave the open step, and the rollback, once
    /// reported, joins no step, so the group leaves no residue in the stacks. Fails with
    /// `doc.undo.sequence_invalid` when no group is open, or with the codes of
    /// [`UndoManager::undo`]; a failure changes nothing.
    pub fn fail_group(
        &mut self,
        journal: &Journal,
        base: &Snapshot,
        meta: TxnMeta,
        limits: TxnLimits,
    ) -> Result<Option<Txn>> {
        let start = self
            .groups
            .last()
            .ok_or_else(|| sequence("no group is open"))?
            .start;
        let members = self.open.txns.get(start..).unwrap_or_default();
        let txn = match members.last() {
            Some(last) => Some(prepare(
                base,
                meta,
                limits,
                TxnKind::UndoOf(*last),
                &inverse_ops(journal, members)?,
            )?),
            None => None,
        };
        self.rollback = members.last().copied();
        let removed = self.open.txns.split_off(start);
        self.open.bytes = self.open.bytes.saturating_sub(memory_of(journal, &removed));
        self.groups.pop();
        Ok(txn)
    }

    /// Starts a gesture, ending the previous one: until [`UndoManager::end_gesture`],
    /// consecutive transactions reported with the same coalesce key form one step.
    pub fn begin_gesture(&mut self) {
        self.gesture = Some(self.next_gesture);
        self.next_gesture = self.next_gesture.wrapping_add(1);
    }

    /// Ends the gesture; later transactions start new steps.
    pub fn end_gesture(&mut self) {
        self.gesture = None;
    }

    /// The transaction that undoes the top undo step on `base`, the current snapshot, with
    /// metadata `meta` and kind `undo_of(last transaction of the step)`, or `None` when there
    /// is nothing to undo. Fails with `doc.undo.sequence_invalid` inside an open group,
    /// `doc.undo.state_mismatch` when `base` does not hold the step's after state, or the codes
    /// of [`Txn`]'s operations (such as `doc.txn.limit_exceeded`).
    pub fn undo(
        &self,
        journal: &Journal,
        base: &Snapshot,
        meta: TxnMeta,
        limits: TxnLimits,
    ) -> Result<Option<Txn>> {
        self.outside_group()?;
        let Some(step) = self.undo.back() else {
            return Ok(None);
        };
        let Some(last) = step.txns.last() else {
            return Ok(None);
        };
        let ops = inverse_ops(journal, &step.txns)?;
        prepare(base, meta, limits, TxnKind::UndoOf(*last), &ops).map(Some)
    }

    /// The transaction that redoes the top redo step on `base` by applying its original
    /// operations again, kind `redo_of(last transaction of the step)`, or `None` when there is
    /// nothing to redo. Fails like [`UndoManager::undo`].
    pub fn redo(
        &self,
        journal: &Journal,
        base: &Snapshot,
        meta: TxnMeta,
        limits: TxnLimits,
    ) -> Result<Option<Txn>> {
        self.outside_group()?;
        let Some(step) = self.redo.last() else {
            return Ok(None);
        };
        let Some(last) = step.txns.last() else {
            return Ok(None);
        };
        let mut ops = Vec::new();
        for id in &step.txns {
            ops.extend(entry(journal, *id)?.record().ops.iter().cloned());
        }
        prepare(base, meta, limits, TxnKind::RedoOf(*last), &ops).map(Some)
    }

    /// Reports the committed transaction `txn`, already appended to `journal`, with the
    /// coalesce key the command layer gave it. By its kind:
    ///
    /// - `normal` and `link_reload`: clears the redo stack; joins the open group's step (its
    ///   group must be the outermost open group), or the top step when a gesture is open and
    ///   the top step was formed in it with the same coalesce key, or forms a new step;
    /// - `undo_of(t)`: the rollback of [`UndoManager::fail_group`], or the undo of the top
    ///   undo step, which moves to the redo stack;
    /// - `redo_of(t)`: the redo of the top redo step, which leaves the redo stack while the
    ///   redo transaction forms a new undo step;
    /// - `merge` and `migration`: nothing.
    ///
    /// Fails with `doc.undo.sequence_invalid`, changing nothing, when `txn` is not in the
    /// journal, a group member carries another group, or an undo or redo is not of the step on
    /// top of its stack.
    pub fn committed(
        &mut self,
        journal: &Journal,
        txn: TransactionId,
        coalesce: Option<&str>,
    ) -> Result<()> {
        let entry = entry(journal, txn)?;
        let record = entry.record();
        let bytes = entry.memory_bytes();
        match record.kind {
            TxnKind::Normal | TxnKind::LinkReload => {
                if let Some(level) = self.groups.first() {
                    if record.group.as_deref() != Some(level.id.as_str()) {
                        return Err(sequence("the transaction is not of the open group"));
                    }
                    self.open.push(txn, bytes);
                    self.redo.clear();
                    return Ok(());
                }
                self.redo.clear();
                let key = self.gesture.zip(coalesce.map(str::to_owned));
                if let Some(top) = self.undo.back_mut()
                    && key.is_some()
                    && top.coalesce == key
                {
                    top.push(txn, bytes);
                    self.undo_bytes = self.undo_bytes.saturating_add(bytes);
                    self.trim();
                    return Ok(());
                }
                let mut step = Step {
                    coalesce: key,
                    ..Step::default()
                };
                step.push(txn, bytes);
                self.push_undo(step);
            }
            TxnKind::UndoOf(target) => {
                if self.rollback == Some(target) {
                    self.rollback = None;
                    return Ok(());
                }
                if self.undo.back().and_then(|step| step.txns.last()) != Some(&target) {
                    return Err(sequence("the undo is not of the top undo step"));
                }
                if let Some(mut step) = self.undo.pop_back() {
                    self.undo_bytes = self.undo_bytes.saturating_sub(step.bytes);
                    step.coalesce = None;
                    self.redo.push(step);
                }
                // A transaction after an undo never joins an older step.
                if let Some(top) = self.undo.back_mut() {
                    top.coalesce = None;
                }
            }
            TxnKind::RedoOf(target) => {
                if self.redo.last().and_then(|step| step.txns.last()) != Some(&target) {
                    return Err(sequence("the redo is not of the top redo step"));
                }
                self.redo.pop();
                let mut step = Step::default();
                step.push(txn, bytes);
                self.push_undo(step);
            }
            TxnKind::Merge | TxnKind::Migration => {}
        }
        Ok(())
    }

    fn outside_group(&self) -> Result<()> {
        if self.groups.is_empty() {
            Ok(())
        } else {
            Err(sequence("a group is open"))
        }
    }

    fn push_undo(&mut self, step: Step) {
        self.undo_bytes = self.undo_bytes.saturating_add(step.bytes);
        self.undo.push_back(step);
        self.trim();
    }

    /// Drops the oldest steps while the stack exceeds a limit.
    fn trim(&mut self) {
        while self.undo.len() > self.limits.steps || self.undo_bytes > self.limits.memory_bytes {
            let Some(step) = self.undo.pop_front() else {
                break;
            };
            self.undo_bytes = self.undo_bytes.saturating_sub(step.bytes);
        }
    }
}

fn sequence(reason: &'static str) -> Error {
    Error::new(codes::UNDO_SEQUENCE_INVALID).with_arg("reason", reason)
}

fn entry(journal: &Journal, id: TransactionId) -> Result<&crate::journal::JournalEntry> {
    journal
        .get(id)
        .ok_or_else(|| sequence("the transaction is not in the journal"))
}

fn memory_of(journal: &Journal, txns: &[TransactionId]) -> u64 {
    txns.iter()
        .filter_map(|id| journal.get(*id))
        .map(crate::journal::JournalEntry::memory_bytes)
        .fold(0, u64::saturating_add)
}

/// The inverses of the operations of `txns`, last operation first.
fn inverse_ops(journal: &Journal, txns: &[TransactionId]) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    for id in txns.iter().rev() {
        ops.extend(
            entry(journal, *id)?
                .record()
                .ops
                .iter()
                .rev()
                .map(Op::inverse),
        );
    }
    Ok(ops)
}

/// A transaction on `base` of kind `kind` that applies `ops`. The operations are first applied
/// strictly to a scratch working set, so a state that does not hold their before values fails
/// with `doc.undo.state_mismatch`; then they are made through the transaction's own operations,
/// which record the same operations.
pub(crate) fn prepare(
    base: &Snapshot,
    mut meta: TxnMeta,
    limits: TxnLimits,
    kind: TxnKind,
    ops: &[Op],
) -> Result<Txn> {
    let mut scratch = WorkingSet::new(base);
    for op in ops {
        op.apply(&mut scratch).map_err(|error| {
            Error::new(codes::UNDO_STATE_MISMATCH)
                .with_detail("id", op.id().to_string())
                .with_source(error)
        })?;
    }
    drop(scratch);
    meta.kind = kind;
    let mut txn = Txn::new(base, meta, limits);
    for op in ops {
        replay(&mut txn, op)?;
    }
    Ok(txn)
}

/// Makes `op` through the operations of `txn`; its before state has been checked.
fn replay(txn: &mut Txn, op: &Op) -> Result<()> {
    match op {
        Op::Create {
            type_key,
            object,
            copied_from,
        } => {
            let current = txn
                .snapshot()
                .registry()
                .get(type_key.as_str())
                .map(|decl| decl.schema.version);
            if current != Some(object.version) {
                return Err(Error::new(codes::UNDO_STATE_MISMATCH)
                    .with_detail("id", object.id.to_string())
                    .with_detail("reason", "schema version is not current"));
            }
            let (header, values) = (object.header.clone(), object.values.clone());
            match copied_from {
                Some(source) => {
                    txn.create_copy(*source, type_key.as_str(), object.id, header, values)
                }
                None => txn.create(type_key.as_str(), object.id, header, values),
            }
            .map(drop)
        }
        Op::Delete { object, .. } => txn.delete(object.id),
        Op::Modify {
            id, path, after, ..
        } => txn.modify(*id, path.clone(), after.clone()),
        Op::ModifyProperties { id, after, .. } => txn.set_properties(*id, after.clone()),
        Op::Relink { id, after, .. } => txn.relink(*id, *after),
    }
}

/// The identifiers of the objects that differ between `a` and `b`, in ascending order: live in
/// only one of them, or live in both with another type key, schema version, header or values,
/// values compared bit for bit. Empty when the two states hold the same objects.
///
/// This is the minimal structural comparison the undo tests need; it walks every object of
/// both snapshots, skipping objects shared by pointer. The full comparison API with changed
/// paths and both values is CORE-DOC-026 (phase 1).
#[must_use]
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<ObjectId> {
    let mut out = Vec::new();
    for (handle, object) in a.iter() {
        let same = b.resolve(object.id).ok().is_some_and(|other_handle| {
            b.get(other_handle).is_ok_and(|other| {
                a.type_key(handle) == b.type_key(other_handle)
                    && (std::sync::Arc::ptr_eq(object, other)
                        || (object.version == other.version
                            && object.header == other.header
                            && options_bits_eq(&object.values, &other.values)))
            })
        });
        if !same {
            out.push(object.id);
        }
    }
    out.extend(
        b.iter()
            .map(|(_, object)| object.id)
            .filter(|id| a.resolve(*id).is_err()),
    );
    out.sort_unstable();
    out
}
