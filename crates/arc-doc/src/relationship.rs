//! The references an object holds and the relationship rules that govern them (CORE-DOC-008).
//!
//! A reference lives in a reference field of the object, in a list of references that is the
//! type of a field, or in such fields of nested records reached through records, lists and maps
//! (the registry allows references nowhere else). The header `owner` is a reference too: it must
//! name a live object and, when that object is deleted, the owned object is deleted with it
//! ([`OWNER_SPEC`]).
//!
//! [`references`] enumerates what an object holds; [`spec_of`] finds the rule of one field;
//! [`copy_references`] applies the on-copy rules to a set of objects being copied.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use arc_base::{Error, ObjectId, Result};

use crate::error::codes;
use crate::registry::TypeRegistry;
use crate::schema::{
    Cardinality, FieldDecl, FieldPath, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Strength,
    TypeKey, Value, ValueType, options_bits_eq,
};
use crate::snapshot::{Header, Object, Snapshot};

/// The place of a reference inside its object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReferenceField {
    /// The `owner` of the object header.
    Owner,
    /// A reference value inside the field values, by its path; a list element's path ends in its
    /// index.
    Field(FieldPath),
}

impl fmt::Display for ReferenceField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Parentheses never occur in a field path.
            Self::Owner => f.write_str("(owner)"),
            Self::Field(path) => path.fmt(f),
        }
    }
}

/// The rule of the header `owner`: any target type; the owned object is deleted with its owner
/// (on-delete `cascade`); the owner is optional. Its strength is `associative` because the owned
/// object holds the reference and depends on its owner; the owner's side of the relation has no
/// field. A copy keeps the owner unless the owner is copied too.
pub static OWNER_SPEC: ReferenceSpec = ReferenceSpec {
    targets: Vec::new(),
    rule: RelationshipRule {
        strength: Strength::Associative,
        on_delete: OnDelete::Cascade,
        on_copy: OnCopy::ShareTarget,
        cardinality: Cardinality::Optional,
    },
};

/// One reference an object holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference<'a> {
    /// Where the object holds it.
    pub field: ReferenceField,
    /// The target identifier.
    pub target: ObjectId,
    /// The allowed target types and the relationship rule.
    pub spec: &'a ReferenceSpec,
}

/// The fields of the schema `object` was written with, if its type is registered.
fn fields_of<'a>(registry: &'a TypeRegistry, type_key: &str, object: &Object) -> &'a [FieldDecl] {
    registry
        .get(type_key)
        .and_then(|decl| decl.schema(object.version.major))
        .map_or(&[], |schema| schema.fields.as_slice())
}

/// Every reference `object` of type `type_key` holds: the header owner first, then the field
/// references in field order, list elements in list order and map entries in key order.
///
/// Values that do not match their declared type hold no references here; the field schema check
/// rejects them. An object of an unregistered type holds its owner only.
#[must_use]
pub fn references<'a>(
    registry: &'a TypeRegistry,
    type_key: &str,
    object: &Object,
) -> Vec<Reference<'a>> {
    references_and_cardinality(registry, type_key, object).0
}

/// The references `object` holds and the path of the first reference field whose cardinality
/// it violates (a `one` field without value, a `many` list longer than its bound).
pub(crate) fn references_and_cardinality<'a>(
    registry: &'a TypeRegistry,
    type_key: &str,
    object: &Object,
) -> (Vec<Reference<'a>>, Option<FieldPath>) {
    let mut walk = Walk::default();
    if let Some(owner) = object.header.owner {
        walk.found.push(Reference {
            field: ReferenceField::Owner,
            target: owner,
            spec: &OWNER_SPEC,
        });
    }
    walk.fields(fields_of(registry, type_key, object), &object.values, None);
    (walk.found, walk.violation)
}

/// The references held in `values`, field values of `fields`, without the header owner: what
/// [`references`] lists after the owner, for a field list that does not come from a registry
/// (the schema of a stored descriptor).
#[cfg(feature = "package")]
pub(crate) fn field_references<'a>(
    fields: &'a [FieldDecl],
    values: &[Option<Value>],
) -> Vec<Reference<'a>> {
    let mut walk = Walk::default();
    walk.fields(fields, values, None);
    walk.found
}

