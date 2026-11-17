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
use crate::op::{Op, Properties, live, modified, path_invalid, schema_of, with_properties};
use crate::schema::{
    ColorValue, ExternalRef, FieldDecl, FieldPath, PathSegment, Value, check_value, count,
    prefix_field,
};
use crate::snapshot::{Change, Header, Object, Savepoint, Snapshot, WorkingSet};
use crate::store::Handle;

/// The most operations one transaction holds (document model, "Transactions").
pub const MAX_OPERATIONS: u64 = 10_000_000;

/// The bounds of one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxnLimits {
    /// The most operations.
    pub operations: u64,
    /// The memory budget of the operation list in bytes, as estimated by the transaction;
    /// `None` leaves the budget to the caller.
    pub memory_bytes: Option<u64>,
}

impl TxnLimits {
    /// [`MAX_OPERATIONS`] and the memory budget of `limits`.
    #[must_use]
    pub fn from_limits(limits: &Limits) -> Self {
        Self {
            operations: MAX_OPERATIONS,
            memory_bytes: limits.memory_bytes,
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
    ops: usize,
    bytes: u64,
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
    /// The estimated memory of `ops`.
    bytes: u64,
    limits: TxnLimits,
    meta: TxnMeta,
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

    /// The estimated memory of the operation list in bytes, charged against
    /// [`TxnLimits::memory_bytes`].
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
    /// value of the wrong type), `doc.value.not_finite`, `doc.value.out_of_range` (enum codes and
    /// bounds of nested records) or `doc.txn.limit_exceeded`. Missing required values and field
    /// bounds are checked at commit.
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
        self.record(op, |set| set.create(type_key, (*object).clone()))
    }

    /// Sets the value at `path` of the object `id` to `value`: a whole field when `path` is a
    /// field's path, or a record field, list element or map entry inside it. `None` makes a
    /// field or record field absent or removes a map entry; a list element cannot be removed.
    ///
    /// Fails with `doc.object.not_found`, `doc.path.invalid` (the path does not resolve against
    /// the object's schema or does not reach a value of the object: an index past the end of a
    /// list, a part of an absent value, `None` for a list element), the value checks of
    /// [`Txn::create`] for the addressed type, or `doc.txn.limit_exceeded`.
    pub fn modify(&mut self, id: ObjectId, path: FieldPath, value: Option<Value>) -> Result<()> {
        if let Some(value) = &value {
            let (handle, object) = live(&self.set, id)?;
            let schema = schema_of(&self.set, handle, &object)?;
            let value_type = path.resolve(schema).ok_or_else(|| path_invalid(&path))?;
            check_value(value_type, None, value)
                .map_err(|error| prefix_field(error, &path.to_string()))?;
        }
        let found = modified(&self.set, id, &path, value.clone())?;
        let op = Op::Modify {
            id,
            version: found.version,
            path,
            before: found.before,
            after: value,
        };
        self.record(op, |set| {
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
        self.record(op, |set| set.replace_header(handle, header).map(drop))
    }

    /// Deletes the object `id`. Fails with `doc.object.not_found` or
    /// `doc.txn.limit_exceeded`. Cascades of relationship rules are added at commit.
    pub fn delete(&mut self, id: ObjectId) -> Result<()> {
        let (handle, object) = live(&self.set, id)?;
        let type_key = self
            .snapshot()
            .type_key(handle)
            .cloned()
            .ok_or_else(|| Error::new(codes::OBJECT_NOT_FOUND))?;
        let op = Op::Delete { type_key, object };
        self.record(op, |set| set.delete(handle).map(drop))
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
        self.record(op, |set| set.replace_header(handle, header).map(drop))
    }

    /// Marks the current state and operation count so that [`Txn::rollback_to`] can restore
    /// them.
    pub fn savepoint(&mut self) -> TxnSavepoint {
        TxnSavepoint {
            inner: self.set.savepoint(),
            ops: self.ops.len(),
            bytes: self.bytes,
        }
    }

    /// Restores the state marked by `savepoint` and truncates the operations to their number
    /// at the savepoint. The transaction stays open and the savepoint stays valid; savepoints
    /// taken after it become invalid. A savepoint of another transaction, or one released or
    /// invalidated, fails with `doc.savepoint.invalid` and changes nothing.
    pub fn rollback_to(&mut self, savepoint: &TxnSavepoint) -> Result<()> {
        self.set.rollback_to(&savepoint.inner)?;
        self.ops.truncate(savepoint.ops);
        self.bytes = savepoint.bytes;
        Ok(())
    }

    /// Releases `savepoint` and every later one, keeping the changes. A savepoint of another
    /// transaction, or one released or invalidated, fails with `doc.savepoint.invalid`.
    pub fn release(&mut self, savepoint: &TxnSavepoint) -> Result<()> {
        self.set.release_savepoint(&savepoint.inner)
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

    /// Charges `op` against the limits, applies the change with `write` and appends `op`;
    /// on any failure nothing changes.
    fn record<T>(&mut self, op: Op, write: impl FnOnce(&mut WorkingSet) -> Result<T>) -> Result<T> {
        let within = u64::try_from(self.ops.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .is_some_and(|n| n <= self.limits.operations);
        if !within {
            return Err(limit_exceeded("operations", self.limits.operations));
        }
        let bytes = self.bytes.saturating_add(op_bytes(&op));
        if let Some(budget) = self.limits.memory_bytes.filter(|budget| bytes > *budget) {
            return Err(limit_exceeded("memory_bytes", budget));
        }
        let written = write(&mut self.set)?;
        self.ops.push(op);
        self.bytes = bytes;
        Ok(written)
    }
}

fn limit_exceeded(limit: &str, value: u64) -> Error {
    Error::new(codes::TXN_LIMIT_EXCEEDED)
        .with_arg("limit", limit)
        .with_detail("value", i64::try_from(value).unwrap_or(i64::MAX))
}

/// One entry per field, each present value of its field's type (bounds aside).
fn check_shape(fields: &[FieldDecl], values: &[Option<Value>]) -> Result<()> {
    if fields.len() != values.len() {
        let field = fields
            .get(values.len())
            .map_or_else(|| fields.len().to_string(), |field| field.name.clone());
        return Err(Error::new(codes::VALUE_TYPE_MISMATCH)
            .with_arg("field", field)
            .with_detail("expected_fields", count(fields.len()))
            .with_detail("actual_fields", count(values.len())));
    }
    for (field, value) in fields.iter().zip(values) {
        if let Some(value) = value {
            check_value(&field.value_type, None, value)
                .map_err(|error| prefix_field(error, &field.name))?;
        }
    }
    Ok(())
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
        Op::Create { object, .. } | Op::Delete { object, .. } => {
            header_bytes(&object.header).saturating_add(values_bytes(&object.values))
        }
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
    };
    bytes_of(size_of::<Op>()).saturating_add(data)
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
