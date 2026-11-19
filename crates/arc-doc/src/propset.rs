//! Property sets (CORE-DOC-005): user-defined groups of typed properties attached to objects.
//!
//! A **definition** is an object of type [`DEFINITION_TYPE`]: its header name, the type keys it
//! applies to (none: every type) and its properties. The properties are held as the JSON field
//! descriptor of a schema whose fields are the properties, so a property has everything a field
//! has (type, unit dimension, enum values, default, bound, optionality) and the descriptor
//! grammar validates it. An **instance** is an object of type [`INSTANCE_TYPE`] that references
//! its target and its definition and holds the property values, in the payload encoding of the
//! definition's schema, in one opaque field. Instances are ordinary objects, so creating,
//! changing and deleting them is journaled, undone and compared like any other change, and the
//! relationship rules delete an object's instances with it and refuse to delete a definition
//! that instances still use.
//!
//! [`redefine`] changes a definition and migrates every instance inside the same transaction.
//! Property values are not references, assets, geometry or opaque bytes, since the reverse
//! reference index and the asset scan do not look inside an instance's encoded values.

use std::collections::BTreeMap;

use arc_base::{Error, Limits, ObjectId, Result};

use crate::descriptor::descriptor;
use crate::error::codes;
use crate::payload::{Encoding, decode, encode};
use crate::registry::TypeRegistry;
use crate::relationship::ReferenceField;
use crate::schema::{
    Cardinality, FieldDecl, FieldPath, Finding, OnCopy, OnDelete, ReferenceSpec, RelationshipRule,
    Schema, SchemaVersion, Severity, Strength, TypeDecl, TypeKey, Value, ValueType, check_value,
};
use crate::snapshot::{Change, Header, Snapshot};
use crate::txn::Txn;
use crate::validate::DocumentValidator;

/// The type key of property set definitions.
pub const DEFINITION_TYPE: &str = "core.doc.property_set_definition";
/// The type key of property set instances.
pub const INSTANCE_TYPE: &str = "core.doc.property_set";

const APPLICABLE: &str = "applicable_types";
const PROPERTIES: &str = "properties";
const TARGET: &str = "target";
const DEFINITION: &str = "definition";
const VALUES: &str = "values";

/// A property set definition as read from the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    /// The name, from the object's header.
    pub name: Option<String>,
    /// The types an instance may be attached to; empty allows every type.
    pub applicable_types: Vec<TypeKey>,
    /// The properties, in order.
    pub properties: Vec<FieldDecl>,
}

/// A property set instance as read from the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// The object the instance is attached to.
    pub target: ObjectId,
    /// The definition the instance follows.
    pub definition: ObjectId,
    /// The values of the properties that have one, by property name.
    pub values: BTreeMap<String, Value>,
}

fn invalid(reason: &'static str) -> Error {
    Error::new(codes::PROPSET_INVALID).with_arg("reason", reason)
}

fn reference(targets: Vec<TypeKey>, on_delete: OnDelete) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets,
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::One,
        },
    })
}

/// Registers the two property set types in `registry`: fails like [`TypeRegistry::register`].
pub fn register(registry: &mut TypeRegistry) -> Result<()> {
    let version = SchemaVersion::new(1, 0);
    let definition = Schema::new(
        version,
        vec![
            FieldDecl::new(APPLICABLE, ValueType::List(Box::new(ValueType::String))),
            FieldDecl::new(PROPERTIES, ValueType::String),
        ],
    );
    registry.register(TypeDecl::new(TypeKey::new(DEFINITION_TYPE)?, definition))?;
    let definition_key = TypeKey::new(DEFINITION_TYPE)?;
    let instance = Schema::new(
        version,
        vec![
            FieldDecl::new(TARGET, reference(Vec::new(), OnDelete::Cascade)),
            FieldDecl::new(DEFINITION, reference(vec![definition_key], OnDelete::Block)),
            FieldDecl::new(VALUES, ValueType::Opaque),
        ],
    );
    registry.register(TypeDecl::new(TypeKey::new(INSTANCE_TYPE)?, instance))
}