/// The references that differ between two states of one object of type `type_key` with the
/// same major schema version: those `old` holds and those `new` holds in the header owner, if it
/// changed, and in the reference-holding fields whose values differ. Unchanged fields are
/// compared but not walked, so the cost is proportional to the changed fields, not to every
/// reference the object holds.
pub(crate) fn changed_references<'a>(
    registry: &'a TypeRegistry,
    type_key: &str,
    old: &Object,
    new: &Object,
) -> (Vec<Reference<'a>>, Vec<Reference<'a>>) {
    let (mut before, mut after) = (Walk::default(), Walk::default());
    if old.header.owner != new.header.owner {
        for (walk, owner) in [
            (&mut before, old.header.owner),
            (&mut after, new.header.owner),
        ] {
            if let Some(target) = owner {
                walk.found.push(Reference {
                    field: ReferenceField::Owner,
                    target,
                    spec: &OWNER_SPEC,
                });
            }
        }
    }
    let fields = fields_of(registry, type_key, new);
    for (at, field) in fields.iter().enumerate() {
        let (Some(was), Some(is)) = (old.values.get(at..=at), new.values.get(at..=at)) else {
            continue;
        };
        if !has_reference(&field.value_type) || was == is {
            continue;
        }
        let field = std::slice::from_ref(field);
        before.fields(field, was, None);
        after.fields(field, is, None);
    }
    (before.found, after.found)
}

/// The spec of the reference at `field` of `object`, or `None` when the path does not address a
/// reference in its schema.
#[must_use]
pub fn spec_of<'a>(
    registry: &'a TypeRegistry,
    type_key: &str,
    object: &Object,
    field: &ReferenceField,
) -> Option<&'a ReferenceSpec> {
    match field {
        ReferenceField::Owner => Some(&OWNER_SPEC),
        ReferenceField::Field(path) => {
            let schema = registry.get(type_key)?.schema(object.version.major)?;
            match path.resolve(schema)? {
                ValueType::Reference(spec) => Some(spec),
                _ => None,
            }
        }
    }
}

/// True when values of `value_type` can hold a reference. Recursion follows the type, whose
/// nesting the registry bounds.
pub(crate) fn has_reference(value_type: &ValueType) -> bool {
    match value_type {
        ValueType::Reference(_) => true,
        ValueType::List(element) | ValueType::Map(element) => has_reference(element),
        ValueType::Record(fields) => fields.iter().any(|field| has_reference(&field.value_type)),
        _ => false,
    }
}

#[derive(Default)]
struct Walk<'a> {
    found: Vec<Reference<'a>>,
    violation: Option<FieldPath>,
}

impl<'a> Walk<'a> {
    /// Walks a field list. Recursion follows the declared types, so it is as deep as the
    /// registry allows types to nest, whatever the values.
    fn fields(
        &mut self,
        fields: &'a [FieldDecl],
        values: &[Option<Value>],
        at: Option<&FieldPath>,
    ) {
        for (field, value) in fields.iter().zip(values) {
            if !has_reference(&field.value_type) {
                continue;
            }
            let path = at.map_or_else(
                || FieldPath::new(field.name.as_str()),
                |at| at.clone().field(field.name.as_str()),
            );
            match value {
                Some(value) => self.value(&field.value_type, value, path),
                None => {
                    if let ValueType::Reference(spec) = &field.value_type
                        && spec.rule.cardinality == Cardinality::One
                    {
                        self.violation.get_or_insert(path);
                    }
                }
            }
        }
    }

    fn value(&mut self, value_type: &'a ValueType, value: &Value, path: FieldPath) {
        match (value_type, value) {
            (ValueType::Reference(spec), Value::Reference(target)) => self.found.push(Reference {
                field: ReferenceField::Field(path),
                target: *target,
                spec,
            }),
            (ValueType::List(element), Value::List(items)) => {
                if let ValueType::Reference(spec) = element.as_ref()
                    && let Cardinality::Many { max } = spec.rule.cardinality
                    && u32::try_from(items.len()).is_ok_and(|n| n > max)
                {
                    self.violation.get_or_insert(path.clone());
                }
                for (index, item) in (0_u64..).zip(items) {
                    self.value(element, item, path.clone().index(index));
                }
            }
            (ValueType::Map(element), Value::Map(entries)) => {
                for (key, item) in entries {
                    self.value(element, item, path.clone().key(key.as_str()));
                }
            }
            (ValueType::Record(fields), Value::Record(values)) => {
                self.fields(fields, values, Some(&path));
            }
            _ => {}
        }
    }
}

/// What to do with one reference value while rewriting.
pub(crate) type Rewrite<'f> = dyn FnMut(&ReferenceSpec, ObjectId) -> Option<ObjectId> + 'f;

/// Rewrites every reference in `values`: `rewrite` returns the new target, or `None` to remove
/// the reference (an optional field becomes absent, a list loses the element). Returns true
/// when anything changed.
pub(crate) fn rewrite_fields(
    fields: &[FieldDecl],
    values: &mut [Option<Value>],
    rewrite: &mut Rewrite<'_>,
) -> bool {
    let mut changed = false;
    for (field, slot) in fields.iter().zip(values.iter_mut()) {
        if !has_reference(&field.value_type) {
            continue;
        }
        match (&field.value_type, slot.as_mut()) {
            (ValueType::Reference(spec), Some(Value::Reference(target))) => {
                match rewrite(spec, *target) {
                    Some(new) if new == *target => {}
                    Some(new) => {
                        *target = new;
                        changed = true;
                    }
                    None => {
                        *slot = None;
                        changed = true;
                    }
                }
            }
            (value_type, Some(value)) => changed |= rewrite_value(value_type, value, rewrite),
            (_, None) => {}
        }
    }
    changed
}

