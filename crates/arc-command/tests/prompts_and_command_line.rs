//! Prompt machines, command-line parsing and repeat (CORE-CMD-011, CORE-CMD-013,
//! CORE-CMD-015) on the sample commands.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::collections::BTreeMap;

use arc_base::ObjectId;
use arc_command::{
    CandidateKind, CommandLine, Invocation, Invoker, Point, PromptInput, Registry, Submitted,
    Target, complete, format_line, parse_line,
};
use arc_doc::Document;
use common::{document, registry, session};
use serde_json::{Value, json};

struct Env {
    registry: Registry,
    document: Document,
    session: arc_command::Session,
    localized: BTreeMap<String, String>,
}

impl Env {
    fn new() -> Self {
        Self {
            registry: registry(),
            document: document(),
            session: session(),
            localized: BTreeMap::from([("test.item.create".into(), "Make Item".into())]),
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

    /// Lengths and names of every object, in identifier order.
    fn dump(&self) -> Vec<String> {
        let snapshot = self.document.snapshot();
        let mut out: Vec<String> = snapshot
            .iter()
            .map(|(_, object)| format!("{:?}", object.values))
            .collect();
        out.sort();
        out
    }

    fn create(&self, name: &str, length: f64) -> ObjectId {
        let outcome = common::invoke(
            &self.registry,
            &self.document,
            &self.session,
            "test.item.create",
            json!({"name": name, "length": length}),
        )
        .unwrap();
        outcome.value["id"].as_str().unwrap().parse().unwrap()
    }
}

fn prompt_key(submitted: &Submitted) -> &str {
    match submitted {
        Submitted::Prompt(p) => &p.key,
        other => panic!("expected a prompt, got {other:?}"),
    }
}

fn rejected_code(submitted: &Submitted) -> &'static str {
    match submitted {
        Submitted::Rejected { error, .. } => error.code(),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn ran(submitted: Submitted) -> Value {
    match submitted {
        Submitted::Ran(outcome) => outcome.value,
        other => panic!("expected a run, got {other:?}"),
    }
}

#[test]
fn a_line_from_two_points_with_defaults_and_whole_copies() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    let first = line.submit(&t, "test.shape.draw").unwrap();
    assert_eq!(prompt_key(&first), "start");
    let Submitted::Prompt(prompt) = &first else {
        panic!("{first:?}")
    };
    assert_eq!(prompt.message.id, "test-prompt-start");
    assert_eq!(prompt_key(&line.submit(&t, "0,0").unwrap()), "end");
    // Session length unit: millimeters; components are quantity expressions.
    assert_eq!(
        prompt_key(&line.submit(&t, "3, (2 mm + 2 mm)").unwrap()),
        "name"
    );
    assert_eq!(prompt_key(&line.submit(&t, "").unwrap()), "copies");
    assert_eq!(
        rejected_code(&line.submit(&t, "11").unwrap()),
        "cmd.prompt.range"
    );
    assert_eq!(
        rejected_code(&line.submit(&t, "1.5").unwrap()),
        "cmd.prompt.range"
    );
    assert_eq!(
        rejected_code(&line.submit(&t, "abc").unwrap()),
        "solve.expr.unknown_name"
    );
    let value = ran(line.submit(&t, "2").unwrap());
    assert_eq!(value.as_array().unwrap().len(), 2);
    assert!(line.active().is_none());
    let dump = env.dump();
    assert_eq!(dump.len(), 2);
    assert!(
        dump[0].contains("0.005") && dump[0].contains("shape"),
        "{dump:?}"
    );
}

#[test]
fn the_circle_keyword_changes_the_path_by_shortcut_or_identifier() {
    for keyword in ["c", "C", "circle", "CIRCLE"] {
        let env = Env::new();
        let t = env.target();
        let mut line = CommandLine::new();
        line.submit(&t, "test.shape.draw").unwrap();
        assert_eq!(prompt_key(&line.submit(&t, keyword).unwrap()), "center");
        // The center is 2D only.
        assert_eq!(
            rejected_code(&line.submit(&t, "1,1,1").unwrap()),
            "cmd.prompt.range"
        );
        assert_eq!(prompt_key(&line.submit(&t, "1,1").unwrap()), "radius");
        assert_eq!(
            rejected_code(&line.submit(&t, "-1").unwrap()),
            "cmd.prompt.range"
        );
        line.submit(&t, "10 mm").unwrap();
        line.submit(&t, "ring").unwrap();
        ran(line.submit(&t, "").unwrap());
        let expected = 2.0 * std::f64::consts::PI * 0.01;
        assert!(env.dump()[0].contains(&format!("{expected}")));
    }
}

#[test]
fn a_distance_with_units_then_an_angle_with_its_default() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    line.submit(&t, "0,0").unwrap();
    // No comma, so not a point: read as a distance.
    assert_eq!(
        prompt_key(&line.submit(&t, "1200 mm + 3 in").unwrap()),
        "angle"
    );
    // The session has no angle unit: a bare number is dimensionless.
    assert_eq!(
        rejected_code(&line.submit(&t, "45").unwrap()),
        "solve.expr.dimension_mismatch"
    );
    assert_eq!(
        rejected_code(&line.submit(&t, "1000 deg").unwrap()),
        "cmd.prompt.range"
    );
    assert_eq!(prompt_key(&line.submit(&t, "").unwrap()), "name");
    line.submit(&t, "rod").unwrap();
    ran(line.submit(&t, "").unwrap());
    assert!(env.dump()[0].contains(&format!("{}", 1.2 + 0.0762)));
}

#[test]
fn ui_inputs_back_cancel_and_preview() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    let start = PromptInput::Point(Point {
        x: 1.0,
        y: 2.0,
        z: None,
    });
    // Wrong kind for the prompt.
    assert_eq!(
        rejected_code(&line.answer(&t, PromptInput::Text("x".into())).unwrap()),
        "cmd.prompt.kind"
    );
    assert_eq!(
        rejected_code(
            &line
                .answer(&t, PromptInput::Keyword("square".into()))
                .unwrap()
        ),
        "cmd.prompt.kind"
    );
    let nan = PromptInput::Point(Point {
        x: f64::NAN,
        y: 0.0,
        z: None,
    });
    assert_eq!(
        rejected_code(&line.answer(&t, nan).unwrap()),
        "cmd.prompt.range"
    );
    assert_eq!(prompt_key(&line.answer(&t, start).unwrap()), "end");
    let snapshot = env.document.snapshot();
    let pointer = PromptInput::Point(Point {
        x: 4.0,
        y: 6.0,
        z: None,
    });
    let preview = line.active().unwrap().preview(&snapshot, Some(&pointer));
    assert_eq!(preview, Some(json!({"segment": [[1.0, 2.0], [4.0, 6.0]]})));
    // Back removes the start point.
    assert_eq!(prompt_key(&line.back(&t).unwrap()), "start");
    assert!(line.active().unwrap().answers().is_empty());
    assert_eq!(
        line.active().unwrap().preview(&snapshot, Some(&pointer)),
        None
    );
    // Back at the first prompt asks it again.
    assert_eq!(prompt_key(&line.back(&t).unwrap()), "start");
    // Enter without a default.
    assert_eq!(
        rejected_code(&line.submit(&t, "").unwrap()),
        "cmd.prompt.no_default"
    );
    assert!(matches!(line.cancel(), Submitted::Cancelled));
    assert!(line.active().is_none());
    assert!(env.dump().is_empty());
    assert_eq!(
        line.answer(&t, PromptInput::Distance(1.0))
            .unwrap_err()
            .code(),
        "cmd.prompt.ended"
    );
    assert!(matches!(line.cancel(), Submitted::Idle));
}

