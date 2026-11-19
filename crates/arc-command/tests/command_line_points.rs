//! The point state of a command line: the last point handed from one prompt session to the next,
//! host settings, transparent commands, cancel, and the scripts that replay typed points
//! (CORE-CMD-011, CORE-CMD-013, CORE-CMD-015 and the typed point grammar).

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;

use arc_base::{Limits, Unit};
use arc_command::macros::{Recorder, parse_macro, play};
use arc_command::{
    Accept, CommandLine, CommandSpec, Context, Invocation, Invoker, LineStep, Outcome, Permission,
    PlaneTransform, Point, PointDims, PromptSequence, PromptStep, Registry, Session, Submitted,
    Surface, Target, UndoBehavior,
};
use arc_doc::Document;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pair {
    a: Vec<f64>,
    b: Vec<f64>,
}

#[derive(Serialize, JsonSchema)]
struct PairResult {
    a: Vec<f64>,
    b: Vec<f64>,
}

fn pair(_: &mut Context<'_>, p: Pair) -> arc_base::Result<PairResult> {
    Ok(PairResult { a: p.a, b: p.b })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Zoom {
    steps: u32,
}

#[derive(Serialize, JsonSchema)]
struct ZoomResult {
    steps: u32,
}

fn zoom(_: &mut Context<'_>, p: Zoom) -> arc_base::Result<ZoomResult> {
    Ok(ZoomResult { steps: p.steps })
}

fn point_accept() -> Accept {
    Accept::Point {
        dims: PointDims::Any,
    }
}

/// Two point commands: `test.line.pair` asks `a` and `b`; `test.view.zoom` is transparent and
/// takes its parameters inline. Neither changes the document.
fn points_registry() -> Registry {
    let mut registry = Registry::new();
    registry
        .register(
            CommandSpec::new("test.line.pair", "test-pair", "test-pair-description", pair)
                .with_undo(UndoBehavior::NotUndoable)
                .with_prompts(
                    PromptSequence::new()
                        .step(PromptStep::new("a", "test-prompt-point", point_accept()))
                        .step(PromptStep::new("b", "test-prompt-point", point_accept())),
                ),
        )
        .unwrap();
    registry
        .register(
            CommandSpec::new("test.view.zoom", "test-zoom", "test-zoom-description", zoom)
                .with_undo(UndoBehavior::NotUndoable)
                .transparent(),
        )
        .unwrap();
    registry
}

/// A session that reads lengths in meters and angles in degrees.
fn session() -> Session {
    let mut session = Session::new(Surface::CommandLine, [Permission::Read, Permission::Edit]);
    session.units = vec![
        *Unit::by_name("meter").unwrap(),
        *Unit::by_name("degree").unwrap(),
    ];
    session
}

struct Env {
    registry: Registry,
    document: Document,
    session: Session,
    localized: BTreeMap<String, String>,
}

impl Env {
    fn new() -> Self {
        Self {
            registry: points_registry(),
            document: common::document(),
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
            seed: 7,
        }
    }
}

fn submit(line: &mut CommandLine, env: &Env, text: &str) -> Submitted {
    line.submit(&env.target(), text).unwrap()
}

fn prompt_key(submitted: &Submitted) -> &str {
    match submitted {
        Submitted::Prompt(prompt) => &prompt.key,
        other => panic!("expected a prompt, got {other:?}"),
    }
}

fn value_of(submitted: Submitted) -> Value {
    match submitted {
        Submitted::Ran(outcome) => outcome.value,
        other => panic!("expected a run, got {other:?}"),
    }
}

fn points(value: &Value, key: &str) -> Vec<f64> {
    value[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

fn close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-9, "{actual:?} vs {expected:?}");
    }
}

/// Starts `pair` on `line`, answers its two prompts with `answers` (typed texts) and returns the
/// run's result.
fn run_pair(line: &mut CommandLine, env: &Env, answers: [&str; 2]) -> Value {
    assert_eq!(prompt_key(&submit(line, env, "test.line.pair")), "a");
    assert_eq!(prompt_key(&submit(line, env, answers[0])), "b");
    value_of(submit(line, env, answers[1]))
}

