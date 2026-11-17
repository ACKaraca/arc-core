//! Model-based and hostile-input tests of loading: a random valid document survives encoding to
//! rows and loading, object for object, with a reverse reference index equal to one built by a
//! working set; random and mutated rows never panic, every row ends up somewhere, and held and
//! unusable rows are written back unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::relationship::references;
use arc_doc::{
    Cardinality, DiagnosticKind, FieldDecl, Flags, Header, LoadOptions, Object, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Snapshot, StoredObject, StoredType,
    Strength, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet, load,
};
use proptest::prelude::*;

const POOL: u128 = 16;

fn reference(cardinality: Cardinality) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: Vec::new(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality,
        },
    })
}

fn schema(key: &str) -> Schema {
    let fields = if key == "test.m" {
        vec![
            FieldDecl::new("a", reference(Cardinality::Optional)).optional(),
            FieldDecl::new(
                "l",
                ValueType::List(Box::new(reference(Cardinality::Many { max: 4 }))),
            ),
            FieldDecl::new("n", ValueType::I64),
        ]
    } else {
        let record = ValueType::Record(vec![
            FieldDecl::new("y", reference(Cardinality::Optional)).optional(),
        ]);
        vec![FieldDecl::new("b", ValueType::List(Box::new(record)))]
    };
    Schema::new(SchemaVersion::new(1, 0), fields)
}

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for key in ["test.m", "test.o"] {
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema(key)))
            .unwrap();
    }
    Arc::new(registry)
}

fn types() -> Vec<StoredType> {
    ["test.m", "test.o"]
        .into_iter()
        .map(|key| StoredType {
            type_key: key.into(),
            major: 1,
            minor: 0,
            descriptor: String::new(),
        })
        .collect()
}

fn target() -> impl Strategy<Value = ObjectId> {
    (1..=POOL + 2).prop_map(ObjectId::from_u128)
}

fn values(key: &'static str) -> BoxedStrategy<Vec<Option<Value>>> {
    let reference = || target().prop_map(Value::Reference);
    if key == "test.m" {
        (
            prop::option::of(reference()),
            prop::collection::vec(reference(), 0..4),
            any::<i64>(),
        )
            .prop_map(|(a, l, n)| vec![a, Some(Value::List(l)), Some(Value::I64(n))])
            .boxed()
    } else {
        let record = prop::option::of(reference()).prop_map(|y| Value::Record(vec![y]));
        prop::collection::vec(record, 0..3)
            .prop_map(|b| vec![Some(Value::List(b))])
            .boxed()
    }
}

/// A valid object of a random type: type key and object.
fn object(id: ObjectId) -> impl Strategy<Value = (&'static str, Object)> {
    let header = || {
        (
            prop::option::of(target()),
            prop::option::of("[a-z]{0,4}"),
            any::<[bool; 3]>(),
        )
            .prop_map::<Header, _>(|(owner, name, [hidden_from_ui, system, read_only])| {
                Header {
                    owner,
                    name,
                    partition: None,
                    flags: Flags {
                        hidden_from_ui,
                        system,
                        read_only,
                    },
                }
            })
    };
    prop_oneof![Just("test.m"), Just("test.o")]
        .prop_flat_map(move |key| (Just(key), header(), values(key)))
        .prop_map(move |(key, header, values)| {
            (
                key,
                Object::new(id, SchemaVersion::new(1, 0), header, values),
            )
        })
}

fn document() -> impl Strategy<Value = Vec<(&'static str, Object)>> {
    prop::collection::btree_set(1..=POOL, 0..POOL as usize).prop_flat_map(|ids| {
        ids.into_iter()
            .map(|id| object(ObjectId::from_u128(id)))
            .collect::<Vec<_>>()
    })
}

