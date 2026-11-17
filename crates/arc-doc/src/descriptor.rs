//! JSON field descriptors (CORE-DOC-004).
//!
//! A descriptor describes one version of one type completely enough for a generic decoder to
//! turn a payload of that version into JSON. It is generated from the declaration alone, with a
//! fixed key order and the shortest round-trip form of every number, so the same declaration
//! yields the same bytes on every run and platform, whatever else the registry holds and in
//! whatever order types were registered. The grammar is specified in `docs/crates/arc-doc.md`.

use serde::Serialize;

use crate::schema::{
    Bound, Cardinality, FieldDecl, OnCopy, OnDelete, SchemaVersion, Strength, TypeDecl, Value,
    ValueType,
};

/// The version of the descriptor grammar.
pub const DESCRIPTOR_FORMAT: u32 = 1;

/// The descriptor of `decl` at `version`, as pretty-printed JSON with two-space indentation and
/// no trailing newline, or `None` when the declaration does not describe that version.
#[must_use]
pub fn descriptor(decl: &TypeDecl, version: SchemaVersion) -> Option<String> {
    let schema = decl.schema(version.major)?;
    if version.minor > schema.version.minor {
        return None;
    }
    let document = Descriptor {
        descriptor_format: DESCRIPTOR_FORMAT,
        type_key: decl.key.as_str(),
        major: version.major,
        minor: version.minor,
        fields: schema.fields_at(version.minor).iter().map(field).collect(),
    };
    // Serialization cannot fail: every map key is text and every value is plain data.
    serde_json::to_string_pretty(&document).ok()
}

#[derive(Serialize)]
struct Descriptor<'a> {
    descriptor_format: u32,
    type_key: &'a str,
    major: u32,
    minor: u32,
    fields: Vec<Field<'a>>,
}

#[derive(Serialize)]
struct Field<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    value_type: Type<'a>,
    optional: bool,
    indexed: bool,
    aspect: &'a str,
    default: Option<&'a Value>,
    bound: Option<BoundJson>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Type<'a> {
    Bool,
    I64,
    F64,
    String,
    Quantity {
        dimension: [i8; 9],
    },
    Enum {
        variants: Vec<Variant<'a>>,
    },
    Point2,
    Point3,
    Vector3,
    Transform,
    Color,
    Reference {
        targets: Vec<&'a str>,
        rule: Rule,
    },
    ExternalReference,
    Asset,
    Geometry {
        encoding: &'a str,
        encoding_version: u32,
    },
    List {
        element: Box<Type<'a>>,
    },
    Map {
        value: Box<Type<'a>>,
    },
    Record {
        fields: Vec<Field<'a>>,
    },
    Opaque,
}

#[derive(Serialize)]
struct Variant<'a> {
    name: &'a str,
    code: u32,
}

#[derive(Serialize)]
struct Rule {
    strength: Strength,
    on_delete: OnDelete,
    on_copy: OnCopy,
    cardinality: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<u32>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BoundJson {
    Int { min: i64, max: i64 },
    Real { min: f64, max: f64 },
    Length { max: u64 },
}

fn field(decl: &FieldDecl) -> Field<'_> {
    Field {
        name: &decl.name,
        value_type: value_type(&decl.value_type),
        optional: decl.optional,
        indexed: decl.indexed,
        aspect: &decl.aspect,
        default: decl.default.as_ref(),
        bound: decl.bound.map(|bound| match bound {
            Bound::Int { min, max } => BoundJson::Int { min, max },
            Bound::Real { min, max } => BoundJson::Real { min, max },
            Bound::Length { max } => BoundJson::Length { max },
        }),
    }
}

