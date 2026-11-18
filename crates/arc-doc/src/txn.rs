//! Transactions and savepoints (CORE-DOC-013, CORE-DOC-014).
//!
//! A [`Txn`] is an open transaction: a [`WorkingSet`] on a base snapshot and the ordered list of
//! [`Op`]s that made its candidate. Reads through the transaction see its changes; nobody else
//! does until the candidate is published. Operations are checked only as far as the working
//! state needs (the object exists, the type is registered, the path resolves, the value has the
//! addressed type); bounds, required fields, references and validators are checked at commit.
//!
//! Exactly one transaction is open per document; the document executor enforces that, not this
//! type.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result, SessionId, TransactionId};

use crate::error::codes;
use crate::integrity::{self, DeleteReport};
use crate::load::StoredForeign;
use crate::op::{
    Op, Properties, check_foreign_row, check_shape, check_write, live, modified, with_properties,
};
use crate::schema::{ColorValue, ExternalRef, FieldPath, PathSegment, TypeKey, Value};
use crate::snapshot::{Change, Header, Object, Savepoint, Snapshot, WorkingSet};
use crate::store::Handle;

/// The most operations one transaction holds (document model, "Transactions").
pub const MAX_OPERATIONS: u64 = 10_000_000;

/// The most savepoints one transaction holds at a time.
pub const MAX_SAVEPOINTS: u64 = 1_024;

/// The bounds of one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxnLimits {
    /// The most operations.
    pub operations: u64,
    /// The memory budget in bytes of what the transaction retains, as estimated by it
    /// ([`Txn::memory_bytes`]); `None` leaves the budget to the caller.
    pub memory_bytes: Option<u64>,
    /// The most savepoints held at a time.
    pub savepoints: u64,
}

impl TxnLimits {
    /// [`MAX_OPERATIONS`], [`MAX_SAVEPOINTS`] and the memory budget of `limits`.
    #[must_use]
    pub fn from_limits(limits: &Limits) -> Self {
        Self {
            operations: MAX_OPERATIONS,
            memory_bytes: limits.memory_bytes,
            savepoints: MAX_SAVEPOINTS,
        }
    }
}

impl Default for TxnLimits {
    /// The limits of [`Limits::DEFAULT`].
    fn default() -> Self {
        Self::from_limits(&Limits::DEFAULT)
    }
}

/// What kind of change a transaction is, for its journal record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxnKind {
    /// A command's change.
    Normal,
    /// The undo of the named transaction or group.
    UndoOf(TransactionId),
    /// The redo of the named transaction or group.
    RedoOf(TransactionId),
    /// A merge of a change set.
    Merge,
    /// A schema migration.
    Migration,
    /// The reload of a linked document.
    LinkReload,
}

/// The metadata of a transaction's journal record besides its operations.
///
/// `command`, `label`, `group` and `selection_after` are opaque here: the command layer, the
/// history list, grouping and the application write them as canonical JSON text, and this
/// crate keeps them unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxnMeta {
    /// The identifier of the transaction.
    pub id: TransactionId,
    /// The transaction this one applies on; `None` for the first of a new document.
    pub parent: Option<TransactionId>,
    /// The session that ran it.
    pub session: SessionId,
    /// The user identity.
    pub author: String,
    /// The UTC time in milliseconds since the Unix epoch.
    pub time_unix_millis: u64,
    /// The kind.
    pub kind: TxnKind,
    /// The command identifier and its parameters (opaque).
    pub command: String,
    /// The message identifier and arguments for the history list (opaque).
    pub label: String,
    /// The group, if the transaction is part of one (opaque).
    pub group: Option<String>,
    /// The selection to restore (opaque).
    pub selection_after: Option<String>,
}

impl TxnMeta {
    /// A normal transaction `id` of `session` without parent, author, time, command, label,
    /// group or selection.
    #[must_use]
    pub fn new(id: TransactionId, session: SessionId) -> Self {
        Self {
            id,
            parent: None,
            session,
            author: String::new(),
            time_unix_millis: 0,
            kind: TxnKind::Normal,
            command: String::new(),
            label: String::new(),
            group: None,
            selection_after: None,
        }
    }
}

