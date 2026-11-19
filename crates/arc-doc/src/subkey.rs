//! Sub-object keys (CORE-DOC-006): the address `(ObjectId, local key)` of an object's part that
//! is not an object, such as a vertex of a polyline, a face role of a wall or a row of a table.
//!
//! A type holds such parts in a **keyed list**: a field of type `list<record>` whose first record
//! field is `key`, a required `i64` in `0..=u32::MAX` ([`keyed_list`] declares one). The key is
//! data of the part, so it stays with the part whatever else changes: inserting, removing or
//! moving other parts, editing the part's other fields, undo and redo, and a save and load all
//! keep it. Only the index of a part is not stable; [`path_of`] resolves a key to the current
//! path of its part. The functions here read through a [`Snapshot`] and write through a
//! [`Txn`], so every edit is an ordinary journaled modification.

use std::fmt;

use arc_base::{Error, ObjectId, Result};
use serde::Serialize;

use crate::error::codes;
use crate::schema::{Bound, FieldDecl, FieldPath, Value, ValueType};
use crate::snapshot::{Object, Snapshot};
use crate::txn::Txn;

/// The name of the key field of a part's record.
pub const KEY_FIELD: &str = "key";

/// The address of an object's part: the object and the local key its type assigned to the part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SubKey {
    /// The object that holds the part.
    pub object: ObjectId,
    /// The key of the part, unique among the parts of one keyed list of the object.
    pub key: u32,
}

impl fmt::Display for SubKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.object, self.key)
    }
}

/// The declaration of a keyed list field named `name`: a list of records with the key field
/// first, then `part_fields`.
#[must_use]
pub fn keyed_list(name: impl Into<String>, part_fields: Vec<FieldDecl>) -> FieldDecl {
    let key = FieldDecl::new(KEY_FIELD, ValueType::I64).with_bound(Bound::Int {
        min: 0,
        max: i64::from(u32::MAX),
    });
    let mut fields = vec![key];
    fields.extend(part_fields);
    FieldDecl::new(name, ValueType::List(Box::new(ValueType::Record(fields))))
}

fn invalid(field: &str, reason: &'static str) -> Error {
    Error::new(codes::PART_INVALID)
        .with_arg("field", field)
        .with_arg("reason", reason)
}

/// The key of one part: the first value of its record.
fn key_of(part: &Value) -> Option<u32> {
    match part {
        Value::Record(values) => match values.first() {
            Some(Some(Value::I64(key))) => u32::try_from(*key).ok(),
            _ => None,
        },
        _ => None,
    }
}

/// The keys of the parts in `list`, the value of the keyed list field `field`, in list order.
///
/// Fails with `doc.part.invalid` when an element is not a record that starts with a key in
/// `0..=u32::MAX` or when two parts have the same key. A type validator can use it to enforce
/// the keyed list rules at commit.
pub fn list_keys(field: &str, list: &Value) -> Result<Vec<u32>> {
    let Value::List(parts) = list else {
        return Err(invalid(field, "not a list"));
    };
    let mut keys = Vec::with_capacity(parts.len());
    for part in parts {
        keys.push(key_of(part).ok_or_else(|| invalid(field, "key"))?);
    }
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != keys.len() {
        return Err(invalid(field, "duplicate key"));
    }
    Ok(keys)
}

/// The object `id` and the index of its keyed list field `field`.
fn locate(snapshot: &Snapshot, id: ObjectId, field: &str) -> Result<(Object, usize)> {
    let handle = snapshot.resolve(id)?;
    let object = snapshot.get(handle)?;
    let schema = snapshot
        .type_key(handle)
        .and_then(|key| snapshot.registry().get(key.as_str()))
        .and_then(|decl| decl.schema(object.version.major))
        .ok_or_else(|| invalid(field, "type"))?;
    let index = schema
        .fields
        .iter()
        .position(|declared| declared.name == field)
        .ok_or_else(|| invalid(field, "no such field"))?;
    let keyed = schema.fields.get(index).is_some_and(|declared| {
        matches!(
            &declared.value_type,
            ValueType::List(element) if matches!(
                element.as_ref(),
                ValueType::Record(fields) if fields.first().is_some_and(|first|
                    first.name == KEY_FIELD && first.value_type == ValueType::I64 && !first.optional)
            )
        )
    });
    if !keyed {
        return Err(invalid(field, "not a keyed list"));
    }
    Ok((Object::clone(object), index))
}

