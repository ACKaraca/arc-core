//! The type registry (CORE-DOC-001).
//!
//! A [`TypeRegistry`] is an explicit value: the application builds one at start, registers every
//! type of the core and of the product, and then shares it immutably (in an `Arc`) with every
//! document it opens. Nothing is global, so tests and tools may hold several registries in one
//! process.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::{Error, Result};

use crate::error::codes;
use crate::schema::{
    Bound, Cardinality, FieldDecl, OnCopy, OnDelete, RelationshipRule, Schema, TypeDecl, ValueType,
    check_value, is_key, is_name, prefix_field,
};

/// The deepest nesting of lists, maps and records inside one field type.
pub const MAX_TYPE_DEPTH: u32 = 16;

/// The registered object types, ordered by type key.
#[derive(Debug, Clone, Default)]
pub struct TypeRegistry {
    types: BTreeMap<String, TypeDecl>,
}

impl TypeRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates `decl` as a whole and registers it. On failure the registry is unchanged.
    ///
    /// Fails with `doc.type.duplicate_key` when the key is registered; otherwise with the code
    /// of the first defect found: `doc.type.version_invalid`, `doc.type.duplicate_field`,
    /// `doc.type.field_invalid`, `doc.type.malformed_key` (a geometry encoding name),
    /// `doc.type.relationship_inconsistent`, `doc.type.default_invalid` or
    /// `doc.type.migration_chain_invalid`. Every error names the type in its `type` argument.
    pub fn register(&mut self, decl: TypeDecl) -> Result<()> {
        let key = decl.key.as_str().to_owned();
        if self.types.contains_key(&key) {
            return Err(Error::new(codes::TYPE_DUPLICATE_KEY).with_arg("key", key));
        }
        validate(&decl).map_err(|error| error.with_arg("type", key.clone()))?;
        self.types.insert(key, decl);
        Ok(())
    }

    /// The declaration registered under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&TypeDecl> {
        self.types.get(key)
    }

    /// Every declaration, ordered by type key.
    pub fn iter(&self) -> impl Iterator<Item = &TypeDecl> {
        self.types.values()
    }

    /// The number of registered types.
    #[must_use]
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// True when no type is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }
}

fn validate(decl: &TypeDecl) -> Result<()> {
    check_schema(&decl.schema)?;
    for migration in &decl.migrations {
        check_schema(&migration.source)?;
    }
    let sources: Vec<u32> = decl
        .migrations
        .iter()
        .map(|migration| migration.source.version.major)
        .collect();
    let mut sorted = sources.clone();
    sorted.sort_unstable();
    if !sorted.iter().copied().eq(1..decl.schema.version.major) {
        return Err(Error::new(codes::TYPE_MIGRATION_CHAIN_INVALID)
            .with_detail("current_major", i64::from(decl.schema.version.major))
            .with_detail("sources", format!("{sources:?}")));
    }
    Ok(())
}

fn invalid(code: arc_base::ErrorCode, reason: &'static str) -> Error {
    Error::new(code).with_detail("reason", reason)
}

fn check_schema(schema: &Schema) -> Result<()> {
    let version = schema.version;
    if version.major == 0 {
        return Err(invalid(codes::TYPE_VERSION_INVALID, "major version 0"));
    }
    check_fields(&schema.fields, 0)?;
    let mut previous = 0;
    for field in &schema.fields {
        let checked = if field.since_minor > version.minor {
            Err(invalid(
                codes::TYPE_VERSION_INVALID,
                "appended after the current minor version",
            ))
        } else if field.since_minor < previous {
            Err(invalid(
                codes::TYPE_FIELD_INVALID,
                "appended fields must follow older ones",
            ))
        } else if field.since_minor > 0 && !field.optional && field.default.is_none() {
            Err(invalid(
                codes::TYPE_FIELD_INVALID,
                "an appended field must be optional or have a default",
            ))
        } else {
            Ok(())
        };
        checked.map_err(|error| prefix_field(error, &field.name))?;
        previous = field.since_minor;
    }
    Ok(())
}

/// Checks a field list at nesting `depth` (0 for the fields of a schema).
fn check_fields(fields: &[FieldDecl], depth: u32) -> Result<()> {
    let mut names = BTreeSet::new();
    for field in fields {
        if !names.insert(field.name.as_str()) {
            return Err(
                Error::new(codes::TYPE_DUPLICATE_FIELD).with_arg("field", field.name.clone())
            );
        }
        check_field(field, depth).map_err(|error| prefix_field(error, &field.name))?;
    }
    Ok(())
}

