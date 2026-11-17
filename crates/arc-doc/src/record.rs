//! Journal records and their binary encoding (CORE-DOC-019).
//!
//! A [`TxnRecord`] is one committed transaction as the journal keeps it: the fields of the
//! document model's record shape and the transaction's final operation list, including the
//! operations the deletion rules added. Its encoding is versioned, deterministic and canonical;
//! the byte layout is specified in `docs/architecture/journal-record.md`. Values use the payload
//! encoding: whole objects with [`payload::encode`], field-level values with
//! [`payload::encode_value`] at the schema version the operation names.
//!
//! [`TxnRecord::decode`] treats its input as hostile: it never panics, checks every count and
//! length against the input and [`Limits`] before it allocates, decodes values against the type
//! registry and accepts only the canonical encoding.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Category, Error, Limits, ObjectId, Result, SessionId, TransactionId};

use crate::error::codes;
use crate::op::{Op, Properties};
use crate::payload::{self, Encoding};
use crate::registry::{MAX_TYPE_DEPTH, TypeRegistry};
use crate::schema::{FieldPath, PathSegment, Schema, SchemaVersion, TypeKey, Value};
use crate::snapshot::{Flags, Header, Object, Snapshot};
use crate::txn::{TxnKind, TxnMeta};

/// The version of the record encoding this build writes and reads.
pub const RECORD_FORMAT: u64 = 1;

/// The most elements reserved ahead of decoding them.
const PREALLOCATE: usize = 1024;

/// One committed transaction as the journal keeps it (document model, "Record shape").
///
/// Besides the fields of the record shape it keeps the type key of the object each operation
/// changes, which the encoding needs to name the schema of field-level values.
#[derive(Debug, Clone, PartialEq)]
pub struct TxnRecord {
    /// The transaction identifier.
    pub txn_id: TransactionId,
    /// The transaction this one was applied on; `None` for the first of a new document.
    pub parent: Option<TransactionId>,
    /// The session that ran it.
    pub session: SessionId,
    /// The user identity.
    pub author: String,
    /// The UTC time in milliseconds since the Unix epoch.
    pub time_unix_millis: u64,
    /// The command identifier and its parameters (opaque canonical JSON).
    pub command: String,
    /// The message identifier and arguments for the history list (opaque).
    pub label: String,
    /// The group, if the transaction is part of one (opaque).
    pub group: Option<String>,
    /// The kind.
    pub kind: TxnKind,
    /// The selection to restore (opaque).
    pub selection_after: Option<String>,
    /// The operations in the order they were applied.
    pub ops: Vec<Op>,
    /// The type key of the object each operation changes, one per operation.
    type_keys: Vec<TypeKey>,
}

impl TxnRecord {
    /// The record of a transaction with metadata `meta` and final operations `ops` that was
    /// applied on `before`. The type key of each operation's object is that of its create or
    /// delete, of an earlier create of the same identifier in `ops`, or of the object in
    /// `before`; an operation on an object none of them knows fails with
    /// `doc.journal.record_invalid`.
    pub fn from_commit(meta: TxnMeta, ops: Vec<Op>, before: &Snapshot) -> Result<Self> {
        let mut current: BTreeMap<ObjectId, TypeKey> = BTreeMap::new();
        let mut type_keys = Vec::with_capacity(ops.len());
        for op in &ops {
            let key = match op {
                Op::Create { type_key, .. } | Op::Delete { type_key, .. } => type_key.clone(),
                _ => match current.get(&op.id()) {
                    Some(key) => key.clone(),
                    None => before
                        .resolve(op.id())
                        .ok()
                        .and_then(|handle| before.type_key(handle).cloned())
                        .ok_or_else(|| invalid("operation on an unknown object"))?,
                },
            };
            current.insert(op.id(), key.clone());
            type_keys.push(key);
        }
        Ok(Self {
            txn_id: meta.id,
            parent: meta.parent,
            session: meta.session,
            author: meta.author,
            time_unix_millis: meta.time_unix_millis,
            command: meta.command,
            label: meta.label,
            group: meta.group,
            kind: meta.kind,
            selection_after: meta.selection_after,
            ops,
            type_keys,
        })
    }

