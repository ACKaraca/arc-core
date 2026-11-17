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

/// The live memory of the process in bytes, from `/proc/self/status` (Linux only).
fn resident() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// The memory a loaded document retains per object, measured by the resident set while the
/// rows are produced one at a time and dropped by the load, against the charge of the
/// `memory_bytes` budget: a load given the measured memory as its budget must fail. Linux
/// only; run alone in a release build with
/// `cargo nextest run -p arc-doc --release --run-ignored only memory_of --no-capture`, which
/// runs each case in a process of its own.
fn memory_per_object_stays_within_the_charge(case: usize) {
    let current = Schema::new(
        SchemaVersion::new(2, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    let old = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    let key = TypeKey::new("test.a").unwrap();
    let mut decl = TypeDecl::new(key.clone(), current.clone());
    decl.migrations = vec![arc_doc::Migration {
        source: old.clone(),
        migrate: Ok,
    }];
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    let registry = Arc::new(registry);
    let declared = |major, minor| StoredType {
        type_key: "test.a".into(),
        major,
        minor,
        descriptor: String::new(),
    };
    let cases: [(&str, &str, &Schema, Vec<StoredType>); 4] = [
        ("current version", "test.a", &current, vec![declared(2, 0)]),
        (
            "migrated",
            "test.a",
            &old,
            vec![declared(1, 0), declared(2, 0)],
        ),
        (
            "migrated, read-only",
            "test.a",
            &old,
            vec![declared(1, 0), declared(2, 0), declared(2, 1)],
        ),
        ("held (unregistered)", "test.u", &old, Vec::new()),
    ];
    for (name, type_key, schema, types) in cases.into_iter().skip(case).take(1) {
        let key = &key;
        let rows = || {
            (1..=OBJECTS).map(move |n| {
                let object = Object::new(
                    ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n),
                    schema.version,
                    Header::default(),
                    vec![Some(Value::I64(i64::try_from(n).unwrap()))],
                );
                let mut row = StoredObject::of(key, schema, &object, &Limits::DEFAULT).unwrap();
                row.type_key = type_key.into();
                row.payload_hash = vec![0; 32];
                row.last_txn = n.to_be_bytes().to_vec();
                row
            })
        };
        let before = resident().unwrap();
        let loaded = load(
            Arc::clone(&registry),
            &types,
            rows(),
            &LoadOptions::default(),
        )
        .unwrap();
        let retained = resident().unwrap().saturating_sub(before);
        eprintln!(
            "{name}: {} B per object (objects {}, held {}, original rows {})",
            retained / 1_000_000,
            loaded.snapshot.len(),
            loaded.snapshot.held_len(),
            loaded.rows.len()
        );
        drop(loaded);
        let options = LoadOptions {
            limits: Limits {
                memory_bytes: Some(retained),
                ..Limits::DEFAULT
            },
            ..LoadOptions::default()
        };
        let error = load(Arc::clone(&registry), &types, rows(), &options).err();
        assert_eq!(
            error.map(|error| error.code()),
            Some("doc.load.limit_exceeded"),
            "{name}: the charge is below the measured memory"
        );
    }
}

#[test]
#[ignore = "loads one million objects and reads the resident set"]
fn memory_of_current_version_objects() {
    memory_per_object_stays_within_the_charge(0);
}

#[test]
#[ignore = "loads one million objects and reads the resident set"]
fn memory_of_migrated_objects() {
    memory_per_object_stays_within_the_charge(1);
}

#[test]
#[ignore = "loads one million objects and reads the resident set"]
fn memory_of_migrated_objects_of_a_read_only_document() {
    memory_per_object_stays_within_the_charge(2);
}

#[test]
#[ignore = "loads one million objects and reads the resident set"]
fn memory_of_held_objects() {
    memory_per_object_stays_within_the_charge(3);
}