/// A state of one [`Txn`] to roll back to, from [`Txn::savepoint`]. It is valid until it is
/// released or a rollback to, or release of, an earlier savepoint invalidates it.
#[derive(Debug, Clone)]
pub struct TxnSavepoint {
    inner: Savepoint,
    serial: u64,
    ops: usize,
    bytes: u64,
}

/// A live savepoint as the transaction keeps it: its serial number and the candidate it marks,
/// which tells whether a replaced object stays alive because of a savepoint.
#[derive(Debug)]
struct Held {
    serial: u64,
    candidate: Snapshot,
}

/// What a finished transaction hands to commit: the working set, the operations and the
/// metadata.
#[derive(Debug)]
#[non_exhaustive]
pub struct TxnParts {
    /// The working set with the candidate snapshot and touched identifiers.
    pub set: WorkingSet,
    /// The operations in the order they were applied.
    pub ops: Vec<Op>,
    /// The metadata of the journal record.
    pub meta: TxnMeta,
}

/// An open transaction.
///
/// Every successful operation appends one [`Op`] to [`Txn::ops`], also when it sets a value it
/// already had and when it changes the same field again: the list keeps every operation in
/// order, and coalescing is the journal's concern. A failed operation changes nothing.
#[derive(Debug)]
pub struct Txn {
    set: WorkingSet,
    ops: Vec<Op>,
    /// The estimated memory retained by `ops` and the live savepoints.
    bytes: u64,
    limits: TxnLimits,
    meta: TxnMeta,
    /// The live savepoints, oldest first.
    held: Vec<Held>,
    next_serial: u64,
}

impl Txn {
    /// Opens a transaction on `base`.
    #[must_use]
    pub fn new(base: &Snapshot, meta: TxnMeta, limits: TxnLimits) -> Self {
        Self {
            set: WorkingSet::new(base),
            ops: Vec::new(),
            bytes: 0,
            limits,
            meta,
            held: Vec::new(),
            next_serial: 0,
        }
    }

    /// The snapshot the transaction applies to.
    #[must_use]
    pub fn base(&self) -> &Snapshot {
        self.set.base()
    }

