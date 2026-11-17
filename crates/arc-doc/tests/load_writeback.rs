//! The write-back contract of loading, differentially: random documents (types with majors and
//! minors, migrations, unregistered types, corrupt rows) are loaded and saved by the contract of
//! `docs/crates/arc-doc.md` ("Write-back contract"), then compared row by row with the input and
//! loaded again; with a random subset of objects touched; and the bulk load path is compared
//! with working-set creates. Also contained panics of migrations and validators, partially
//! valid headers, and the memory charge of held rows.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers build valid literal fixtures; two registered functions panic on purpose"
)]

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Error, Limits, ObjectId};
use arc_doc::error::codes;
use arc_doc::{
    Cardinality, FieldDecl, Finding, Header, HeldKind, LoadOptions, Loaded, Migration, OnCopy,
    OnDelete, ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Snapshot,
    StoredObject, StoredType, Strength, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
    WorkingSet, apply_delete_rules, check_references, descriptor, load, payload,
};
use proptest::prelude::*;

/// The transaction id a save records for objects changed since load.
const NEW_TXN: [u8; 16] = [0xEE; 16];

fn any_reference() -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: Vec::new(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    })
}

fn a1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 1),
        vec![
            FieldDecl::new("x", ValueType::I64),
            FieldDecl::new("y", ValueType::I64)
                .optional()
                .since_minor(1),
        ],
    )
}

fn a2() -> Schema {
    Schema::new(
        SchemaVersion::new(2, 0),
        vec![
            FieldDecl::new("x", ValueType::I64),
            FieldDecl::new("tag", ValueType::String),
        ],
    )
}

fn a3() -> Schema {
    Schema::new(
        SchemaVersion::new(3, 1),
        vec![
            FieldDecl::new("x", ValueType::I64),
            FieldDecl::new("tag", ValueType::String),
            FieldDecl::new("z", ValueType::I64)
                .optional()
                .since_minor(1),
        ],
    )
}

/// Version 3.2 of `t.a`, which this registry does not know: a newer minor version.
fn a3_2() -> Schema {
    let mut fields = a3().fields;
    fields.push(
        FieldDecl::new("w", ValueType::I64)
            .optional()
            .since_minor(2),
    );
    Schema::new(SchemaVersion::new(3, 2), fields)
}

fn b1() -> Schema {
    Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("r", any_reference()).optional()],
    )
}

/// 1 → 2. x = 13 fails, x = 666 panics.
fn one_to_two(values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    let x = match values.first() {
        Some(Some(Value::I64(x))) => *x,
        _ => return Err(Error::new(codes::VALUE_TYPE_MISMATCH)),
    };
    match x {
        13 => Err(Error::new(codes::VALUE_OUT_OF_RANGE)),
        666 => panic!("a defect of the migration"),
        _ => Ok(vec![Some(Value::I64(x)), Some(Value::String("m".into()))]),
    }
}

/// 2 → 3. x = 17 returns a value of the wrong shape.
fn two_to_three(mut values: Vec<Option<Value>>) -> arc_base::Result<Vec<Option<Value>>> {
    if values.first() != Some(&Some(Value::I64(17))) {
        values.push(None);
    }
    Ok(values)
}

/// x = 99 is an error, 98 a warning, 667 panics.
fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    let finding = |severity| {
        vec![Finding {
            severity,
            error: Error::new(codes::VALUE_OUT_OF_RANGE),
        }]
    };
    match values.first() {
        Some(Some(Value::I64(99))) => finding(Severity::Error),
        Some(Some(Value::I64(98))) => finding(Severity::Warning),
        Some(Some(Value::I64(667))) => panic!("a defect of the validator"),
        _ => Vec::new(),
    }
}

fn decl_a() -> TypeDecl {
    let mut decl = TypeDecl::new(TypeKey::new("t.a").unwrap(), a3());
    decl.migrations = vec![
        Migration {
            source: a1(),
            migrate: one_to_two,
        },
        Migration {
            source: a2(),
            migrate: two_to_three,
        },
    ];
    decl.validators = vec![validator];
    decl
}