fn row((key, object): &(&'static str, Object)) -> StoredObject {
    StoredObject::of(
        &TypeKey::new(*key).unwrap(),
        &schema(key),
        object,
        &Limits::DEFAULT,
    )
    .unwrap()
}

fn index_of(snapshot: &Snapshot) -> BTreeMap<ObjectId, Vec<String>> {
    (0..=POOL + 3)
        .map(ObjectId::from_u128)
        .map(|target| {
            let referrers = snapshot
                .referrers(target)
                .map(|referrer| format!("{} {}", referrer.source, referrer.field))
                .collect();
            (target, referrers)
        })
        .collect()
}

proptest! {
    #[test]
    fn a_valid_document_survives_rows_and_loading(document in document()) {
        let registry = registry();
        let rows: Vec<StoredObject> = document.iter().map(row).collect();
        let loaded = load(Arc::clone(&registry), &types(), rows, &LoadOptions::default()).unwrap();
        let snapshot = &loaded.snapshot;
        prop_assert_eq!(snapshot.len(), document.len());
        prop_assert_eq!(snapshot.held_len(), 0);
        prop_assert!(loaded.rows.is_empty());
        prop_assert!(snapshot.read_only().is_empty());
        let live: BTreeSet<ObjectId> = document.iter().map(|(_, object)| object.id).collect();
        let mut dangling = 0;
        let mut rebuilt = WorkingSet::new(&Snapshot::empty(Arc::clone(&registry)));
        for (key, object) in &document {
            let handle = snapshot.resolve(object.id).unwrap();
            prop_assert_eq!(&**snapshot.get(handle).unwrap(), object);
            prop_assert_eq!(snapshot.type_key(handle).unwrap().as_str(), *key);
            dangling += references(&registry, key, object)
                .iter()
                .filter(|reference| !live.contains(&reference.target))
                .count();
            rebuilt.create(key, object.clone()).unwrap();
        }
        prop_assert_eq!(loaded.counts.dangling, dangling as u64);
        prop_assert!(loaded.diagnostics.iter().all(|d| d.kind == DiagnosticKind::Dangling));
        prop_assert_eq!(snapshot.reference_count(), rebuilt.current().reference_count());
        prop_assert_eq!(index_of(snapshot), index_of(rebuilt.current()));
    }

    #[test]
    fn hostile_rows_never_panic_and_every_row_is_kept(
        valid in document(),
        hostile in prop::collection::vec(hostile_row(), 0..24),
        mutations in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), any::<u8>()), 0..6),
        extra_types in prop::collection::vec((type_key(), small(), small()), 0..3),
    ) {
        let mut rows: Vec<StoredObject> = valid.iter().map(row).collect();
        // Mutate payload bytes of valid rows, and duplicate some rows.
        for (at, position, byte) in mutations {
            if rows.is_empty() {
                break;
            }
            let index = at.index(rows.len());
            let mut copy = rows[index].clone();
            let at = usize::from(position) % copy.payload.len().max(1);
            if let Some(slot) = copy.payload.get_mut(at) {
                *slot ^= byte | 1;
            }
            if byte % 2 == 0 { rows.push(copy) } else { rows[index] = copy }
        }
        rows.extend(hostile);
        let mut types = types();
        types.extend(extra_types.into_iter().map(|(type_key, major, minor)| StoredType {
            type_key,
            major,
            minor,
            descriptor: String::new(),
        }));
        let options = LoadOptions {
            limits: Limits {
                record_count: 48,
                array_length: 64,
                string_bytes: 64,
                memory_bytes: Some(1 << 20),
                ..Limits::DEFAULT
            },
            ..LoadOptions::default()
        };
        match load(registry(), &types, rows.clone(), &options) {
            Err(error) => prop_assert!(
                matches!(error.code(), "doc.load.limit_exceeded" | "doc.load.newer_major"),
                "{}", error.code()
            ),
            Ok(loaded) => {
                let counts = loaded.counts;
                let snapshot = &loaded.snapshot;
                prop_assert_eq!(counts.rows, rows.len() as u64);
                prop_assert_eq!(
                    counts.objects + counts.preserved + counts.quarantined + counts.unaddressable,
                    counts.rows
                );
                prop_assert_eq!(snapshot.len() as u64, counts.objects);
                prop_assert_eq!(snapshot.held_len() as u64, counts.preserved + counts.quarantined);
                // Held and unusable rows are input rows, unchanged.
                for (_, held) in snapshot.held() {
                    prop_assert!(rows.contains(held.row()));
                }
                for kept in loaded.rows.unaddressable() {
                    prop_assert!(rows.contains(kept));
                }
                for (handle, object) in snapshot.iter() {
                    let key = snapshot.type_key(handle).unwrap().as_str();
                    prop_assert!(arc_doc::schema::check_values(&schema(key).fields, &object.values).is_ok());
                }
            }
        }
    }
}

fn small() -> impl Strategy<Value = i64> {
    prop_oneof![0..3_i64, any::<i64>()]
}

fn type_key() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("test.m".to_owned()),
        Just("test.o".to_owned()),
        Just("other.x".to_owned()),
        "\\PC{0,8}",
    ]
}

fn id_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        (0..=POOL + 2).prop_map(|n| n.to_be_bytes().to_vec()),
        prop::collection::vec(any::<u8>(), 0..20),
    ]
}

fn payload() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..48),
        // A huge list count after an absent first field.
        prop::collection::vec(any::<u8>(), 0..8).prop_map(|tail| {
            let mut bytes = vec![0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
            bytes.extend(tail);
            bytes
        }),
    ]
}

fn hostile_row() -> impl Strategy<Value = StoredObject> {
    (
        id_bytes(),
        type_key(),
        (small(), small()),
        prop::option::of(id_bytes()),
        prop::option::of("\\PC{0,80}"),
        prop_oneof![0..8_i64, any::<i64>()],
        prop_oneof![Just(0_i64), Just(1), any::<i64>()],
        payload(),
    )
        .prop_map(
            |(id, type_key, (major, minor), owner, name, flags, encoding, payload)| StoredObject {
                id,
                type_key,
                major,
                minor,
                owner,
                name,
                partition: None,
                flags,
                encoding,
                payload,
                payload_hash: Vec::new(),
                last_txn: Vec::new(),
            },
        )
}