#[test]
fn picks_and_selections_check_existence_and_type() {
    let env = Env::new();
    let a = env.create("a", 1.0);
    let b = env.create("b", 1.0);
    let t = env.target();
    let mut line = CommandLine::new();
    assert_eq!(
        prompt_key(&line.submit(&t, "test.item.set_length").unwrap()),
        "id"
    );
    let unknown = "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c";
    assert_eq!(
        rejected_code(&line.submit(&t, unknown).unwrap()),
        "cmd.prompt.object"
    );
    assert_eq!(
        rejected_code(&line.submit(&t, "nonsense").unwrap()),
        "cmd.prompt.unreadable"
    );
    assert_eq!(
        prompt_key(&line.submit(&t, &a.to_string()).unwrap()),
        "length"
    );
    ran(line.submit(&t, "2.5 m").unwrap());

    assert_eq!(
        prompt_key(&line.submit(&t, "test.item.rename").unwrap()),
        "ids"
    );
    let twice = format!("{a}, {a}");
    assert_eq!(
        rejected_code(&line.submit(&t, &twice).unwrap()),
        "cmd.prompt.range"
    );
    assert_eq!(
        prompt_key(&line.submit(&t, &format!("{a} {b}")).unwrap()),
        "name"
    );
    // The step's own check rejects; a too long name violates its constraint.
    assert_eq!(
        rejected_code(&line.submit(&t, "forbidden").unwrap()),
        "doc.object.not_found"
    );
    assert_eq!(
        rejected_code(&line.submit(&t, &"x".repeat(65)).unwrap()),
        "cmd.prompt.range"
    );
    ran(line.submit(&t, "renamed").unwrap());
    let dump = env.dump();
    assert!(dump.iter().all(|d| d.contains("renamed")), "{dump:?}");
    assert!(dump.iter().any(|d| d.contains("2.5")), "{dump:?}");
}