/// True when property sets may hold values of `value_type` (see the module documentation).
/// Recursion is bounded by the registry's type depth, which every caller has checked.
fn allowed(value_type: &ValueType) -> bool {
    match value_type {
        ValueType::Reference(_)
        | ValueType::ExternalReference
        | ValueType::Asset
        | ValueType::Geometry(_)
        | ValueType::Opaque => false,
        ValueType::List(inner) | ValueType::Map(inner) => allowed(inner),
        ValueType::Record(fields) => fields.iter().all(|f| allowed(&f.value_type)),
        _ => true,
    }
}

fn properties_decl(properties: &[FieldDecl]) -> Result<(TypeDecl, SchemaVersion)> {
    let version = SchemaVersion::new(1, 0);
    let schema = Schema::new(version, properties.to_vec());
    Ok((TypeDecl::new(TypeKey::new(INSTANCE_TYPE)?, schema), version))
}

/// The descriptor text of `properties`, after checking that they form a valid schema of
/// allowed types.
fn encode_properties(properties: &[FieldDecl]) -> Result<String> {
    let (decl, version) = properties_decl(properties)?;
    let text = descriptor(&decl, version).ok_or_else(|| invalid("properties"))?;
    if !properties.iter().all(|p| allowed(&p.value_type)) {
        return Err(invalid("property type"));
    }
    Ok(text)
}

fn decode_properties(text: &str, limits: &Limits) -> Result<Schema> {
    let (key, schema) =
        Schema::from_descriptor(text, limits).map_err(|e| invalid("descriptor").with_source(e))?;
    if key.as_str() != INSTANCE_TYPE
        || schema.version != SchemaVersion::new(1, 0)
        || !schema.fields.iter().all(|p| allowed(&p.value_type))
    {
        return Err(invalid("descriptor"));
    }
    Ok(schema)
}

