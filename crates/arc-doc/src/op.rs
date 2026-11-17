//! The operations of a transaction (CORE-DOC-013): what changed, by identifier, with field-level
//! before and after values.
//!
//! An [`Op`] names objects by [`ObjectId`] and type key, never by handle. Every operation carries
//! the state it replaces as well as the state it writes, so [`Op::inverse`] needs nothing else,
//! and [`Op::apply`] refuses to apply an operation to a state that does not hold its before
//! values. Applying a transaction's operations in order to its base reproduces its candidate;
//! applying their inverses in reverse order to the candidate reproduces the base.

use std::sync::Arc;

use arc_base::{Error, ObjectId, Result};

use crate::error::codes;
use crate::schema::{
    FieldDecl, FieldPath, PathSegment, Schema, SchemaVersion, TypeKey, Value, ValueType,
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
}

impl Op {
    /// The identifier of the object the operation changes.
    #[must_use]
    pub fn id(&self) -> ObjectId {
        match self {
            Self::Create { object, .. } | Self::Delete { object, .. } => object.id,
            Self::Modify { id, .. }
            | Self::ModifyProperties { id, .. }
            | Self::Relink { id, .. } => *id,
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
        }
    }

    /// Applies the operation to `set`. The state it applies to must hold the before values:
    /// a modified, relinked or deleted object must be live with exactly the recorded before
    /// state (and, for a delete, the recorded type and schema version), and a created
    /// identifier must not be live. Otherwise the operation fails with `doc.op.mismatch`, or
    /// with the code of the working set or path that refused it, and changes nothing.
    pub fn apply(&self, set: &mut WorkingSet) -> Result<()> {
        match self {
            Self::Create {
                type_key, object, ..
            } => set.create(type_key.as_str(), (**object).clone()).map(drop),
            Self::Delete { type_key, object } => {
                let handle = set.current().resolve(object.id)?;
                let current = set.current().get(handle)?;
                let same_type = set.current().type_key(handle) == Some(type_key);
                if !same_type || !(Arc::ptr_eq(current, object) || **current == **object) {
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
                let found = modified(set, *id, path, after.clone())?;
                if found.version != *version || found.before != *before {
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
        }
    }
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
        .ok_or_else(|| Error::new(codes::TYPE_UNKNOWN).with_arg("type", key.as_str()))
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
