//! The operations of a transaction (CORE-DOC-013): what changed, by identifier, with field-level
//! before and after values.
//!
//! An [`Op`] names objects by [`ObjectId`] and type key, never by handle. Every operation carries
//! the state it replaces as well as the state it writes, so [`Op::inverse`] needs nothing else,
//! and [`Op::apply`] refuses to apply an operation to a state that does not hold its before
//! values. Applying a transaction's operations in order to its base reproduces its candidate;
//! applying their inverses in reverse order to the candidate reproduces the base.

use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId, Result};

use crate::error::codes;
use crate::load::StoredForeign;
use crate::schema::{
    FieldDecl, FieldPath, PathSegment, Schema, SchemaVersion, TypeKey, Value, ValueType,
    check_value, count, options_bits_eq, prefix_field,
};
use crate::snapshot::{Flags, Header, Object, WorkingSet};
use crate::store::Handle;

/// The header fields a property operation changes: everything in the header but the owner,
/// which [`Op::Relink`] changes. Property-set values join these with CORE-DOC-005.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    /// The name.
    pub name: Option<String>,
    /// The partition key.
    pub partition: Option<String>,
    /// The flags.
    pub flags: Flags,
}

impl Properties {
    /// The properties of `header`.
    #[must_use]
    pub fn of(header: &Header) -> Self {
        Self {
            name: header.name.clone(),
            partition: header.partition.clone(),
            flags: header.flags,
        }
    }
}

/// One operation of a transaction.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Op {
    /// An object was created.
    Create {
        /// The type key of the object.
        type_key: TypeKey,
        /// The object as created: identifier, schema version, header and values.
        object: Arc<Object>,
        /// The object it is a copy of, for lineage; [`Op::inverse`] drops it.
        copied_from: Option<ObjectId>,
    },
    /// One value of an object was set: a whole field, or a part of it inside records, lists and
    /// maps.
    Modify {
        /// The object.
        id: ObjectId,
        /// The schema version of the object's values, against which `path` resolves.
        version: SchemaVersion,
        /// The value that changed.
        path: FieldPath,
        /// The value before; `None` for an absent field or record field, or a map entry that did
        /// not exist.
        before: Option<Value>,
        /// The value after; `None` makes a field or record field absent or removes a map entry.
        after: Option<Value>,
    },
    /// The name, partition or flags of an object changed.
    ModifyProperties {
        /// The object.
        id: ObjectId,
        /// The properties before.
        before: Properties,
        /// The properties after.
        after: Properties,
    },
    /// An object was deleted.
    Delete {
        /// The type key of the object.
        type_key: TypeKey,
        /// The object as it was before deletion.
        object: Arc<Object>,
    },
    /// The owner of an object changed.
    Relink {
        /// The object.
        id: ObjectId,
        /// The owner before.
        before: Option<ObjectId>,
        /// The owner after.
        after: Option<ObjectId>,
    },
    /// A foreign data row (`arc_foreign`) was added, replaced or removed. It is independent of
    /// the objects: it names no type key and changes no derived data.
    ModifyForeign {
        /// The key of the row, its `id` column.
        id: i64,
        /// The row before; `None` for a row that did not exist. Its `id` is the key.
        before: Option<Arc<StoredForeign>>,
        /// The row after; `None` removes the row. Its `id` is the key.
        after: Option<Arc<StoredForeign>>,
    },
}

impl Op {
    /// The identifier of the object the operation changes. For a foreign data operation it is
    /// the object the row belongs to, or the nil identifier for a document-level row; use
    /// [`Op::object_id`] to tell them apart from objects the operation changes.
    #[must_use]
    pub fn id(&self) -> ObjectId {
        match self {
            Self::ModifyForeign { before, after, .. } => before
                .iter()
                .chain(after)
                .find_map(|row| row.object.as_deref().and_then(crate::load::parse_id))
                .unwrap_or_else(|| ObjectId::from_u128(0)),
            _ => self.object_id().unwrap_or_else(|| ObjectId::from_u128(0)),
        }
    }