    /// The candidate snapshot, with every change of the transaction: get, resolve and iterate
    /// through it to read inside the transaction.
    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        self.set.current()
    }

    /// The handle of the live object `id` in the candidate.
    pub fn resolve(&self, id: ObjectId) -> Result<Handle> {
        self.snapshot().resolve(id)
    }

    /// The live object `id` in the candidate.
    pub fn get(&self, id: ObjectId) -> Result<&Arc<Object>> {
        let snapshot = self.snapshot();
        snapshot.get(snapshot.resolve(id)?)
    }

    /// The operations so far, in order.
    #[must_use]
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// The net change of each identifier relative to the base ([`WorkingSet::touched`]).
    #[must_use]
    pub fn touched(&self) -> &BTreeMap<ObjectId, Change> {
        self.set.touched()
    }

    /// The estimated memory the transaction retains in bytes, charged against
    /// [`TxnLimits::memory_bytes`]: the operation list (each operation and the values, texts and
    /// path it holds), each savepoint's copy of the touched map, and each object version that a
    /// live savepoint keeps alive after an operation replaced it. Copied store chunks and index
    /// nodes are not counted.
    #[must_use]
    pub fn memory_bytes(&self) -> u64 {
        self.bytes
    }

    /// The metadata.
    #[must_use]
    pub fn meta(&self) -> &TxnMeta {
        &self.meta
    }

    /// The metadata, to set what is known only at the end, such as the selection.
    pub fn meta_mut(&mut self) -> &mut TxnMeta {
        &mut self.meta
    }

    /// Creates the object `id` of type `type_key` at the type's current schema version.
    ///
    /// Fails with `doc.type.unknown`, `doc.object.duplicate_id`,
    /// `doc.store.capacity_exceeded`, `doc.value.type_mismatch` (a wrong number of values, or a
    /// value of the wrong type), `doc.value.not_finite`, `doc.value.out_of_range` (enum codes,
    /// bounds of nested records and the bound of a `many` reference list) or
    /// `doc.txn.limit_exceeded`. Missing required values and the bounds of the object's own
    /// fields are checked at commit. The operation and the store share the created object.
    pub fn create(
        &mut self,
        type_key: &str,
        id: ObjectId,
        header: Header,
        values: Vec<Option<Value>>,
    ) -> Result<Handle> {
        self.create_op(type_key, id, header, values, None)
    }

    /// Creates the object `id` like [`Txn::create`] as a copy of `source`, which the operation
    /// records for lineage.
    pub fn create_copy(
        &mut self,
        source: ObjectId,
        type_key: &str,
        id: ObjectId,
        header: Header,
        values: Vec<Option<Value>>,
    ) -> Result<Handle> {
        self.create_op(type_key, id, header, values, Some(source))
    }

    fn create_op(
        &mut self,
        type_key: &str,
        id: ObjectId,
        header: Header,
        values: Vec<Option<Value>>,
        copied_from: Option<ObjectId>,
    ) -> Result<Handle> {
        let decl = self
            .snapshot()
            .registry()
            .get(type_key)
            .ok_or_else(|| Error::new(codes::TYPE_UNKNOWN).with_arg("type", type_key))?;
        check_shape(&decl.schema.fields, &values)?;
        let object = Arc::new(Object::new(id, decl.schema.version, header, values));
        let op = Op::Create {
            type_key: decl.key.clone(),
            object: Arc::clone(&object),
            copied_from,
        };
        self.record(op, 0, |set| set.create_shared(type_key, object))
    }

    /// Sets the value at `path` of the object `id` to `value`: a whole field when `path` is a
    /// field's path, or a record field, list element or map entry inside it. `None` makes a
    /// field or record field absent or removes a map entry; a list element cannot be removed.
    ///
    /// Fails with `doc.object.not_found`, `doc.path.invalid` (the path does not resolve against
    /// the object's schema or does not reach a value of the object: an index past the end of a
    /// list, a part of an absent value, `None` for a list element), the value checks of
    /// [`Txn::create`] for the addressed type, `doc.value.missing` (`None` for a required field
    /// of a nested record, whose records are checked in full) or `doc.txn.limit_exceeded`.
    ///
    /// The new state of the object is a copy of all its values with one value written, so a
    /// modify costs time and memory proportional to the object's values, however small the
    /// change; large data belongs in assets.
    pub fn modify(&mut self, id: ObjectId, path: FieldPath, value: Option<Value>) -> Result<()> {
        let (_, object) = live(&self.set, id)?;
        check_write(&self.set, id, &path, value.as_ref())?;
        let retained = self.retained(id, &object);
        let found = modified(&self.set, id, &path, value.clone())?;
        let op = Op::Modify {
            id,
            version: found.version,
            path,
            before: found.before,
            after: value,
        };
        self.record(op, retained, |set| {
            set.replace_values(found.handle, found.values).map(drop)
        })
    }

    /// Sets the name, partition and flags of the object `id`. Fails with
    /// `doc.object.not_found` or `doc.txn.limit_exceeded`.
    pub fn set_properties(&mut self, id: ObjectId, properties: Properties) -> Result<()> {
        let (handle, object) = live(&self.set, id)?;
        let header = with_properties(&object.header, properties.clone());
        let op = Op::ModifyProperties {
            id,
            before: Properties::of(&object.header),
            after: properties,
        };
        let retained = self.retained(id, &object);
        self.record(op, retained, |set| {
            set.replace_header(handle, header).map(drop)
        })
    }

    /// Deletes the object `id`. Fails with `doc.object.not_found` or
    /// `doc.txn.limit_exceeded`. The relationship rules it triggers are applied by
    /// [`Txn::apply_delete_rules`].
    pub fn delete(&mut self, id: ObjectId) -> Result<()> {
        let (handle, object) = live(&self.set, id)?;
        let type_key = self
            .snapshot()
            .type_key(handle)
            .cloned()
            .ok_or_else(|| Error::new(codes::OBJECT_NOT_FOUND))?;
        let op = Op::Delete { type_key, object };
        self.record(op, 0, |set| set.delete(handle).map(drop))
    }

    /// Sets the owner of the object `id`. Fails with `doc.object.not_found` or
    /// `doc.txn.limit_exceeded`; whether the owner exists is checked at commit.
    pub fn relink(&mut self, id: ObjectId, owner: Option<ObjectId>) -> Result<()> {
        let (handle, object) = live(&self.set, id)?;
        let header = Header {
            owner,
            ..object.header.clone()
        };
        let op = Op::Relink {
            id,
            before: object.header.owner,
            after: owner,
        };
        let retained = self.retained(id, &object);
        self.record(op, retained, |set| {
            set.replace_header(handle, header).map(drop)
        })
    }

    /// Sets the foreign data row `row.id` to `row`, adding it or replacing the row that has that
    /// key. Foreign data rows are not objects: deleting the object a row names leaves the row,
    /// and only [`Txn::remove_foreign`] removes it.
    ///
    /// Fails with `doc.foreign.invalid` (see [`crate::op`]: `object` must be absent or 16
    /// identifier bytes, `asset` absent or 32 bytes, `source_format` and `record_kind` not
    /// empty), `doc.document.read_only` or `doc.txn.limit_exceeded`; the number of rows and the
    /// size of their columns are bounded by `Limits` at commit.
    pub fn set_foreign(&mut self, row: StoredForeign) -> Result<()> {
        check_foreign_row(row.id, &row)?;
        let id = row.id;
        let after = Arc::new(row);
        let op = Op::ModifyForeign {
            id,
            before: self.snapshot().foreign(id).cloned(),
            after: Some(Arc::clone(&after)),
        };
        self.record(op, 0, |set| set.set_foreign(after).map(drop))
    }

    /// Adds `row` under a new key, one more than the highest key in the candidate (1 for none),
    /// ignoring `row.id`, and returns the key. Fails like [`Txn::set_foreign`], and with
    /// `doc.foreign.invalid` (reason `id`) when no key is left.
    pub fn add_foreign(&mut self, mut row: StoredForeign) -> Result<i64> {
        row.id = self.snapshot().next_foreign_id().ok_or_else(|| {
            Error::new(codes::FOREIGN_INVALID)
                .with_arg("row", i64::MAX)
                .with_arg("reason", "id")
        })?;
        let id = row.id;
        self.set_foreign(row)?;
        Ok(id)
    }

    /// Removes the foreign data row `id`. Fails with `doc.foreign.not_found`,
    /// `doc.document.read_only` or `doc.txn.limit_exceeded`.
    pub fn remove_foreign(&mut self, id: i64) -> Result<()> {
        let before = self
            .snapshot()
            .foreign(id)
            .cloned()
            .ok_or_else(|| Error::new(codes::FOREIGN_NOT_FOUND).with_arg("row", id))?;
        let op = Op::ModifyForeign {
            id,
            before: Some(before),
            after: None,
        };
        self.record(op, 0, |set| set.remove_foreign(id).map(drop))
    }

    /// Applies the deletion rules to the transaction's deletions and records what they change
    /// as operations of the transaction ([`DeleteReport::operations`]), charged against the
    /// limits like the transaction's own operations, so that the operation list still replays
    /// to the candidate and inverts to the base.
    ///
    /// The rules are those of [`integrity::apply_delete_rules`], except that the owning
    /// references of a deleted object, and of an object recreated with another type, are read
    /// from the states it had when the transaction deleted it, not from the base: an owning
    /// reference added before the delete takes its target along, and one removed before the
    /// delete leaves it. Every [`Op::Delete`] of the identifier contributes its state, except one
    /// that a create of the same type key follows, which replaces the object and keeps what it
    /// owned. An identifier deleted and created again counts as a type change when one of its
    /// deletes contributes, even if the last create restored the base's type key. Fails with the codes of the
    /// rules or `doc.txn.limit_exceeded`; any failure restores the working set, the operations
    /// and the memory estimate, and the transaction's savepoints stay valid. Running the rules
    /// again without further deletions changes nothing.
    pub fn apply_delete_rules(&mut self, limits: &Limits) -> Result<DeleteReport> {
        let mut deleted: BTreeMap<ObjectId, Vec<(TypeKey, Arc<Object>)>> = BTreeMap::new();
        for op in &self.ops {
            match op {
                Op::Delete { type_key, object } => deleted
                    .entry(object.id)
                    .or_default()
                    .push((type_key.clone(), Arc::clone(object))),
                // A create of the same type right after a delete replaces the object.
                Op::Create {
                    type_key, object, ..
                } => {
                    if let Some(states) = deleted.get_mut(&object.id)
                        && states.last().is_some_and(|(key, _)| key == type_key)
                    {
                        states.pop();
                    }
                }
                _ => {}
            }
        }
        let (ops, bytes) = (self.ops.len(), self.bytes);
        let savepoint = self.set.savepoint();
        let states = |id: ObjectId| Some(deleted.get(&id).cloned().unwrap_or_default());
        let outcome = integrity::run(&mut self.set, limits, &states)
            .and_then(|report| self.append_derived(&report).map(|()| report));
        if outcome.is_err() {
            self.set.rollback_to(&savepoint)?;
            self.ops.truncate(ops);
            self.bytes = bytes;
        }
        self.set.release_savepoint(&savepoint)?;
        outcome
    }

    /// Charges and appends the operations of `report`, whose changes the working set already
    /// holds.
    fn append_derived(&mut self, report: &DeleteReport) -> Result<()> {
        let retained = report
            .cleared
            .iter()
            .map(|cleared| self.retained(cleared.before.id, &cleared.before))
            .fold(0, u64::saturating_add);
        self.bytes = self.bytes.saturating_add(retained);
        for op in report.operations() {
            self.bytes = self.admit(&op, 0)?;
            self.ops.push(op);
        }
        Ok(())
    }

    /// Marks the current state and operation count so that [`Txn::rollback_to`] can restore
    /// them. The savepoint is charged a copy of the touched map, and every object version it
    /// keeps alive is charged when an operation replaces that version. Fails with
    /// `doc.txn.limit_exceeded` (argument `limit`: `savepoints` when [`TxnLimits::savepoints`]
    /// savepoints are live, or `memory_bytes`) and changes nothing.
    pub fn savepoint(&mut self) -> Result<TxnSavepoint> {
        let live = u64::try_from(self.held.len()).unwrap_or(u64::MAX);
        if live >= self.limits.savepoints {
            return Err(limit_exceeded("savepoints", self.limits.savepoints));
        }
        let charge = bytes_of(self.set.touched().len())
            .saturating_mul(TOUCHED_ENTRY_BYTES)
            .saturating_add(bytes_of(size_of::<Held>()));
        let bytes = self.within_budget(charge)?;
        let serial = self.next_serial;
        self.next_serial = serial.saturating_add(1);
        self.held.push(Held {
            serial,
            candidate: self.set.current().clone(),
        });
        self.bytes = bytes;
        Ok(TxnSavepoint {
            inner: self.set.savepoint(),
            serial,
            ops: self.ops.len(),
            bytes,
        })
    }

    /// Restores the state marked by `savepoint`, with the operations and the memory estimate,
    /// truncating the operations to their number at the savepoint. The transaction stays open
    /// and the savepoint stays valid; savepoints taken after it become invalid. A savepoint of
    /// another transaction, or one released or invalidated, fails with `doc.savepoint.invalid`
    /// and changes nothing.
    pub fn rollback_to(&mut self, savepoint: &TxnSavepoint) -> Result<()> {
        self.set.rollback_to(&savepoint.inner)?;
        self.ops.truncate(savepoint.ops);
        self.bytes = savepoint.bytes;
        let at = self.held_position(savepoint.serial);
        self.held.truncate(at.saturating_add(1));
        Ok(())
    }

    /// Releases `savepoint` and every later one, keeping the changes. The memory they were
    /// charged stays charged. A savepoint of another transaction, or one released or
    /// invalidated, fails with `doc.savepoint.invalid`.
    pub fn release(&mut self, savepoint: &TxnSavepoint) -> Result<()> {
        self.set.release_savepoint(&savepoint.inner)?;
        let at = self.held_position(savepoint.serial);
        self.held.truncate(at);
        Ok(())
    }

    /// The position of the live savepoint `serial`, which the working set has just accepted.
    fn held_position(&self, serial: u64) -> usize {
        self.held
            .binary_search_by_key(&serial, |held| held.serial)
            .unwrap_or_else(|at| at)
    }

    /// The memory `object`, the live version of `id`, stays charged for when an operation
    /// replaces it: its size when the newest savepoint holds it, otherwise nothing. A version
    /// that an older savepoint holds is current at every later savepoint until it is replaced,
    /// so the newest savepoint decides.
    fn retained(&self, id: ObjectId, object: &Arc<Object>) -> u64 {
        let Some(held) = self.held.last() else {
            return 0;
        };
        let kept = held
            .candidate
            .resolve(id)
            .and_then(|handle| held.candidate.get(handle))
            .is_ok_and(|kept| Arc::ptr_eq(kept, object));
        if kept { object_bytes(object) } else { 0 }
    }

    /// Ends the transaction and hands its working set, operations and metadata to commit.
    #[must_use]
    pub fn into_parts(self) -> TxnParts {
        TxnParts {
            set: self.set,
            ops: self.ops,
            meta: self.meta,
        }
    }

    /// Ends the transaction and drops every change.
    pub fn abandon(self) {}

    /// Charges `op` and `retained` further bytes against the limits, applies the change with
    /// `write` and appends `op`; on any failure nothing changes.
    fn record<T>(
        &mut self,
        op: Op,
        retained: u64,
        write: impl FnOnce(&mut WorkingSet) -> Result<T>,
    ) -> Result<T> {
        let bytes = self.admit(&op, retained)?;
        let written = write(&mut self.set)?;
        self.ops.push(op);
        self.bytes = bytes;
        Ok(written)
    }

    /// The memory estimate with `op` and `retained` further bytes, if the limits allow one more
    /// operation of that size.
    fn admit(&self, op: &Op, retained: u64) -> Result<u64> {
        let within = u64::try_from(self.ops.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .is_some_and(|n| n <= self.limits.operations);
        if !within {
            return Err(limit_exceeded("operations", self.limits.operations));
        }
        self.within_budget(op_bytes(op).saturating_add(retained))
    }

    /// The memory estimate with `charge` further bytes, if the budget allows it.
    fn within_budget(&self, charge: u64) -> Result<u64> {
        let bytes = self.bytes.saturating_add(charge);
        match self.limits.memory_bytes.filter(|budget| bytes > *budget) {
            Some(budget) => Err(limit_exceeded("memory_bytes", budget)),
            None => Ok(bytes),
        }
    }
}

