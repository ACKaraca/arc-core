//! JSON field descriptors (CORE-DOC-004).
//!
//! A descriptor describes one version of one type completely enough for a generic decoder to
//! turn a payload of that version into JSON. It is generated from the declaration alone, with a
//! fixed key order and the shortest round-trip form of every number, so the same declaration
//! yields the same bytes on every run and platform, whatever else the registry holds and in
//! whatever order types were registered. The grammar is specified in `docs/crates/arc-doc.md`
//! and published as the JSON Schema `schemas/package-descriptor.schema.json`.
//!
//! [`Schema::from_descriptor`] parses a stored descriptor back into a schema that decodes the
//! payloads of the version it describes, treating the text as hostile.

use std::collections::BTreeMap;
use std::str::FromStr;

use arc_base::{Color, Dimension, Error, Limits, ObjectId, Quantity, Result};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::registry::{MAX_TYPE_DEPTH, check_schema};
use crate::schema::{
    AssetHash, Bound, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    GeometryBytes, GeometryEncoding, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Schema,
    SchemaVersion, Strength, TypeDecl, TypeKey, Value, ValueType, transform_from_entries,
};

/// The version of the descriptor grammar.
pub const DESCRIPTOR_FORMAT: u32 = 1;

/// The descriptor of `decl` at `version`, as pretty-printed JSON with two-space indentation and
/// no trailing newline, or `None` when the declaration does not describe that version or would
/// not register (see [`crate::TypeRegistry::register`]), for example because a default is not
/// finite.
#[must_use]
pub fn descriptor(decl: &TypeDecl, version: SchemaVersion) -> Option<String> {
    crate::registry::validate(decl).ok()?;
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

impl Schema {
    /// Parses a descriptor stored in a package (an `arc_type.descriptor`) and returns its type
    /// key and the schema of the version it describes: version `major.minor`, the fields it
    /// lists in order, every field with `since_minor` 0. The schema decodes, with
    /// [`crate::payload::decode`] and `written_minor` equal to its minor version, exactly the
    /// payloads that the declaration the descriptor was generated from decodes at that version;
    /// it does not know at which minor version a field was appended, so it decodes no other
    /// minor version.
    ///
    /// The text is hostile: it must be at most `input_bytes` long, nest JSON at most
    /// [`MAX_DESCRIPTOR_NESTING`] deep, follow the grammar exactly (no unknown or missing key,
    /// `descriptor_format` [`DESCRIPTOR_FORMAT`]), describe a schema the type registry accepts
    /// (as [`crate::TypeRegistry::register`] checks it, without the migration chain) and have
    /// defaults that encode within `limits`. Any failure is `doc.descriptor.invalid` (detail
    /// `reason`, the underlying error as its source); nothing panics.
    pub fn from_descriptor(text: &str, limits: &Limits) -> Result<(TypeKey, Self)> {
        let invalid = |reason: &'static str| {
            Error::new(codes::DESCRIPTOR_INVALID).with_detail("reason", reason)
        };
        if u64::try_from(text.len()).unwrap_or(u64::MAX) > limits.input_bytes {
            return Err(invalid("input_bytes"));
        }
        if nesting(text) > MAX_DESCRIPTOR_NESTING {
            return Err(invalid("nesting"));
        }
        let parsed: InDescriptor =
            serde_json::from_str(text).map_err(|e| invalid("grammar").with_source(e))?;
        if parsed.descriptor_format != DESCRIPTOR_FORMAT {
            return Err(invalid("descriptor_format"));
        }
        let key = TypeKey::new(parsed.type_key).map_err(|e| invalid("type_key").with_source(e))?;
        let fields = in_fields(parsed.fields, 0, limits)?;
        let schema = Self::new(SchemaVersion::new(parsed.major, parsed.minor), fields);
        check_schema(&schema).map_err(|e| invalid("schema").with_source(e))?;
        Ok((key, schema))
    }
}

/// The deepest JSON nesting (objects and arrays) [`Schema::from_descriptor`] accepts: enough
/// for types nested [`MAX_TYPE_DEPTH`] deep, whose every level adds a type object and a key
/// inside it, with their defaults.
pub const MAX_DESCRIPTOR_NESTING: u32 = 4 * MAX_TYPE_DEPTH + 8;

