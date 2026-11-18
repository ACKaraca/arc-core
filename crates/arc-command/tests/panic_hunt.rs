//! The panic hunt (docs/testing.md): every registered command runs with adversarial parameters
//! through `Invoker::invoke`. Every call returns a result or an error, no fault escapes (the
//! only `cmd.fault` errors come from the commands built to fault), and the document passes
//! validation afterward.

#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use arc_base::id::Identifier;
use arc_base::{Category, ObjectId};
use arc_command::{Registry, Session, parse_params};
use arc_doc::{Document, WorkingSet, check_references};
use common::{document, invoke, registry, session};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// A document with live items and the identifier of a deleted one.
fn seeded(r: &Registry, s: &Session) -> (Document, Vec<String>, String) {
    let doc = document();
    let mut ids = Vec::new();
    for n in 0..3 {
        let out = invoke(
            r,
            &doc,
            s,
            "test.item.create",
            json!({"name": n.to_string(), "length": n}),
        );
        ids.push(out.unwrap().value["id"].as_str().unwrap().to_owned());
    }
    let deleted = ids.pop().unwrap();
    invoke(r, &doc, s, "test.item.delete", json!({"id": deleted})).unwrap();
    (doc, ids, deleted)
}

fn adversarial_values(live: &str, deleted: &str) -> Vec<Value> {
    let mut deep = json!(1);
    for _ in 0..60 {
        deep = json!([deep]);
    }
    vec![
        Value::Null,
        json!(true),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(1e300),
        json!(-1e300),
        json!(5e-324),
        json!(u64::MAX),
        json!(i64::MIN),
        json!(""),
        json!("x".repeat(1 << 20)),
        json!("NaN"),
        json!("inf"),
        json!("-0"),
        json!("1e999 mm"),
        json!("1e300 km * 1e300"),
        json!("3'6\""),
        json!("((((((((((1))))))))))"),
        json!(ObjectId::from_u128(42).to_string()),
        json!(live),
        json!(deleted),
        json!([]),
        json!({}),
        json!({"nested": {"id": live}}),
        deep,
    ]
}

/// The property names of a command's parameter schema.
fn properties(schema: &Value) -> Vec<String> {
    schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

fn check(r: &Registry, command: &str, outcome: &arc_base::Result<arc_command::Outcome>) {
    if let Err(error) = outcome
        && !command.starts_with("test.fault.")
    {
        assert_ne!(error.category(), Category::Internal, "{command}: {error:?}");
    }
    assert!(r.get(command).is_some());
}

fn assert_valid(doc: &Document) {
    check_references(&WorkingSet::new(&doc.snapshot())).unwrap();
}

#[test]
fn every_command_survives_schema_directed_adversarial_parameters() {
    let (r, s) = (registry(), session());
    let (doc, live, deleted) = seeded(&r, &s);
    let values = adversarial_values(&live[0], &deleted);
    let mut calls = 0;
    for spec in r.iter() {
        let id = spec.id().to_owned();
        let base: Map<String, Value> = spec
            .examples()
            .first()
            .and_then(|e| e.as_object().cloned())
            .unwrap_or_default();
        let mut sets: Vec<Value> = values.clone();
        for name in properties(spec.params_schema()) {
            for value in &values {
                let mut params = base.clone();
                params.insert(name.clone(), value.clone());
                sets.push(Value::Object(params.clone()));
                params.insert("unknown".into(), value.clone());
                sets.push(Value::Object(params));
            }
            let mut without = base.clone();
            without.remove(&name);
            sets.push(Value::Object(without));
        }
        for params in sets {
            let outcome = invoke(&r, &doc, &s, &id, params);
            check(&r, &id, &outcome);
            calls += 1;
        }
    }
    assert!(calls > 500);
    // Deep and huge texts stop at the parser.
    for text in [
        "[".repeat(100_000),
        format!("{{\"a\":\"{}\"}}", "y".repeat(1 << 16)),
    ] {
        let parsed = parse_params(&text, &arc_base::Limits::STRICT);
        if let Ok(params) = parsed {
            check(
                &r,
                "test.item.create",
                &invoke(&r, &doc, &s, "test.item.create", params),
            );
        }
    }
    assert_valid(&doc);
}

fn json_value() -> impl Strategy<Value = Value> {
    let key = prop_oneof![
        Just("name".to_owned()),
        Just("length".to_owned()),
        Just("id".to_owned()),
        Just("steps".to_owned()),
        Just("boom".to_owned()),
        "\\PC{0,4}",
    ];
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |v| v.is_finite())
            .prop_map(Value::from),
        "\\PC{0,12}".prop_map(Value::from),
        prop::sample::select(vec![
            "2400 mm",
            "NaN",
            "-inf",
            "1e308 m * 10",
            "3'6\"",
            "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c"
        ])
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 32, 4, move |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(key.clone(), inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn every_command_survives_random_parameters(
        command in 0_usize..64,
        params in json_value(),
        seed in any::<u64>(),
    ) {
        let (r, s) = (registry(), session());
        let (doc, _, _) = seeded(&r, &s);
        let ids: Vec<String> = r.iter().map(|spec| spec.id().to_owned()).collect();
        let id = &ids[command % ids.len()];
        let mut call = arc_command::Invocation::new(id.clone(), params);
        call.seed = seed;
        let outcome = arc_command::Invoker::new(&r).invoke(&doc, &s, call);
        check(&r, id, &outcome);
        assert_valid(&doc);
    }
}