#[test]
fn commands_without_a_machine_get_one_from_their_schema() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    let first = line.submit(&t, "test.long.run").unwrap();
    let Submitted::Prompt(prompt) = &first else {
        panic!("{first:?}")
    };
    assert_eq!(prompt.message.id, "cmd-prompt-param");
    assert_eq!(
        prompt.message.args.get("name").map(String::as_str),
        Some("steps")
    );
    assert_eq!(
        rejected_code(&line.submit(&t, "2.5").unwrap()),
        "cmd.prompt.range"
    );
    // A whole number over the schema maximum is rejected by the final validation.
    assert_eq!(
        rejected_code(&line.submit(&t, "5000").unwrap()),
        "cmd.params.invalid"
    );
    assert_eq!(ran(line.submit(&t, "2").unwrap()), json!(2));
    let id = env.create("x", 1.0);
    let t = env.target();
    assert_eq!(
        prompt_key(&line.submit(&t, "test.item.delete").unwrap()),
        "id"
    );
    ran(line.submit(&t, &id.to_string()).unwrap());
}

#[test]
fn inline_parameters_run_at_once_or_continue_with_prompts() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    // Positional name, named length with spaces inside parentheses.
    ran(line
        .submit(&t, "test.item.create a length=(1200 mm + 3 in)")
        .unwrap());
    // The localized name, quoted, with a quoted positional quantity.
    ran(line
        .submit(&t, r#""make item" b "1200 mm + 3 in""#)
        .unwrap());
    // Missing length: the machine asks only for it.
    assert_eq!(
        prompt_key(&line.submit(&t, "test.item.create c").unwrap()),
        "length"
    );
    assert_eq!(
        line.active().unwrap().preset().get("name"),
        Some(&json!("c"))
    );
    ran(line.submit(&t, "1200 mm + 3 in").unwrap());
    let dump = env.dump();
    assert_eq!(dump.len(), 3);
    let expected = format!("{}", 1.2 + 0.0762);
    assert!(dump.iter().all(|d| d.contains(&expected)), "{dump:?}");
}