#[test]
fn absolute_then_relative_within_a_command() {
    let env = Env::new();
    let mut line = CommandLine::new();
    let value = run_pair(&mut line, &env, ["10,10", "@1,0"]);
    close(&points(&value, "a"), &[10.0, 10.0]);
    close(&points(&value, "b"), &[11.0, 10.0]);
    assert_eq!(
        line.base_point(),
        Some(Point {
            x: 11.0,
            y: 10.0,
            z: None
        })
    );
}

#[test]
fn a_relative_first_answer_measures_from_the_previous_command() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["10,10", "@1,0"]);
    let value = run_pair(&mut line, &env, ["@0,2", "@1,0"]);
    close(&points(&value, "a"), &[11.0, 12.0]);
    close(&points(&value, "b"), &[12.0, 12.0]);
}

#[test]
fn a_direct_distance_follows_the_host_direction() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["10,10", "@1,0"]);
    line.set_pointer_direction(Some([0.0, 2.0])).unwrap();
    // The direction is normalized; the base point is the previous command's last point.
    assert_eq!(line.direction(), Some([0.0, 1.0]));
    assert_eq!(
        line.base_point(),
        Some(Point {
            x: 11.0,
            y: 10.0,
            z: None
        })
    );
    let value = run_pair(&mut line, &env, ["5", "@1,0"]);
    close(&points(&value, "a"), &[11.0, 15.0]);
    close(&points(&value, "b"), &[12.0, 15.0]);
}

#[test]
fn an_angle_lock_overrides_the_pointer_direction_for_the_next_distance() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["11,15", "@1,0"]);
    line.set_pointer_direction(Some([1.0, 0.0])).unwrap();

    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    // `<90` keeps the prompt and locks the direction: the effective direction is the lock.
    assert_eq!(prompt_key(&submit(&mut line, &env, "<90")), "a");
    close(&line.direction().unwrap(), &[0.0, 1.0]);
    assert_eq!(prompt_key(&submit(&mut line, &env, "3")), "b");
    let value = value_of(submit(&mut line, &env, "@1,0"));
    close(&points(&value, "a"), &[12.0, 18.0]);
    close(&points(&value, "b"), &[13.0, 18.0]);
    // The accepted point released the lock; the host's direction is in effect again.
    assert_eq!(line.point_entry().locked_direction(), None);
    close(&line.direction().unwrap(), &[1.0, 0.0]);
}

#[test]
fn a_transparent_command_shares_the_entry_and_leaves_it_unchanged() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["10,10", "@1,0"]);

    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    assert_eq!(prompt_key(&submit(&mut line, &env, "20,20")), "b");
    let zoomed = value_of(submit(&mut line, &env, "'test.view.zoom steps=2"));
    assert_eq!(zoomed["steps"], 2);
    // The interrupted prompt is still asked, and the point it accepted is still the base.
    assert_eq!(line.active().unwrap().prompt().unwrap().key, "b");
    assert_eq!(
        line.base_point(),
        Some(Point {
            x: 20.0,
            y: 20.0,
            z: None
        })
    );
    let value = value_of(submit(&mut line, &env, "@1,0"));
    close(&points(&value, "b"), &[21.0, 20.0]);
}

#[test]
fn a_cancel_before_any_point_keeps_the_previous_command_last_point() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["20,20", "@1,0"]);

    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    assert!(matches!(line.cancel(), Submitted::Cancelled));
    assert!(line.active().is_none());
    let value = run_pair(&mut line, &env, ["@1,0", "@1,0"]);
    close(&points(&value, "a"), &[22.0, 20.0]);
}

#[test]
fn a_cancelled_command_keeps_the_points_it_accepted() {
    // Rule: a cancelled command's accepted points are the line's last point, so the next
    // command's relative answer measures from the last point the cancelled command accepted.
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["20,20", "@1,0"]);

    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    assert_eq!(prompt_key(&submit(&mut line, &env, "30,30")), "b");
    assert!(matches!(line.cancel(), Submitted::Cancelled));
    let value = run_pair(&mut line, &env, ["@0,1", "@1,0"]);
    close(&points(&value, "a"), &[30.0, 31.0]);
    close(&points(&value, "b"), &[31.0, 31.0]);
}

