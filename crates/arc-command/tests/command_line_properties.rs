//! Property tests of the command line and prompt sessions: arbitrary text never panics and
//! fails with a valid span, the canonical line reads back, and random answers end every prompt
//! session in validated parameters or a cancel.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::Limits;
use arc_base::error::Value as Detail;
use arc_command::{
    Feedback, Invocation, Point, PromptInput, PromptSession, format_line, parse_line,
};
use common::{document, registry, session};
use proptest::prelude::*;
use serde_json::{Value, json};

fn check_span(error: &arc_base::Error, len: usize) {
    let get = |name| match error.details().get(name) {
        Some(Detail::Int(n)) => Some(usize::try_from(*n).unwrap()),
        None => None,
        Some(other) => panic!("{other:?}"),
    };
    let (start, end) = (get("start"), get("end"));
    let spanned = error.code().starts_with("cmd.line.")
        || error.code() == "cmd.unknown"
        || error.code() == "cmd.name.ambiguous";
    if spanned {
        let (start, end) = (start.unwrap(), end.unwrap());
        assert!(start <= end && end <= len, "{error:?} in {len}");
    }
}

fn soup() -> impl Strategy<Value = String> {
    let token = prop_oneof![
        Just("test.item.create".to_owned()),
        Just("test.shape.draw".to_owned()),
        Just("test.item.rename".to_owned()),
        Just(" ".to_owned()),
        Just("=".to_owned()),
        Just("name=".to_owned()),
        Just("length=".to_owned()),
        Just("ids=".to_owned()),
        Just("\"".to_owned()),
        Just("\\".to_owned()),
        Just("[".to_owned()),
        Just("]".to_owned()),
        Just(",".to_owned()),
        Just("(".to_owned()),
        Just(")".to_owned()),
        Just("$".to_owned()),
        Just("{".to_owned()),
        Just("1e308".to_owned()),
        Just("-0".to_owned()),
        Just("mm".to_owned()),
        Just("é".to_owned()),
        Just("\t".to_owned()),
        "[a-z0-9]{1,6}",
    ];
    prop::collection::vec(token, 0..40).prop_map(|t| t.concat())
}

fn value_text() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        "[a-z][a-z0-9_.-]{0,8}",
        (-1e300..1e300_f64).prop_map(|v| v.to_string()),
        any::<i64>().prop_map(|v| v.to_string()),
        "[ -~]{0,10}"
            .prop_map(|s| { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }),
        "[0-9 +*a-z]{0,10}".prop_map(|s| format!("({s})")),
        Just("$null".to_owned()),
        Just("${\"a\":[1,2]}".to_owned()),
        Just("0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c".to_owned()),
    ];
    leaf.prop_recursive(3, 12, 4, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(|items| format!("[{}]", items.join(", ")))
    })
}

fn structured() -> impl Strategy<Value = String> {
    let command = prop_oneof![
        Just("test.item.create"),
        Just("test.item.rename"),
        Just("test.shape.draw"),
        Just("test.long.run"),
        Just("test.item.make"),
    ];
    let name = prop_oneof![
        Just(""),
        Just("name="),
        Just("length="),
        Just("ids="),
        Just("copies="),
        Just("steps="),
    ];
    (command, prop::collection::vec((name, value_text()), 0..4)).prop_map(|(c, args)| {
        let mut line = c.to_owned();
        for (n, v) in args {
            line.push(' ');
            line.push_str(n);
            line.push_str(&v);
        }
        line
    })
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        (-1e300..1e300_f64).prop_map(Value::from),
        "\\PC{0,12}".prop_map(Value::String),
        "[ \"\\\\,()\\[\\]$=\t\n]{0,6}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z]{1,4}", inner, 0..3)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

fn round_trip(line: &str) {
    let r = registry();
    let names = BTreeMap::new();
    if let Ok(Some(parsed)) = parse_line(&r, &names, line) {
        let invocation = Invocation::new(&parsed.command, Value::Object(parsed.params.clone()));
        let text = format_line(&r, &invocation).unwrap();
        let again = parse_line(&r, &names, &text).unwrap().unwrap();
        assert_eq!(again.command, parsed.command, "{text}");
        assert_eq!(again.params, parsed.params, "{line} -> {text}");
    }
}

/// A typed or UI answer.
#[derive(Debug, Clone)]
enum Action {
    Text(String),
    Input(PromptInput),
    Default,
    Back,
}

