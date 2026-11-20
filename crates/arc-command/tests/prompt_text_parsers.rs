//! The single-value parsers a product calls for its dynamic input fields: they read the same
//! lengths and angles as the command line, with the session's context units.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use arc_base::Unit;
use arc_command::Session;
use arc_command::prompt::{parse_angle, parse_length};
use arc_command::spec::Surface;

fn session(units: &[&str]) -> Session {
    let mut session = Session::new(Surface::Automation, []);
    session.units = units
        .iter()
        .map(|name| *Unit::by_name(name).unwrap())
        .collect();
    session
}

#[test]
fn lengths_read_the_solver_and_the_architectural_notation_with_the_context_unit() {
    let meters = session(&["meter"]);
    assert!((parse_length("2.5", &meters).unwrap() - 2.5).abs() < 1e-12);
    assert!((parse_length("1200 mm + 3 in", &meters).unwrap() - 1.2762).abs() < 1e-9);
    assert!((parse_length("5'-3\"", &meters).unwrap() - 63.0 * 0.0254).abs() < 1e-12);
    assert!((parse_length("3 1/2\"", &meters).unwrap() - 3.5 * 0.0254).abs() < 1e-12);
    assert!(parse_length("abc", &meters).is_err());
    assert!(parse_length("", &meters).is_err());
}

#[test]
fn angles_read_the_solver_degrees_minutes_seconds_and_bearings() {
    let degrees = session(&["degree"]);
    assert!((parse_angle("90", &degrees).unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    assert!(
        (parse_angle("45°30'15\"", &degrees).unwrap()
            - (45.0_f64 + 30.0 / 60.0 + 15.0 / 3600.0).to_radians())
        .abs()
            < 1e-12
    );
    assert!(
        (parse_angle("N 45°0'0\" E", &degrees).unwrap() - std::f64::consts::FRAC_PI_4).abs()
            < 1e-12
    );
    // A bare number needs an angle context unit, as in the prompts.
    assert!(parse_angle("90", &session(&["meter"])).is_err());
    assert!(parse_angle("N 95°0'0\" E", &degrees).is_err());
}

#[test]
fn the_parsers_agree_with_a_distance_prompt() {
    use arc_command::prompt::{Accept, Feedback, PromptSequence, PromptSession, PromptStep};
    use arc_command::spec::CommandSpec;
    use common::document;
    use serde_json::Map;
    use std::sync::Arc;

    let snapshot = document().snapshot();
    let session = session(&["meter"]);
    let spec = CommandSpec::new("test.distance.run", "l", "d", |_, _: Scalar| Ok(()))
        .with_prompts(PromptSequence::new().step(PromptStep::new("at", "m", Accept::distance())));
    let (mut prompts, _) = PromptSession::start(Arc::new(spec), Map::new(), &session, &snapshot);
    let typed = "5'-3\"";
    let Feedback::Done(invocation) = prompts.answer_text(typed, &snapshot) else {
        panic!("the distance prompt did not accept {typed}")
    };
    let prompted = invocation.params["at"].as_f64().unwrap();
    assert!((prompted - parse_length(typed, &session).unwrap()).abs() < 1e-12);
}

#[test]
fn the_parsers_refuse_text_over_the_line_cap_after_trimming() {
    use arc_command::line::MAX_LINE_BYTES;

    let session = session(&["meter", "degree"]);
    let too_long = "1".repeat(MAX_LINE_BYTES + 1);
    for result in [
        parse_length(&too_long, &session).map(|_| ()),
        parse_angle(&too_long, &session).map(|_| ()),
    ] {
        let error = result.unwrap_err();
        assert_eq!(error.code(), "cmd.prompt.limit_exceeded");
    }
    // The cap applies to the trimmed text, as it does for a typed answer.
    let padded = format!("{}2.5", " ".repeat(MAX_LINE_BYTES));
    assert!((parse_length(&padded, &session).unwrap() - 2.5).abs() < 1e-12);
}

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct Scalar {
    at: f64,
}
