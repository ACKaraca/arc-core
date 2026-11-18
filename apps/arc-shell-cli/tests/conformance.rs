//! The protocol conformance requests of `crates/arc-command/tests/protocol.rs` replayed against
//! the CLI process (`serve --stdio`). The requests are read from that test's recorded transcript,
//! so the two cannot drift. Every line the process writes must be a valid message of the
//! published schema and answer its request in order; framing and session answers, which do not
//! depend on the commands registered, must equal the recorded ones.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use arc_command::schema::{check_schema, validate};
use serde_json::Value;

const TRANSCRIPT: &str =
    include_str!("../../../crates/arc-command/tests/snapshots/protocol__conformance.snap");
const RESPONSES: &str = include_str!("../../../schemas/automation/responses.json");

/// Codes that framing and session handling give, whatever the registry holds. The limit code
/// -32008 is left out: the recorded session ran with smaller limits than the CLI's defaults.
const REGISTRY_FREE: [i64; 5] = [-32700, -32600, -32601, -32602, -32011];

fn schema(text: &str) -> Value {
    let mut value: Value = serde_json::from_str(text).unwrap();
    value.as_object_mut().unwrap().remove("$id");
    check_schema(&value).unwrap();
    value
}

#[test]
fn conformance_requests_against_the_cli_process() {
    let responses = schema(RESPONSES);
    let body = TRANSCRIPT.splitn(3, "---\n").nth(2).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_arc-shell-cli"))
        .args(["serve", "--stdio", "--allow-edit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut to = child.stdin.take().unwrap();
    let mut from = BufReader::new(child.stdout.take().unwrap());
    let mut method = Value::Null;
    let mut compared = 0;
    for line in body.lines() {
        if let Some(request) = line.strip_prefix("> ") {
            method = serde_json::from_str::<Value>(request)
                .ok()
                .and_then(|r| r.get("method").cloned())
                .unwrap_or(Value::Null);
            writeln!(to, "{request}").unwrap();
            to.flush().unwrap();
        } else if line == ">" {
            writeln!(to).unwrap();
        } else if let Some(expected) = line.strip_prefix("< ") {
            let expected: Value = serde_json::from_str(expected).unwrap();
            let mut actual = String::new();
            assert!(
                from.read_line(&mut actual).unwrap() > 0,
                "no answer to {expected}"
            );
            let actual: Value = serde_json::from_str(&actual).unwrap();
            assert!(
                validate(&responses, &actual).unwrap().is_empty(),
                "invalid message {actual}"
            );
            assert_eq!(actual["id"], expected["id"]);
            let code = expected["error"]["code"].as_i64();
            if code.is_some_and(|c| REGISTRY_FREE.contains(&c)) {
                assert_eq!(
                    actual["error"]["code"], expected["error"]["code"],
                    "{actual}"
                );
                assert_eq!(actual["error"]["message"], expected["error"]["message"]);
                compared += 1;
            } else if matches!(
                method.as_str(),
                Some("arc.ping" | "arc.capabilities" | "arc.shutdown")
            ) {
                assert_eq!(actual, expected);
                compared += 1;
            }
        }
    }
    assert!(compared >= 15, "only {compared} answers compared");
    drop(to);
    let mut rest = String::new();
    assert_eq!(from.read_line(&mut rest).unwrap(), 0, "unexpected {rest}");
    assert!(child.wait().unwrap().success());
}