    /// The metadata of the record.
    #[must_use]
    pub fn meta(&self) -> TxnMeta {
        TxnMeta {
            id: self.txn_id,
            parent: self.parent,
            session: self.session,
            author: self.author.clone(),
            time_unix_millis: self.time_unix_millis,
            kind: self.kind,
            command: self.command.clone(),
            label: self.label.clone(),
            group: self.group.clone(),
            selection_after: self.selection_after.clone(),
        }
    }

    /// The type key of the object each operation changes, one per operation of
    /// [`TxnRecord::ops`].
    #[must_use]
    pub fn type_keys(&self) -> &[TypeKey] {
        &self.type_keys
    }

    /// The canonical encoding of the record (format [`RECORD_FORMAT`]). Values are encoded with
    /// the schemas of `registry` at the version each operation names, which must be a registered
    /// schema version (`doc.journal.record_invalid` otherwise); value failures carry the
    /// payload codec's codes. A record that [`TxnRecord::decode`] would reject under `limits`
    /// fails with `doc.journal.record_limit_exceeded`.
    pub fn encode(&self, registry: &TypeRegistry, limits: &Limits) -> Result<Vec<u8>> {
        if self.ops.len() != self.type_keys.len() {
            return Err(invalid("operation and type counts differ"));
        }
        let mut out = Writer {
            bytes: Vec::new(),
            limits,
        };
        out.varint(RECORD_FORMAT);
        out.id(self.txn_id.as_u128());
        out.option(self.parent, |out, id| out.id(id.as_u128()));
        out.id(self.session.as_u128());
        out.text(&self.author)?;
        out.varint(self.time_unix_millis);
        match self.kind {
            TxnKind::Normal => out.varint(0),
            TxnKind::UndoOf(id) => {
                out.varint(1);
                out.id(id.as_u128());
            }
            TxnKind::RedoOf(id) => {
                out.varint(2);
                out.id(id.as_u128());
            }
            TxnKind::Merge => out.varint(3),
            TxnKind::Migration => out.varint(4),
            TxnKind::LinkReload => out.varint(5),
        }
        out.text(&self.command)?;
        out.text(&self.label)?;
        out.option_text(self.group.as_deref())?;
        out.option_text(self.selection_after.as_deref())?;
        // The type-key table: each key once, in order of first use.
        let mut table: Vec<&TypeKey> = Vec::new();
        let mut indices = Vec::with_capacity(self.type_keys.len());
        for key in &self.type_keys {
            let at = table
                .iter()
                .position(|known| *known == key)
                .unwrap_or_else(|| {
                    table.push(key);
                    table.len().saturating_sub(1)
                });
            indices.push(at);
        }
        out.count(table.len(), limits.record_count, "record_count")?;
        for key in &table {
            out.text(key.as_str())?;
        }
        out.count(self.ops.len(), limits.record_count, "record_count")?;
        for ((op, key), index) in self.ops.iter().zip(&self.type_keys).zip(indices) {
            out.op(op, key, index, registry)?;
        }
        if u64::try_from(out.bytes.len()).map_or(true, |n| n > limits.input_bytes) {
            return Err(limit("input_bytes", limits.input_bytes));
        }
        Ok(out.bytes)
    }

    /// Decodes a record that [`TxnRecord::encode`] wrote, with values decoded against
    /// `registry`. Fails with `doc.journal.record_limit_exceeded` when the input, a string, a
    /// count, a value or the memory budget exceeds `limits`, and with
    /// `doc.journal.record_invalid` when the bytes are truncated, malformed, of another format
    /// version, not canonical, followed by other bytes, name an unregistered type or schema
    /// version, or hold values their types reject. The payload codec's error is the source.
    pub fn decode(bytes: &[u8], registry: &TypeRegistry, limits: &Limits) -> Result<Self> {
        if u64::try_from(bytes.len()).map_or(true, |n| n > limits.input_bytes) {
            return Err(limit("input_bytes", limits.input_bytes));
        }
        let mut input = Reader {
            bytes,
            at: 0,
            limits,
        };
        let record = input.record(registry)?;
        if input.at != bytes.len() {
            return Err(invalid("trailing bytes"));
        }
        if record.encode(registry, limits).map_err(as_record_error)? != bytes {
            return Err(invalid("not canonical"));
        }
        Ok(record)
    }
}