/// The parts of the keyed list at `index` of `object`; an absent optional list has none.
fn parts_of(object: &Object, index: usize, field: &str) -> Result<Vec<Value>> {
    match object.values.get(index) {
        Some(Some(Value::List(parts))) => Ok(parts.clone()),
        Some(None) => Ok(Vec::new()),
        _ => Err(invalid(field, "not a list")),
    }
}

/// The addresses of the parts in the keyed list `field` of `id`, in list order.
///
/// Fails with `doc.object.not_found`, or `doc.part.invalid` when `field` is not a keyed list of
/// the object's type or its keys are not valid and distinct.
pub fn parts(snapshot: &Snapshot, id: ObjectId, field: &str) -> Result<Vec<SubKey>> {
    let (object, index) = locate(snapshot, id, field)?;
    let list = Value::List(parts_of(&object, index, field)?);
    Ok(list_keys(field, &list)?
        .into_iter()
        .map(|key| SubKey { object: id, key })
        .collect())
}

/// The current path of the part `key` in the keyed list `field` of its object: the field and the
/// part's index. Keys are stable; the path is valid for the snapshot it was resolved in.
///
/// Fails with `doc.part.not_found` when no part has the key, and with the codes of [`parts`].
pub fn path_of(snapshot: &Snapshot, key: SubKey, field: &str) -> Result<FieldPath> {
    let position = parts(snapshot, key.object, field)?
        .iter()
        .position(|part| *part == key)
        .ok_or_else(|| {
            Error::new(codes::PART_NOT_FOUND)
                .with_arg("object", key.object.to_string())
                .with_arg("key", i64::from(key.key))
                .with_arg("field", field)
        })?;
    Ok(FieldPath::new(field).index(u64::try_from(position).unwrap_or(u64::MAX)))
}

/// Adds a part to the keyed list `field` of the object `id` and returns its key.
///
/// `part` holds the values of the part's record after the key field, in declared order. The
/// key is one more than the highest key in the list, or 0 for an empty list, so a key is not
/// reused while a part with a higher key exists, but it is reused after the part with the
/// highest key was removed. A part inserted at `at` goes before the part now at that index; an
/// index past the end, and `None`, append. Fails with the codes of [`parts`] and
/// [`Txn::modify`], and `doc.part.invalid` when the key space (`u32`) is exhausted.
pub fn insert(
    txn: &mut Txn,
    id: ObjectId,
    field: &str,
    part: Vec<Option<Value>>,
    at: Option<usize>,
) -> Result<SubKey> {
    let (object, index) = locate(txn.snapshot(), id, field)?;
    let mut list = parts_of(&object, index, field)?;
    let keys = list_keys(field, &Value::List(list.clone()))?;
    let key = keys
        .iter()
        .max()
        .map_or(Some(0), |highest| highest.checked_add(1))
        .ok_or_else(|| invalid(field, "key space"))?;
    let mut values = Vec::with_capacity(part.len().saturating_add(1));
    values.push(Some(Value::I64(i64::from(key))));
    values.extend(part);
    let position = at.map_or(list.len(), |at| at.min(list.len()));
    list.insert(position, Value::Record(values));
    txn.modify(id, FieldPath::new(field), Some(Value::List(list)))?;
    Ok(SubKey { object: id, key })
}

/// Removes the part `key` from the keyed list `field`; the other parts keep their keys. Fails
/// with `doc.part.not_found` and the codes of [`insert`].
pub fn remove(txn: &mut Txn, key: SubKey, field: &str) -> Result<()> {
    // Fails with `doc.part.not_found` when the part is missing.
    path_of(txn.snapshot(), key, field)?;
    let (object, index) = locate(txn.snapshot(), key.object, field)?;
    let mut list = parts_of(&object, index, field)?;
    list.retain(|part| key_of(part) != Some(key.key));
    txn.modify(key.object, FieldPath::new(field), Some(Value::List(list)))
}

/// Sets the field `part_field` of the part `key` in the keyed list `field` to `value`
/// (`None` makes an optional field absent). Fails with `doc.part.not_found`, and with the
/// codes of [`Txn::modify`] (a path that does not resolve is `doc.path.invalid`).
pub fn set(
    txn: &mut Txn,
    key: SubKey,
    field: &str,
    part_field: &str,
    value: Option<Value>,
) -> Result<()> {
    let path = path_of(txn.snapshot(), key, field)?.field(part_field);
    txn.modify(key.object, path, value)
}
