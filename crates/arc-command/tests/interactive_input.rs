//! Transparent commands, completion of object names and units, and macro inputs
//! (CORE-CMD-012, CORE-CMD-014, CORE-CMD-017).

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;

use arc_base::{Dimension, Limits, Quantity};
use arc_command::macros::{MacroStep, Recorder, parse_macro, play, play_with};
use arc_command::{
    CandidateKind, CommandLine, CommandSpec, Invocation, Permission, Registry, Submitted, Target,
    UndoBehavior, complete_in,
};
use arc_doc::{Document, Header, Value as Field};
use common::{ITEM, Nothing, document, registry, session};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Named {
    name: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Zoom {
    factor: f64,
}

fn item(ctx: &mut arc_command::Context<'_>, name: &str) -> arc_base::Result<()> {
    let id = ctx.new_id();
    ctx.txn()?
        .create(
            ITEM,
            id,
            Header {
                name: Some(name.to_owned()),
                ..Header::default()
            },
            vec![
                Some(Field::Quantity(Quantity::new(1.0, Dimension::LENGTH))),
                Some(Field::String(name.to_owned())),
            ],
        )
        .map(|_| ())
}

/// The sample registry plus named items and transparent commands.
fn registry_with_views() -> Registry {
    let mut r = registry();
    r.register(CommandSpec::new(
        "test.item.named",
        "test-named",
        "test-named-description",
        |ctx, p: Named| item(ctx, &p.name),
    ))
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.view.zoom",
            "test-zoom",
            "test-zoom-description",
            |_, p: Zoom| Ok(p.factor),
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
        .transparent(),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.view.mark",
            "test-mark",
            "test-mark-description",
            |ctx, _: Nothing| item(ctx, "mark"),
        )
        .transparent(),
    )
    .unwrap();
    r
}

struct Env {
    registry: Registry,
    document: Document,
    session: arc_command::Session,
    localized: BTreeMap<String, String>,
}

impl Env {
    fn new() -> Self {
        Self {
            registry: registry_with_views(),
            document: document(),
            session: session(),
            localized: BTreeMap::new(),
        }
    }

    fn target(&self) -> Target<'_> {
        Target {
            registry: &self.registry,
            document: &self.document,
            session: &self.session,
            selection: &[],
            localized: &self.localized,
            seed: 1,
        }
    }

    fn run(&self, command: &str, params: Value) -> arc_base::Result<arc_command::Outcome> {
        common::invoke(
            &self.registry,
            &self.document,
            &self.session,
            command,
            params,
        )
    }

    fn count(&self) -> usize {
        self.document.snapshot().iter().count()
    }
}

#[test]
fn a_transparent_command_runs_during_a_prompt_without_cancelling_it() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    let Submitted::Prompt(first) = line.submit(&t, "test.shape.draw").unwrap() else {
        panic!("expected a prompt");
    };
    // The method and the prefixed line run the command; the prompt stays where it was.
    for typed in ["'test.view.zoom 2", "'test.view.zoom factor=3"] {
        let Submitted::Ran(outcome) = line.submit(&t, typed).unwrap() else {
            panic!("expected a run");
        };
        assert_eq!(outcome.command, "test.view.zoom");
    }
    let Submitted::Ran(outcome) = line.transparent(&t, "test.view.zoom 4").unwrap() else {
        panic!("expected a run");
    };
    assert_eq!(outcome.value, json!(4.0));
    assert_eq!(line.active().unwrap().prompt().unwrap().key, first.key);
    // A transparent command is not the command repeat would run.
    assert!(line.last().is_none());
    // The prompt still takes its answers and finishes.
    line.submit(&t, "0,0").unwrap();
    line.submit(&t, "3,4").unwrap();
    line.submit(&t, "").unwrap();
    assert!(matches!(line.submit(&t, "").unwrap(), Submitted::Ran(_)));
}

#[test]
fn only_transparent_commands_with_their_parameters_run_beside_a_prompt() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    let reason = |error: arc_base::Error| {
        assert_eq!(error.code(), "cmd.transparent.refused");
        format!("{:?}", error.details().get("reason"))
    };
    let refused = line
        .transparent(&t, "test.item.create name=a length=1")
        .unwrap_err();
    assert!(reason(refused).contains("not_transparent"));
    let refused = line.transparent(&t, "test.view.zoom").unwrap_err();
    assert!(reason(refused).contains("missing_params"));
    let refused = line.transparent(&t, "  ").unwrap_err();
    assert!(reason(refused).contains("empty"));
    assert_eq!(env.count(), 0);
    assert!(line.active().is_some());
}

