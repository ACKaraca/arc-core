//! Deterministic run (CORE-CMD-005): the same command sequence with the same parameters and
//! seeds, run on two fresh documents, gives byte-identical results and object-for-object equal
//! documents.

#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use arc_doc::Document;
use common::{document, registry, session};
use proptest::prelude::*;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
enum Step {
    Create(String, f64, bool),
    SetLength(usize, String),
    Delete(usize),
    Summary,
    Random,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        ("\\PC{0,6}", 0.0..1e5_f64, any::<bool>()).prop_map(|(n, l, t)| Step::Create(n, l, t)),
        (
            any::<usize>(),
            prop::sample::select(vec!["2 m", "1200", "3'6\"", "1e9 km", "x"])
        )
            .prop_map(|(i, l)| Step::SetLength(i, l.to_owned())),
        any::<usize>().prop_map(Step::Delete),
        Just(Step::Summary),
        Just(Step::Random),
    ]
}

/// Runs `steps` on a fresh document; returns each outcome as text and the document.
fn run(steps: &[(Step, u64)]) -> (Vec<String>, Document) {
    let (r, s, doc) = (registry(), session(), document());
    let invoker = arc_command::Invoker::new(&r);
    let mut ids: Vec<String> = Vec::new();
    let mut outcomes = Vec::new();
    for (step, seed) in steps {
        let pick = |i: &usize| ids.get(i % ids.len().max(1)).cloned().unwrap_or_default();
        let (command, params) = match step {
            Step::Create(name, length, text) => {
                let length: Value = if *text {
                    json!(format!("{length} mm"))
                } else {
                    json!(length)
                };
                ("test.item.create", json!({"name": name, "length": length}))
            }
            Step::SetLength(i, length) => (
                "test.item.set_length",
                json!({"id": pick(i), "length": length}),
            ),
            Step::Delete(i) => ("test.item.delete", json!({"id": pick(i)})),
            Step::Summary => ("test.item.summary", json!({})),
            Step::Random => ("test.item.random", json!({})),
        };
        let mut call = arc_command::Invocation::new(command, params);
        call.seed = *seed;
        let text = match invoker.invoke(&doc, &s, call) {
            Ok(outcome) => {
                if let Some(id) = outcome.value.get("id").and_then(Value::as_str) {
                    ids.push(id.to_owned());
                }
                format!(
                    "ok {} {}",
                    outcome.revision,
                    serde_json::to_string(&outcome.value).unwrap()
                )
            }
            Err(error) => format!("err {}", error.code()),
        };
        outcomes.push(text);
    }
    (outcomes, doc)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn same_state_parameters_and_seeds_give_the_same_results_and_documents(
        steps in prop::collection::vec((step(), any::<u64>()), 1..12),
    ) {
        let (first, a) = run(&steps);
        let (second, b) = run(&steps);
        prop_assert_eq!(first, second);
        let (a, b) = (a.snapshot(), b.snapshot());
        prop_assert_eq!(a.revision(), b.revision());
        prop_assert_eq!(a.len(), b.len());
        for ((_, x), (_, y)) in a.iter().zip(b.iter()) {
            prop_assert_eq!(x.as_ref(), y.as_ref());
        }
    }
}

#[test]
fn different_seeds_give_different_identifiers() {
    let create = Step::Create("a".into(), 1.0, false);
    let (one, _) = run(&[(create.clone(), 1)]);
    let (two, _) = run(&[(create, 2)]);
    assert_ne!(one, two);
}