fn check_field(field: &FieldDecl, depth: u32) -> Result<()> {
    if !is_name(&field.name) {
        return Err(invalid(codes::TYPE_FIELD_INVALID, "malformed field name"));
    }
    if !is_name(&field.aspect) {
        return Err(invalid(codes::TYPE_FIELD_INVALID, "malformed aspect name"));
    }
    if depth > 0 && field.since_minor != 0 {
        return Err(invalid(
            codes::TYPE_FIELD_INVALID,
            "fields of nested records cannot be appended",
        ));
    }
    let place = Place::Field {
        optional: field.optional,
    };
    check_type(&field.value_type, place, depth)?;
    if let Some(bound) = &field.bound {
        check_bound(&field.value_type, bound)?;
    }
    if let Some(default) = &field.default {
        check_value(&field.value_type, field.bound.as_ref(), default)
            .map_err(|cause| Error::new(codes::TYPE_DEFAULT_INVALID).with_source(cause))?;
    }
    Ok(())
}

/// Where a value type occurs, which decides the cardinality a reference there may have.
#[derive(Clone, Copy)]
enum Place {
    /// The type of a field.
    Field { optional: bool },
    /// The element type of a list that is the type of a field.
    ListElement,
    /// Anywhere else.
    Nested,
}

fn check_type(value_type: &ValueType, place: Place, depth: u32) -> Result<()> {
    if depth > MAX_TYPE_DEPTH {
        return Err(invalid(codes::TYPE_FIELD_INVALID, "nested too deeply"));
    }
    match value_type {
        ValueType::Enum(variants) => {
            let mut names = BTreeSet::new();
            let mut values = BTreeSet::new();
            let valid = !variants.is_empty()
                && variants.iter().all(|variant| {
                    is_name(&variant.name)
                        && names.insert(variant.name.as_str())
                        && values.insert(variant.code)
                });
            if !valid {
                return Err(invalid(
                    codes::TYPE_FIELD_INVALID,
                    "enum variants must be non-empty with unique well-formed names and codes",
                ));
            }
        }
        ValueType::Geometry(encoding) if !is_key(&encoding.name) => {
            return Err(
                Error::new(codes::TYPE_MALFORMED_KEY).with_arg("key", encoding.name.clone())
            );
        }
        ValueType::Reference(spec) => check_rule(&spec.rule, place)?,
        ValueType::List(element) => {
            let inner = match place {
                Place::Field { .. } => Place::ListElement,
                Place::ListElement | Place::Nested => Place::Nested,
            };
            check_type(element, inner, depth + 1)?;
        }
        ValueType::Map(element) => check_type(element, Place::Nested, depth + 1)?,
        ValueType::Record(fields) => {
            if fields.is_empty() {
                return Err(invalid(codes::TYPE_FIELD_INVALID, "a record needs a field"));
            }
            check_fields(fields, depth + 1)?;
        }
        _ => {}
    }
    Ok(())
}

fn check_rule(rule: &RelationshipRule, place: Place) -> Result<()> {
    let inconsistent = |reason| Err(invalid(codes::TYPE_RELATIONSHIP_INCONSISTENT, reason));
    match (rule.cardinality, place) {
        (Cardinality::One, Place::Field { optional: false })
        | (Cardinality::Optional, Place::Field { optional: true }) => {}
        (Cardinality::Many { max }, Place::ListElement) if max > 0 => {}
        (Cardinality::Many { .. }, Place::ListElement) => {
            return inconsistent("cardinality many needs a bound of at least 1");
        }
        (Cardinality::One, _) => return inconsistent("cardinality one needs a required field"),
        (Cardinality::Optional, _) => {
            return inconsistent("cardinality optional needs an optional field");
        }
        (Cardinality::Many { .. }, _) => {
            return inconsistent("cardinality many needs a field whose type is a list");
        }
    }
    let clears = rule.on_delete == OnDelete::Clear || rule.on_copy == OnCopy::Clear;
    if clears && rule.cardinality == Cardinality::One {
        return inconsistent("clear needs cardinality optional or many");
    }
    Ok(())
}