fn action() -> impl Strategy<Value = Action> {
    let text = prop_oneof![
        Just("0,0".to_owned()),
        Just("3,4,5".to_owned()),
        Just("c".to_owned()),
        Just("circle".to_owned()),
        Just("10 mm".to_owned()),
        Just("1200 mm + 3 in".to_owned()),
        Just("90 deg".to_owned()),
        Just("2".to_owned()),
        Just("11".to_owned()),
        Just("1e300 m".to_owned()),
        Just("name".to_owned()),
        Just("forbidden".to_owned()),
        Just("-5".to_owned()),
        Just("ITEM".to_owned()),
        "\\PC{0,20}",
    ];
    let input = prop_oneof![
        (
            -1e300..1e300_f64,
            -1e300..1e300_f64,
            prop::option::of(-1e3..1e3_f64)
        )
            .prop_map(|(x, y, z)| PromptInput::Point(Point { x, y, z })),
        (-1e3..1e3_f64).prop_map(PromptInput::Distance),
        (-10.0..10.0_f64).prop_map(PromptInput::Angle),
        Just(PromptInput::Distance(f64::NAN)),
        "\\PC{0,8}".prop_map(PromptInput::Text),
        "[a-z]{0,6}".prop_map(PromptInput::Keyword),
        Just(PromptInput::Expression("1 +".into())),
    ];
    prop_oneof![
        4 => text.prop_map(Action::Text),
        3 => input.prop_map(Action::Input),
        1 => Just(Action::Default),
        1 => Just(Action::Back),
    ]
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let text = String::from_utf8_lossy(&bytes);
        let r = registry();
        match parse_line(&r, &BTreeMap::new(), &text) {
            Ok(_) => {}
            Err(error) => check_span(&error, text.len()),
        }
        let _ = arc_command::complete(&r, &BTreeMap::new(), &text, bytes.len() / 2);
    }

    #[test]
    fn token_soups_never_panic_and_round_trip(text in soup()) {
        let r = registry();
        match parse_line(&r, &BTreeMap::new(), &text) {
            Ok(_) => round_trip(&text),
            Err(error) => check_span(&error, text.len()),
        }
        let _ = arc_command::complete(&r, &BTreeMap::new(), &text, text.len());
    }

    #[test]
    fn structured_lines_round_trip(text in structured()) {
        let r = registry();
        match parse_line(&r, &BTreeMap::new(), &text) {
            Ok(_) => round_trip(&text),
            Err(error) => check_span(&error, text.len()),
        }
    }

    #[test]
    fn deep_and_huge_lines_fail_cleanly(depth in 0..400_usize, open in prop_oneof![Just("("), Just("["), Just("$["), Just("\"")]) {
        let text = format!("test.item.create name=a length={}{}", open.repeat(depth), ")".repeat(depth / 2));
        let r = registry();
        if let Err(error) = parse_line(&r, &BTreeMap::new(), &text) {
            check_span(&error, text.len());
        }
        let huge = format!("test.item.create {}", "a".repeat(70_000 + depth));
        prop_assert_eq!(parse_line(&r, &BTreeMap::new(), &huge).unwrap_err().code(), "cmd.line.limit_exceeded");
    }

    #[test]
    fn formatted_parameters_read_back(name in json_value(), length in json_value(), ids in json_value(), steps in json_value()) {
        let r = registry();
        for (command, params) in [
            ("test.item.create", json!({"name": name, "length": length})),
            ("test.item.rename", json!({"ids": ids, "name": name})),
            ("test.long.run", json!({"steps": steps})),
        ] {
            let text = format_line(&r, &Invocation::new(command, params.clone())).unwrap();
            // Parameters the schema rejects fail as values; all others read back exactly.
            match parse_line(&r, &BTreeMap::new(), &text) {
                Ok(parsed) => prop_assert_eq!(Value::Object(parsed.unwrap().params), params, "{}", text),
                Err(error) => prop_assert_eq!(error.code(), "cmd.line.value", "{}", text),
            }
        }
    }

    #[test]
    fn random_answers_end_in_valid_parameters_or_a_cancel(
        command in prop_oneof![
            Just("test.shape.draw"),
            Just("test.item.create"),
            Just("test.item.rename"),
            Just("test.item.set_length"),
            Just("test.long.run"),
        ],
        actions in prop::collection::vec(action(), 0..30),
    ) {
        let r = registry();
        let doc = document();
        let s = session();
        let id = common::invoke(&r, &doc, &s, "test.item.create", json!({"name": "a", "length": 1.0}))
            .unwrap()
            .value["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let snapshot = doc.snapshot();
        let spec = Arc::clone(r.get(command).unwrap());
        let (mut prompts, mut feedback) =
            PromptSession::start(Arc::clone(&spec), serde_json::Map::new(), &s, &snapshot);
        let actions = actions.into_iter().map(|a| match a {
            Action::Text(t) if t == "ITEM" => Action::Text(id.clone()),
            other => other,
        });
        for action in actions {
            match &feedback {
                Feedback::Ask(p) | Feedback::Rejected { prompt: p, .. } => {
                    prop_assert!(!p.accepts.is_empty());
                    prop_assert_eq!(prompts.prompt(), Some(p));
                }
                _ => break,
            }
            feedback = match action {
                Action::Text(t) => prompts.answer_text(&t, &snapshot),
                Action::Input(i) => prompts.answer(i, &snapshot),
                Action::Default => prompts.accept_default(&snapshot),
                Action::Back => prompts.back(&snapshot),
            };
        }
        match feedback {
            Feedback::Done(invocation) => {
                prop_assert!(spec.validate(&invocation.params, &Limits::DEFAULT).unwrap().is_empty());
                prop_assert!(prompts.prompt().is_none());
            }
            Feedback::Ask(_) | Feedback::Rejected { .. } => {
                prop_assert!(matches!(prompts.cancel(), Feedback::Cancelled));
                prop_assert!(prompts.prompt().is_none());
            }
            Feedback::Cancelled => {}
            Feedback::Failed(error) => prop_assert!(false, "{:?}", error),
        }
    }
}