    /// The identifier of the object the operation changes; `None` for a foreign data
    /// operation, which changes no object.
    #[must_use]
    pub fn object_id(&self) -> Option<ObjectId> {
        match self {
            Self::Create { object, .. } | Self::Delete { object, .. } => Some(object.id),
            Self::Modify { id, .. }
            | Self::ModifyProperties { id, .. }
            | Self::Relink { id, .. } => Some(*id),
            Self::ModifyForeign { .. } => None,
        }
    }

    /// The operation that undoes this one: a create becomes a delete of the same object and the
    /// other way round, and the other operations swap their before and after values. The
    /// inverse of a list of operations is the inverse of each, in reverse order.
    #[must_use]
    pub fn inverse(&self) -> Self {
        match self.clone() {
            Self::Create {
                type_key, object, ..
            } => Self::Delete { type_key, object },
            Self::Delete { type_key, object } => Self::Create {
                type_key,
                object,
                copied_from: None,
            },
            Self::Modify {
                id,
                version,
                path,
                before,
                after,
            } => Self::Modify {
                id,
                version,
                path,
                before: after,
                after: before,
            },
            Self::ModifyProperties { id, before, after } => Self::ModifyProperties {
                id,
                before: after,
                after: before,
            },
            Self::Relink { id, before, after } => Self::Relink {
                id,
                before: after,
                after: before,
            },
            Self::ModifyForeign { id, before, after } => Self::ModifyForeign {
                id,
                before: after,
                after: before,
            },
        }
    }

    /// Applies the operation to `set`. The state it applies to must hold the before values:
    /// a modified, relinked or deleted object must be live with exactly the recorded before
    /// state (and, for a delete, the recorded type and schema version), and a created
    /// identifier must not be live. Values compare bit for bit, so `0.0` and `-0.0` differ
    /// ([`Value::bits_eq`]). Otherwise the operation fails with `doc.op.mismatch`, or with the
    /// code of the working set or path that refused it, and changes nothing.
    ///
    /// An operation may come from a journal or recovery file, so what it writes is checked like
    /// the operations of a [`crate::txn::Txn`]: a create needs a registered type and schema
    /// version and one value per field of that schema, and every written value must have the
    /// addressed type, be finite and use declared enum codes (`doc.type.unknown`,
    /// `doc.value.type_mismatch`, `doc.value.not_finite`, `doc.value.out_of_range`). Required
    /// fields, bounds, references and validators remain the concern of commit validation.
    pub fn apply(&self, set: &mut WorkingSet) -> Result<()> {
        match self {
            Self::Create {
                type_key, object, ..
            } => {
                let decl = set
                    .current()
                    .registry()
                    .get(type_key.as_str())
                    .ok_or_else(|| unknown_type(type_key.as_str()))?;
                let schema = decl
                    .schema(object.version.major)
                    .ok_or_else(|| unknown_type(type_key.as_str()))?;
                check_shape(&schema.fields, &object.values)?;
                set.create_shared(type_key.as_str(), Arc::clone(object))
                    .map(drop)
            }
            Self::Delete { type_key, object } => {
                let handle = set.current().resolve(object.id)?;
                let current = set.current().get(handle)?;
                let same_type = set.current().type_key(handle) == Some(type_key);
                if !same_type || !(Arc::ptr_eq(current, object) || same_object(current, object)) {
                    return Err(mismatch(object.id));
                }
                set.delete(handle).map(drop)
            }
            Self::Modify {
                id,
                version,
                path,
                before,
                after,
            } => {
                check_write(set, *id, path, after.as_ref())?;
                let found = modified(set, *id, path, after.clone())?;
                if found.version != *version
                    || !options_bits_eq(
                        std::slice::from_ref(&found.before),
                        std::slice::from_ref(before),
                    )
                {
                    return Err(mismatch(*id));
                }
                set.replace_values(found.handle, found.values).map(drop)
            }
            Self::ModifyProperties { id, before, after } => {
                let (handle, object) = live(set, *id)?;
                if Properties::of(&object.header) != *before {
                    return Err(mismatch(*id));
                }
                let header = with_properties(&object.header, after.clone());
                set.replace_header(handle, header).map(drop)
            }
            Self::Relink { id, before, after } => {
                let (handle, object) = live(set, *id)?;
                if object.header.owner != *before {
                    return Err(mismatch(*id));
                }
                let header = Header {
                    owner: *after,
                    ..object.header.clone()
                };
                set.replace_header(handle, header).map(drop)
            }
            Self::ModifyForeign { id, before, after } => {
                let current = set.current().foreign(*id);
                let held = match (current, before) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b) || **a == **b,
                    _ => false,
                };
                if !held {
                    return Err(Error::new(codes::OP_MISMATCH).with_detail("foreign_row", *id));
                }
                match after {
                    Some(row) => {
                        check_foreign_row(*id, row)?;
                        set.set_foreign(Arc::clone(row)).map(drop)
                    }
                    None if current.is_some() => set.remove_foreign(*id).map(drop),
                    None => Ok(()),
                }
            }
        }
    }
}