fn check_bound(value_type: &ValueType, bound: &Bound) -> Result<()> {
    let valid = match (bound, value_type) {
        (Bound::Int { min, max }, ValueType::I64) => min <= max,
        (Bound::Real { min, max }, ValueType::F64 | ValueType::Quantity(_)) => {
            min.is_finite() && max.is_finite() && min <= max
        }
        (
            Bound::Length { .. },
            ValueType::String | ValueType::List(_) | ValueType::Map(_) | ValueType::Opaque,
        ) => true,
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            codes::TYPE_FIELD_INVALID,
            "the bound does not fit the value type or is empty",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        EnumVariant, GeometryEncoding, Migration, ReferenceSpec, SchemaVersion, Strength, TypeKey,
        Value,
    };

    fn key(text: &str) -> TypeKey {
        TypeKey::new(text).unwrap()
    }

    fn decl(fields: Vec<FieldDecl>) -> TypeDecl {
        TypeDecl::new(
            key("test.thing"),
            Schema::new(SchemaVersion::new(1, 0), fields),
        )
    }

    fn rule(cardinality: Cardinality, on_delete: OnDelete) -> RelationshipRule {
        RelationshipRule {
            strength: Strength::Associative,
            on_delete,
            on_copy: OnCopy::ShareTarget,
            cardinality,
        }
    }

    fn reference(cardinality: Cardinality, on_delete: OnDelete) -> ValueType {
        ValueType::Reference(ReferenceSpec {
            targets: vec![key("test.target")],
            rule: rule(cardinality, on_delete),
        })
    }

    fn migrate(values: Vec<Option<Value>>) -> Result<Vec<Option<Value>>> {
        Ok(values)
    }

    fn migration(major: u32) -> Migration {
        Migration {
            source: Schema::new(SchemaVersion::new(major, 0), Vec::new()),
            migrate,
        }
    }

    /// Registers `decl` into a registry that holds one other type and checks that a failure
    /// leaves it unchanged; returns the error code or "ok".
    fn outcome(decl: TypeDecl) -> &'static str {
        let mut registry = TypeRegistry::new();
        registry
            .register(TypeDecl::new(
                key("test.other"),
                Schema::new(SchemaVersion::new(1, 0), Vec::new()),
            ))
            .unwrap();
        match registry.register(decl) {
            Ok(()) => "ok",
            Err(error) => {
                assert_eq!(
                    registry.len(),
                    1,
                    "a failed registration changed the registry"
                );
                assert!(registry.get("test.thing").is_none());
                error.code()
            }
        }
    }

    #[test]
    fn registered_types_are_found_and_iterate_by_key() {
        let mut registry = TypeRegistry::new();
        assert!(registry.is_empty());
        for name in ["test.zeta", "test.alpha", "other.mid"] {
            registry
                .register(TypeDecl::new(
                    key(name),
                    Schema::new(SchemaVersion::new(1, 0), Vec::new()),
                ))
                .unwrap();
        }
        let keys: Vec<&str> = registry.iter().map(|decl| decl.key.as_str()).collect();
        assert_eq!(keys, ["other.mid", "test.alpha", "test.zeta"]);
        assert!(registry.get("test.alpha").is_some());
        assert!(registry.get("test.beta").is_none());
    }

    #[test]
    fn a_duplicate_key_fails() {
        let mut registry = TypeRegistry::new();
        registry.register(decl(Vec::new())).unwrap();
        let fields = vec![FieldDecl::new("a", ValueType::Bool)];
        let error = registry.register(decl(fields)).unwrap_err();
        assert_eq!(error.code(), "doc.type.duplicate_key");
        assert!(registry.get("test.thing").unwrap().schema.fields.is_empty());
    }

    #[test]
    fn malformed_keys_fail() {
        for text in [
            "",
            "test",
            "Test.thing",
            "test..thing",
            "test.1thing",
            "test.thing-x",
        ] {
            assert_eq!(
                TypeKey::new(text).unwrap_err().code(),
                "doc.type.malformed_key",
                "{text}"
            );
        }
        let long = format!("test.{}", "x".repeat(crate::schema::MAX_KEY_BYTES));
        assert!(TypeKey::new(long).is_err());
        assert!(TypeKey::new("core.doc.link").is_ok());
        assert!(serde_json::from_str::<TypeKey>("\"Bad\"").is_err());
        let encoding = ValueType::Geometry(GeometryEncoding {
            name: "curve".to_owned(),
            version: 1,
        });
        assert_eq!(
            outcome(decl(vec![FieldDecl::new("g", encoding)])),
            "doc.type.malformed_key"
        );
    }

    #[test]
    fn major_version_zero_and_fields_beyond_the_current_minor_fail() {
        let mut zero = decl(Vec::new());
        zero.schema.version = SchemaVersion::new(0, 1);
        assert_eq!(outcome(zero), "doc.type.version_invalid");
        let ahead = FieldDecl::new("a", ValueType::Bool)
            .optional()
            .since_minor(1);
        assert_eq!(outcome(decl(vec![ahead])), "doc.type.version_invalid");
    }

    #[test]
    fn duplicate_field_names_fail_at_every_level() {
        let twice = vec![
            FieldDecl::new("a", ValueType::Bool),
            FieldDecl::new("a", ValueType::I64),
        ];
        assert_eq!(outcome(decl(twice.clone())), "doc.type.duplicate_field");
        let nested = FieldDecl::new("r", ValueType::Record(twice));
        let error = TypeRegistry::new()
            .register(decl(vec![nested]))
            .unwrap_err();
        assert_eq!(error.code(), "doc.type.duplicate_field");
        assert_eq!(
            error.arguments().get("field"),
            Some(&arc_base::error::Value::Text("r.a".to_owned()))
        );
    }

    #[test]
    fn malformed_field_declarations_fail() {
        let invalid = [
            FieldDecl::new("Bad", ValueType::Bool),
            FieldDecl::new("a", ValueType::Bool).in_aspect("Geometry"),
            FieldDecl::new("a", ValueType::Enum(Vec::new())),
            FieldDecl::new(
                "a",
                ValueType::Enum(vec![EnumVariant::new("x", 1), EnumVariant::new("y", 1)]),
            ),
            FieldDecl::new(
                "a",
                ValueType::Enum(vec![EnumVariant::new("x", 1), EnumVariant::new("x", 2)]),
            ),
            FieldDecl::new("a", ValueType::Record(Vec::new())),
            FieldDecl::new("a", ValueType::Bool).with_bound(Bound::Length { max: 1 }),
            FieldDecl::new("a", ValueType::I64).with_bound(Bound::Int { min: 2, max: 1 }),
            FieldDecl::new("a", ValueType::F64).with_bound(Bound::Real {
                min: 0.0,
                max: f64::INFINITY,
            }),
            FieldDecl::new("a", ValueType::I64).with_bound(Bound::Real { min: 0.0, max: 1.0 }),
            FieldDecl::new(
                "a",
                ValueType::Record(vec![FieldDecl::new("b", ValueType::Bool).since_minor(1)]),
            ),
        ];
        for field in invalid {
            assert_eq!(
                outcome(decl(vec![field.clone()])),
                "doc.type.field_invalid",
                "{field:?}"
            );
        }
        let mut deep = ValueType::Bool;
        for _ in 0..=MAX_TYPE_DEPTH {
            deep = ValueType::List(Box::new(deep));
        }
        assert_eq!(
            outcome(decl(vec![FieldDecl::new("a", deep)])),
            "doc.type.field_invalid"
        );
    }

    #[test]
    fn appended_fields_must_be_optional_or_defaulted_and_in_order() {
        let base = || FieldDecl::new("a", ValueType::Bool);
        let mut schema = decl(vec![
            base(),
            FieldDecl::new("b", ValueType::I64).since_minor(1),
        ]);
        schema.schema.version = SchemaVersion::new(1, 1);
        assert_eq!(outcome(schema.clone()), "doc.type.field_invalid");
        schema.schema.fields[1].default = Some(Value::I64(0));
        assert_eq!(outcome(schema.clone()), "ok");
        schema
            .schema
            .fields
            .push(FieldDecl::new("c", ValueType::I64).optional());
        assert_eq!(outcome(schema), "doc.type.field_invalid");
    }

    #[test]
    fn inconsistent_relationship_rules_fail() {
        let list = |t| ValueType::List(Box::new(t));
        let many = Cardinality::Many { max: 4 };
        let consistent = [
            FieldDecl::new("a", reference(Cardinality::One, OnDelete::Cascade)),
            FieldDecl::new("a", reference(Cardinality::Optional, OnDelete::Clear)).optional(),
            FieldDecl::new("a", list(reference(many, OnDelete::Clear))),
            FieldDecl::new(
                "a",
                list(ValueType::Record(vec![FieldDecl::new(
                    "b",
                    reference(Cardinality::One, OnDelete::Block),
                )])),
            ),
        ];
        for field in consistent {
            assert_eq!(outcome(decl(vec![field.clone()])), "ok", "{field:?}");
        }
        let inconsistent = [
            FieldDecl::new("a", reference(Cardinality::One, OnDelete::Clear)),
            FieldDecl::new("a", reference(Cardinality::One, OnDelete::Block)).optional(),
            FieldDecl::new("a", reference(Cardinality::Optional, OnDelete::Clear)),
            FieldDecl::new("a", reference(many, OnDelete::Clear)),
            FieldDecl::new("a", list(reference(Cardinality::One, OnDelete::Block))),
            FieldDecl::new(
                "a",
                list(reference(Cardinality::Many { max: 0 }, OnDelete::Block)),
            ),
            FieldDecl::new("a", list(list(reference(many, OnDelete::Block)))),
            FieldDecl::new(
                "a",
                ValueType::Map(Box::new(reference(many, OnDelete::Block))),
            ),
        ];
        for field in inconsistent {
            assert_eq!(
                outcome(decl(vec![field.clone()])),
                "doc.type.relationship_inconsistent",
                "{field:?}"
            );
        }
        let mut clear_on_copy = rule(Cardinality::One, OnDelete::Block);
        clear_on_copy.on_copy = OnCopy::Clear;
        let field = FieldDecl::new(
            "a",
            ValueType::Reference(ReferenceSpec {
                targets: Vec::new(),
                rule: clear_on_copy,
            }),
        );
        assert_eq!(
            outcome(decl(vec![field])),
            "doc.type.relationship_inconsistent"
        );
    }

    #[test]
    fn defaults_must_match_type_and_bound() {
        let bounded = FieldDecl::new("a", ValueType::I64).with_bound(Bound::Int { min: 0, max: 5 });
        assert_eq!(
            outcome(decl(vec![bounded.clone().with_default(Value::I64(5))])),
            "ok"
        );
        assert_eq!(
            outcome(decl(vec![bounded.clone().with_default(Value::I64(6))])),
            "doc.type.default_invalid"
        );
        assert_eq!(
            outcome(decl(vec![bounded.with_default(Value::Bool(true))])),
            "doc.type.default_invalid"
        );
        let colors = ValueType::Enum(vec![EnumVariant::new("red", 3)]);
        let unknown = FieldDecl::new("c", colors).with_default(Value::Enum(4));
        assert_eq!(outcome(decl(vec![unknown])), "doc.type.default_invalid");
    }

    #[test]
    fn migrations_must_chain_from_major_one() {
        let at = |major: u32, migrations: Vec<Migration>| {
            let mut decl = decl(Vec::new());
            decl.schema.version = SchemaVersion::new(major, 0);
            decl.migrations = migrations;
            outcome(decl)
        };
        assert_eq!(at(1, Vec::new()), "ok");
        assert_eq!(at(3, vec![migration(2), migration(1)]), "ok");
        assert_eq!(at(2, Vec::new()), "doc.type.migration_chain_invalid");
        assert_eq!(
            at(3, vec![migration(1)]),
            "doc.type.migration_chain_invalid"
        );
        assert_eq!(
            at(3, vec![migration(1), migration(1)]),
            "doc.type.migration_chain_invalid"
        );
        assert_eq!(
            at(2, vec![migration(1), migration(2)]),
            "doc.type.migration_chain_invalid"
        );
        assert_eq!(
            at(1, vec![migration(1)]),
            "doc.type.migration_chain_invalid"
        );
    }

    #[test]
    fn the_declaration_lists_every_version_and_schema() {
        let mut decl = decl(Vec::new());
        decl.schema.version = SchemaVersion::new(2, 1);
        decl.migrations = vec![Migration {
            source: Schema::new(SchemaVersion::new(1, 2), Vec::new()),
            migrate,
        }];
        let versions: Vec<String> = decl.versions().iter().map(ToString::to_string).collect();
        assert_eq!(versions, ["1.0", "1.1", "1.2", "2.0", "2.1"]);
        assert_eq!(decl.schema(1).map(|s| s.version.minor), Some(2));
        assert!(decl.schema(3).is_none());
    }
}