#[test]
fn line_prompts_and_json_give_the_same_document_and_result() {
    let lines = Env::new();
    let prompts = Env::new();
    let json_path = Env::new();
    let mut a = CommandLine::new();
    let mut b = CommandLine::new();
    let ra = ran(a
        .submit(
            &lines.target(),
            r#"test.item.create widget "1200 mm + 3 in""#,
        )
        .unwrap());
    b.submit(&prompts.target(), "test.item.create").unwrap();
    b.submit(&prompts.target(), "widget").unwrap();
    let rb = ran(b.submit(&prompts.target(), "1200 mm + 3 in").unwrap());
    let mut invocation = Invocation::new(
        "test.item.create",
        json!({"name": "widget", "length": "1200 mm + 3 in"}),
    );
    invocation.seed = 7;
    let rc = Invoker::new(&json_path.registry)
        .invoke(&json_path.document, &json_path.session, invocation)
        .unwrap()
        .value;
    assert_eq!(ra, rb);
    assert_eq!(rb, rc);
    assert_eq!(lines.dump(), prompts.dump());
    assert_eq!(prompts.dump(), json_path.dump());

    // The hand-written machine and inline positional values agree as well.
    let lines = Env::new();
    let prompts = Env::new();
    let ra = ran(a
        .submit(&lines.target(), "test.shape.draw s 0.005 2")
        .unwrap());
    for text in ["test.shape.draw", "0,0", "3,4", "s"] {
        b.submit(&prompts.target(), text).unwrap();
    }
    let rb = ran(b.submit(&prompts.target(), "2").unwrap());
    assert_eq!(
        b.last().unwrap().params,
        json!({"name": "s", "length": 0.005, "copies": 2})
    );
    assert_eq!(ra, rb);
    assert_eq!(lines.dump(), prompts.dump());
}

#[test]
fn repeat_runs_the_last_command_with_the_same_parameters() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    assert!(matches!(line.submit(&t, "   ").unwrap(), Submitted::Idle));
    assert_eq!(line.repeat_last(&t).unwrap_err().code(), "cmd.repeat.none");
    ran(line.submit(&t, "test.item.make a 2").unwrap());
    let last = line.last().unwrap().clone();
    assert_eq!(last.command, "test.item.create");
    assert_eq!(last.params, json!({"name": "a", "length": "2"}));
    // Transparent commands are not recorded.
    ran(line.submit(&t, "test.item.summary").unwrap());
    assert_eq!(line.last(), Some(&last));
    // The empty line repeats.
    ran(line.submit(&t, "").unwrap());
    ran(line.repeat_last(&t).unwrap());
    assert_eq!(env.dump().len(), 3);
    // A failed run is not recorded.
    assert!(line.submit(&t, "test.fail.always").is_err());
    assert_eq!(line.last(), Some(&last));
}

#[test]
fn parse_errors_carry_codes_and_spans() {
    let r = registry();
    let names = BTreeMap::new();
    let cases: &[(&str, &str, i64, i64)] = &[
        ("nope.cmd.x", "cmd.unknown", 0, 10),
        ("test.item.create a b c", "cmd.line.too_many_values", 21, 22),
        (
            "test.item.create name=a name=b",
            "cmd.line.duplicate_param",
            24,
            30,
        ),
        (
            "test.item.create a name=b",
            "cmd.line.duplicate_param",
            19,
            25,
        ),
        (
            "test.item.create colour=red",
            "cmd.line.unknown_param",
            17,
            27,
        ),
        ("test.item.create name=a 2", "cmd.line.syntax", 24, 25),
        ("test.item.create \"open", "cmd.line.syntax", 17, 22),
        ("test.item.create (1 + 2", "cmd.line.syntax", 17, 23),
        ("test.item.create a)", "cmd.line.syntax", 18, 19),
        ("test.item.create \"a\"b", "cmd.line.syntax", 20, 20),
        ("test.item.create \"\\q\"", "cmd.line.syntax", 18, 20),
        ("test.item.create name=", "cmd.line.syntax", 22, 22),
        ("test.item.create $nul", "cmd.line.syntax", 17, 21),
        ("test.item.create $1 2", "cmd.line.value", 17, 19),
        ("test.long.run steps=true", "cmd.line.value", 14, 24),
        ("test.long.run steps=2000", "cmd.line.value", 14, 24),
    ];
    for (text, code, start, end) in cases {
        let error = parse_line(&r, &names, text).unwrap_err();
        assert_eq!(error.code(), *code, "{text}");
        assert_eq!(
            (error.details().get("start"), error.details().get("end")),
            (
                Some(&arc_base::error::Value::Int(*start)),
                Some(&arc_base::error::Value::Int(*end))
            ),
            "{text}"
        );
    }
    let long = "x".repeat(arc_command::line::MAX_LINE_BYTES + 1);
    assert_eq!(
        parse_line(&r, &names, &long).unwrap_err().code(),
        "cmd.line.limit_exceeded"
    );
    let deep = format!("test.item.create {}", "[".repeat(40));
    assert_eq!(
        parse_line(&r, &names, &deep).unwrap_err().code(),
        "cmd.line.limit_exceeded"
    );
    assert!(parse_line(&r, &names, " \t ").unwrap().is_none());
}