/// The deepest nesting of JSON objects and arrays in `text`, counted without parsing, so that
/// the recursive parse that follows is bounded.
fn nesting(text: &str) -> u32 {
    let (mut depth, mut deepest, mut in_string, mut escaped) = (0_u32, 0_u32, false, false);
    for byte in text.bytes() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                deepest = deepest.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InDescriptor {
    descriptor_format: u32,
    type_key: String,
    major: u32,
    minor: u32,
    fields: Vec<InField>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InField {
    name: String,
    #[serde(rename = "type")]
    value_type: InType,
    optional: bool,
    indexed: bool,
    aspect: String,
    default: serde_json::Value,
    bound: Option<InBound>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InType {
    Bool,
    I64,
    F64,
    String,
    Quantity {
        dimension: [i8; 9],
    },
    Enum {
        variants: Vec<InVariant>,
    },
    Point2,
    Point3,
    Vector3,
    Transform,
    Color,
    Reference {
        targets: Vec<String>,
        rule: InRule,
    },
    ExternalReference,
    Asset,
    Geometry {
        encoding: String,
        encoding_version: u32,
    },
    List {
        element: Box<InType>,
    },
    Map {
        value: Box<InType>,
    },
    Record {
        fields: Vec<InField>,
    },
    Opaque,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InVariant {
    name: String,
    code: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InRule {
    strength: Strength,
    on_delete: OnDelete,
    on_copy: OnCopy,
    cardinality: String,
    max: Option<u32>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InBound {
    Int { min: i64, max: i64 },
    Real { min: f64, max: f64 },
    Length { max: u64 },
}

fn invalid_at(reason: &'static str, field: &str) -> Error {
    Error::new(codes::DESCRIPTOR_INVALID)
        .with_detail("reason", reason)
        .with_detail("field", field.to_owned())
}

/// The field declarations of a parsed field list at type nesting `depth`.
fn in_fields(fields: Vec<InField>, depth: u32, limits: &Limits) -> Result<Vec<FieldDecl>> {
    if u64::try_from(fields.len()).unwrap_or(u64::MAX) > limits.array_length {
        return Err(invalid_at("array_length", ""));
    }
    fields
        .into_iter()
        .map(|field| in_field(field, depth, limits))
        .collect()
}

fn in_field(field: InField, depth: u32, limits: &Limits) -> Result<FieldDecl> {
    let value_type = in_type(field.value_type, depth, limits)
        .map_err(|e| e.with_detail("field", field.name.clone()))?;
    let default = match &field.default {
        serde_json::Value::Null => None,
        json => {
            let value = value_of(&value_type, json, depth)
                .ok_or_else(|| invalid_at("default", &field.name))?;
            crate::payload::encode_value(&value_type, &value, limits)
                .map_err(|e| invalid_at("default", &field.name).with_source(e))?;
            Some(value)
        }
    };
    Ok(FieldDecl {
        name: field.name,
        value_type,
        default,
        bound: field.bound.map(|bound| match bound {
            InBound::Int { min, max } => Bound::Int { min, max },
            InBound::Real { min, max } => Bound::Real { min, max },
            InBound::Length { max } => Bound::Length { max },
        }),
        optional: field.optional,
        indexed: field.indexed,
        aspect: field.aspect,
        since_minor: 0,
    })
}

fn in_type(value_type: InType, depth: u32, limits: &Limits) -> Result<ValueType> {
    if depth > MAX_TYPE_DEPTH {
        return Err(invalid_at("nesting", ""));
    }
    let too_long = |n: usize| u64::try_from(n).unwrap_or(u64::MAX) > limits.array_length;
    Ok(match value_type {
        InType::Bool => ValueType::Bool,
        InType::I64 => ValueType::I64,
        InType::F64 => ValueType::F64,
        InType::String => ValueType::String,
        InType::Quantity { dimension } => ValueType::Quantity(Dimension::from_exponents(dimension)),
        InType::Enum { variants } => {
            if too_long(variants.len()) {
                return Err(invalid_at("array_length", ""));
            }
            ValueType::Enum(
                variants
                    .into_iter()
                    .map(|v| EnumVariant::new(v.name, v.code))
                    .collect(),
            )
        }
        InType::Point2 => ValueType::Point2,
        InType::Point3 => ValueType::Point3,
        InType::Vector3 => ValueType::Vector3,
        InType::Transform => ValueType::Transform,
        InType::Color => ValueType::Color,
        InType::Reference { targets, rule } => {
            if too_long(targets.len()) {
                return Err(invalid_at("array_length", ""));
            }
            let cardinality = match (rule.cardinality.as_str(), rule.max) {
                ("one", None) => Cardinality::One,
                ("optional", None) => Cardinality::Optional,
                ("many", Some(max)) => Cardinality::Many { max },
                _ => return Err(invalid_at("cardinality", "")),
            };
            let targets = targets
                .into_iter()
                .map(TypeKey::new)
                .collect::<Result<Vec<_>>>()
                .map_err(|e| invalid_at("targets", "").with_source(e))?;
            ValueType::Reference(ReferenceSpec {
                targets,
                rule: RelationshipRule {
                    strength: rule.strength,
                    on_delete: rule.on_delete,
                    on_copy: rule.on_copy,
                    cardinality,
                },
            })
        }
        InType::ExternalReference => ValueType::ExternalReference,
        InType::Asset => ValueType::Asset,
        InType::Geometry {
            encoding,
            encoding_version,
        } => ValueType::Geometry(GeometryEncoding {
            name: encoding,
            version: encoding_version,
        }),
        InType::List { element } => {
            ValueType::List(Box::new(in_type(*element, depth + 1, limits)?))
        }
        InType::Map { value } => ValueType::Map(Box::new(in_type(*value, depth + 1, limits)?)),
        InType::Record { fields } => ValueType::Record(in_fields(fields, depth + 1, limits)?),
        InType::Opaque => ValueType::Opaque,
    })
}

/// The color value whose canonical JSON form is `json`.
fn color_of(json: &serde_json::Value) -> Option<Value> {
    let object = json.as_object().filter(|o| o.len() == 2)?;
    let rgba = object
        .get("rgba")?
        .as_array()
        .filter(|items| items.len() == 4)?;
    #[expect(clippy::cast_possible_truncation, reason = "the JSON form of an f32")]
    let c = |at: usize| rgba.get(at)?.as_f64().map(|v| v as f32);
    let source: Option<ColorSource> = serde_json::from_value(object.get("source")?.clone()).ok()?;
    Some(Value::Color(Box::new(ColorValue {
        rgba: Color::new(c(0)?, c(1)?, c(2)?, c(3)?),
        source,
    })))
}

/// The value of `value_type` whose canonical JSON form ([`Value`]'s `Serialize`) is `json`, or
/// `None` when `json` is not such a form. Recursion follows `value_type`, which is at most
/// [`MAX_TYPE_DEPTH`] deep below `depth`.
fn value_of(value_type: &ValueType, json: &serde_json::Value, depth: u32) -> Option<Value> {
    use serde_json::Value as J;
    if depth > MAX_TYPE_DEPTH + 1 {
        return None;
    }
    let reals = |json: &J, n: usize| -> Option<Vec<f64>> {
        let items = json.as_array().filter(|items| items.len() == n)?;
        items.iter().map(J::as_f64).collect()
    };
    let bytes = |json: &J| -> Option<Vec<u8>> {
        let text = json.as_str()?;
        if text.len() % 2 != 0 {
            return None;
        }
        (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
            .collect()
    };
    let id = |json: &J| ObjectId::from_str(json.as_str()?).ok();
    Some(match value_type {
        ValueType::Bool => Value::Bool(json.as_bool()?),
        ValueType::I64 => Value::I64(json.as_i64()?),
        ValueType::F64 => Value::F64(json.as_f64()?),
        ValueType::String => Value::String(json.as_str()?.to_owned()),
        ValueType::Quantity(dimension) => {
            Value::Quantity(Quantity::new(json.as_f64()?, *dimension))
        }
        ValueType::Enum(_) => Value::Enum(u32::try_from(json.as_u64()?).ok()?),
        ValueType::Point2 => {
            let p = reals(json, 2)?;
            Value::Point2(arc_base::math::Point2::new(*p.first()?, *p.get(1)?))
        }
        ValueType::Point3 => {
            let p = reals(json, 3)?;
            Value::Point3(arc_base::math::Point3::new(
                *p.first()?,
                *p.get(1)?,
                *p.get(2)?,
            ))
        }
        ValueType::Vector3 => {
            let p = reals(json, 3)?;
            Value::Vector3(arc_base::math::Vector3::new(
                *p.first()?,
                *p.get(1)?,
                *p.get(2)?,
            ))
        }
        ValueType::Transform => {
            let entries: [f64; 12] = reals(json, 12)?.try_into().ok()?;
            Value::Transform(Box::new(transform_from_entries(&entries)))
        }
        ValueType::Color => color_of(json)?,
        ValueType::Reference(_) => Value::Reference(id(json)?),
        ValueType::ExternalReference => {
            let object = json.as_object().filter(|o| o.len() == 2)?;
            Value::ExternalReference(Box::new(ExternalRef {
                link: id(object.get("link")?)?,
                target: id(object.get("target")?)?,
            }))
        }
        ValueType::Asset => Value::Asset(Box::new(AssetHash(bytes(json)?.try_into().ok()?))),
        ValueType::Geometry(_) => Value::Geometry(GeometryBytes(bytes(json)?)),
        ValueType::Opaque => Value::Opaque(bytes(json)?),
        ValueType::List(element) => Value::List(
            json.as_array()?
                .iter()
                .map(|item| value_of(element, item, depth + 1))
                .collect::<Option<_>>()?,
        ),
        ValueType::Map(element) => Value::Map(
            json.as_object()?
                .iter()
                .map(|(key, item)| Some((key.clone(), value_of(element, item, depth + 1)?)))
                .collect::<Option<BTreeMap<_, _>>>()?,
        ),
        ValueType::Record(fields) => {
            let items = json
                .as_array()
                .filter(|items| items.len() == fields.len())?;
            Value::Record(
                fields
                    .iter()
                    .zip(items)
                    .map(|(field, item)| match item {
                        J::Null => Some(None),
                        item => value_of(&field.value_type, item, depth + 1).map(Some),
                    })
                    .collect::<Option<_>>()?,
            )
        }
    })
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

    #[test]
    fn declarations_that_would_not_register_have_no_descriptor() {
        let key = TypeKey::new("test.thing").unwrap();
        let nan =
            FieldDecl::new("x", crate::schema::ValueType::F64).with_default(Value::F64(f64::NAN));
        let decl = TypeDecl::new(key, Schema::new(SchemaVersion::new(1, 0), vec![nan]));
        assert!(descriptor(&decl, SchemaVersion::new(1, 0)).is_none());
    }

    /// The published JSON Schema of the descriptor grammar.
    const SCHEMA: &str = include_str!("../../../schemas/package-descriptor.schema.json");

    /// A validator of the JSON Schema keywords the published schema uses. `pattern`,
    /// `maxLength` and the annotations are not checked: the parser checks names and keys.
    fn conforms(
        root: &serde_json::Value,
        schema: &serde_json::Value,
        v: &serde_json::Value,
    ) -> bool {
        use serde_json::Value as J;
        let object = match schema {
            J::Bool(accept) => return *accept,
            J::Object(object) => object,
            _ => return false,
        };
        object.iter().all(|(keyword, arg)| match keyword.as_str() {
            "$ref" => {
                let name = arg.as_str().unwrap().strip_prefix("#/$defs/").unwrap();
                conforms(root, &root["$defs"][name], v)
            }
            "type" => match arg.as_str().unwrap() {
                "object" => v.is_object(),
                "array" => v.is_array(),
                "string" => v.is_string(),
                "boolean" => v.is_boolean(),
                "null" => v.is_null(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                other => panic!("type {other}"),
            },
            "const" => v == arg,
            "enum" => arg.as_array().unwrap().contains(v),
            "required" => arg
                .as_array()
                .unwrap()
                .iter()
                .all(|key| v.get(key.as_str().unwrap()).is_some() || !v.is_object()),
            "properties" => v.as_object().is_none_or(|members| {
                members
                    .iter()
                    .all(|(key, member)| arg.get(key).is_none_or(|sub| conforms(root, sub, member)))
            }),
            "additionalProperties" => v.as_object().is_none_or(|members| {
                members.iter().all(|(key, member)| {
                    object["properties"].get(key).is_some() || conforms(root, arg, member)
                })
            }),
            "items" => v
                .as_array()
                .is_none_or(|items| items.iter().all(|item| conforms(root, arg, item))),
            "minItems" => v
                .as_array()
                .is_none_or(|items| items.len() as u64 >= arg.as_u64().unwrap()),
            "maxItems" => v
                .as_array()
                .is_none_or(|items| items.len() as u64 <= arg.as_u64().unwrap()),
            "minimum" => v.as_f64().is_none_or(|n| n >= arg.as_f64().unwrap()),
            "maximum" => v.as_f64().is_none_or(|n| n <= arg.as_f64().unwrap()),
            "oneOf" => {
                arg.as_array()
                    .unwrap()
                    .iter()
                    .filter(|sub| conforms(root, sub, v))
                    .count()
                    == 1
            }
            "$schema" | "$id" | "$defs" | "title" | "description" | "pattern" | "maxLength" => true,
            other => panic!("keyword {other} is not checked by this test"),
        })
    }

    fn schema_json() -> serde_json::Value {
        serde_json::from_str(SCHEMA).unwrap()
    }

    fn valid(text: &str) -> bool {
        let root = schema_json();
        conforms(&root, &root, &serde_json::from_str(text).unwrap())
    }

    fn decl_of(fields: Vec<FieldDecl>) -> TypeDecl {
        let key = TypeKey::new("test.thing").unwrap();
        TypeDecl::new(key, Schema::new(SchemaVersion::new(1, 0), fields))
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Every generated descriptor validates against the published schema, parses back into
        /// the declared schema, prints the same text again and decodes the same payloads.
        #[test]
        fn descriptors_parse_back_and_decode_the_same_payloads(
            (fields, values) in defaulted_fields().prop_flat_map(|fields| {
                let values = values(&fields);
                (Just(fields), values)
            }),
        ) {
            let decl = decl_of(fields);
            let text = descriptor(&decl, SchemaVersion::new(1, 0)).unwrap();
            prop_assert!(valid(&text), "{}", text);
            let (key, parsed) = Schema::from_descriptor(&text, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(&key, &decl.key);
            prop_assert_eq!(&parsed, &decl.schema);
            let again = TypeDecl::new(key, parsed.clone());
            prop_assert_eq!(descriptor(&again, SchemaVersion::new(1, 0)), Some(text));
            let limits = Limits::DEFAULT;
            let payload = crate::payload::encode(&decl.schema, &values, &limits).unwrap();
            let decoded =
                crate::payload::decode(&parsed, 0, payload.encoding, &payload.bytes, &limits)
                    .unwrap();
            prop_assert!(crate::schema::options_bits_eq(&decoded.values, &values));
        }

        /// Arbitrary bytes and mutations of a valid descriptor fail with the stable code or
        /// parse into a schema the registry accepts; nothing panics.
        #[test]
        fn hostile_descriptors_fail_with_the_stable_code(
            fields in fields(),
            at in any::<usize>(),
            byte in any::<u8>(),
            noise in prop::collection::vec(any::<u8>(), 0..64),
        ) {
            let text = descriptor(&decl_of(fields), SchemaVersion::new(1, 0)).unwrap();
            let mut bytes = text.into_bytes();
            if !bytes.is_empty() {
                let at = at % bytes.len();
                bytes[at] = byte;
            }
            for candidate in [bytes, noise] {
                let text = String::from_utf8_lossy(&candidate);
                match Schema::from_descriptor(&text, &Limits::DEFAULT) {
                    Ok((_, schema)) => prop_assert!(check_schema(&schema).is_ok()),
                    Err(error) => {
                        prop_assert_eq!(error.code(), codes::DESCRIPTOR_INVALID.code);
                    }
                }
            }
        }
    }

    #[test]
    fn a_minor_version_parses_into_the_fields_of_that_version() {
        let mut decl = decl_of(vec![
            FieldDecl::new("a", ValueType::I64),
            FieldDecl::new("b", ValueType::String)
                .with_default(Value::String("x".into()))
                .since_minor(2),
        ]);
        decl.schema.version = SchemaVersion::new(1, 3);
        for (minor, count) in [(1, 1), (2, 2), (3, 2)] {
            let text = descriptor(&decl, SchemaVersion::new(1, minor)).unwrap();
            assert!(valid(&text));
            let (_, schema) = Schema::from_descriptor(&text, &Limits::DEFAULT).unwrap();
            assert_eq!(schema.version, SchemaVersion::new(1, minor));
            assert_eq!(schema.fields.len(), count);
            assert!(schema.fields.iter().all(|field| field.since_minor == 0));
            // A payload written at this minor version holds the fields present at it.
            let values = &[Some(Value::I64(4)), Some(Value::String("y".into()))][..count];
            let written = Schema::new(schema.version, decl.schema.fields_at(minor).to_vec());
            let payload = crate::payload::encode(&written, values, &Limits::DEFAULT).unwrap();
            let decode = |schema: &Schema| {
                crate::payload::decode(
                    schema,
                    minor,
                    payload.encoding,
                    &payload.bytes,
                    &Limits::DEFAULT,
                )
                .unwrap()
                .values
            };
            assert_eq!(decode(&schema), values);
            assert_eq!(&decode(&decl.schema)[..count], values);
        }
    }

    #[test]
    fn invalid_descriptors_are_rejected_with_reasons() {
        let good = descriptor(
            &decl_of(vec![FieldDecl::new("a", ValueType::I64)]),
            SchemaVersion::new(1, 0),
        )
        .unwrap();
        let reason = |text: &str, limits: &Limits| {
            let error = Schema::from_descriptor(text, limits).unwrap_err();
            assert_eq!(error.code(), codes::DESCRIPTOR_INVALID.code);
            match error.details().get("reason") {
                Some(arc_base::error::Value::Text(reason)) => reason.clone(),
                other => panic!("{other:?}"),
            }
        };
        let limits = Limits::DEFAULT;
        let small = Limits {
            input_bytes: 8,
            ..Limits::DEFAULT
        };
        assert_eq!(reason(&good, &small), "input_bytes");
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(reason(&deep, &limits), "nesting");
        assert_eq!(reason("{", &limits), "grammar");
        let cases = [
            (
                "\"descriptor_format\": 1",
                "\"descriptor_format\": 2",
                "descriptor_format",
            ),
            ("\"test.thing\"", "\"Test\"", "type_key"),
            ("\"major\": 1", "\"major\": 0", "schema"),
            ("\"name\": \"a\"", "\"name\": \"A\"", "schema"),
            ("\"default\": null", "\"default\": \"x\"", "default"),
            ("\"kind\": \"i64\"", "\"kind\": \"i65\"", "grammar"),
            (
                "\"optional\": false,",
                "\"optional\": false, \"extra\": 1,",
                "grammar",
            ),
            (
                "\"bound\": null",
                "\"bound\": {\"kind\": \"length\", \"max\": 3}",
                "schema",
            ),
        ];
        for (from, to, expected) in cases {
            assert!(good.contains(from), "{from}");
            let text = good.replacen(from, to, 1);
            assert_eq!(reason(&text, &limits), expected, "{text}");
        }
        // A list nested deeper than the registry allows.
        let mut kind = "{\"kind\": \"i64\"}".to_owned();
        for _ in 0..=MAX_TYPE_DEPTH + 1 {
            kind = format!("{{\"kind\": \"list\", \"element\": {kind}}}");
        }
        let text = good.replacen("{\n        \"kind\": \"i64\"\n      }", &kind, 1);
        assert_ne!(text, good);
        let error = Schema::from_descriptor(&text, &limits).unwrap_err();
        assert_eq!(error.code(), codes::DESCRIPTOR_INVALID.code);
        // A default string longer than `string_bytes`.
        let text = good
            .replacen("\"kind\": \"i64\"", "\"kind\": \"string\"", 1)
            .replacen("\"default\": null", "\"default\": \"abcdef\"", 1);
        let short = Limits {
            string_bytes: 3,
            ..Limits::DEFAULT
        };
        assert!(Schema::from_descriptor(&text, &limits).is_ok());
        assert_eq!(reason(&text, &short), "default");
    }

    #[test]
    fn the_schema_rejects_what_the_grammar_rejects() {
        let good = descriptor(
            &decl_of(vec![FieldDecl::new("a", ValueType::I64)]),
            SchemaVersion::new(1, 0),
        )
        .unwrap();
        assert!(valid(&good));
        for (from, to) in [
            ("\"descriptor_format\": 1", "\"descriptor_format\": 2"),
            ("\"kind\": \"i64\"", "\"kind\": \"i65\""),
            ("\"optional\": false,", "\"optional\": false, \"extra\": 1,"),
            ("\"indexed\": false,", ""),
        ] {
            assert!(!valid(&good.replacen(from, to, 1)), "{to}");
        }
    }
}
