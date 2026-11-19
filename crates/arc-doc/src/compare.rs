//! State comparison (CORE-DOC-026): the per-object differences between two states, with the
//! changed field paths and both values, and a canonical JSON form.
//!
//! The states are snapshots: two revisions of a session, a document and the snapshot loaded
//! from its saved file, two package files, or a snapshot and the same snapshot with the
//! operations of a change set applied ([`compare_ops`]). The comparison skips every chunk of a
//! typed store that both snapshots share by pointer, so its cost follows the changed objects,
//! not the size of the document; it never reads the journal.
//!
//! Compared are the live objects of the typed stores: header (owner, name, partition, flags),
//! type, schema version and values. Preserved and quarantined objects and foreign data rows are
//! not compared. Differences in the header and in the type or version are reported at paths in
//! parentheses: `(owner)`, `(name)`, `(partition)`, `(hidden_from_ui)`, `(system)`,
//! `(read_only)`, `(type)` and `(version)`. When the type or the schema version differs, the
//! values are not compared, since their fields are not the same fields. The values of a
//! property set instance are compared by property, at `values["name"]`.

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_base::{Error, Limits, ObjectId, Result};
use serde::Serialize;

use crate::error::codes;
use crate::op::Op;
use crate::propset::{INSTANCE_TYPE, instance};
use crate::schema::{FieldPath, Value, ValueType};
use crate::snapshot::{Object, Snapshot, WorkingSet};

/// How an object differs between two states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Live in the second state only.
    Created,
    /// Live in the first state only.
    Deleted,
    /// Live in both, with other values, header parts or type.
    Modified,
    /// Live in both with another owner (its other changes are listed too).
    Relinked,
}

/// One changed value of an object: its path (see [`FieldPath`], and the module documentation
/// for header paths) and the values before and after; `None` is absent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldChange {
    /// The canonical text of the path.
    pub path: String,
    /// The value in the first state.
    pub before: Option<Value>,
    /// The value in the second state.
    pub after: Option<Value>,
}

/// The difference of one object.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectChange {
    /// The object's identifier, hyphenated.
    pub id: String,
    /// How it differs.
    pub kind: ChangeKind,
    /// Its type key in the first state; `None` when it does not exist there.
    pub before_type: Option<String>,
    /// Its type key in the second state; `None` when it does not exist there.
    pub after_type: Option<String>,
    /// The changed values in path order of discovery; empty for a created or deleted object.
    pub changes: Vec<FieldChange>,
}

/// The differences between two states, by ascending object identifier.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Comparison {
    /// One entry per differing object.
    pub objects: Vec<ObjectChange>,
}

impl Comparison {
    /// True when the states do not differ.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// The canonical JSON form: pretty-printed with two-space indentation, objects in
    /// identifier order, keys in declared order, no trailing newline. The same states give the
    /// same text. Fails with `doc.payload.invalid` (detail `reason` = `json`) only if a value
    /// cannot be written, which finite values never are.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| {
            Error::new(codes::PAYLOAD_INVALID)
                .with_detail("reason", "json")
                .with_source(e)
        })
    }
}

/// Compares `a` (before) with `b` (after).
#[must_use]
pub fn compare(a: &Snapshot, b: &Snapshot) -> Comparison {
    let mut ids = BTreeSet::new();
    a.unshared_ids(b, &mut ids);
    b.unshared_ids(a, &mut ids);
    Comparison {
        objects: ids
            .into_iter()
            .filter_map(|id| object_change(a, b, id))
            .collect(),
    }
}

/// Compares `base` with the state that results from applying `ops` to it, for example the
/// records of a change set. Fails like [`Op::apply`] when an operation does not fit the state.
pub fn compare_ops(base: &Snapshot, ops: &[Op]) -> Result<Comparison> {
    let mut set = WorkingSet::new(base);
    for op in ops {
        op.apply(&mut set)?;
    }
    Ok(compare(base, set.current()))
}

fn lookup(snapshot: &Snapshot, id: ObjectId) -> Option<(String, Arc<Object>)> {
    let handle = snapshot.resolve(id).ok()?;
    if snapshot.held_object(handle).is_some() {
        return None;
    }
    let key = snapshot.type_key(handle)?.as_str().to_owned();
    Some((key, Arc::clone(snapshot.get(handle).ok()?)))
}

fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.bits_eq(y),
        _ => false,
    }
}

fn push(out: &mut Vec<FieldChange>, path: String, before: Option<Value>, after: Option<Value>) {
    if !same(before.as_ref(), after.as_ref()) {
        out.push(FieldChange {
            path,
            before,
            after,
        });
    }
}

