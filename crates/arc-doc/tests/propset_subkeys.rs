//! Property sets (CORE-DOC-005) and sub-object keys (CORE-DOC-006).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::graph::Evaluators;
use arc_doc::propset::{self, DEFINITION_TYPE, INSTANCE_TYPE};
use arc_doc::subkey::{self, SubKey, keyed_list};
use arc_doc::{
    Document, DocumentConfig, FieldDecl, FieldPath, Header, Schema, SchemaVersion, Snapshot,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType, compare,
};
use proptest::prelude::*;

const THING: &str = "test.thing";
const POLY: &str = "test.poly";

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    propset::register(&mut registry).unwrap();
    let thing = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new(THING).unwrap(), thing))
        .unwrap();
    let poly = Schema::new(
        SchemaVersion::new(1, 0),
        vec![keyed_list("pts", vec![FieldDecl::new("x", ValueType::F64)])],
    );
    registry
        .register(TypeDecl::new(TypeKey::new(POLY).unwrap(), poly))
        .unwrap();
    Arc::new(registry)
}

fn document() -> Document {
    let config = DocumentConfig {
        workers: 0,
        ..DocumentConfig::default()
    };
    let doc = Document::open(
        Snapshot::empty(registry()),
        Arc::new(Evaluators::new()),
        config,
    )
    .unwrap();
    doc.add_validator(propset::validator(Limits::DEFAULT));
    doc
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn thing(doc: &Document, n: u128) {
    doc.submit(move |txn| {
        txn.create(THING, id(n), Header::default(), vec![Some(Value::I64(0))])
            .map(drop)
    })
    .wait()
    .unwrap();
}

fn props(fields: Vec<FieldDecl>) -> Vec<FieldDecl> {
    fields
}

fn values(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

const DEF: u128 = 100;
const INST: u128 = 200;

fn define_basic(doc: &Document) {
    doc.submit(|txn| {
        propset::define(
            txn,
            id(DEF),
            "Cost",
            &[],
            &props(vec![
                FieldDecl::new("price", ValueType::F64).with_default(Value::F64(1.0)),
                FieldDecl::new("code", ValueType::String).optional(),
            ]),
        )
    })
    .wait()
    .unwrap();
}

#[test]
fn definitions_and_instances_are_typed_and_read_back() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[("code", Value::String("A1".into()))]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let snap = doc.snapshot();
    let def = propset::definition(&snap, id(DEF), &Limits::DEFAULT).unwrap();
    assert_eq!(def.name.as_deref(), Some("Cost"));
    assert_eq!(def.properties.len(), 2);
    let inst = propset::instance(&snap, id(INST), &Limits::DEFAULT).unwrap();
    assert_eq!(inst.values["price"], Value::F64(1.0), "default applied");
    assert_eq!(inst.values["code"], Value::String("A1".into()));
    assert_eq!(propset::instance_of(&snap, id(1), id(DEF)), Some(id(INST)));
    assert_eq!(propset::instances_of(&snap, id(1)), vec![id(INST)]);
    // A second instance of the same definition on the target is refused and changes nothing.
    let err = doc
        .submit(|txn| propset::attach(txn, id(201), id(1), id(DEF), &values(&[]), &Limits::DEFAULT))
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.propset.duplicate");
    // Unknown properties and wrongly typed values are refused.
    thing(&doc, 2);
    let err = doc
        .submit(|txn| {
            propset::attach(
                txn,
                id(202),
                id(2),
                id(DEF),
                &values(&[("nope", Value::Bool(true))]),
                &Limits::DEFAULT,
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.propset.invalid");
    let err = doc
        .submit(|txn| {
            propset::attach(
                txn,
                id(202),
                id(2),
                id(DEF),
                &values(&[("price", Value::String("x".into()))]),
                &Limits::DEFAULT,
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.value.type_mismatch");
    assert_eq!(doc.snapshot().iter_type(INSTANCE_TYPE).unwrap().count(), 1);
}

#[test]
fn applicable_types_and_forbidden_property_types_are_enforced() {
    let doc = document();
    thing(&doc, 1);
    let err = doc
        .submit(|txn| {
            propset::define(
                txn,
                id(DEF),
                "Bad",
                &[],
                &[FieldDecl::new("blob", ValueType::Opaque)],
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.propset.invalid");
    doc.submit(|txn| {
        propset::define(
            txn,
            id(DEF),
            "Poly only",
            &[TypeKey::new(POLY).unwrap()],
            &[FieldDecl::new("a", ValueType::Bool).optional()],
        )
    })
    .wait()
    .unwrap();
    let err = doc
        .submit(|txn| {
            propset::attach(
                txn,
                id(INST),
                id(1),
                id(DEF),
                &values(&[]),
                &Limits::DEFAULT,
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.propset.invalid");
}

#[test]
fn deleting_the_target_deletes_instances_and_a_used_definition_cannot_be_deleted() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let err = doc.submit(|txn| txn.delete(id(DEF))).wait().unwrap_err();
    assert_eq!(err.code(), "doc.reference.delete_blocked");
    doc.submit(|txn| txn.delete(id(1))).wait().unwrap();
    assert!(doc.snapshot().resolve(id(INST)).is_err());
    assert!(doc.undo().wait().unwrap().value);
    assert!(doc.snapshot().resolve(id(INST)).is_ok());
}

#[test]
fn editing_a_definition_migrates_instances_in_one_transaction() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[
                ("price", Value::F64(9.5)),
                ("code", Value::String("A1".into())),
            ]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let before = doc.snapshot();
    // `price` is renamed to `cost`, `code` changes type (its value no longer fits) and a
    // required property with a default is added.
    doc.submit(|txn| {
        propset::redefine(
            txn,
            id(DEF),
            &[],
            &[
                FieldDecl::new("cost", ValueType::F64),
                FieldDecl::new("code", ValueType::I64).optional(),
                FieldDecl::new("tier", ValueType::I64).with_default(Value::I64(3)),
            ],
            &[("price", "cost")],
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let after = doc.snapshot();
    let inst = propset::instance(&after, id(INST), &Limits::DEFAULT).unwrap();
    assert_eq!(
        inst.values,
        values(&[("cost", Value::F64(9.5)), ("tier", Value::I64(3))])
    );
    // Definition and instance changed in one revision, and one undo restores both.
    assert_eq!(after.revision(), before.revision() + 1);
    assert!(doc.undo().wait().unwrap().value);
    assert!(compare(&before, &doc.snapshot()).is_empty());
    // A migration that cannot give a required property a value fails and changes nothing.
    let err = doc
        .submit(|txn| {
            propset::redefine(
                txn,
                id(DEF),
                &[],
                &[FieldDecl::new("new_required", ValueType::I64)],
                &[],
                &Limits::DEFAULT,
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.value.missing");
    assert!(compare(&before, &doc.snapshot()).is_empty());
}

#[test]
fn the_validator_rejects_a_definition_edit_that_skips_migration() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let text = match &doc
        .snapshot()
        .get(doc.snapshot().resolve(id(DEF)).unwrap())
        .unwrap()
        .values[1]
    {
        Some(Value::String(text)) => text.replace("f64", "bool"),
        other => panic!("{other:?}"),
    };
    let err = doc
        .submit(move |txn| {
            txn.modify(
                id(DEF),
                FieldPath::new("properties"),
                Some(Value::String(text)),
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.propset.invalid", "{err:?}");
    assert_eq!(
        propset::instance(&doc.snapshot(), id(INST), &Limits::DEFAULT)
            .unwrap()
            .values["price"],
        Value::F64(1.0)
    );
    let _ = DEFINITION_TYPE;
}

#[test]
fn set_values_and_detach() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[]),
            &Limits::DEFAULT,
        )?;
        propset::set_values(
            txn,
            id(INST),
            &BTreeMap::from([
                ("price".to_owned(), Some(Value::F64(2.0))),
                ("code".to_owned(), Some(Value::String("z".into()))),
            ]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let inst = propset::instance(&doc.snapshot(), id(INST), &Limits::DEFAULT).unwrap();
    assert_eq!(inst.values["price"], Value::F64(2.0));
    doc.submit(|txn| propset::detach(txn, id(1), id(DEF)))
        .wait()
        .unwrap();
    assert!(propset::instances_of(&doc.snapshot(), id(1)).is_empty());
}

#[derive(Debug, Clone, Copy)]
enum Fate {
    Keep,
    ToString,
    Remove,
}

proptest! {
    /// Migration keeps every instance value that still fits its property and only those.
    #[test]
    fn migration_keeps_the_values_that_still_fit(
        cells in prop::collection::vec((any::<i64>(), prop_oneof![Just(Fate::Keep), Just(Fate::ToString), Just(Fate::Remove)]), 1..6),
    ) {
        let doc = document();
        thing(&doc, 1);
        let old: Vec<FieldDecl> = (0..cells.len())
            .map(|i| FieldDecl::new(format!("p{i}"), ValueType::I64))
            .collect();
        let old2 = old.clone();
        doc.submit(move |txn| propset::define(txn, id(DEF), "S", &[], &old2)).wait().unwrap();
        let given: BTreeMap<String, Value> = cells
            .iter()
            .enumerate()
            .map(|(i, (v, _))| (format!("p{i}"), Value::I64(*v)))
            .collect();
        let g = given.clone();
        doc.submit(move |txn| propset::attach(txn, id(INST), id(1), id(DEF), &g, &Limits::DEFAULT)).wait().unwrap();
        let mut new = Vec::new();
        let mut expected = BTreeMap::new();
        for (i, (v, fate)) in cells.iter().enumerate() {
            match fate {
                Fate::Keep => {
                    new.push(FieldDecl::new(format!("p{i}"), ValueType::I64));
                    expected.insert(format!("p{i}"), Value::I64(*v));
                }
                Fate::ToString => {
                    new.push(FieldDecl::new(format!("p{i}"), ValueType::String).with_default(Value::String("d".into())));
                    expected.insert(format!("p{i}"), Value::String("d".into()));
                }
                Fate::Remove => {}
            }
        }
        new.push(FieldDecl::new("extra", ValueType::Bool).optional());
        doc.submit(move |txn| propset::redefine(txn, id(DEF), &[], &new, &[], &Limits::DEFAULT)).wait().unwrap();
        let inst = propset::instance(&doc.snapshot(), id(INST), &Limits::DEFAULT).unwrap();
        prop_assert_eq!(inst.values, expected);
    }
}

// ---- sub-object keys ------------------------------------------------------------------

fn pts(doc: &Document, obj: ObjectId) -> Vec<(u32, f64)> {
    let snap = doc.snapshot();
    subkey::parts(&snap, obj, "pts")
        .unwrap()
        .into_iter()
        .map(|k| {
            let path = subkey::path_of(&snap, k, "pts").unwrap();
            let handle = snap.resolve(obj).unwrap();
            let Some(Value::List(list)) = &snap.get(handle).unwrap().values[0] else {
                panic!()
            };
            let segs = path.segments();
            let arc_doc::PathSegment::Index(i) = segs[1] else {
                panic!()
            };
            let Value::Record(rec) = &list[usize::try_from(i).unwrap()] else {
                panic!()
            };
            let Some(Value::F64(x)) = rec[1] else {
                panic!()
            };
            (k.key, x)
        })
        .collect()
}

#[test]
fn keys_are_assigned_resolved_and_survive_edits() {
    let doc = document();
    let obj = id(5);
    doc.submit(move |txn| {
        txn.create(
            POLY,
            obj,
            Header::default(),
            vec![Some(Value::List(vec![]))],
        )
        .map(drop)
    })
    .wait()
    .unwrap();
    let keys = doc
        .submit(move |txn| {
            let a = subkey::insert(txn, obj, "pts", vec![Some(Value::F64(1.0))], None)?;
            let b = subkey::insert(txn, obj, "pts", vec![Some(Value::F64(2.0))], None)?;
            // Inserted at the front: the others keep their keys, not their indices.
            let c = subkey::insert(txn, obj, "pts", vec![Some(Value::F64(3.0))], Some(0))?;
            Ok((a, b, c))
        })
        .wait()
        .unwrap()
        .value;
    assert_eq!(
        keys.0,
        SubKey {
            object: obj,
            key: 0
        }
    );
    assert_eq!(pts(&doc, obj), vec![(2, 3.0), (0, 1.0), (1, 2.0)]);
    assert_eq!(
        subkey::path_of(&doc.snapshot(), keys.1, "pts")
            .unwrap()
            .to_string(),
        "pts[2]"
    );
    doc.submit(move |txn| {
        subkey::set(txn, keys.0, "pts", "x", Some(Value::F64(10.0)))?;
        subkey::remove(txn, keys.2, "pts")
    })
    .wait()
    .unwrap();
    assert_eq!(pts(&doc, obj), vec![(0, 10.0), (1, 2.0)]);
    let err = subkey::path_of(&doc.snapshot(), keys.2, "pts").unwrap_err();
    assert_eq!(err.code(), "doc.part.not_found");
    let err = subkey::parts(&doc.snapshot(), obj, "nope").unwrap_err();
    assert_eq!(err.code(), "doc.part.invalid");
    // Undo restores the part with its key.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(pts(&doc, obj), vec![(2, 3.0), (0, 1.0), (1, 2.0)]);
}

#[derive(Debug, Clone)]
enum PartOp {
    Insert(usize, f64),
    Remove(usize),
    Set(usize, f64),
    Reverse,
}

fn part_op() -> impl Strategy<Value = PartOp> {
    prop_oneof![
        (0..8_usize, -5.0..5.0_f64).prop_map(|(a, b)| PartOp::Insert(a, b)),
        (0..8_usize).prop_map(PartOp::Remove),
        (0..8_usize, -5.0..5.0_f64).prop_map(|(a, b)| PartOp::Set(a, b)),
        Just(PartOp::Reverse),
    ]
}

proptest! {
    /// Against a model list of `(key, x)`: surviving parts keep their keys and values through
    /// inserts, removals, edits and reordering, and keys are never repeated.
    #[test]
    fn keys_survive_unrelated_edits(ops in prop::collection::vec(part_op(), 1..25)) {
        let doc = document();
        let obj = id(5);
        doc.submit(move |txn| {
            txn.create(POLY, obj, Header::default(), vec![Some(Value::List(vec![]))]).map(drop)
        }).wait().unwrap();
        let mut model: Vec<(u32, f64)> = Vec::new();
        for op in ops {
            match op {
                PartOp::Insert(at, x) => {
                    let key = doc.submit(move |txn| {
                        subkey::insert(txn, obj, "pts", vec![Some(Value::F64(x))], Some(at))
                    }).wait().unwrap().value;
                    prop_assert!(model.iter().all(|(k, _)| *k != key.key));
                    model.insert(at.min(model.len()), (key.key, x));
                }
                PartOp::Remove(i) => {
                    if model.is_empty() { continue; }
                    let (k, _) = model.remove(i % model.len());
                    doc.submit(move |txn| subkey::remove(txn, SubKey { object: obj, key: k }, "pts")).wait().unwrap();
                }
                PartOp::Set(i, x) => {
                    if model.is_empty() { continue; }
                    let n = i % model.len();
                    model[n].1 = x;
                    let k = model[n].0;
                    doc.submit(move |txn| subkey::set(txn, SubKey { object: obj, key: k }, "pts", "x", Some(Value::F64(x)))).wait().unwrap();
                }
                PartOp::Reverse => {
                    model.reverse();
                    let snap = doc.snapshot();
                    let handle = snap.resolve(obj).unwrap();
                    let Some(Value::List(list)) = snap.get(handle).unwrap().values[0].clone() else { panic!() };
                    let reversed: Vec<Value> = list.into_iter().rev().collect();
                    doc.submit(move |txn| txn.modify(obj, FieldPath::new("pts"), Some(Value::List(reversed)))).wait().unwrap();
                }
            }
            prop_assert_eq!(pts(&doc, obj), model.clone());
        }
    }
}

#[test]
fn comparison_reports_instance_values_by_property() {
    let doc = document();
    thing(&doc, 1);
    define_basic(&doc);
    doc.submit(|txn| {
        propset::attach(
            txn,
            id(INST),
            id(1),
            id(DEF),
            &values(&[]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let before = doc.snapshot();
    doc.submit(|txn| {
        propset::set_values(
            txn,
            id(INST),
            &BTreeMap::from([("price".to_owned(), Some(Value::F64(4.0)))]),
            &Limits::DEFAULT,
        )
    })
    .wait()
    .unwrap();
    let cmp = compare(&before, &doc.snapshot());
    assert_eq!(cmp.objects.len(), 1);
    let change = &cmp.objects[0].changes;
    assert_eq!(change.len(), 1);
    assert_eq!(change[0].path, "values[\"price\"]");
    assert_eq!(change[0].before, Some(Value::F64(1.0)));
    assert_eq!(change[0].after, Some(Value::F64(4.0)));
}
