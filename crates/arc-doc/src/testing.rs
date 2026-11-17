//! Strategies shared by the property tests of this crate: random valid field declarations
//! together with values that satisfy them, covering every value type.

use std::collections::BTreeMap;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Color, Dimension, ObjectId, Quantity};
use proptest::prelude::*;

use crate::schema::{
    AssetHash, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    GeometryBytes, GeometryEncoding, OnCopy, OnDelete, ReferenceSpec, RelationshipRule, Strength,
    TypeKey, Value, ValueType, transform_from_entries,
};

fn finite() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite", |v| v.is_finite())
}

fn finite32() -> impl Strategy<Value = f32> {
    any::<f32>().prop_filter("finite", |v| v.is_finite())
}

fn text() -> impl Strategy<Value = String> {
    "\\PC{0,6}"
}

fn bytes() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..12)
}

fn id() -> impl Strategy<Value = ObjectId> {
    any::<u128>().prop_map(ObjectId::from_u128)
}

fn enum_type() -> impl Strategy<Value = ValueType> {
    prop::collection::btree_set(any::<u32>(), 1..4).prop_map(|codes| {
        ValueType::Enum(
            codes
                .into_iter()
                .enumerate()
                .map(|(i, code)| EnumVariant::new(format!("v{i}"), code))
                .collect(),
        )
    })
}

/// Every value type except references, which only fields and lists of fields hold.
fn leaf_type() -> impl Strategy<Value = ValueType> {
    prop_oneof![
        Just(ValueType::Bool),
        Just(ValueType::I64),
        Just(ValueType::F64),
        Just(ValueType::String),
        any::<[i8; 9]>().prop_map(|e| ValueType::Quantity(Dimension::from_exponents(e))),
        enum_type(),
        Just(ValueType::Point2),
        Just(ValueType::Point3),
        Just(ValueType::Vector3),
        Just(ValueType::Transform),
        Just(ValueType::Color),
        Just(ValueType::ExternalReference),
        Just(ValueType::Asset),
        Just(ValueType::Geometry(GeometryEncoding {
            name: "test.curve".to_owned(),
            version: 1,
        })),
        Just(ValueType::Opaque),
    ]
}

fn named(fields: Vec<(ValueType, bool)>) -> Vec<FieldDecl> {
    fields
        .into_iter()
        .enumerate()
        .map(|(i, (value_type, optional))| FieldDecl {
            optional,
            ..FieldDecl::new(format!("f{i}"), value_type)
        })
        .collect()
}

/// A value type without references, nested up to three levels.
pub fn nested_type() -> impl Strategy<Value = ValueType> {
    leaf_type().prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|t| ValueType::List(Box::new(t))),
            inner.clone().prop_map(|t| ValueType::Map(Box::new(t))),
            prop::collection::vec((inner, any::<bool>()), 1..4)
                .prop_map(|fields| ValueType::Record(named(fields))),
        ]
    })
}

fn rule(cardinality: Cardinality) -> impl Strategy<Value = RelationshipRule> {
    let clearing = cardinality != Cardinality::One;
    (0..3_u8, 0..3_u8, 0..3_u8).prop_map(move |(s, d, c)| RelationshipRule {
        strength: [
            Strength::Owning,
            Strength::Associative,
            Strength::Informational,
        ][usize::from(s)],
        on_delete: match d {
            0 => OnDelete::Cascade,
            1 if clearing => OnDelete::Clear,
            _ => OnDelete::Block,
        },
        on_copy: match c {
            0 => OnCopy::CopyTarget,
            1 if clearing => OnCopy::Clear,
            _ => OnCopy::ShareTarget,
        },
        cardinality,
    })
}

fn reference(cardinality: Cardinality) -> impl Strategy<Value = ValueType> {
    rule(cardinality).prop_map(|rule| {
        ValueType::Reference(ReferenceSpec {
            targets: vec![TypeKey::new("test.target").unwrap()],
            rule,
        })
    })
}

/// A valid field of any type, references included, named later.
fn field_shape() -> impl Strategy<Value = (ValueType, bool)> {
    prop_oneof![
        6 => (nested_type(), any::<bool>()),
        1 => reference(Cardinality::One).prop_map(|t| (t, false)),
        1 => reference(Cardinality::Optional).prop_map(|t| (t, true)),
        1 => (1..4_u32)
            .prop_flat_map(|max| reference(Cardinality::Many { max }))
            .prop_map(|t| (ValueType::List(Box::new(t)), false)),
    ]
}