fn invalid(reason: &'static str) -> Error {
    Error::new(codes::JOURNAL_RECORD_INVALID).with_detail("reason", reason)
}

fn limit(name: &'static str, value: u64) -> Error {
    Error::new(codes::JOURNAL_RECORD_LIMIT_EXCEEDED)
        .with_arg("limit", name)
        .with_detail("value", i64::try_from(value).unwrap_or(i64::MAX))
}

/// A failure of the payload codec or the registry inside a record, as a record error with the
/// original as its source.
fn as_record_error(error: Error) -> Error {
    if error.code() == codes::JOURNAL_RECORD_INVALID.code
        || error.code() == codes::JOURNAL_RECORD_LIMIT_EXCEEDED.code
    {
        return error;
    }
    let code = if error.category() == Category::LimitExceeded {
        Error::new(codes::JOURNAL_RECORD_LIMIT_EXCEEDED).with_arg("limit", "value")
    } else {
        invalid("value")
    };
    code.with_source(error)
}

/// The schema of `key` at exactly `version`, which must be the registered schema of its major
/// version.
fn schema_at<'a>(
    registry: &'a TypeRegistry,
    key: &TypeKey,
    version: SchemaVersion,
) -> Result<&'a Schema> {
    registry
        .get(key.as_str())
        .and_then(|decl| decl.schema(version.major))
        .filter(|schema| schema.version == version)
        .ok_or_else(|| invalid("schema version not registered"))
}

const OP_CREATE: u64 = 0;
const OP_MODIFY: u64 = 1;
const OP_MODIFY_PROPERTIES: u64 = 2;
const OP_DELETE: u64 = 3;
const OP_RELINK: u64 = 4;

const SEGMENT_FIELD: u64 = 0;
const SEGMENT_INDEX: u64 = 1;
const SEGMENT_KEY: u64 = 2;

struct Writer<'a> {
    bytes: Vec<u8>,
    limits: &'a Limits,
}