fn rewrite_value(value_type: &ValueType, value: &mut Value, rewrite: &mut Rewrite<'_>) -> bool {
    let mut changed = false;
    match (value_type, value) {
        (ValueType::List(element), Value::List(items)) => {
            if let ValueType::Reference(spec) = element.as_ref() {
                items.retain_mut(|item| {
                    let Value::Reference(target) = item else {
                        return true;
                    };
                    match rewrite(spec, *target) {
                        Some(new) => {
                            changed |= new != *target;
                            *target = new;
                            true
                        }
                        None => {
                            changed = true;
                            false
                        }
                    }
                });
            } else {
                for item in items {
                    changed |= rewrite_value(element, item, rewrite);
                }
            }
        }
        (ValueType::Map(element), Value::Map(entries)) => {
            for item in entries.values_mut() {
                changed |= rewrite_value(element, item, rewrite);
            }
        }
        (ValueType::Record(fields), Value::Record(values)) => {
            changed |= rewrite_fields(fields, values, rewrite);
        }
        _ => {}
    }
    changed
}

/// The paths of the object fields whose values differ between `before` and `after`.
pub(crate) fn changed_fields(
    fields: &[FieldDecl],
    before: &[Option<Value>],
    after: &[Option<Value>],
) -> Vec<FieldPath> {
    fields
        .iter()
        .zip(before.iter().zip(after))
        .filter(|(_, (before, after))| {
            !options_bits_eq(std::slice::from_ref(*before), std::slice::from_ref(*after))
        })
        .map(|(field, _)| FieldPath::new(field.name.as_str()))
        .collect()
}

/// One object of a copy with its references rewritten, from [`copy_references`].
#[derive(Debug, Clone, PartialEq)]
pub struct CopiedObject {
    /// The object copied.
    pub source: ObjectId,
    /// The identifier of the copy.
    pub copy: ObjectId,
    /// The type key of both.
    pub type_key: TypeKey,
    /// The header of the copy: the owner is the copy of the owner when the owner is copied too,
    /// otherwise the same owner.
    pub header: Header,
    /// The field values of the copy.
    pub values: Vec<Option<Value>>,
}

/// The result of [`copy_references`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CopyPlan {
    /// The copies, in ascending order of source identifier.
    pub objects: Vec<CopiedObject>,
    /// Targets of `copy_target` references that are not in the mapping, ascending. When it is
    /// not empty the copy is incomplete: the caller adds these objects to the mapping and plans
    /// again; their references still name the originals in `objects`.
    pub copy_targets: Vec<ObjectId>,
}

/// Applies the on-copy rules to the objects of `snapshot` being copied: `mapping` maps each of
/// them to the identifier of its copy.
///
/// A reference to an object that is copied too names that object's copy, whatever its rule, so
/// a copied group stays connected. Any other reference follows its rule: `copy_target` keeps the
/// original target and lists it in [`CopyPlan::copy_targets`], `share_target` keeps it, and
/// `clear` removes it (an optional field becomes absent, a list loses the element). Fails with
/// `doc.object.not_found` when a key of `mapping` is not live.
///
/// Running a copy (assigning identifiers, creating the objects, recording `copied_from`) is a
/// command of an upper layer; this function only plans the values.
pub fn copy_references(
    snapshot: &Snapshot,
    mapping: &BTreeMap<ObjectId, ObjectId>,
) -> Result<CopyPlan> {
    let registry = snapshot.registry();
    let mut plan = CopyPlan::default();
    let mut needed = BTreeSet::new();
    for (&source, &copy) in mapping {
        let handle = snapshot.resolve(source)?;
        let object = snapshot.get(handle)?;
        let type_key = snapshot
            .type_key(handle)
            .cloned()
            .ok_or_else(|| Error::new(codes::OBJECT_NOT_FOUND))?;
        let mut header = object.header.clone();
        if let Some(owner) = header.owner
            && let Some(&copied) = mapping.get(&owner)
        {
            header.owner = Some(copied);
        }
        let mut values = object.values.clone();
        let fields = fields_of(registry, type_key.as_str(), object);
        rewrite_fields(fields, &mut values, &mut |spec, target| {
            if let Some(&copied) = mapping.get(&target) {
                return Some(copied);
            }
            match spec.rule.on_copy {
                OnCopy::CopyTarget => {
                    needed.insert(target);
                    Some(target)
                }
                OnCopy::ShareTarget => Some(target),
                OnCopy::Clear => None,
            }
        });
        plan.objects.push(CopiedObject {
            source,
            copy,
            type_key,
            header,
            values,
        });
    }
    plan.copy_targets = needed.into_iter().collect();
    Ok(plan)
}