#[test]
fn host_setters_reach_the_active_session_and_the_line() {
    let env = Env::new();
    let mut line = CommandLine::new();
    run_pair(&mut line, &env, ["30,30", "@1,0"]);

    // Idle: the settings are the line's and the next command starts with them.
    line.set_relative_default(true);
    assert!(line.point_entry().relative_default());
    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    // Active: the setter reaches the session that is asking.
    assert!(line.active().unwrap().point_entry().relative_default());
    assert_eq!(prompt_key(&submit(&mut line, &env, "1,0")), "b");
    let value = value_of(submit(&mut line, &env, "#0,0"));
    close(&points(&value, "a"), &[32.0, 30.0]);
    close(&points(&value, "b"), &[0.0, 0.0]);
    line.set_relative_default(false);
}

#[test]
fn the_transform_is_applied_to_typed_absolute_points_until_reset() {
    let env = Env::new();
    let mut line = CommandLine::new();
    line.set_transform(PlaneTransform::new([[1.0, 0.0], [0.0, 1.0]], [5.0, 0.0]).unwrap());
    let value = run_pair(&mut line, &env, ["1,1", "*1,1"]);
    close(&points(&value, "a"), &[6.0, 1.0]);
    close(&points(&value, "b"), &[1.0, 1.0]);
    line.set_transform(PlaneTransform::IDENTITY);
}

#[test]
fn an_invalid_direction_changes_nothing() {
    let env = Env::new();
    let mut line = CommandLine::new();
    line.set_pointer_direction(Some([0.0, 1.0])).unwrap();
    assert!(line.set_pointer_direction(Some([0.0, 0.0])).is_err());
    assert!(
        line.set_last_point(Some(Point {
            x: f64::NAN,
            y: 0.0,
            z: None
        }))
        .is_err()
    );
    assert_eq!(line.direction(), Some([0.0, 1.0]));
    assert_eq!(line.base_point(), None);
    // Nothing was forwarded: the line still has no last point for a relative answer.
    assert_eq!(prompt_key(&submit(&mut line, &env, "test.line.pair")), "a");
    assert_eq!(
        match submit(&mut line, &env, "@1,0") {
            Submitted::Rejected { error, .. } => error.code(),
            other => panic!("{other:?}"),
        },
        "cmd.prompt.no_base"
    );
}

/// Runs `step` through `line`, recording every invocation that runs.
fn drive_recording(
    line: &mut CommandLine,
    env: &Env,
    text: &str,
    captured: &mut Vec<Invocation>,
) -> Submitted {
    let invoker = Invoker::new(&env.registry);
    let mut runner = |invocation: Invocation| -> arc_base::Result<Outcome> {
        captured.push(invocation.clone());
        invoker.invoke(&env.document, &env.session, invocation)
    };
    line.drive(&env.target(), LineStep::Submit(text), &mut runner)
        .unwrap()
}

#[test]
fn a_recorded_script_replays_the_typed_points_it_recorded() {
    // Macros store the resolved invocation, not the typed text: a relative answer is recorded
    // as its absolute point, so replay does not depend on the line's point state.
    let env = Env::new();
    let mut line = CommandLine::new();
    let mut captured = Vec::new();
    let mut results = Vec::new();
    for text in [
        "test.line.pair",
        "10,10",
        "@1,0",
        "test.line.pair",
        "@0,2",
        "@1,0",
    ] {
        if let Submitted::Ran(outcome) = drive_recording(&mut line, &env, text, &mut captured) {
            results.push(outcome.value);
        }
    }
    assert_eq!(captured.len(), 2);
    assert_eq!(results.len(), 2);

    let limits = Limits::DEFAULT;
    let recorder = Recorder::new("arc-test", "1", &limits);
    for invocation in &captured {
        recorder.record(&env.registry, invocation, 0).unwrap();
    }
    let text = recorder.text();
    assert!(!text.contains('@'), "{text}");
    let parsed = parse_macro(&text, &limits).unwrap();
    assert_eq!(parsed.steps.len(), 2);

    // Replay on a fresh document with a fresh line, whose point state is empty.
    let replay_env = Env::new();
    let outcomes = play(
        &replay_env.registry,
        &replay_env.document,
        &replay_env.session,
        &parsed.steps,
        "replay",
    )
    .unwrap();
    assert_eq!(outcomes.len(), 2);
    for (replayed, original) in outcomes.iter().zip(&results) {
        assert_eq!(&replayed.value, original);
    }
    close(&points(&outcomes[1].value, "b"), &[12.0, 12.0]);
}