fn decl_b() -> TypeDecl {
    TypeDecl::new(TypeKey::new("t.b").unwrap(), b1())
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    registry.register(decl_a()).unwrap();
    registry.register(decl_b()).unwrap();
    Arc::new(registry)
}

fn stored_type(key: &str, major: i64, minor: i64) -> StoredType {
    StoredType {
        type_key: key.into(),
        major,
        minor,
        descriptor: format!("{key} {major}.{minor}"),
    }
}

fn types(newer_minor: bool) -> Vec<StoredType> {
    let mut types: Vec<StoredType> = [
        ("t.a", 1, 0),
        ("t.a", 1, 1),
        ("t.a", 2, 0),
        ("t.a", 3, 0),
        ("t.a", 3, 1),
        ("t.b", 1, 0),
        ("x.u", 4, 2),
    ]
    .into_iter()
    .map(|(key, major, minor)| stored_type(key, major, minor))
    .collect();
    if newer_minor {
        types.push(stored_type("t.a", 3, 2));
    }
    types
}

/// A stand-in for the BLAKE3 payload hash the package derives.
fn hash(payload: &[u8]) -> Vec<u8> {
    let sum = payload.iter().fold(0xcbf2_9ce4_8422_2325_u64, |sum, byte| {
        (sum ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    sum.to_be_bytes().to_vec()
}

fn encode(schema: &Schema, values: &[Option<Value>]) -> Vec<u8> {
    payload::encode(schema, values, &Limits::DEFAULT)
        .unwrap()
        .bytes
}

const KINDS: u8 = 15;

/// A stored row of case `kind` with identifier `n`, value `x`, owner `owner` and transaction
/// `txn`. Kind 14 is a row of the newer minor version 3.2.
fn stored(kind: u8, n: u128, x: i64, owner: Option<u128>, txn: u8) -> StoredObject {
    let id = |n: u128| n.to_be_bytes().to_vec();
    let mut row = StoredObject {
        id: id(n),
        owner: owner.map(id),
        name: Some(format!("n{n}")),
        partition: n.is_multiple_of(3).then(|| "p".into()),
        flags: i64::try_from(n % 8).unwrap(),
        last_txn: vec![txn; 16],
        ..StoredObject::default()
    };
    let x = || Some(Value::I64(x));
    let tag = || Some(Value::String("t".into()));
    let (key, major, minor, payload) = match kind % KINDS {
        0 => {
            let v1_0 = Schema::new(SchemaVersion::new(1, 0), a1().fields_at(0).to_vec());
            ("t.a", 1, 0, encode(&v1_0, &[x()]))
        }
        1 => ("t.a", 1, 1, encode(&a1(), &[x(), Some(Value::I64(1))])),
        2 => ("t.a", 2, 0, encode(&a2(), &[x(), tag()])),
        3 => {
            let v3_0 = Schema::new(SchemaVersion::new(3, 0), a3().fields_at(0).to_vec());
            ("t.a", 3, 0, encode(&v3_0, &[x(), tag()]))
        }
        4 | 5 => (
            "t.a",
            3,
            1,
            encode(&a3(), &[x(), tag(), Some(Value::I64(2))]),
        ),
        6 => {
            let reference = owner.map(|o| Value::Reference(ObjectId::from_u128(o)));
            ("t.b", 1, 0, encode(&b1(), &[reference]))
        }
        7 => ("x.u", 4, 2, vec![1, 2, 3, n.to_be_bytes()[15]]),
        8 => ("t.a", 2, 5, vec![0]),
        9 => ("t.a", 3, 1, vec![0xFF; 3]),
        10 => {
            row.id = vec![1, 2, 3];
            ("t.b", 1, 0, vec![0])
        }
        11 => {
            row.flags = 64;
            ("t.b", 1, 0, encode(&b1(), &[None]))
        }
        12 => {
            row.owner = Some(vec![9; 5]);
            ("t.b", 1, 0, encode(&b1(), &[None]))
        }
        13 => {
            row.encoding = 7;
            ("t.b", 1, 0, encode(&b1(), &[None]))
        }
        _ => {
            let values = [x(), tag(), None, Some(Value::I64(4))];
            ("t.a", 3, 2, encode(&a3_2(), &values))
        }
    };
    row.type_key = key.into();
    row.major = major;
    row.minor = minor;
    row.payload_hash = hash(&payload);
    row.payload = payload;
    row
}

/// A document: whether it declares the newer minor version, and its rows.
fn document() -> impl Strategy<Value = (bool, Vec<StoredObject>)> {
    let row = (
        0..KINDS,
        1_u128..40,
        prop_oneof![Just(13_i64), Just(17), Just(98), Just(99), -5_i64..5],
        prop::option::of(1_u128..40),
        0_u8..3,
    );
    (any::<bool>(), prop::collection::vec(row, 0..40)).prop_map(|(newer_minor, rows)| {
        let rows = rows
            .into_iter()
            .filter(|(kind, ..)| newer_minor || *kind != KINDS - 1)
            .map(|(kind, n, x, owner, txn)| stored(kind, n, x, owner, txn))
            .collect();
        (newer_minor, rows)
    })
}

fn object_id(bytes: &[u8]) -> Option<ObjectId> {
    let raw = u128::from_be_bytes(bytes.try_into().ok()?);
    (raw != 0).then(|| ObjectId::from_u128(raw))
}

/// A save by the write-back contract: the type rows and the object rows. `touched` lists the
/// objects changed since load, which carry [`NEW_TXN`].
fn save(
    loaded: &Loaded,
    snapshot: &Snapshot,
    touched: &BTreeSet<ObjectId>,
) -> (Vec<StoredType>, Vec<StoredObject>) {
    let registry = snapshot.registry();
    let read_only = !snapshot.read_only().is_empty();
    let mut kept: Vec<StoredObject> = snapshot
        .held()
        .map(|(_, held)| held.row().clone())
        .collect();
    let mut rows = Vec::new();
    for (handle, object) in snapshot.iter() {
        let key = snapshot.type_key(handle).unwrap();
        if read_only && let Some(original) = loaded.rows.get(object.id) {
            kept.push(original.stored.clone());
            continue;
        }
        let schema = &registry.get(key.as_str()).unwrap().schema;
        let mut row = StoredObject::of(key, schema, object, &Limits::DEFAULT).unwrap();
        row.payload_hash = hash(&row.payload);
        row.last_txn = if touched.contains(&object.id) {
            NEW_TXN.to_vec()
        } else {
            loaded.rows.last_txn(object.id).unwrap().to_vec()
        };
        rows.push(row);
    }
    // Unaddressable rows go last, so the loaded row of an identifier precedes its duplicates.
    kept.extend(loaded.rows.unaddressable().iter().cloned());
    // A copy of a read-only document writes every type row as read.
    if read_only {
        rows.extend(kept);
        return (loaded.types.clone(), rows);
    }
    // The current type rows of the registry, then the stored type row of every row written
    // as stored.
    let mut types: BTreeMap<(String, i64, i64), StoredType> = BTreeMap::new();
    for key in ["t.a", "t.b"] {
        let decl = registry.get(key).unwrap();
        let version = decl.schema.version;
        let row = StoredType {
            type_key: key.into(),
            major: version.major.into(),
            minor: version.minor.into(),
            descriptor: descriptor(decl, version).unwrap(),
        };
        types.insert((row.type_key.clone(), row.major, row.minor), row);
    }
    for row in &kept {
        if let Some(stored) = loaded.types.iter().find(|stored| {
            (&stored.type_key, stored.major, stored.minor) == (&row.type_key, row.major, row.minor)
        }) {
            types
                .entry((stored.type_key.clone(), stored.major, stored.minor))
                .or_insert_with(|| stored.clone());
        }
    }
    rows.extend(kept);
    (types.into_values().collect(), rows)
}

fn sorted(mut rows: Vec<StoredObject>) -> Vec<StoredObject> {
    rows.sort_by_key(|row| format!("{row:?}"));
    rows
}

fn current(row: &StoredObject) -> bool {
    (row.type_key == "t.a" && (row.major, row.minor) == (3, 1))
        || (row.type_key == "t.b" && (row.major, row.minor) == (1, 0))
}

/// The objects of the typed stores by identifier.
fn objects(snapshot: &Snapshot) -> BTreeMap<ObjectId, (String, Header, Vec<Option<Value>>)> {
    snapshot
        .iter()
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
            (
                object.id,
                (key, object.header.clone(), object.values.clone()),
            )
        })
        .collect()
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|cases| cases.parse().ok())
        .unwrap_or(48)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    /// Load, save by the contract, compare with the input and load again.
    #[test]
    fn a_save_by_the_contract_keeps_every_row(
        (newer_minor, input) in document(),
        touch in prop::collection::vec(any::<bool>(), 40),
    ) {
        let options = LoadOptions::default();
        let loaded = load(registry(), &types(newer_minor), input.clone(), &options).unwrap();
        let snapshot = &loaded.snapshot;
        let read_only = !snapshot.read_only().is_empty();
        prop_assert_eq!(read_only, newer_minor);
        let (types, output) = save(&loaded, snapshot, &BTreeSet::new());
        if read_only {
            // A copy of a read-only document writes every row as stored.
            prop_assert_eq!(sorted(output.clone()), sorted(input.clone()));
        } else {
            // Rows at the current version, held rows and unusable rows come back byte for
            // byte; older versions of registered types come back at the current version with
            // their header columns and `last_txn`.
            let typed = objects(snapshot);
            let mut seen = BTreeSet::new();
            let expected: Vec<StoredObject> = input
                .iter()
                .map(|row| {
                    // The first row of an identifier is the one loaded.
                    let id = object_id(&row.id).filter(|id| seen.insert(*id));
                    if id.is_some_and(|id| typed.contains_key(&id)) && !current(row) {
                        let out = output
                            .iter()
                            .find(|out| out.id == row.id && current(out))
                            .unwrap();
                        StoredObject {
                            major: out.major,
                            minor: out.minor,
                            payload: out.payload.clone(),
                            payload_hash: hash(&out.payload),
                            ..row.clone()
                        }
                    } else {
                        row.clone()
                    }
                })
                .collect();
            prop_assert_eq!(sorted(output.clone()), sorted(expected));
        }
        // Every row a save writes as stored keeps its declared type row.
        let declared = |row: &StoredObject| {
            types.iter().any(|t| (&t.type_key, t.major, t.minor) == (&row.type_key, row.major, row.minor))
        };
        for row in &input {
            let was = loaded.types.iter().any(|t| (&t.type_key, t.major, t.minor) == (&row.type_key, row.major, row.minor));
            if was && output.contains(row) {
                prop_assert!(declared(row), "type row of {:?} dropped", row);
            }
        }
        // Loading the saved rows gives the same document.
        let again = load(registry(), &types, output, &options).unwrap();
        prop_assert_eq!(objects(&again.snapshot), objects(snapshot));
        prop_assert_eq!(again.snapshot.read_only(), snapshot.read_only());
        let held = |snapshot: &Snapshot| {
            sorted(snapshot.held().map(|(_, held)| held.row().clone()).collect())
        };
        prop_assert_eq!(held(&again.snapshot), held(snapshot));
        prop_assert_eq!(sorted(again.rows.unaddressable().to_vec()), sorted(loaded.rows.unaddressable().to_vec()));
        if !read_only {
            prop_assert_eq!(again.counts.migrated + again.counts.older_minor, 0);
        }

        // Touch a random subset through a working set: only touched objects change, at the
        // current version and with the new transaction.
        if !read_only {
            let mut set = WorkingSet::new(snapshot);
            let mut touched = BTreeSet::new();
            let live: Vec<_> = snapshot.iter().map(|(handle, object)| (handle, Arc::clone(object))).collect();
            for (at, (handle, object)) in live.iter().enumerate() {
                if touch[at % touch.len()] {
                    set.replace_header(*handle, Header { name: Some("edited".into()), ..object.header.clone() }).unwrap();
                    touched.insert(object.id);
                }
            }
            let (_, edited) = save(&loaded, set.current(), &touched);
            let (_, untouched) = save(&loaded, snapshot, &BTreeSet::new());
            for row in &edited {
                let id = object_id(&row.id);
                if id.is_some_and(|id| touched.contains(&id)) && !untouched.contains(row) {
                    prop_assert!(current(row));
                    prop_assert_eq!(&row.last_txn, &NEW_TXN.to_vec());
                    prop_assert_eq!(row.name.as_deref(), Some("edited"));
                } else {
                    prop_assert!(untouched.contains(row), "untouched row changed: {:?}", row);
                }
            }
        }
    }

    /// The bulk load path builds the handles, identifier index and reverse index that the same
    /// creates through a working set build.
    #[test]
    fn the_bulk_load_equals_working_set_creates((_, input) in document()) {
        let loaded = load(registry(), &types(false), input, &LoadOptions::default()).unwrap();
        let snapshot = &loaded.snapshot;
        let mut set = WorkingSet::new(&Snapshot::empty(registry()));
        for (handle, object) in snapshot.iter() {
            let key = snapshot.type_key(handle).unwrap().as_str();
            prop_assert_eq!(set.create(key, (**object).clone()).unwrap(), handle);
        }
        let built = set.current();
        for (_, object) in snapshot.iter() {
            prop_assert_eq!(snapshot.resolve(object.id).unwrap(), built.resolve(object.id).unwrap());
        }
        for n in 1..40 {
            let id = ObjectId::from_u128(n);
            let typed = |source: ObjectId| {
                snapshot.resolve(source).is_ok_and(|handle| snapshot.held_object(handle).is_none())
            };
            let bulk: Vec<_> = snapshot
                .referrers(id)
                .filter(|referrer| typed(referrer.source))
                .map(|referrer| (referrer.source, referrer.field.to_string()))
                .collect();
            let created: Vec<_> = built
                .referrers(id)
                .map(|referrer| (referrer.source, referrer.field.to_string()))
                .collect();
            prop_assert_eq!(bulk, created);
        }
    }
}

