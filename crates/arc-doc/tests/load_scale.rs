//! Bulk load time of one million small objects. Ignored by default; run with
//! `cargo nextest run -p arc-doc --release --run-ignored only load_scale --no-capture`.

#![allow(
    clippy::unwrap_used,
    clippy::print_stderr,
    reason = "the ignored scale test reports its measurements"
)]

use std::sync::Arc;
use std::time::Instant;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::{
    FieldDecl, Header, LoadOptions, Object, Schema, SchemaVersion, StoredObject, StoredType,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType, load,
};

const OBJECTS: u128 = 1_000_000;

#[test]
#[ignore = "loads one million objects"]
fn one_million_small_objects_load_in_bulk() {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    let key = TypeKey::new("test.a").unwrap();
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(key.clone(), schema.clone()))
        .unwrap();
    let rows: Vec<StoredObject> = (0..OBJECTS)
        .map(|n| {
            let object = Object::new(
                ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n),
                SchemaVersion::new(1, 0),
                Header::default(),
                vec![Some(Value::I64(i64::try_from(n).unwrap()))],
            );
            StoredObject::of(&key, &schema, &object, &Limits::DEFAULT).unwrap()
        })
        .collect();
    let types = [StoredType {
        type_key: "test.a".into(),
        major: 1,
        minor: 0,
        descriptor: String::new(),
    }];
    let start = Instant::now();
    let loaded = load(Arc::new(registry), &types, rows, &LoadOptions::default()).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(loaded.snapshot.len() as u128, OBJECTS);
    eprintln!("load of {OBJECTS} objects: {elapsed:?}");
}
