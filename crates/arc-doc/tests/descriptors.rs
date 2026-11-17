//! The descriptors of a type that uses every value type, compared with reviewed snapshots
//! (CORE-DOC-004). A changed snapshot is a changed package contract.

#![allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
    reason = "the sample is built from valid literal declarations, one field per line"
)]

use std::collections::BTreeMap;

use arc_base::id::Identifier;
use arc_base::math::{Point2, Point3, Transform3, Vector3};
use arc_base::{Color, Dimension, ObjectId, Quantity, Result};
use arc_doc::{
    AssetHash, Bound, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    GeometryBytes, GeometryEncoding, Migration, OnCopy, OnDelete, ReferenceSpec, RelationshipRule,
    Schema, SchemaVersion, Strength, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, descriptor,
};

fn key(text: &str) -> TypeKey {
    TypeKey::new(text).unwrap()
}

fn reference(cardinality: Cardinality, on_delete: OnDelete) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: vec![key("test.target")],
        rule: RelationshipRule {
            strength: Strength::Owning,
            on_delete,
            on_copy: OnCopy::CopyTarget,
            cardinality,
        },
    })
}

fn id(raw: u128) -> ObjectId {
    ObjectId::from_u128(raw)
}

fn lift(values: Vec<Option<Value>>) -> Result<Vec<Option<Value>>> {
    Ok(values)
}

fn sample() -> TypeDecl {
    let colors = vec![EnumVariant::new("red", 1), EnumVariant::new("blue", 7)];
    let fields = vec![
        FieldDecl::new("visible", ValueType::Bool)
            .with_default(Value::Bool(true))
            .in_aspect("appearance"),
        FieldDecl::new("count", ValueType::I64)
            .with_bound(Bound::Int {
                min: -3,
                max: 1_000,
            })
            .with_default(Value::I64(-3))
            .indexed(),
        FieldDecl::new("ratio", ValueType::F64)
            .with_bound(Bound::Real { min: 0.0, max: 1.0 })
            .with_default(Value::F64(0.1)),
        FieldDecl::new("label", ValueType::String)
            .with_bound(Bound::Length { max: 64 })
            .with_default(Value::String("wall \"A\"".to_owned()))
            .in_aspect("identity")
            .indexed(),
        FieldDecl::new("height", ValueType::Quantity(Dimension::LENGTH))
            .with_default(Value::Quantity(Quantity::new(2.7, Dimension::LENGTH)))
            .in_aspect("geometry"),
        FieldDecl::new("color", ValueType::Enum(colors)).with_default(Value::Enum(7)),
        FieldDecl::new("origin", ValueType::Point2)
            .with_default(Value::Point2(Point2::new(-0.0, 1e300))),
        FieldDecl::new("position", ValueType::Point3)
            .with_default(Value::Point3(Point3::new(1.0, 2.5, -3.0))),
        FieldDecl::new("normal", ValueType::Vector3)
            .with_default(Value::Vector3(Vector3::new(0.0, 0.0, 1.0))),
        FieldDecl::new("placement", ValueType::Transform)
            .with_default(Value::Transform(Transform3::identity()))
            .in_aspect("geometry"),
        FieldDecl::new("tint", ValueType::Color)
            .optional()
            .with_default(Value::Color(ColorValue {
                rgba: Color::new(1.0, 0.5, 0.25, 1.0),
                source: Some(ColorSource::Indexed(7)),
            })),
        FieldDecl::new("host", reference(Cardinality::One, OnDelete::Cascade)),
        FieldDecl::new("style", reference(Cardinality::Optional, OnDelete::Clear)).optional(),
        FieldDecl::new(
            "openings",
            ValueType::List(Box::new(reference(
                Cardinality::Many { max: 16 },
                OnDelete::Clear,
            ))),
        ),
        FieldDecl::new("linked", ValueType::ExternalReference)
            .optional()
            .with_default(Value::ExternalReference(ExternalRef {
                link: id(1),
                target: id(0x0190_0000_0000_7000_8000_0000_0000_0002),
            })),
        FieldDecl::new("texture", ValueType::Asset)
            .optional()
            .with_default(Value::Asset(AssetHash([0xab; 32]))),
        FieldDecl::new(
            "outline",
            ValueType::Geometry(GeometryEncoding {
                name: "geom.curve2".to_owned(),
                version: 1,
            }),
        )
        .with_default(Value::Geometry(GeometryBytes(vec![0, 1, 0xff])))
        .in_aspect("geometry"),
        FieldDecl::new("tags", ValueType::List(Box::new(ValueType::String)))
            .with_bound(Bound::Length { max: 8 })
            .with_default(Value::List(vec![Value::String("x".to_owned())])),
        FieldDecl::new("costs", ValueType::Map(Box::new(ValueType::F64))).with_default(Value::Map(
            BTreeMap::from([
                ("b".to_owned(), Value::F64(2.0)),
                ("a".to_owned(), Value::F64(1.5)),
            ]),
        )),
        FieldDecl::new(
            "layer",
            ValueType::Record(vec![
                FieldDecl::new("index", ValueType::I64),
                FieldDecl::new("note", ValueType::String).optional(),
            ]),
        )
        .with_default(Value::Record(vec![Some(Value::I64(0)), None])),
        FieldDecl::new("extra", ValueType::Opaque)
            .optional()
            .with_default(Value::Opaque(vec![0xde, 0xad])),
        FieldDecl::new("revision", ValueType::I64)
            .since_minor(1)
            .with_default(Value::I64(0)),
    ];
    let mut decl = TypeDecl::new(
        key("test.sample"),
        Schema::new(SchemaVersion::new(2, 1), fields),
    );
    decl.migrations = vec![Migration {
        source: Schema::new(
            SchemaVersion::new(1, 0),
            vec![FieldDecl::new("count", ValueType::I64)],
        ),
        migrate: lift,
    }];
    decl
}

#[test]
fn descriptors_of_every_version_match_the_reviewed_snapshots() {
    let mut registry = TypeRegistry::new();
    registry.register(sample()).unwrap();
    let decl = registry.get("test.sample").unwrap();
    let versions: Vec<String> = decl.versions().iter().map(ToString::to_string).collect();
    assert_eq!(versions, ["1.0", "2.0", "2.1"]);
    for version in decl.versions() {
        let text = descriptor(decl, version).unwrap();
        insta::assert_snapshot!(
            format!("test_sample_{}_{}", version.major, version.minor),
            text
        );
    }
}