/// Checks a foreign data row written at key `id`: its `id` is the key, `object` is absent or 16
/// identifier bytes, `asset` is absent or a 32-byte hash and the format and kind are not empty
/// (`doc.foreign.invalid`, argument `reason` the column). Rows loaded from a package are not
/// checked: they are kept as stored.
pub(crate) fn check_foreign_row(id: i64, row: &StoredForeign) -> Result<()> {
    let reason = if row.id != id {
        "id"
    } else if row.object.as_ref().is_some_and(|object| object.len() != 16) {
        "object"
    } else if row.asset.as_ref().is_some_and(|asset| asset.len() != 32) {
        "asset"
    } else if row.source_format.is_empty() {
        "source_format"
    } else if row.record_kind.is_empty() {
        "record_kind"
    } else {
        return Ok(());
    };
    Err(Error::new(codes::FOREIGN_INVALID)
        .with_arg("row", id)
        .with_arg("reason", reason))
}

/// Checks a write of `value` at `path` of the live object `id` of `set`: the path resolves, and
/// the value has the addressed type, is finite and uses declared enum codes, nested records
/// checked in full. Making a required field of a nested record absent fails with
/// `doc.value.missing` for the same reason; the object's own required fields are checked at
/// commit.
pub(crate) fn check_write(
    set: &WorkingSet,
    id: ObjectId,
    path: &FieldPath,
    value: Option<&Value>,
) -> Result<()> {
    let (handle, object) = live(set, id)?;
    let schema = schema_of(set, handle, &object)?;
    let value_type = path.resolve(schema).ok_or_else(|| path_invalid(path))?;
    match value {
        Some(value) => check_value(value_type, None, value)
            .map_err(|error| prefix_field(error, &path.to_string())),
        None if nested_required(schema, path) => {
            Err(Error::new(codes::VALUE_MISSING).with_arg("field", path.to_string()))
        }
        None => Ok(()),
    }
}

/// True when `path` ends at a required field of a nested record. The walk is as long as the
/// path, which has resolved against the schema.
fn nested_required(schema: &Schema, path: &FieldPath) -> bool {
    let mut current: Option<&ValueType> = None;
    let mut required = false;
    for segment in path.segments() {
        match segment {
            PathSegment::Field(name) => {
                let fields = match current {
                    None => schema.fields.as_slice(),
                    Some(ValueType::Record(fields)) => fields.as_slice(),
                    Some(_) => return false,
                };
                let Some(decl) = fields.iter().find(|field| field.name == *name) else {
                    return false;
                };
                required = current.is_some() && !decl.optional;
                current = Some(&decl.value_type);
            }
            PathSegment::Index(_) | PathSegment::Key(_) => {
                required = false;
                current = match current {
                    Some(ValueType::List(element) | ValueType::Map(element)) => Some(element),
                    _ => return false,
                };
            }
        }
    }
    required
}

/// True when two objects are equal with values compared bit for bit.
fn same_object(a: &Object, b: &Object) -> bool {
    a.id == b.id
        && a.version == b.version
        && a.header == b.header
        && options_bits_eq(&a.values, &b.values)
}

fn unknown_type(key: &str) -> Error {
    Error::new(codes::TYPE_UNKNOWN).with_arg("type", key)
}