fn value_type(value_type: &ValueType) -> Type<'_> {
    match value_type {
        ValueType::Bool => Type::Bool,
        ValueType::I64 => Type::I64,
        ValueType::F64 => Type::F64,
        ValueType::String => Type::String,
        ValueType::Quantity(dimension) => Type::Quantity {
            dimension: dimension.exponents(),
        },
        ValueType::Enum(variants) => Type::Enum {
            variants: variants
                .iter()
                .map(|variant| Variant {
                    name: &variant.name,
                    code: variant.code,
                })
                .collect(),
        },
        ValueType::Point2 => Type::Point2,
        ValueType::Point3 => Type::Point3,
        ValueType::Vector3 => Type::Vector3,
        ValueType::Transform => Type::Transform,
        ValueType::Color => Type::Color,
        ValueType::Reference(spec) => {
            let (cardinality, max) = match spec.rule.cardinality {
                Cardinality::One => ("one", None),
                Cardinality::Optional => ("optional", None),
                Cardinality::Many { max } => ("many", Some(max)),
            };
            Type::Reference {
                targets: spec.targets.iter().map(|key| key.as_str()).collect(),
                rule: Rule {
                    strength: spec.rule.strength,
                    on_delete: spec.rule.on_delete,
                    on_copy: spec.rule.on_copy,
                    cardinality,
                    max,
                },
            }
        }
        ValueType::ExternalReference => Type::ExternalReference,
        ValueType::Asset => Type::Asset,
        ValueType::Geometry(encoding) => Type::Geometry {
            encoding: &encoding.name,
            encoding_version: encoding.version,
        },
        ValueType::List(element) => Type::List {
            element: Box::new(self::value_type(element)),
        },
        ValueType::Map(element) => Type::Map {
            value: Box::new(self::value_type(element)),
        },
        ValueType::Record(fields) => Type::Record {
            fields: fields.iter().map(field).collect(),
        },
        ValueType::Opaque => Type::Opaque,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::registry::TypeRegistry;
    use crate::schema::{FieldDecl, Schema, TypeKey};
    use crate::testing::{fields, values};

    /// Field lists with every present value also set as the field's default.
    fn defaulted_fields() -> impl Strategy<Value = Vec<FieldDecl>> {
        fields().prop_flat_map(|fields| {
            let values = values(&fields);
            (Just(fields), values).prop_map(|(mut fields, values)| {
                for (field, value) in fields.iter_mut().zip(values) {
                    field.default = value;
                }
                fields
            })
        })
    }

    fn all_descriptors(registry: &TypeRegistry) -> Vec<String> {
        registry
            .iter()
            .flat_map(|decl| decl.versions().into_iter().map(move |v| (decl, v)))
            .filter_map(|(decl, version)| descriptor(decl, version))
            .collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        #[test]
        fn registration_order_does_not_change_descriptors(
            types in prop::collection::vec(defaulted_fields(), 1..5)
                .prop_flat_map(|types| {
                    let order: Vec<usize> = (0..types.len()).collect();
                    (Just(types), Just(order).prop_shuffle())
                }),
        ) {
            let (types, order) = types;
            let decls: Vec<TypeDecl> = types
                .into_iter()
                .enumerate()
                .map(|(i, fields)| {
                    let key = TypeKey::new(format!("test.t{i}")).unwrap();
                    TypeDecl::new(key, Schema::new(SchemaVersion::new(1, 0), fields))
                })
                .collect();
            let mut forward = TypeRegistry::new();
            for decl in &decls {
                forward.register(decl.clone()).unwrap();
            }
            let mut shuffled = TypeRegistry::new();
            for &i in &order {
                shuffled.register(decls[i].clone()).unwrap();
            }
            let expected = all_descriptors(&forward);
            prop_assert_eq!(expected.len(), decls.len());
            prop_assert_eq!(&all_descriptors(&shuffled), &expected);
            for text in &expected {
                prop_assert!(serde_json::from_str::<serde_json::Value>(text).is_ok());
            }
        }
    }

    #[test]
    fn versions_outside_the_declaration_have_no_descriptor() {
        let key = TypeKey::new("test.thing").unwrap();
        let decl = TypeDecl::new(key, Schema::new(SchemaVersion::new(1, 0), Vec::new()));
        assert!(descriptor(&decl, SchemaVersion::new(1, 0)).is_some());
        assert!(descriptor(&decl, SchemaVersion::new(1, 1)).is_none());
        assert!(descriptor(&decl, SchemaVersion::new(2, 0)).is_none());
    }
}