fn text_of(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

/// The definition `id`. Fails with `doc.object.not_found` and `doc.propset.invalid` (not a
/// definition, or malformed).
pub fn definition(snapshot: &Snapshot, id: ObjectId, limits: &Limits) -> Result<Definition> {
    let handle = snapshot.resolve(id)?;
    let object = snapshot.get(handle)?;
    if snapshot.type_key(handle).map(TypeKey::as_str) != Some(DEFINITION_TYPE) {
        return Err(invalid("not a definition"));
    }
    let Some(Some(Value::List(types))) = object.values.first() else {
        return Err(invalid("applicable_types"));
    };
    let applicable_types = types
        .iter()
        .map(|t| match t {
            Value::String(key) => TypeKey::new(key.clone()),
            _ => Err(invalid("applicable_types")),
        })
        .collect::<Result<Vec<_>>>()?;
    let text = text_of(object.values.get(1).and_then(Option::as_ref))
        .ok_or_else(|| invalid("properties"))?;
    Ok(Definition {
        name: object.header.name.clone(),
        applicable_types,
        properties: decode_properties(text, limits)?.fields,
    })
}

fn applicable(types: &[TypeKey], target_type: Option<&TypeKey>) -> bool {
    types.is_empty() || target_type.is_some_and(|t| types.contains(t))
}

fn names(types: &[TypeKey]) -> Value {
    Value::List(
        types
            .iter()
            .map(|t| Value::String(t.as_str().to_owned()))
            .collect(),
    )
}

/// Creates the definition `id` named `name`. Fails with `doc.propset.invalid` for properties
/// that are not a valid schema of allowed types, and with the codes of [`Txn::create`].
pub fn define(
    txn: &mut Txn,
    id: ObjectId,
    name: &str,
    applicable_types: &[TypeKey],
    properties: &[FieldDecl],
) -> Result<()> {
    let text = encode_properties(properties)?;
    let header = Header {
        name: Some(name.to_owned()),
        ..Header::default()
    };
    txn.create(
        DEFINITION_TYPE,
        id,
        header,
        vec![Some(names(applicable_types)), Some(Value::String(text))],
    )
    .map(drop)
}

/// The instances of definition `definition` and their targets, in identifier order.
fn instances_using(snapshot: &Snapshot, definition: ObjectId) -> Vec<ObjectId> {
    referring(snapshot, definition, DEFINITION)
}

fn referring(snapshot: &Snapshot, to: ObjectId, field: &str) -> Vec<ObjectId> {
    let wanted = ReferenceField::Field(FieldPath::new(field));
    let mut out: Vec<ObjectId> = snapshot
        .referrers(to)
        .filter(|r| r.field == wanted)
        .filter(|r| {
            snapshot
                .resolve(r.source)
                .ok()
                .and_then(|h| snapshot.type_key(h))
                .map(TypeKey::as_str)
                == Some(INSTANCE_TYPE)
        })
        .map(|r| r.source)
        .collect();
    out.dedup();
    out
}

/// The instances attached to `target`, in identifier order.
#[must_use]
pub fn instances_of(snapshot: &Snapshot, target: ObjectId) -> Vec<ObjectId> {
    referring(snapshot, target, TARGET)
}

/// The instance of `definition` attached to `target`, if any.
pub fn instance_of(
    snapshot: &Snapshot,
    target: ObjectId,
    definition: ObjectId,
) -> Option<ObjectId> {
    instances_of(snapshot, target).into_iter().find(|id| {
        snapshot
            .resolve(*id)
            .and_then(|h| snapshot.get(h))
            .is_ok_and(|o| o.values.get(1) == Some(&Some(Value::Reference(definition))))
    })
}

struct Raw {
    target: ObjectId,
    definition: ObjectId,
    bytes: Vec<u8>,
}

fn raw(snapshot: &Snapshot, id: ObjectId) -> Result<Raw> {
    let handle = snapshot.resolve(id)?;
    let object = snapshot.get(handle)?;
    if snapshot.type_key(handle).map(TypeKey::as_str) != Some(INSTANCE_TYPE) {
        return Err(invalid("not an instance"));
    }
    match object.values.as_slice() {
        [
            Some(Value::Reference(target)),
            Some(Value::Reference(definition)),
            Some(Value::Opaque(bytes)),
        ] => Ok(Raw {
            target: *target,
            definition: *definition,
            bytes: bytes.clone(),
        }),
        _ => Err(invalid("instance fields")),
    }
}

fn decode_values(schema: &Schema, bytes: &[u8], limits: &Limits) -> Result<Vec<Option<Value>>> {
    decode(
        schema,
        schema.version.minor,
        Encoding::Postcard,
        bytes,
        limits,
    )
    .map(|decoded| decoded.values)
    .map_err(|e| invalid("values").with_source(e))
}

fn by_name(schema: &Schema, values: Vec<Option<Value>>) -> BTreeMap<String, Value> {
    schema
        .fields
        .iter()
        .zip(values)
        .filter_map(|(field, value)| Some((field.name.clone(), value?)))
        .collect()
}

/// The instance `id` with its values decoded by its definition's schema. Fails with
/// `doc.object.not_found` and `doc.propset.invalid`.
pub fn instance(snapshot: &Snapshot, id: ObjectId, limits: &Limits) -> Result<Instance> {
    let raw = raw(snapshot, id)?;
    let definition = definition(snapshot, raw.definition, limits)?;
    let schema = Schema::new(SchemaVersion::new(1, 0), definition.properties);
    let values = by_name(&schema, decode_values(&schema, &raw.bytes, limits)?);
    Ok(Instance {
        target: raw.target,
        definition: raw.definition,
        values,
    })
}

/// Positional values of `schema` from `given`; a property without a value takes its default.
fn assemble(schema: &Schema, given: &BTreeMap<String, Value>) -> Result<Vec<Option<Value>>> {
    if given
        .keys()
        .any(|k| !schema.fields.iter().any(|f| f.name == *k))
    {
        return Err(invalid("unknown property"));
    }
    Ok(schema
        .fields
        .iter()
        .map(|f| given.get(&f.name).cloned().or_else(|| f.default.clone()))
        .collect())
}

fn instance_values(
    definition: &Definition,
    values: &[Option<Value>],
    limits: &Limits,
) -> Result<Value> {
    let schema = Schema::new(SchemaVersion::new(1, 0), definition.properties.clone());
    Ok(Value::Opaque(encode(&schema, values, limits)?.bytes))
}

/// Attaches an instance `instance` of `definition` to `target` with `values` (a property
/// without a value takes its default). Fails with `doc.propset.duplicate` when the target has
/// an instance of the definition, `doc.propset.invalid` for an unknown property or a target
/// type the definition does not apply to, the value codes (`doc.value.*`) and the codes of
/// [`Txn::create`].
pub fn attach(
    txn: &mut Txn,
    instance: ObjectId,
    target: ObjectId,
    definition_id: ObjectId,
    values: &BTreeMap<String, Value>,
    limits: &Limits,
) -> Result<()> {
    let handle = txn.snapshot().resolve(target)?;
    let def = definition(txn.snapshot(), definition_id, limits)?;
    if !applicable(&def.applicable_types, txn.snapshot().type_key(handle)) {
        return Err(invalid("target type"));
    }
    if instance_of(txn.snapshot(), target, definition_id).is_some() {
        return Err(Error::new(codes::PROPSET_DUPLICATE)
            .with_arg("target", target.to_string())
            .with_arg("definition", definition_id.to_string()));
    }
    let schema = Schema::new(SchemaVersion::new(1, 0), def.properties.clone());
    let positional = assemble(&schema, values)?;
    let stored = instance_values(&def, &positional, limits)?;
    txn.create(
        INSTANCE_TYPE,
        instance,
        Header::default(),
        vec![
            Some(Value::Reference(target)),
            Some(Value::Reference(definition_id)),
            Some(stored),
        ],
    )
    .map(drop)
}

/// Deletes the instance of `definition` on `target`. Fails with `doc.object.not_found` when
/// there is none.
pub fn detach(txn: &mut Txn, target: ObjectId, definition: ObjectId) -> Result<()> {
    let id = instance_of(txn.snapshot(), target, definition)
        .ok_or_else(|| Error::new(codes::OBJECT_NOT_FOUND).with_detail("id", target.to_string()))?;
    txn.delete(id)
}

/// Sets property values of the instance `id`: a `Some` value replaces, `None` removes the value
/// of an optional property. Fails with `doc.propset.invalid` for an unknown property and with
/// the value codes and the codes of [`Txn::modify`].
pub fn set_values(
    txn: &mut Txn,
    id: ObjectId,
    changes: &BTreeMap<String, Option<Value>>,
    limits: &Limits,
) -> Result<()> {
    let raw = raw(txn.snapshot(), id)?;
    let def = definition(txn.snapshot(), raw.definition, limits)?;
    let schema = Schema::new(SchemaVersion::new(1, 0), def.properties.clone());
    let mut values = decode_values(&schema, &raw.bytes, limits)?;
    for (name, value) in changes {
        let slot = schema
            .fields
            .iter()
            .position(|f| f.name == *name)
            .and_then(|i| values.get_mut(i))
            .ok_or_else(|| invalid("unknown property"))?;
        slot.clone_from(value);
    }
    let stored = instance_values(&def, &values, limits)?;
    txn.modify(id, FieldPath::new(VALUES), Some(stored))
}

/// Migrates one instance's values from `old` to `new`: a property keeps its value when it has
/// one (under its old name, see `renames`) that the new property accepts, and otherwise takes
/// the new default.
fn migrate(
    old: &Schema,
    values: &[Option<Value>],
    new: &Schema,
    renames: &[(&str, &str)],
) -> Vec<Option<Value>> {
    new.fields
        .iter()
        .map(|field| {
            let from = renames
                .iter()
                .find(|(_, to)| *to == field.name)
                .map_or(field.name.as_str(), |(from, _)| *from);
            let kept = old
                .fields
                .iter()
                .position(|f| f.name == from)
                .and_then(|i| values.get(i))
                .and_then(Option::as_ref)
                .filter(|v| check_value(&field.value_type, field.bound.as_ref(), v).is_ok());
            kept.cloned().or_else(|| field.default.clone())
        })
        .collect()
}

/// Replaces the applicable types and properties of the definition `id` and migrates every
/// instance in the same transaction. A property keeps each instance's value when the value
/// still fits the new property; `renames` pairs an old property name with the new name that
/// continues it. A value that no longer fits becomes the new default, or absent for an optional
/// property without one.
///
/// Fails with `doc.propset.invalid` (malformed properties, a rename naming no property, or an
/// instance on a target type the new definition does not apply to), with `doc.value.missing`
/// when a required property has neither a kept value nor a default, and with the codes of
/// [`Txn::modify`]. A failure leaves the transaction as it was.
pub fn redefine(
    txn: &mut Txn,
    id: ObjectId,
    applicable_types: &[TypeKey],
    properties: &[FieldDecl],
    renames: &[(&str, &str)],
    limits: &Limits,
) -> Result<()> {
    let old = definition(txn.snapshot(), id, limits)?;
    let text = encode_properties(properties)?;
    let old_schema = Schema::new(SchemaVersion::new(1, 0), old.properties.clone());
    let new_schema = Schema::new(SchemaVersion::new(1, 0), properties.to_vec());
    let known = |schema: &Schema, name: &str| schema.fields.iter().any(|f| f.name == name);
    if renames
        .iter()
        .any(|(from, to)| !known(&old_schema, from) || !known(&new_schema, to))
    {
        return Err(invalid("rename"));
    }
    let new_def = Definition {
        name: old.name.clone(),
        applicable_types: applicable_types.to_vec(),
        properties: properties.to_vec(),
    };
    let mut rewrites = Vec::new();
    for instance_id in instances_using(txn.snapshot(), id) {
        let raw = raw(txn.snapshot(), instance_id)?;
        let target_type = txn
            .snapshot()
            .resolve(raw.target)
            .ok()
            .and_then(|h| txn.snapshot().type_key(h));
        if !applicable(applicable_types, target_type) {
            return Err(invalid("target type"));
        }
        let values = decode_values(&old_schema, &raw.bytes, limits)?;
        let migrated = migrate(&old_schema, &values, &new_schema, renames);
        rewrites.push((instance_id, instance_values(&new_def, &migrated, limits)?));
    }
    let savepoint = txn.savepoint()?;
    let applied = (|| {
        txn.modify(
            id,
            FieldPath::new(APPLICABLE),
            Some(names(applicable_types)),
        )?;
        txn.modify(id, FieldPath::new(PROPERTIES), Some(Value::String(text)))?;
        for (instance_id, stored) in rewrites {
            txn.modify(instance_id, FieldPath::new(VALUES), Some(stored))?;
        }
        Ok(())
    })();
    if applied.is_err() {
        txn.rollback_to(&savepoint)?;
    }
    txn.release(&savepoint)?;
    applied
}

/// A document validator that checks, at commit, every touched definition (its properties
/// parse; every instance of it decodes under them) and every touched instance (it decodes under
/// its definition, the definition applies to its target and it is the only instance of that
/// definition on the target). This catches a definition or instance written by
/// [`Txn::modify`] directly, which migrates nothing.
#[must_use]
pub fn validator(limits: Limits) -> DocumentValidator {
    std::sync::Arc::new(move |_base, candidate, touched| {
        let mut findings = Vec::new();
        for (&id, &change) in touched {
            if change == Change::Deleted {
                continue;
            }
            let Ok(handle) = candidate.resolve(id) else {
                continue;
            };
            let checked = match candidate.type_key(handle).map(TypeKey::as_str) {
                Some(DEFINITION_TYPE) => check_definition(candidate, id, &limits),
                Some(INSTANCE_TYPE) => check_instance(candidate, id, &limits),
                _ => Ok(()),
            };
            if let Err(error) = checked {
                findings.push(Finding {
                    severity: Severity::Error,
                    error: error.with_detail("object", id.to_string()),
                });
            }
        }
        findings
    })
}

fn check_definition(snapshot: &Snapshot, id: ObjectId, limits: &Limits) -> Result<()> {
    definition(snapshot, id, limits)?;
    instances_using(snapshot, id)
        .into_iter()
        .try_for_each(|instance_id| check_instance(snapshot, instance_id, limits))
}

fn check_instance(snapshot: &Snapshot, id: ObjectId, limits: &Limits) -> Result<()> {
    let raw = raw(snapshot, id)?;
    let def = definition(snapshot, raw.definition, limits)?;
    let target = snapshot.resolve(raw.target)?;
    if !applicable(&def.applicable_types, snapshot.type_key(target)) {
        return Err(invalid("target type"));
    }
    let schema = Schema::new(SchemaVersion::new(1, 0), def.properties);
    let values = decode_values(&schema, &raw.bytes, limits)?;
    crate::schema::check_values(&schema.fields, &values)?;
    let same = instances_of(snapshot, raw.target)
        .into_iter()
        .filter(|other| raw_definition(snapshot, *other) == Some(raw.definition))
        .count();
    if same > 1 {
        return Err(Error::new(codes::PROPSET_DUPLICATE)
            .with_arg("target", raw.target.to_string())
            .with_arg("definition", raw.definition.to_string()));
    }
    Ok(())
}

fn raw_definition(snapshot: &Snapshot, id: ObjectId) -> Option<ObjectId> {
    raw(snapshot, id).ok().map(|r| r.definition)
}