#[test]
fn a_transaction_is_refused_while_the_outer_command_holds_a_group() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    env.document.begin_group("outer").wait().unwrap();
    let refused = line.transparent(&t, "test.view.mark").unwrap_err();
    assert_eq!(refused.code(), "cmd.transparent.refused");
    assert_eq!(env.count(), 0);
    // A command that opens no transaction runs inside the group.
    assert!(line.transparent(&t, "test.view.zoom 2").is_ok());
    env.document.end_group().wait().unwrap();
    assert!(line.transparent(&t, "test.view.mark").is_ok());
    assert_eq!(env.count(), 1);
    assert!(line.active().is_some());
}

fn named(env: &Env, names: &[&str]) {
    for name in names {
        env.run("test.item.named", json!({"name": name})).unwrap();
    }
}

#[test]
fn completion_offers_object_names_by_identifier() {
    let env = Env::new();
    named(&env, &["Wall A", "Wall B", "Door"]);
    let snapshot = env.document.snapshot();
    let complete = |text: &str| {
        complete_in(
            &env.registry,
            &env.localized,
            Some(&snapshot),
            text,
            text.len(),
        )
    };
    let labels = |text: &str| -> Vec<String> {
        complete(text)
            .candidates
            .iter()
            .map(|c| c.label.clone().unwrap())
            .collect()
    };
    let text = "test.item.set_length id=wa";
    let c = complete(text);
    assert_eq!(labels(text), ["Wall A", "Wall B"]);
    assert!(c.candidates.iter().all(|c| c.kind == CandidateKind::Object));
    assert!(
        c.candidates
            .iter()
            .all(|c| c.text.parse::<arc_base::ObjectId>().is_ok())
    );
    let span = c.span.unwrap();
    assert_eq!(&text[span.start..span.end], "wa");
    // Positionally, and as the last element of a list.
    assert_eq!(labels("test.item.set_length do"), ["Door"]);
    let text = "test.item.rename ids=[x,d";
    let c = complete(text);
    assert_eq!(labels(text), ["Door"]);
    let span = c.span.unwrap();
    assert_eq!(&text[span.start..span.end], "d");
    // Without a document nothing is offered.
    let c = arc_command::complete(
        &env.registry,
        &env.localized,
        "test.item.set_length id=w",
        26,
    );
    assert!(c.candidates.is_empty());
}

#[test]
fn completion_offers_units_after_a_number() {
    let env = Env::new();
    let complete = |text: &str| complete_in(&env.registry, &env.localized, None, text, text.len());
    let text = "test.item.create name=a length=12m";
    let c = complete(text);
    assert!(c.candidates.iter().any(|c| c.text == "mm"));
    assert!(c.candidates.iter().all(|c| c.kind == CandidateKind::Unit));
    let span = c.span.unwrap();
    assert_eq!(&text[span.start..span.end], "m");
    // Letters without a number before them are not a unit.
    assert!(
        complete("test.item.create name=a length=m")
            .candidates
            .is_empty()
    );
    // The names of other parameters are still offered by prefix.
    assert!(
        complete("test.item.create le")
            .candidates
            .iter()
            .any(|c| c.text == "length=")
    );
}

#[test]
fn prompts_complete_units_of_their_dimension_and_pick_names() {
    let env = Env::new();
    named(&env, &["Wall A", "Door"]);
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.item.create").unwrap();
    line.submit(&t, "x").unwrap();
    let c = line.complete(&t, "5m", 2);
    assert!(c.candidates.iter().any(|c| c.text == "mm"));
    assert!(c.candidates.iter().all(|c| c.kind == CandidateKind::Unit));
    assert_eq!(c.span.unwrap().start, 1);
    // Only lengths: no time unit.
    assert!(c.candidates.iter().all(|c| c.text != "s"));
    line.cancel();
    line.submit(&t, "test.item.set_length").unwrap();
    let c = line.complete(&t, "wa", 2);
    assert_eq!(c.candidates.len(), 1);
    assert_eq!(c.candidates[0].label.as_deref(), Some("Wall A"));
}

fn recorded(env: &Env) -> (Recorder, String) {
    let recorder = Recorder::new("p", "1", &Limits::DEFAULT);
    let mut invocation = Invocation::new("test.item.create", json!({"name": "a", "length": 2.4}));
    invocation.seed = 5;
    recorder.record(&env.registry, &invocation, 0).unwrap();
    recorder
        .set_input(&env.registry, 0, "name", "item_name")
        .unwrap();
    let text = recorder.text();
    (recorder, text)
}