fn quarantined(row: StoredObject) -> Error {
    let loaded = load(
        registry(),
        &types(false),
        [row.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(loaded.counts.quarantined, 1);
    let (_, held) = loaded.snapshot.held().next().unwrap();
    assert_eq!(held.kind(), HeldKind::Quarantined);
    assert_eq!(held.row(), &row);
    loaded.diagnostics.into_iter().next().unwrap().error
}

#[test]
fn a_panicking_migration_quarantines_the_object() {
    let error = quarantined(stored(0, 5, 666, None, 0));
    assert_eq!(error.code(), "doc.migration.failed");
    assert_eq!(error.details()["reason"], "panicked".into());
    assert_eq!(error.details()["step"], 1_i64.into());
}

#[test]
fn a_panicking_validator_quarantines_the_object() {
    let error = quarantined(stored(4, 5, 667, None, 0));
    assert_eq!(error.code(), "doc.load.validator_panicked");
    assert_eq!(error.arguments()["type"], "t.a".into());
}

#[test]
fn every_valid_header_column_of_a_quarantined_row_is_kept() {
    let owner = stored(6, 1, 0, None, 0);
    let mut held = stored(7, 2, 0, Some(1), 0);
    held.flags = 64;
    let loaded = load(
        registry(),
        &types(false),
        [owner, held.clone()],
        &LoadOptions::default(),
    )
    .unwrap();
    let (_, kept) = loaded.snapshot.held().next().unwrap();
    assert_eq!(kept.reason().arguments()["column"], "flags".into());
    let header = &kept.object().header;
    assert_eq!(header.owner, Some(ObjectId::from_u128(1)));
    assert_eq!(header.name, held.name);
    assert_eq!(header.partition, held.partition);
    // The owner stays a reference: deleting it is refused, naming the held object.
    let mut set = WorkingSet::new(&loaded.snapshot);
    set.delete(loaded.snapshot.resolve(ObjectId::from_u128(1)).unwrap())
        .unwrap();
    let error = apply_delete_rules(&mut set, &Limits::DEFAULT).unwrap_err();
    assert_eq!(error.code(), "doc.object.not_modifiable");
    assert_eq!(
        error.arguments()["id"],
        ObjectId::from_u128(2).to_string().into()
    );
    assert_eq!(error.details()["kind"], "quarantined".into());
}

/// Enabled by the change of `allows` in `integrity.rs` that accepts a live target without a
/// type key when the field allows any type (the load already treats it so).
#[test]
fn a_source_referring_to_a_held_object_without_a_type_key_stays_editable() {
    let mut target = stored(7, 2, 0, None, 0);
    target.type_key = "Not A Key".into();
    let source = stored(6, 1, 0, Some(2), 0);
    let source = StoredObject {
        owner: None,
        ..source
    };
    let loaded = load(
        registry(),
        &types(false),
        [target, source],
        &LoadOptions::default(),
    )
    .unwrap();
    assert_eq!(loaded.counts.dangling, 0);
    let snapshot = &loaded.snapshot;
    let handle = snapshot.resolve(ObjectId::from_u128(1)).unwrap();
    let mut set = WorkingSet::new(snapshot);
    let header = Header {
        name: Some("renamed".into()),
        ..snapshot.get(handle).unwrap().header.clone()
    };
    set.replace_header(handle, header).unwrap();
    check_references(&set).unwrap();
}

#[test]
fn a_type_key_beyond_string_bytes_quarantines_the_row() {
    let mut row = stored(7, 2, 0, None, 0);
    row.type_key = format!("x.{}", "u".repeat(40));
    let options = LoadOptions {
        limits: Limits {
            string_bytes: 16,
            ..Limits::DEFAULT
        },
        ..LoadOptions::default()
    };
    let loaded = load(registry(), &types(false), [row.clone()], &options).unwrap();
    let (_, held) = loaded.snapshot.held().next().unwrap();
    assert_eq!(held.kind(), HeldKind::Quarantined);
    assert_eq!(held.type_key(), None);
    assert_eq!(held.reason().arguments()["column"], "type_key".into());
    assert_eq!(held.row(), &row);
}

/// The measured live heap of one tiny held row (see `load_scale`), in bytes.
const MEASURED_HELD_BYTES: u64 = 1_900;
/// How far loading may exceed `memory_bytes` with tiny held rows: not at all.
const OVERRUN_FACTOR: u64 = 1;

#[test]
fn tiny_unknown_rows_fail_the_load_within_the_memory_budget() {
    let budget = 4 << 20;
    let options = LoadOptions {
        limits: Limits {
            memory_bytes: Some(budget),
            ..Limits::DEFAULT
        },
        ..LoadOptions::default()
    };
    let read = Cell::new(0_u64);
    let rows = (1..=1_000_000_u128).map(|n| {
        read.set(read.get() + 1);
        StoredObject {
            id: n.to_be_bytes().to_vec(),
            type_key: "x.u".into(),
            major: 1,
            payload: vec![0],
            ..StoredObject::default()
        }
    });
    let error = load(registry(), &[], rows, &options).unwrap_err();
    assert_eq!(error.code(), "doc.load.limit_exceeded");
    assert_eq!(error.arguments()["limit"], "memory_bytes".into());
    let accepted = read.get() - 1;
    assert!(
        accepted * MEASURED_HELD_BYTES <= OVERRUN_FACTOR * budget,
        "{accepted} rows of {MEASURED_HELD_BYTES} bytes exceed the budget of {budget} bytes"
    );
}