/// One entry per field, each present value of its field's type (bounds aside).
pub(crate) fn check_shape(fields: &[FieldDecl], values: &[Option<Value>]) -> Result<()> {
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

fn mismatch(id: ObjectId) -> Error {
    Error::new(codes::OP_MISMATCH).with_detail("id", id.to_string())
}

pub(crate) fn path_invalid(path: &FieldPath) -> Error {
    Error::new(codes::PATH_INVALID).with_arg("path", path.to_string())
}

/// The handle and object of the live object `id` in the candidate of `set`.
pub(crate) fn live(set: &WorkingSet, id: ObjectId) -> Result<(Handle, Arc<Object>)> {
    let handle = set.current().resolve(id)?;
    Ok((handle, Arc::clone(set.current().get(handle)?)))
}

/// `header` with the name, partition and flags of `properties`.
pub(crate) fn with_properties(header: &Header, properties: Properties) -> Header {
    Header {
        owner: header.owner,
        name: properties.name,
        partition: properties.partition,
        flags: properties.flags,
    }
}

/// The schema the values of the object at `handle` in `set` follow.
pub(crate) fn schema_of<'a>(
    set: &'a WorkingSet,
    handle: Handle,
    object: &Object,
) -> Result<&'a Schema> {
    let snapshot = set.current();
    let key = snapshot
        .type_key(handle)
        .ok_or_else(|| Error::new(codes::OBJECT_NOT_FOUND))?;
    snapshot
        .registry()
        .get(key.as_str())
        .and_then(|decl| decl.schema(object.version.major))
        .ok_or_else(|| unknown_type(key.as_str()))
}

/// An object's values with one value written, from [`modified`].
pub(crate) struct Modified {
    pub(crate) handle: Handle,
    pub(crate) version: SchemaVersion,
    pub(crate) values: Vec<Option<Value>>,
    /// The value replaced.
    pub(crate) before: Option<Value>,
}

/// Reads the live object `id` of `set`, resolves `path` against its schema and writes `after`
/// at `path` into a copy of its values. Nothing in `set` changes.
pub(crate) fn modified(
    set: &WorkingSet,
    id: ObjectId,
    path: &FieldPath,
    after: Option<Value>,
) -> Result<Modified> {
    let (handle, object) = live(set, id)?;
    let schema = schema_of(set, handle, &object)?;
    // Resolving first bounds the path by the nesting depth of the type.
    if path.resolve(schema).is_none() {
        return Err(path_invalid(path));
    }
    let mut values = object.values.clone();
    let before = write_field(&schema.fields, &mut values, path.segments(), after)
        .ok_or_else(|| path_invalid(path))?;
    Ok(Modified {
        handle,
        version: object.version,
        values,
        before,
    })
}

/// Writes `after` at `segments` into the values of `fields` and returns the previous value, or
/// `None` (outer) when the path does not reach a value: an index past the end of a list, a part
/// of an absent value, or an absent list element.
fn write_field(
    fields: &[FieldDecl],
    values: &mut [Option<Value>],
    segments: &[PathSegment],
    after: Option<Value>,
) -> Option<Option<Value>> {
    let (PathSegment::Field(name), rest) = segments.split_first()? else {
        return None;
    };
    let at = fields.iter().position(|field| field.name == *name)?;
    let field = fields.get(at)?;
    let slot = values.get_mut(at)?;
    if rest.is_empty() {
        return Some(std::mem::replace(slot, after));
    }
    write_inside(&field.value_type, slot.as_mut()?, rest, after)
}

fn write_inside(
    value_type: &ValueType,
    value: &mut Value,
    segments: &[PathSegment],
    after: Option<Value>,
) -> Option<Option<Value>> {
    match (value_type, value, segments.split_first()?) {
        (ValueType::Record(fields), Value::Record(values), _) => {
            write_field(fields, values, segments, after)
        }
        (ValueType::List(element), Value::List(items), (PathSegment::Index(index), rest)) => {
            let item = items.get_mut(usize::try_from(*index).ok()?)?;
            if rest.is_empty() {
                // A list element cannot be absent: lists change length only as a whole.
                Some(Some(std::mem::replace(item, after?)))
            } else {
                write_inside(element, item, rest, after)
            }
        }
        (ValueType::Map(element), Value::Map(entries), (PathSegment::Key(key), rest)) => {
            if rest.is_empty() {
                Some(match after {
                    Some(value) => entries.insert(key.clone(), value),
                    None => entries.remove(key),
                })
            } else {
                write_inside(element, entries.get_mut(key)?, rest, after)
            }
        }
        _ => None,
    }
}