#[test]
fn macro_inputs_are_prompted_values_supplied_at_playback() {
    let env = Env::new();
    let (_, text) = recorded(&env);
    assert!(text.contains(r#""version":2"#), "{text}");
    assert!(text.contains(r#""inputs":{"name":"item_name"}"#), "{text}");
    let parsed = parse_macro(&text, &Limits::DEFAULT).unwrap();
    let inputs = parsed.inputs(&env.registry).unwrap();
    assert_eq!(inputs.len(), 1);
    assert_eq!(
        (
            inputs[0].name.as_str(),
            inputs[0].command.as_str(),
            inputs[0].param.as_str()
        ),
        ("item_name", "test.item.create", "name")
    );
    assert_eq!(inputs[0].default, json!("a"));
    // Without a value nothing runs.
    let missing = play(
        &env.registry,
        &env.document,
        &env.session,
        &parsed.steps,
        "g",
    )
    .unwrap_err();
    assert_eq!(missing.code(), "cmd.macro.input_missing");
    assert_eq!(env.count(), 0);
    let values = BTreeMap::from([
        ("item_name".to_owned(), json!("chosen")),
        ("other".to_owned(), json!(1)),
    ]);
    let outcomes = play_with(
        &env.registry,
        &env.document,
        &env.session,
        &parsed.steps,
        "g",
        &values,
    )
    .unwrap();
    assert_eq!(outcomes.len(), 1);
    let snapshot = env.document.snapshot();
    let (_, object) = snapshot.iter().next().unwrap();
    assert!(format!("{:?}", object.values).contains("chosen"));
    // The value is checked like any parameter: a number is not a name.
    let bad = BTreeMap::from([("item_name".to_owned(), json!(7))]);
    let e = play_with(
        &env.registry,
        &env.document,
        &env.session,
        &parsed.steps,
        "g",
        &bad,
    )
    .unwrap_err();
    assert_eq!(e.code(), "cmd.params.invalid");
    assert_eq!(env.count(), 1);
}

#[test]
fn inputs_are_validated_and_version_one_is_still_read() {
    let env = Env::new();
    let (recorder, _) = recorded(&env);
    for (index, param, name) in [
        (0, "nope", "x"),
        (0, "name", "Bad Name"),
        (0, "name", ""),
        (1, "name", "x"),
    ] {
        let e = recorder
            .set_input(&env.registry, index, param, name)
            .unwrap_err();
        assert_eq!(e.code(), "cmd.macro.malformed", "{param} {name}");
    }
    let header = |version: u32| {
        format!(
            r#"{{"format":"arc-macro","version":{version},"protocol":"1.0","product":{{"name":"p","version":"1"}}}}"#
        )
    };
    let step = r#"{"line":"test.item.create length=1 name=a","inputs":{"name":"n"}}"#;
    // Version 1 has no inputs; version 2 reads them; hostile names are refused.
    let e = parse_macro(&format!("{}\n{step}", header(1)), &Limits::DEFAULT).unwrap_err();
    assert_eq!(e.code(), "cmd.macro.malformed");
    assert!(parse_macro(&format!("{}\n{step}", header(2)), &Limits::DEFAULT).is_ok());
    let hostile = step.replace(r#""n""#, r#""N!""#);
    assert!(parse_macro(&format!("{}\n{hostile}", header(2)), &Limits::DEFAULT).is_err());
    let v1 = format!(
        "{}\n{{\"line\":\"test.item.create length=1 name=a\"}}",
        header(1)
    );
    let parsed = parse_macro(&v1, &Limits::DEFAULT).unwrap();
    assert!(parsed.steps.iter().all(|s: &MacroStep| s.inputs.is_empty()));
    play(
        &env.registry,
        &env.document,
        &env.session,
        &parsed.steps,
        "g",
    )
    .unwrap();
    assert_eq!(env.count(), 1);
}

#[test]
fn a_doubled_prefix_answers_a_text_prompt_with_a_prefix() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    line.submit(&t, "0,0").unwrap();
    line.submit(&t, "3,4").unwrap();
    // The name prompt takes free text: `''` is an escape for the prefix, not a transparent command.
    let Submitted::Prompt(next) = line.submit(&t, "''abc").unwrap() else {
        panic!("expected the next prompt");
    };
    assert_eq!(next.key, "copies");
    let answers = format!("{:?}", line.active().unwrap().answers());
    assert!(answers.contains(r#"Text("'abc")"#), "{answers}");
    assert_eq!(env.count(), 0);
}

#[test]
fn completion_clamps_a_cursor_inside_a_character_to_its_start() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.item.create").unwrap();
    line.submit(&t, "x").unwrap();
    // The cursor at byte 3 is inside the two-byte "é" and completes as if it were at byte 2.
    let c = line.complete(&t, "5mé", 3);
    let span = c.span.unwrap();
    assert_eq!((span.start, span.end), (1, 2));
    assert!(c.candidates.iter().any(|c| c.text == "mm"));
}