/// The estimated memory of one entry of a touched map copied by a savepoint.
const TOUCHED_ENTRY_BYTES: u64 = 64;

fn limit_exceeded(limit: &str, value: u64) -> Error {
    Error::new(codes::TXN_LIMIT_EXCEEDED)
        .with_arg("limit", limit)
        .with_detail("value", i64::try_from(value).unwrap_or(i64::MAX))
}

const VALUE_BYTES: u64 = size_of::<Value>() as u64;

fn bytes_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// The estimated memory an operation retains: its own size and the heap data of its values,
/// texts and path. Values are type-checked before they are charged, so their nesting is bounded
/// by the type's.
fn op_bytes(op: &Op) -> u64 {
    let data = match op {
        Op::Create { object, .. } | Op::Delete { object, .. } => object_bytes(object),
        Op::Modify {
            path,
            before,
            after,
            ..
        } => path_bytes(path)
            .saturating_add(option_bytes(before.as_ref()))
            .saturating_add(option_bytes(after.as_ref())),
        Op::ModifyProperties { before, after, .. } => [before, after]
            .iter()
            .flat_map(|p| [&p.name, &p.partition])
            .map(|text| bytes_of(text.as_ref().map_or(0, String::len)))
            .fold(0, u64::saturating_add),
        Op::Relink { .. } => 0,
        Op::ModifyForeign { before, after, .. } => before
            .iter()
            .chain(after)
            .map(|row| foreign_bytes(row))
            .fold(0, u64::saturating_add),
    };
    bytes_of(size_of::<Op>()).saturating_add(data)
}