impl Writer<'_> {
    fn varint(&mut self, mut value: u64) {
        loop {
            let low = u8::try_from(value & 0x7f).unwrap_or(0);
            value >>= 7;
            if value == 0 {
                self.bytes.push(low);
                return;
            }
            self.bytes.push(low | 0x80);
        }
    }

    fn count(&mut self, n: usize, max: u64, name: &'static str) -> Result<()> {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        if n > max {
            return Err(limit(name, max));
        }
        self.varint(n);
        Ok(())
    }

    fn id(&mut self, raw: u128) {
        self.bytes.extend_from_slice(&raw.to_be_bytes());
    }

    fn flag(&mut self, on: bool) {
        self.bytes.push(u8::from(on));
    }

    fn option<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Self, T)) {
        self.flag(value.is_some());
        if let Some(value) = value {
            write(self, value);
        }
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.count(text.len(), self.limits.string_bytes, "string_bytes")?;
        self.bytes.extend_from_slice(text.as_bytes());
        Ok(())
    }

    fn option_text(&mut self, text: Option<&str>) -> Result<()> {
        self.flag(text.is_some());
        text.map_or(Ok(()), |text| self.text(text))
    }

    fn blob(&mut self, bytes: &[u8]) {
        self.varint(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.bytes.extend_from_slice(bytes);
    }

    fn version(&mut self, version: SchemaVersion) {
        self.varint(u64::from(version.major));
        self.varint(u64::from(version.minor));
    }

    fn flags(&mut self, flags: Flags) {
        self.bytes.push(
            u8::from(flags.hidden_from_ui)
                | u8::from(flags.system) << 1
                | u8::from(flags.read_only) << 2,
        );
    }

    fn properties(&mut self, properties: &Properties) -> Result<()> {
        self.option_text(properties.name.as_deref())?;
        self.option_text(properties.partition.as_deref())?;
        self.flags(properties.flags);
        Ok(())
    }

    fn object(&mut self, key: &TypeKey, object: &Object, registry: &TypeRegistry) -> Result<()> {
        self.id(object.id.as_u128());
        self.version(object.version);
        self.option(object.header.owner, |out, id| out.id(id.as_u128()));
        self.properties(&Properties::of(&object.header))?;
        let schema = schema_at(registry, key, object.version)?;
        let payload = payload::encode(schema, &object.values, self.limits)?;
        self.blob(&payload.bytes);
        Ok(())
    }

    fn op(&mut self, op: &Op, key: &TypeKey, index: usize, registry: &TypeRegistry) -> Result<()> {
        let tag = match op {
            Op::Create { .. } => OP_CREATE,
            Op::Modify { .. } => OP_MODIFY,
            Op::ModifyProperties { .. } => OP_MODIFY_PROPERTIES,
            Op::Delete { .. } => OP_DELETE,
            Op::Relink { .. } => OP_RELINK,
        };
        self.varint(tag);
        self.varint(u64::try_from(index).unwrap_or(u64::MAX));
        match op {
            Op::Create {
                type_key,
                object,
                copied_from,
            } => {
                if type_key != key {
                    return Err(invalid("type key differs"));
                }
                self.object(key, object, registry)?;
                self.option(*copied_from, |out, id| out.id(id.as_u128()));
            }
            Op::Delete { type_key, object } => {
                if type_key != key {
                    return Err(invalid("type key differs"));
                }
                self.object(key, object, registry)?;
            }
            Op::Modify {
                id,
                version,
                path,
                before,
                after,
            } => {
                self.id(id.as_u128());
                self.version(*version);
                let schema = schema_at(registry, key, *version)?;
                let value_type = path
                    .resolve(schema)
                    .ok_or_else(|| invalid("path does not resolve"))?;
                self.path(path)?;
                for value in [before, after] {
                    self.flag(value.is_some());
                    if let Some(value) = value {
                        let bytes = payload::encode_value(value_type, value, self.limits)?;
                        self.blob(&bytes);
                    }
                }
            }
            Op::ModifyProperties { id, before, after } => {
                self.id(id.as_u128());
                self.properties(before)?;
                self.properties(after)?;
            }
            Op::Relink { id, before, after } => {
                self.id(id.as_u128());
                self.option(*before, |out, id| out.id(id.as_u128()));
                self.option(*after, |out, id| out.id(id.as_u128()));
            }
        }
        Ok(())
    }

    fn path(&mut self, path: &FieldPath) -> Result<()> {
        self.varint(u64::try_from(path.segments().len()).unwrap_or(u64::MAX));
        for segment in path.segments() {
            match segment {
                PathSegment::Field(name) => {
                    self.varint(SEGMENT_FIELD);
                    self.text(name)?;
                }
                PathSegment::Index(index) => {
                    self.varint(SEGMENT_INDEX);
                    self.varint(*index);
                }
                PathSegment::Key(key) => {
                    self.varint(SEGMENT_KEY);
                    self.text(key)?;
                }
            }
        }
        Ok(())
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    limits: &'a Limits,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| invalid("truncated"))?;
        let taken = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| invalid("truncated"))?;
        self.at = end;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| invalid("truncated"))
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            let low = u64::from(byte & 0x7f);
            if shift == 63 && low > 1 {
                return Err(invalid("varint overflow"));
            }
            value |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid("varint overflow"))
    }

    fn u32(&mut self) -> Result<u32> {
        u32::try_from(self.varint()?).map_err(|_| invalid("integer out of range"))
    }

    /// A count of elements that each take at least one byte, bounded by the rest of the input
    /// and `max`.
    fn count(&mut self, max: u64, name: &'static str) -> Result<usize> {
        let n = self.varint()?;
        if n > max {
            return Err(limit(name, max));
        }
        usize::try_from(n)
            .ok()
            .filter(|n| *n <= self.remaining())
            .ok_or_else(|| invalid("truncated"))
    }

    fn id(&mut self) -> Result<u128> {
        let bytes: [u8; 16] = self
            .take(16)?
            .try_into()
            .map_err(|_| invalid("truncated"))?;
        Ok(u128::from_be_bytes(bytes))
    }

    fn flag(&mut self) -> Result<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("flag not 0 or 1")),
        }
    }

    fn option<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if self.flag()? {
            read(self).map(Some)
        } else {
            Ok(None)
        }
    }

    fn text(&mut self) -> Result<String> {
        let n = self.count(self.limits.string_bytes, "string_bytes")?;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| invalid("text is not UTF-8"))
    }

    fn blob(&mut self) -> Result<&'a [u8]> {
        let n = self.count(self.limits.input_bytes, "input_bytes")?;
        self.take(n)
    }

    fn version(&mut self) -> Result<SchemaVersion> {
        Ok(SchemaVersion::new(self.u32()?, self.u32()?))
    }

    fn flags(&mut self) -> Result<Flags> {
        let bits = self.byte()?;
        if bits > 0b111 {
            return Err(invalid("unknown flags"));
        }
        Ok(Flags {
            hidden_from_ui: bits & 1 != 0,
            system: bits & 2 != 0,
            read_only: bits & 4 != 0,
        })
    }

    fn properties(&mut self) -> Result<Properties> {
        Ok(Properties {
            name: self.option(Self::text)?,
            partition: self.option(Self::text)?,
            flags: self.flags()?,
        })
    }

    fn object_id(&mut self) -> Result<ObjectId> {
        self.id().map(ObjectId::from_u128)
    }

    fn object(&mut self, key: &TypeKey, registry: &TypeRegistry) -> Result<Arc<Object>> {
        let id = self.object_id()?;
        let version = self.version()?;
        let owner = self.option(Self::object_id)?;
        let properties = self.properties()?;
        let bytes = self.blob()?;
        let schema = schema_at(registry, key, version)?;
        let decoded = payload::decode(
            schema,
            version.minor,
            Encoding::Postcard,
            bytes,
            self.limits,
        )
        .map_err(as_record_error)?;
        let header = Header {
            owner,
            name: properties.name,
            partition: properties.partition,
            flags: properties.flags,
        };
        Ok(Arc::new(Object::new(id, version, header, decoded.values)))
    }

    fn path(&mut self) -> Result<FieldPath> {
        let n = self.count(u64::from(MAX_TYPE_DEPTH) + 1, "nesting_depth")?;
        if self.varint()? != SEGMENT_FIELD {
            return Err(invalid("path does not start with a field"));
        }
        let mut path = FieldPath::new(self.text()?);
        for _ in 1..n {
            path = match self.varint()? {
                SEGMENT_FIELD => path.field(self.text()?),
                SEGMENT_INDEX => path.index(self.varint()?),
                SEGMENT_KEY => path.key(self.text()?),
                _ => return Err(invalid("unknown path segment")),
            };
        }
        Ok(path)
    }

    fn value(&mut self, value_type: &crate::schema::ValueType) -> Result<Option<Value>> {
        if !self.flag()? {
            return Ok(None);
        }
        let bytes = self.blob()?;
        payload::decode_value(value_type, bytes, self.limits)
            .map(Some)
            .map_err(as_record_error)
    }

    fn op(&mut self, keys: &[TypeKey], registry: &TypeRegistry) -> Result<(Op, TypeKey)> {
        let tag = self.varint()?;
        let key = usize::try_from(self.varint()?)
            .ok()
            .and_then(|at| keys.get(at))
            .ok_or_else(|| invalid("type index out of range"))?
            .clone();
        let op = match tag {
            OP_CREATE => Op::Create {
                type_key: key.clone(),
                object: self.object(&key, registry)?,
                copied_from: self.option(Self::object_id)?,
            },
            OP_DELETE => Op::Delete {
                type_key: key.clone(),
                object: self.object(&key, registry)?,
            },
            OP_MODIFY => {
                let id = self.object_id()?;
                let version = self.version()?;
                let path = self.path()?;
                let schema = schema_at(registry, &key, version)?;
                let value_type = path
                    .resolve(schema)
                    .ok_or_else(|| invalid("path does not resolve"))?;
                Op::Modify {
                    id,
                    version,
                    before: self.value(value_type)?,
                    after: self.value(value_type)?,
                    path,
                }
            }
            OP_MODIFY_PROPERTIES => Op::ModifyProperties {
                id: self.object_id()?,
                before: self.properties()?,
                after: self.properties()?,
            },
            OP_RELINK => Op::Relink {
                id: self.object_id()?,
                before: self.option(Self::object_id)?,
                after: self.option(Self::object_id)?,
            },
            _ => return Err(invalid("unknown operation")),
        };
        Ok((op, key))
    }

    fn kind(&mut self) -> Result<TxnKind> {
        Ok(match self.varint()? {
            0 => TxnKind::Normal,
            1 => TxnKind::UndoOf(TransactionId::from_u128(self.id()?)),
            2 => TxnKind::RedoOf(TransactionId::from_u128(self.id()?)),
            3 => TxnKind::Merge,
            4 => TxnKind::Migration,
            5 => TxnKind::LinkReload,
            _ => return Err(invalid("unknown kind")),
        })
    }

    fn record(&mut self, registry: &TypeRegistry) -> Result<TxnRecord> {
        if self.varint()? != RECORD_FORMAT {
            return Err(invalid("unknown format version"));
        }
        let txn_id = TransactionId::from_u128(self.id()?);
        let parent = self.option(|input| input.id().map(TransactionId::from_u128))?;
        let session = SessionId::from_u128(self.id()?);
        let author = self.text()?;
        let time_unix_millis = self.varint()?;
        let kind = self.kind()?;
        let command = self.text()?;
        let label = self.text()?;
        let group = self.option(Self::text)?;
        let selection_after = self.option(Self::text)?;
        let n = self.count(self.limits.record_count, "record_count")?;
        let mut keys = Vec::with_capacity(n.min(PREALLOCATE));
        for _ in 0..n {
            keys.push(
                TypeKey::new(self.text()?)
                    .map_err(|error| invalid("type key").with_source(error))?,
            );
        }
        let n = self.count(self.limits.record_count, "record_count")?;
        let mut ops = Vec::with_capacity(n.min(PREALLOCATE));
        let mut type_keys = Vec::with_capacity(n.min(PREALLOCATE));
        for _ in 0..n {
            let (op, key) = self.op(&keys, registry)?;
            ops.push(op);
            type_keys.push(key);
        }
        Ok(TxnRecord {
            txn_id,
            parent,
            session,
            author,
            time_unix_millis,
            command,
            label,
            group,
            kind,
            selection_after,
            ops,
            type_keys,
        })
    }
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;
    use proptest::prelude::*;

    use super::*;
    use crate::schema::{FieldDecl, TypeDecl};
    use crate::testing::{object, values};

    const KEY: &str = "test.record";

    fn registry(fields: Vec<FieldDecl>) -> TypeRegistry {
        let mut registry = TypeRegistry::new();
        let schema = Schema::new(SchemaVersion::new(1, 0), fields);
        registry
            .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
            .unwrap();
        registry
    }

    /// A record of every operation kind on one object of a random type.
    fn record() -> impl Strategy<Value = (TxnRecord, TypeRegistry)> {
        object()
            .prop_flat_map(|(fields, before)| {
                let after = values(&fields);
                (
                    Just(fields),
                    Just(before),
                    after,
                    any::<(u128, u128, bool)>(),
                )
            })
            .prop_map(|(fields, before, after, (id, owner, copy))| {
                let key = TypeKey::new(KEY).unwrap();
                let id = ObjectId::from_u128(id);
                let header = Header {
                    owner: copy.then(|| ObjectId::from_u128(owner)),
                    name: Some("n".to_owned()),
                    ..Header::default()
                };
                let object = Arc::new(Object::new(
                    id,
                    SchemaVersion::new(1, 0),
                    header.clone(),
                    before.clone(),
                ));
                let mut ops = vec![Op::Create {
                    type_key: key.clone(),
                    object: Arc::clone(&object),
                    copied_from: copy.then(|| ObjectId::from_u128(owner)),
                }];
                for ((field, b), a) in fields.iter().zip(&before).zip(&after) {
                    ops.push(Op::Modify {
                        id,
                        version: SchemaVersion::new(1, 0),
                        path: FieldPath::new(field.name.clone()),
                        before: b.clone(),
                        after: a.clone(),
                    });
                }
                let props = Properties::of(&header);
                ops.push(Op::ModifyProperties {
                    id,
                    before: props.clone(),
                    after: Properties {
                        partition: Some("p".to_owned()),
                        flags: Flags {
                            system: copy,
                            read_only: true,
                            ..Flags::default()
                        },
                        ..props
                    },
                });
                ops.push(Op::Relink {
                    id,
                    before: header.owner,
                    after: None,
                });
                ops.push(Op::Delete {
                    type_key: key.clone(),
                    object,
                });
                let mut meta =
                    TxnMeta::new(TransactionId::from_u128(owner ^ 7), SessionId::from_u128(3));
                meta.kind = if copy {
                    TxnKind::UndoOf(TransactionId::from_u128(9))
                } else {
                    TxnKind::Normal
                };
                meta.group = copy.then(|| "\"g\"".to_owned());
                let type_keys = vec![key; ops.len()];
                let record = TxnRecord {
                    ops,
                    type_keys,
                    ..TxnRecord::from_commit(meta, Vec::new(), &empty()).unwrap()
                };
                (record, registry(fields))
            })
    }

    fn empty() -> Snapshot {
        Snapshot::empty(Arc::new(TypeRegistry::new()))
    }

    fn check_rejection(error: &Error) {
        assert!(
            [
                codes::JOURNAL_RECORD_INVALID.code,
                codes::JOURNAL_RECORD_LIMIT_EXCEEDED.code
            ]
            .contains(&error.code()),
            "{error:?}"
        );
    }

    const TIGHT: Limits = Limits {
        input_bytes: 256,
        string_bytes: 8,
        array_length: 3,
        record_count: 4,
        nesting_depth: 2,
        ..Limits::STRICT
    };

    proptest! {
        #[test]
        fn records_round_trip_canonically((record, registry) in record()) {
            let bytes = record.encode(&registry, &Limits::DEFAULT).unwrap();
            let decoded = TxnRecord::decode(&bytes, &registry, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(&decoded, &record);
            prop_assert_eq!(decoded.encode(&registry, &Limits::DEFAULT).unwrap(), bytes);
        }

        #[test]
        fn mutated_records_are_rejected_or_canonical(
            (record, registry) in record(),
            at in any::<prop::sample::Index>(),
            byte in any::<u8>(),
            cut in any::<prop::sample::Index>(),
        ) {
            let bytes = record.encode(&registry, &Limits::DEFAULT).unwrap();
            let mut mutated = bytes.clone();
            let at = at.index(mutated.len());
            mutated[at] = byte;
            let truncated = &bytes[..cut.index(bytes.len())];
            for input in [mutated.as_slice(), truncated] {
                for limits in [&Limits::DEFAULT, &TIGHT] {
                    match TxnRecord::decode(input, &registry, limits) {
                        Ok(decoded) => {
                            prop_assert_eq!(decoded.encode(&registry, limits).unwrap(), input);
                        }
                        Err(error) => check_rejection(&error),
                    }
                }
            }
        }

        #[test]
        fn arbitrary_bytes_never_panic(
            (_, registry) in record(),
            bytes in prop::collection::vec(any::<u8>(), 0..300),
        ) {
            for limits in [&Limits::DEFAULT, &TIGHT] {
                if let Err(error) = TxnRecord::decode(&bytes, &registry, limits) {
                    check_rejection(&error);
                }
            }
        }
    }

    #[test]
    fn huge_counts_fail_before_allocating() {
        let registry = registry(Vec::new());
        // Format 1, ids, empty texts, kind normal, then a type-key count of 2^63.
        let mut bytes = vec![1];
        bytes.extend([0; 16]);
        bytes.push(0);
        bytes.extend([0; 16]);
        bytes.extend([0, 0, 0, 0, 0, 0, 0]);
        bytes.extend([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
        let error = TxnRecord::decode(&bytes, &registry, &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), "doc.journal.record_limit_exceeded");
        let error = TxnRecord::decode(
            &bytes,
            &registry,
            &Limits {
                record_count: u64::MAX,
                ..Limits::DEFAULT
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "doc.journal.record_invalid");
        let mut long = vec![0xff; 10];
        long.push(2);
        let error = TxnRecord::decode(&long, &registry, &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), "doc.journal.record_invalid");
    }
}