fn object_change(a: &Snapshot, b: &Snapshot, id: ObjectId) -> Option<ObjectChange> {
    let (before, after) = (lookup(a, id), lookup(b, id));
    let entry = |kind, before_type: Option<&String>, after_type: Option<&String>, changes| {
        Some(ObjectChange {
            id: id.to_string(),
            kind,
            before_type: before_type.cloned(),
            after_type: after_type.cloned(),
            changes,
        })
    };
    match (before, after) {
        (None, None) => None,
        (Some((t, _)), None) => entry(ChangeKind::Deleted, Some(&t), None, Vec::new()),
        (None, Some((t, _))) => entry(ChangeKind::Created, None, Some(&t), Vec::new()),
        (Some((ta, oa)), Some((tb, ob))) => {
            if ta == tb && Arc::ptr_eq(&oa, &ob) {
                return None;
            }
            let mut changes = Vec::new();
            let (ha, hb) = (&oa.header, &ob.header);
            let owner = |o: Option<ObjectId>| o.map(Value::Reference);
            push(
                &mut changes,
                "(owner)".into(),
                owner(ha.owner),
                owner(hb.owner),
            );
            let text = |t: &Option<String>| t.clone().map(Value::String);
            push(
                &mut changes,
                "(name)".into(),
                text(&ha.name),
                text(&hb.name),
            );
            push(
                &mut changes,
                "(partition)".into(),
                text(&ha.partition),
                text(&hb.partition),
            );
            let flag = |v: bool| Some(Value::Bool(v));
            push(
                &mut changes,
                "(hidden_from_ui)".into(),
                flag(ha.flags.hidden_from_ui),
                flag(hb.flags.hidden_from_ui),
            );
            push(
                &mut changes,
                "(system)".into(),
                flag(ha.flags.system),
                flag(hb.flags.system),
            );
            push(
                &mut changes,
                "(read_only)".into(),
                flag(ha.flags.read_only),
                flag(hb.flags.read_only),
            );
            let relinked = changes.iter().any(|c| c.path == "(owner)");
            if ta != tb {
                let key = |t: &String| Some(Value::String(t.clone()));
                push(&mut changes, "(type)".into(), key(&ta), key(&tb));
            } else if oa.version != ob.version {
                let v = |v: crate::schema::SchemaVersion| Some(Value::String(v.to_string()));
                push(
                    &mut changes,
                    "(version)".into(),
                    v(oa.version),
                    v(ob.version),
                );
            } else {
                value_changes(a, b, id, &ta, (&oa, &ob), &mut changes);
            }
            if changes.is_empty() {
                return None;
            }
            let kind = if relinked {
                ChangeKind::Relinked
            } else {
                ChangeKind::Modified
            };
            entry(kind, Some(&ta), Some(&tb), changes)
        }
    }
}

fn value_changes(
    a: &Snapshot,
    b: &Snapshot,
    id: ObjectId,
    type_key: &str,
    (oa, ob): (&Object, &Object),
    out: &mut Vec<FieldChange>,
) {
    let Some(schema) = a
        .registry()
        .get(type_key)
        .and_then(|decl| decl.schema(oa.version.major))
    else {
        return;
    };
    let decoded = if type_key == INSTANCE_TYPE {
        instance(a, id, &Limits::DEFAULT)
            .ok()
            .zip(instance(b, id, &Limits::DEFAULT).ok())
    } else {
        None
    };
    for ((field, x), y) in schema.fields.iter().zip(&oa.values).zip(&ob.values) {
        if let (Some((ia, ib)), "values") = (&decoded, field.name.as_str()) {
            let names: BTreeSet<&String> = ia.values.keys().chain(ib.values.keys()).collect();
            for name in names {
                let path = FieldPath::new("values").key(name.as_str()).to_string();
                push(
                    out,
                    path,
                    ia.values.get(name).cloned(),
                    ib.values.get(name).cloned(),
                );
            }
            continue;
        }
        diff(
            &field.value_type,
            x.as_ref(),
            y.as_ref(),
            FieldPath::new(&field.name),
            out,
        );
    }
}

/// Descends into records, equally long lists and maps; any other difference is one change.
/// The descent follows the declared type, which the registry bounds in depth.
fn diff(
    value_type: &ValueType,
    a: Option<&Value>,
    b: Option<&Value>,
    path: FieldPath,
    out: &mut Vec<FieldChange>,
) {
    if same(a, b) {
        return;
    }
    match (value_type, a, b) {
        (ValueType::Record(fields), Some(Value::Record(x)), Some(Value::Record(y))) => {
            for ((field, p), q) in fields.iter().zip(x).zip(y) {
                diff(
                    &field.value_type,
                    p.as_ref(),
                    q.as_ref(),
                    path.clone().field(&field.name),
                    out,
                );
            }
        }
        (ValueType::List(inner), Some(Value::List(x)), Some(Value::List(y)))
            if x.len() == y.len() =>
        {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                let index = u64::try_from(i).unwrap_or(u64::MAX);
                diff(inner, Some(p), Some(q), path.clone().index(index), out);
            }
        }
        (ValueType::Map(inner), Some(Value::Map(x)), Some(Value::Map(y))) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for key in keys {
                diff(
                    inner,
                    x.get(key),
                    y.get(key),
                    path.clone().key(key.as_str()),
                    out,
                );
            }
        }
        _ => out.push(FieldChange {
            path: path.to_string(),
            before: a.cloned(),
            after: b.cloned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;
    use crate::registry::TypeRegistry;
    use crate::schema::{FieldDecl, Schema, SchemaVersion, TypeDecl, TypeKey};
    use crate::snapshot::Header;

    #[test]
    fn only_the_chunks_a_write_copied_are_visited() {
        let mut registry = TypeRegistry::new();
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("n", ValueType::I64)],
        );
        registry
            .register(TypeDecl::new(TypeKey::new("t.n").unwrap(), schema))
            .unwrap();
        let base = Snapshot::empty(Arc::new(registry));
        let mut set = WorkingSet::new(&base);
        let make = |n: u128, v: i64| {
            Object::new(
                ObjectId::from_u128(n),
                SchemaVersion::new(1, 0),
                Header::default(),
                vec![Some(Value::I64(v))],
            )
        };
        for n in 0..3000 {
            set.create("t.n", make(n, 0)).unwrap();
        }
        let a = set.current().clone();
        let mut set = WorkingSet::new(&a);
        let handle = a.resolve(ObjectId::from_u128(2500)).unwrap();
        set.replace_values(handle, vec![Some(Value::I64(1))])
            .unwrap();
        let b = set.current().clone();
        let mut ids = BTreeSet::new();
        a.unshared_ids(&b, &mut ids);
        // The chunk of slot 2500 holds slots 2048..3000: 952 objects, not 3000.
        assert_eq!(ids.len(), 952);
        assert_eq!(compare(&a, &b).objects.len(), 1);
    }
}