/// The estimated memory of a foreign data row: its size and the heap data of its columns.
fn foreign_bytes(row: &StoredForeign) -> u64 {
    let texts = [
        Some(&row.source_format),
        row.source_version.as_ref(),
        row.source_id.as_ref(),
        Some(&row.record_kind),
    ];
    let blobs = [Some(&row.data), row.object.as_ref(), row.asset.as_ref()];
    let heap = texts
        .into_iter()
        .flatten()
        .map(String::len)
        .chain(blobs.into_iter().flatten().map(Vec::len))
        .map(bytes_of)
        .fold(0, u64::saturating_add);
    bytes_of(size_of::<StoredForeign>()).saturating_add(heap)
}

/// The estimated memory of an object: its size, texts and values.
fn object_bytes(object: &Object) -> u64 {
    header_bytes(&object.header).saturating_add(values_bytes(&object.values))
}

fn header_bytes(header: &Header) -> u64 {
    [&header.name, &header.partition]
        .iter()
        .map(|text| bytes_of(text.as_ref().map_or(0, String::len)))
        .fold(bytes_of(size_of::<Object>()), u64::saturating_add)
}

fn path_bytes(path: &FieldPath) -> u64 {
    path.segments()
        .iter()
        .map(|segment| match segment {
            PathSegment::Field(text) | PathSegment::Key(text) => bytes_of(text.len()),
            PathSegment::Index(_) => 0,
        })
        .fold(bytes_of(size_of_val(path.segments())), u64::saturating_add)
}