/// A valid field list of up to five fields.
pub fn fields() -> impl Strategy<Value = Vec<FieldDecl>> {
    prop::collection::vec(field_shape(), 0..6).prop_map(named)
}

/// A value of `value_type`.
pub fn value(value_type: &ValueType) -> BoxedStrategy<Value> {
    match value_type {
        ValueType::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        ValueType::I64 => any::<i64>().prop_map(Value::I64).boxed(),
        ValueType::F64 => finite().prop_map(Value::F64).boxed(),
        ValueType::String => text().prop_map(Value::String).boxed(),
        ValueType::Quantity(d) => {
            let d = *d;
            finite()
                .prop_map(move |v| Value::Quantity(Quantity::new(v, d)))
                .boxed()
        }
        ValueType::Enum(variants) => {
            let codes: Vec<u32> = variants.iter().map(|v| v.code).collect();
            prop::sample::select(codes).prop_map(Value::Enum).boxed()
        }
        ValueType::Point2 => (finite(), finite())
            .prop_map(|(x, y)| Value::Point2(Point2::new(x, y)))
            .boxed(),
        ValueType::Point3 => (finite(), finite(), finite())
            .prop_map(|(x, y, z)| Value::Point3(Point3::new(x, y, z)))
            .boxed(),
        ValueType::Vector3 => (finite(), finite(), finite())
            .prop_map(|(x, y, z)| Value::Vector3(Vector3::new(x, y, z)))
            .boxed(),
        ValueType::Transform => prop::array::uniform12(finite())
            .prop_map(|entries| Value::Transform(transform_from_entries(&entries)))
            .boxed(),
        ValueType::Color => (
            prop::array::uniform4(finite32()),
            prop::option::of(prop_oneof![
                any::<u8>().prop_map(ColorSource::Indexed),
                text().prop_map(ColorSource::Named),
            ]),
        )
            .prop_map(|([r, g, b, a], source)| {
                Value::Color(ColorValue {
                    rgba: Color::new(r, g, b, a),
                    source,
                })
            })
            .boxed(),
        ValueType::Reference(_) => id().prop_map(Value::Reference).boxed(),
        ValueType::ExternalReference => (id(), id())
            .prop_map(|(link, target)| Value::ExternalReference(ExternalRef { link, target }))
            .boxed(),
        ValueType::Asset => any::<[u8; 32]>()
            .prop_map(|hash| Value::Asset(AssetHash(hash)))
            .boxed(),
        ValueType::Geometry(_) => bytes()
            .prop_map(|b| Value::Geometry(GeometryBytes(b)))
            .boxed(),
        ValueType::Opaque => bytes().prop_map(Value::Opaque).boxed(),
        ValueType::List(element) => {
            let max = match element.as_ref() {
                ValueType::Reference(spec) => match spec.rule.cardinality {
                    Cardinality::Many { max } => usize::try_from(max).unwrap_or(1),
                    _ => 1,
                },
                _ => 3,
            };
            prop::collection::vec(value(element), 0..=max)
                .prop_map(Value::List)
                .boxed()
        }
        ValueType::Map(element) => prop::collection::btree_map(text(), value(element), 0..3)
            .prop_map(|entries: BTreeMap<String, Value>| Value::Map(entries))
            .boxed(),
        ValueType::Record(fields) => values(fields).prop_map(Value::Record).boxed(),
    }
}

/// Values for `fields`: optional fields are sometimes absent.
pub fn values(fields: &[FieldDecl]) -> BoxedStrategy<Vec<Option<Value>>> {
    fields
        .iter()
        .map(|field| {
            if field.optional {
                prop::option::of(value(&field.value_type)).boxed()
            } else {
                value(&field.value_type).prop_map(Some).boxed()
            }
        })
        .collect::<Vec<_>>()
        .boxed()
}

/// A field list together with values that satisfy it.
pub fn object() -> impl Strategy<Value = (Vec<FieldDecl>, Vec<Option<Value>>)> {
    fields().prop_flat_map(|fields| {
        let values = values(&fields);
        (Just(fields), values)
    })
}