#[test]
fn values_convert_by_parameter_kind() {
    let r = registry();
    let names = BTreeMap::new();
    let id = "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c";
    let parsed = parse_line(
        &r,
        &names,
        &format!("test.item.rename ids=[{id}, {id}] name=\"a \\\"b\\\"\""),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        parsed.params,
        serde_json::from_value::<serde_json::Map<String, Value>>(
            json!({"ids": [id, id], "name": "a \"b\""})
        )
        .unwrap()
    );
    let parsed = parse_line(&r, &names, &format!("test.item.rename ids={id},{id}"))
        .unwrap()
        .unwrap();
    assert_eq!(parsed.params["ids"], json!([id, id]));
    assert_eq!(parsed.missing, vec!["name".to_owned()]);
    let parsed = parse_line(&r, &names, "test.item.create length=$2.4 name=$\"x y\"")
        .unwrap()
        .unwrap();
    assert_eq!(parsed.params["length"], json!(2.4));
    assert_eq!(parsed.params["name"], json!("x y"));
    let parsed = parse_line(&r, &names, "test.long.run 3").unwrap().unwrap();
    assert_eq!(parsed.params["steps"], json!(3));
    assert!(parsed.missing.is_empty());
}

#[test]
fn the_canonical_line_reads_back() {
    let r = registry();
    let names = BTreeMap::new();
    for params in [
        json!({"name": "a b", "length": 2.4}),
        json!({"name": "$x", "length": "3'6\""}),
        json!({"name": "", "length": "(1 m + 2 in)"}),
        json!({"name": "tab\tnew\nline \\", "length": 0}),
    ] {
        let invocation = Invocation::new("test.item.make", params.clone());
        let text = format_line(&r, &invocation).unwrap();
        assert!(text.starts_with("test.item.create "), "{text}");
        let parsed = parse_line(&r, &names, &text).unwrap().unwrap();
        assert_eq!(Value::Object(parsed.params), params, "{text}");
    }
    let text = format_line(
        &r,
        &Invocation::new("test.item.create", json!({"name": "a", "length": "2 m"})),
    )
    .unwrap();
    assert_eq!(text, r#"test.item.create length="2 m" name=a"#);
}

#[test]
fn completion_offers_commands_parameters_and_keywords() {
    let r = registry();
    let names = BTreeMap::from([("test.item.create".into(), "Make Item".into())]);
    let c = complete(&r, &names, "test.item.s", 11);
    let texts: Vec<&str> = c.candidates.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["test.item.set_length", "test.item.summary"]);
    assert!(
        c.candidates
            .iter()
            .all(|c| c.kind == CandidateKind::Command)
    );
    // Commands not exposed on the command line are not offered.
    assert!(
        complete(&r, &names, "test.item.r", 11)
            .candidates
            .iter()
            .all(|c| c.text != "test.item.random")
    );
    assert_eq!(
        complete(&r, &names, "ma", 2).candidates[0].text,
        "Make Item"
    );
    let c = complete(&r, &names, "test.item.create name=a ", 24);
    let texts: Vec<&str> = c.candidates.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["length="]);
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    let c = line.complete(&t, "ci", 2);
    assert_eq!(c.candidates.len(), 1);
    assert_eq!(c.candidates[0].text, "circle");
    assert_eq!(c.candidates[0].kind, CandidateKind::Keyword);
    // Cursor positions past the end or inside a character are clamped.
    let _ = complete(&r, &names, "tést", 2);
    let _ = complete(&r, &names, "x", 99);
}