fn option_bytes(value: Option<&Value>) -> u64 {
    value.map_or(VALUE_BYTES, value_bytes)
}

fn values_bytes(values: &[Option<Value>]) -> u64 {
    values
        .iter()
        .map(|value| option_bytes(value.as_ref()))
        .fold(0, u64::saturating_add)
}

fn value_bytes(value: &Value) -> u64 {
    let heap = match value {
        Value::String(text) => bytes_of(text.len()),
        Value::Geometry(bytes) => bytes_of(bytes.0.len()),
        Value::Opaque(bytes) => bytes_of(bytes.len()),
        Value::Transform(_) => bytes_of(size_of::<arc_base::math::Transform3>()),
        Value::Color(color) => {
            let named = match &color.source {
                Some(crate::schema::ColorSource::Named(name)) => name.len(),
                _ => 0,
            };
            bytes_of(size_of::<ColorValue>().saturating_add(named))
        }
        Value::ExternalReference(_) => bytes_of(size_of::<ExternalRef>()),
        Value::Asset(_) => 32,
        Value::List(items) => items.iter().map(value_bytes).fold(0, u64::saturating_add),
        Value::Map(entries) => entries
            .iter()
            .map(|(key, item)| bytes_of(key.len()).saturating_add(value_bytes(item)))
            .fold(0, u64::saturating_add),
        Value::Record(values) => values_bytes(values),
        _ => 0,
    };
    VALUE_BYTES.saturating_add(heap)
}
