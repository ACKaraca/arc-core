//! Protocol conformance (CORE-CMD-019, CORE-CMD-021, CORE-CMD-022, CORE-CMD-027): sessions
//! driven over in-memory byte streams with scripted request lines. Every request sent and every
//! line the server writes is validated against the published schemas in `schemas/automation/`.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "tests"
)]

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, Cursor, Read, Write};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use arc_base::{Limits, Unit};
use arc_command::protocol::{PipeReader, PipeWriter, pipe};
use arc_command::schema::{check_schema, validate};
use arc_command::{
    CommandSpec, Log, LogLevel, NoLog, Permission, Registry, Roots, Server, ServerConfig,
    UndoBehavior,
};
use arc_doc::Document;
use common::{Nothing, SteppingClock, document};
use proptest::prelude::*;
use serde_json::{Value, json};

const REQUESTS: &str = include_str!("../../../schemas/automation/requests.json");
const RESPONSES: &str = include_str!("../../../schemas/automation/responses.json");

/// The sample commands plus one that runs until it is cancelled, reporting progress.
fn registry() -> Registry {
    let mut r = common::registry();
    r.register(
        CommandSpec::new(
            "test.wait.cancel",
            "test-wait",
            "test-wait-description",
            |ctx, _: Nothing| -> arc_base::Result<()> {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(20) {
                    ctx.checkpoint()?;
                    ctx.progress(0.5, "test-wait-step");
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(())
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r.register(CommandSpec::new(
        "test.fail.operation",
        "test-fail",
        "test-fail-description",
        |_, _: Nothing| -> arc_base::Result<()> {
            Err(arc_base::Error::new(arc_doc::error::codes::GRAPH_CYCLE))
        },
    ))
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.file.read",
            "test-read",
            "test-read-description",
            |ctx, p: ReadFile| -> arc_base::Result<String> {
                let mut text = String::new();
                ctx.open_read(std::path::Path::new(&p.path))?
                    .read_to_string(&mut text)
                    .map_err(|_| arc_base::Error::new(arc_command::error::codes::PATH_IO))?;
                Ok(text)
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read, Permission::FileRead]),
    )
    .unwrap();
    r
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadFile {
    path: String,
}

fn documents() -> BTreeMap<String, Document> {
    BTreeMap::from([
        ("main".to_owned(), document()),
        ("empty".to_owned(), document()),
    ])
}

fn config() -> ServerConfig {
    let mut config = ServerConfig::new(
        "arc-test",
        "0.1.0",
        [
            Permission::Read,
            Permission::Edit,
            Permission::FileRead,
            Permission::FileWrite,
        ],
    );
    config.limits = Limits {
        nesting_depth: 8,
        array_length: 16,
        ..Limits::DEFAULT
    };
    config.max_message_bytes = 4096;
    config.units = vec![*Unit::by_symbol("mm").unwrap()];
    config.clock = Arc::new(SteppingClock {
        now: AtomicU64::new(0),
        step: 100,
    });
    config
}

/// A published schema, prepared for the crate's validator (which has no `$id`).
fn schema(text: &str) -> Value {
    let mut value: Value = serde_json::from_str(text).unwrap();
    assert!(value.get("$schema").is_some() && value.get("$id").is_some());
    value.as_object_mut().unwrap().remove("$id");
    check_schema(&value).unwrap();
    value
}

/// `root` with its `$ref` replaced by the definition `name`.
fn def(root: &Value, name: &str) -> Value {
    assert!(root["$defs"].get(name).is_some(), "no definition {name}");
    json!({"$defs": root["$defs"], "$ref": format!("#/$defs/{name}")})
}

fn assert_valid(schema: &Value, instance: &Value, what: &str) {
    let violations = validate(schema, instance).unwrap();
    assert!(
        violations.is_empty(),
        "{what}: {violations:?} in {instance}"
    );
}

/// Checks a line the server wrote; `methods` maps request ids (as JSON text) to methods.
fn check_output(line: &str, methods: &BTreeMap<String, String>, responses: &Value) {
    let message: Value = serde_json::from_str(line).unwrap();
    assert_valid(responses, &message, "server message");
    if let Some(result) = message.get("result") {
        let method = &methods[&message["id"].to_string()];
        assert_valid(&def(responses, method), result, method);
    }
}

/// A client on the other end of a session.
struct Client {
    to: PipeWriter,
    from: PipeReader,
    transcript: Vec<String>,
    methods: BTreeMap<String, String>,
    requests: Value,
    responses: Value,
    unchecked: bool,
}

impl Client {
    /// Sends a request that is deliberately invalid, without the schema check.
    fn call_invalid(&mut self, message: &Value) -> Value {
        self.unchecked = true;
        let response = self.call(message);
        self.unchecked = false;
        response
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        self.transcript
            .push(format!("> {}", String::from_utf8_lossy(bytes)));
        // A well-formed request of a provided method must match the published schema.
        if let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(bytes)
            && let (Some(id), Some(Value::String(method))) =
                (message.get("id"), message.get("method"))
            && message.get("jsonrpc") == Some(&json!("2.0"))
            && (id.is_string() || id.is_u64() || id.is_i64())
            && self.requests["$defs"].get(method).is_some()
            && message.get("params").is_none_or(Value::is_object)
            && !self.unchecked
        {
            self.methods.insert(id.to_string(), method.clone());
            assert_valid(&self.requests, &Value::Object(message.clone()), "request");
            let params = message.get("params").cloned().unwrap_or(json!({}));
            assert_valid(&def(&self.requests, method), &params, method);
        }
        self.to.write_all(bytes).unwrap();
        self.to.write_all(b"\n").unwrap();
    }

    fn send(&mut self, message: &Value) {
        self.send_bytes(serde_json::to_string(message).unwrap().as_bytes());
    }

    /// The next line the server writes.
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.from.read_line(&mut line).unwrap();
        let line = line.trim_end_matches('\n').to_owned();
        assert!(!line.is_empty(), "the session ended");
        check_output(&line, &self.methods, &self.responses);
        self.transcript.push(format!("< {line}"));
        serde_json::from_str(&line).unwrap()
    }

    fn call(&mut self, message: &Value) -> Value {
        self.send(message);
        self.recv()
    }
}

fn request(id: impl Into<Value>, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id.into(), "method": method, "params": params})
}

fn initialize(id: u64, min: &str, max: &str, permissions: &[&str]) -> Value {
    request(
        id,
        "arc.initialize",
        json!({"clientName": "conformance", "protocolVersions": {"min": min, "max": max},
               "permissions": permissions}),
    )
}

/// Runs `script` against a fresh session and returns the transcript.
fn session(script: impl FnOnce(&mut Client)) -> Vec<String> {
    session_with(config(), &NoLog, script)
}

fn session_with(
    config: ServerConfig,
    log: &dyn Log,
    script: impl FnOnce(&mut Client),
) -> Vec<String> {
    let (registry, documents) = (registry(), documents());
    let server = Server::new(&registry, &documents, config, log);
    let (to_server, server_in) = pipe();
    let (server_out, from_server) = pipe();
    let mut client = Client {
        to: to_server,
        from: from_server.with_timeout(Duration::from_secs(30)),
        transcript: Vec::new(),
        methods: BTreeMap::new(),
        requests: schema(REQUESTS),
        responses: schema(RESPONSES),
        unchecked: false,
    };
    std::thread::scope(|scope| {
        let served = scope.spawn(|| server.serve(server_in, server_out));
        script(&mut client);
        let Client {
            to,
            mut from,
            mut transcript,
            methods,
            responses,
            ..
        } = client;
        drop(to);
        let mut rest = String::new();
        while from.read_line(&mut rest).unwrap() > 0 {
            let line = rest.trim_end_matches('\n');
            check_output(line, &methods, &responses);
            transcript.push(format!("< {line}"));
            rest.clear();
        }
        served.join().unwrap().unwrap();
        transcript
    })
}

fn error_code(response: &Value) -> i64 {
    response["error"]["code"].as_i64().unwrap()
}

fn data_code(response: &Value) -> &str {
    response["error"]["data"]["code"].as_str().unwrap()
}

/// The conformance script: every method and every error code of the table.
fn conformance(c: &mut Client) {
    // Before initialize.
    let r = c.call(&request(1, "arc.ping", json!({})));
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32600, "cmd.protocol.not_initialized")
    );
    let r = c.call(&request(2, "no.such", json!({})));
    assert_eq!(error_code(&r), -32601);
    let r = c.call(&initialize(3, "2.0", "2.5", &["read"]));
    assert_eq!(error_code(&r), -32011);
    assert_eq!(r["error"]["data"]["details"]["supported"], json!(["1.0"]));
    let r = c.call_invalid(&request(
        4,
        "arc.initialize",
        json!({"protocolVersions": {"min": "1.0", "max": "1.0"}}),
    ));
    assert_eq!(error_code(&r), -32602);
    let r = c.call(&initialize(5, "1.x", "1.0", &[]));
    assert_eq!(error_code(&r), -32602);

    // Initialize: process and network are beyond the host's ceiling.
    let r = c.call(&initialize(
        6,
        "0.9",
        "1.7",
        &["read", "edit", "network", "process"],
    ));
    assert_eq!(r["result"]["protocolVersion"], "1.0");
    assert_eq!(r["result"]["permissions"], json!(["read", "edit"]));
    assert_eq!(r["result"]["server"]["name"], "arc-test");
    let r = c.call(&initialize(7, "1.0", "1.0", &[]));
    assert_eq!(data_code(&r), "cmd.protocol.already_initialized");
    let r = c.call(&request("caps", "arc.capabilities", json!({})));
    assert_eq!(r["result"]["preview"], false);
    let r = c.call(&request("p", "arc.ping", json!(null)));
    assert_eq!(error_code(&r), -32600, "params must be an object");
    let r = c.call(&request("p", "arc.ping", json!({})));
    assert_eq!(r["result"], json!({}));

    // Listing and describing.
    let r = c.call(&request(10, "command.list", json!({})));
    let ids: Vec<&str> = r["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"test.item.random") && !ids.contains(&"test.hidden.noop"));
    let r = c.call(&request(11, "command.list", json!({"document": "empty"})));
    let set_length = r["result"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "test.item.set_length")
        .unwrap();
    assert_eq!(set_length["enabled"], false);
    assert_eq!(
        set_length["disabledReason"]["messageId"],
        "test-nothing-to-change"
    );
    let r = c.call(&request(
        12,
        "command.describe",
        json!({"command": "test.item.make", "document": "main"}),
    ));
    assert_eq!(r["result"]["id"], "test.item.create");
    assert_eq!(r["result"]["permitted"], true);
    let r = c.call(&request(
        13,
        "command.describe",
        json!({"command": "test.file.touch"}),
    ));
    assert_eq!(r["result"]["permitted"], false);
    let r = c.call(&request(
        14,
        "command.describe",
        json!({"command": "test.hidden.noop"}),
    ));
    assert_eq!((error_code(&r), data_code(&r)), (-32001, "cmd.not_exposed"));
    let r = c.call(&request(
        15,
        "command.describe",
        json!({"command": "no.such.command"}),
    ));
    assert_eq!((error_code(&r), data_code(&r)), (-32001, "cmd.unknown"));
    let r = c.call(&request(
        16,
        "command.describe",
        json!({"command": "test.item.create", "document": "nope"}),
    ));
    assert_eq!(error_code(&r), -32005);

    // Validation.
    let r = c.call(&request(
        20,
        "command.validate",
        json!({"command": "test.item.create", "params": {"name": "a", "length": 1}}),
    ));
    assert_eq!(r["result"], json!({"valid": true, "violations": []}));
    let r = c.call(&request(
        21,
        "command.validate",
        json!({"command": "test.item.create", "params": {"name": 3, "extra": 1}}),
    ));
    assert_eq!(r["result"]["valid"], false);
    assert_eq!(r["result"]["violations"].as_array().unwrap().len(), 3);
    let r = c.call(&request(
        22,
        "command.validate",
        json!({"command": "test.item.create", "params": {"name": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}}),
    ));
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32008, "cmd.params.limit_exceeded")
    );

    // Runs: expected revision, preview, permissions, predicates, failures and faults.
    let create = |id: i64, expected: Option<u64>| {
        let mut params = json!({"document": "main", "command": "test.item.create",
                                "params": {"name": "a", "length": "1200"}, "seed": 9});
        if let Some(expected) = expected {
            params["expectedRevision"] = json!(expected);
        }
        request(id, "command.run", params)
    };
    let r = c.call(&create(30, Some(0)));
    assert_eq!(r["result"]["revision"], 1);
    let r = c.call(&create(31, Some(0)));
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32006, "cmd.revision.mismatch")
    );
    assert_eq!(
        r["error"]["data"]["details"],
        json!({"expected": 0, "current": 1})
    );
    let r = c.call(&create(32, None));
    assert_eq!(r["result"]["revision"], 2);
    let run = |id: i64, document: &str, command: &str, params: Value| {
        request(
            id,
            "command.run",
            json!({"document": document, "command": command, "params": params}),
        )
    };
    let mut preview = create(33, None);
    preview["params"]["preview"] = json!(true);
    let r = c.call(&preview);
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32001, "cmd.preview.unsupported")
    );
    let r = c.call(&run(34, "nope", "test.item.summary", json!({})));
    assert_eq!(error_code(&r), -32005);
    let r = c.call(&run(35, "main", "test.file.touch", json!({})));
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32004, "cmd.permission_denied")
    );
    let r = c.call(&run(
        36,
        "empty",
        "test.item.set_length",
        json!({"id": "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c", "length": 1}),
    ));
    assert_eq!((error_code(&r), data_code(&r)), (-32003, "cmd.disabled"));
    let r = c.call(&run(37, "main", "test.item.create", json!({"name": 1})));
    assert_eq!(error_code(&r), -32002);
    assert_eq!(
        r["error"]["data"]["details"]["violations"],
        json!([{"pointer": "/length", "kind": "required"}, {"pointer": "/name", "kind": "type"}])
    );
    let r = c.call(&run(38, "main", "test.fail.always", json!({})));
    assert_eq!(error_code(&r), -32005);
    let r = c.call(&run(39, "main", "test.fault.panic", json!({})));
    assert_eq!((error_code(&r), data_code(&r)), (-32012, "cmd.fault"));
    assert!(
        !r.to_string().contains("guard test fault"),
        "panic text leaked"
    );
    let r = c.call(&run(45, "main", "test.fail.operation", json!({})));
    assert_eq!((error_code(&r), data_code(&r)), (-32010, "doc.graph.cycle"));
    let r = c.call(&run(40, "main", "test.item.summary", json!({})));
    assert_eq!(r["result"]["value"]["count"], 2);
    assert_eq!(r["result"]["revision"], 2);
    let mut random = run(41, "main", "test.item.random", json!({}));
    random["params"]["seed"] = json!(7);
    c.call(&random);
    let r = c.call(&run(
        42,
        "main",
        "test.item.make",
        json!({"name": "b", "length": 2}),
    ));
    assert_eq!(
        r["result"]["diagnostics"][0]["code"],
        "cmd.alias.deprecated"
    );
    let r = c.call(&request(43, "command.cancel", json!({"id": 30})));
    assert_eq!(r["result"], json!({"cancelled": false}));
    let r = c.call(&request(44, "view.render", json!({})));
    assert_eq!(
        (error_code(&r), data_code(&r)),
        (-32001, "cmd.protocol.method_unsupported")
    );

    // Hostile framing; the stream stays synchronized.
    let r = c.call(&json!({"jsonrpc": "2.0", "id": 50, "method": "arc.ping", "params": [1]}));
    assert_eq!(error_code(&r), -32602);
    let r = c.call(&json!({"jsonrpc": "1.0", "id": 51, "method": "arc.ping"}));
    assert_eq!((error_code(&r), r["id"].clone()), (-32600, json!(51)));
    let r = c.call(&json!({"jsonrpc": "2.0", "id": 1.5, "method": "arc.ping"}));
    assert_eq!((error_code(&r), r["id"].clone()), (-32600, Value::Null));
    let r = c.call(&json!([{"jsonrpc": "2.0", "id": 52, "method": "arc.ping"}]));
    assert_eq!(data_code(&r), "cmd.protocol.invalid_request");
    assert_eq!(r["error"]["data"]["details"]["reason"], "batch");
    c.send_bytes(b"{\"jsonrpc\": \"2.0\", \"id\": 53, ");
    assert_eq!(error_code(&c.recv()), -32700);
    c.send_bytes(b"\xff\xfe{}");
    assert_eq!(error_code(&c.recv()), -32700);
    c.send_bytes(&b"[".repeat(11));
    let r = c.recv();
    assert_eq!(
        (
            error_code(&r),
            r["error"]["data"]["details"]["limit"].clone()
        ),
        (-32008, json!("nesting_depth"))
    );
    c.send_bytes(&b"x".repeat(5000));
    let r = c.recv();
    assert_eq!(
        (
            error_code(&r),
            r["error"]["data"]["details"]["limit"].clone()
        ),
        (-32008, json!("message_bytes"))
    );
    c.send_bytes(b"   ");
    c.send(&json!({"jsonrpc": "2.0", "method": "arc.ping"}));
    let r = c.call(&request("after", "arc.ping", json!({})));
    assert_eq!(
        r["id"], "after",
        "notifications and blank lines are not answered"
    );
    let r = c.call(&request(u64::MAX, "arc.ping", json!({})));
    assert_eq!(r["id"], json!(u64::MAX));

    let r = c.call(&request("bye", "arc.shutdown", json!({})));
    assert_eq!(r["result"], Value::Null);
}

#[test]
fn conformance_transcript() {
    let transcript = session(conformance);
    insta::assert_snapshot!("conformance", transcript.join("\n"));
}

#[test]
fn identical_inputs_and_seeds_give_identical_bytes() {
    assert_eq!(session(conformance), session(conformance));
}

#[test]
fn cancel_reaches_a_running_command_and_requests_are_answered_meanwhile() {
    let transcript = session(|c| {
        c.call(&initialize(1, "1.0", "1.0", &["read", "edit"]));
        c.send(&request(
            "long",
            "command.run",
            json!({"document": "main", "command": "test.wait.cancel"}),
        ));
        // Running: its progress arrives once the session clock passed 300 ms.
        let mut seen = vec![c.recv()];
        assert_eq!(seen[0]["params"]["requestId"], "long");
        // Answered while the command runs; a run with the same id is refused.
        c.send(&request("ping", "arc.ping", json!({})));
        c.send(&request(
            "long",
            "command.run",
            json!({"document": "main", "command": "test.item.summary"}),
        ));
        c.send(&request("stop", "command.cancel", json!({"id": "long"})));
        while seen
            .iter()
            .filter(|m: &&Value| m.get("id").is_some())
            .count()
            < 4
        {
            seen.push(c.recv());
        }
        let by_id = |id: &str| -> Vec<&Value> { seen.iter().filter(|m| m["id"] == id).collect() };
        assert_eq!(by_id("ping")[0]["result"], json!({}));
        assert_eq!(by_id("stop")[0]["result"], json!({"cancelled": true}));
        let long = by_id("long");
        assert_eq!(long.len(), 2);
        assert!(
            long.iter()
                .any(|m| data_code(m) == "cmd.protocol.invalid_request"
                    && m["error"]["data"]["details"]["reason"] == "duplicate_id")
        );
        assert!(long.iter().any(|m| error_code(m) == -32007));
        let progress = seen
            .iter()
            .position(|m| m["method"] == "event.notify" && m["params"]["requestId"] == "long")
            .unwrap();
        let cancelled = seen
            .iter()
            .position(|m| m["id"] == "long" && m.get("error").is_some_and(|e| e["code"] == -32007))
            .unwrap();
        assert!(progress < cancelled, "progress comes before the response");
        assert_eq!(seen[progress]["params"]["messageId"], "test-wait-step");
    });
    assert!(transcript.iter().any(|l| l.contains("cmd.cancelled")));
}

#[test]
fn shutdown_cancels_the_running_command_first() {
    let transcript = session(|c| {
        c.call(&initialize(1, "1.0", "1.0", &["read"]));
        c.send(&request(
            "long",
            "command.run",
            json!({"document": "main", "command": "test.wait.cancel"}),
        ));
        c.send(&request("bye", "arc.shutdown", json!({})));
        let mut last = c.recv();
        while last["id"] != "long" {
            last = c.recv();
        }
        assert_eq!(error_code(&last), -32007);
        assert_eq!(
            c.recv(),
            json!({"jsonrpc": "2.0", "id": "bye", "result": null})
        );
        // Nothing after the shutdown is read; the input may already be closed.
        let _ =
            c.to.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"late\",\"method\":\"arc.ping\"}\n");
    });
    assert!(
        !transcript
            .iter()
            .any(|l| l.starts_with("< ") && l.contains("late"))
    );
}

#[test]
fn end_of_stream_ends_the_session() {
    let transcript = session(|c| {
        c.call(&initialize(1, "1.0", "1.0", &["read"]));
        c.send_bytes(b"{\"jsonrpc\":\"2.0\",\"id\":\"tail\",\"method\":\"arc.ping\"}");
    });
    assert!(transcript.last().unwrap().contains("\"tail\""));
}

/// A log that panics on one client name, to fault the session while it handles a request.
struct FaultyLog;

impl Log for FaultyLog {
    fn log(&self, _: LogLevel, event: &str, detail: &str) {
        assert!(
            !(event == "protocol.initialized" && detail == "boom"),
            "log fault"
        );
    }
}

#[test]
fn a_fault_while_handling_a_request_is_contained() {
    session_with(config(), &FaultyLog, |c| {
        let mut boom = initialize(1, "1.0", "1.0", &["read"]);
        boom["params"]["clientName"] = json!("boom");
        let r = c.call(&boom);
        assert_eq!(
            (error_code(&r), data_code(&r)),
            (-32603, "cmd.protocol.internal")
        );
        assert!(!r.to_string().contains("log fault"));
        let r = c.call(&initialize(2, "1.0", "1.0", &["read"]));
        assert_eq!(r["result"]["protocolVersion"], "1.0");
    });
}

#[test]
fn file_access_goes_through_the_roots() {
    let base = std::env::temp_dir().join(format!("arc-protocol-roots-{}", std::process::id()));
    let root = base.join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("in.txt"), "inside").unwrap();
    std::fs::write(base.join("canary.txt"), "CANARY").unwrap();
    let mut config = config();
    config.roots = Roots::new([&root], []).unwrap();
    let read = |id: i64, path: &std::path::Path| {
        request(
            id,
            "command.run",
            json!({"document": "main", "command": "test.file.read",
                                          "params": {"path": path.to_string_lossy()}}),
        )
    };
    session_with(config.clone(), &NoLog, |c| {
        let r = c.call(&initialize(1, "1.0", "1.0", &["read", "file_read"]));
        assert_eq!(r["result"]["roots"]["read"].as_array().unwrap().len(), 1);
        assert_eq!(r["result"]["roots"]["write"], json!([]));
        assert_eq!(
            c.call(&read(2, &root.join("in.txt")))["result"]["value"],
            "inside"
        );
        let r = c.call(&read(3, &base.join("canary.txt")));
        assert_eq!(
            (error_code(&r), data_code(&r)),
            (-32009, "cmd.path.outside_roots")
        );
        let r = c.call(&read(4, &root.join("..").join("canary.txt")));
        assert_eq!(error_code(&r), -32009);
        assert!(!r.to_string().contains("CANARY") && !r.to_string().contains("canary.txt\""));
        let r = c.call(&read(5, &root.join("missing.txt")));
        assert_eq!(
            (error_code(&r), data_code(&r)),
            (-32005, "cmd.path.not_found")
        );
        let r = c.call(&read(6, &root));
        assert_eq!(
            (error_code(&r), data_code(&r)),
            (-32002, "cmd.path.rejected")
        );
    });
    session_with(config, &NoLog, |c| {
        let r = c.call(&initialize(1, "1.0", "1.0", &["read"]));
        assert_eq!(r["result"]["roots"], json!({"read": [], "write": []}));
        let r = c.call(&read(2, &root.join("in.txt")));
        assert_eq!(
            (error_code(&r), data_code(&r)),
            (-32004, "cmd.permission_denied")
        );
    });
    std::fs::remove_dir_all(&base).unwrap();
}

/// Valid request lines after initialization, each with its id.
fn templates(id: &str) -> Vec<Value> {
    vec![
        request(id, "arc.ping", json!({})),
        request(id, "arc.capabilities", json!({})),
        request(
            id,
            "command.describe",
            json!({"command": "test.item.summary"}),
        ),
        request(
            id,
            "command.validate",
            json!({"command": "test.item.create", "params": {"name": "x", "length": [1]}}),
        ),
        request(
            id,
            "command.run",
            json!({"document": "main", "command": "test.item.summary"}),
        ),
        request(id, "command.cancel", json!({"id": "none"})),
        request(id, "query.objects", json!({})),
    ]
}

#[derive(Debug, Clone)]
enum Line {
    Garbage(Vec<u8>),
    Valid(usize),
    Mutated(usize, usize, u8, u8),
}

fn line() -> impl Strategy<Value = Line> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..300).prop_map(Line::Garbage),
        prop::collection::vec(any::<u8>(), 4000..5000).prop_map(Line::Garbage),
        (0..7_usize).prop_map(Line::Valid),
        (0..7_usize, any::<usize>(), any::<u8>(), 0..3_u8)
            .prop_map(|(t, at, b, op)| Line::Mutated(t, at, b, op)),
    ]
}

/// Serves `input` as one stream and returns the lines written.
fn serve_bytes(input: Vec<u8>) -> Vec<String> {
    let (registry, documents) = (registry(), documents());
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let mut output = Vec::new();
    server.serve(Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Random bytes and mutated valid messages never panic; every intact request gets exactly
    /// one response; every written line is a valid message; a request after the garbage is
    /// answered correctly.
    #[test]
    fn hostile_input_never_desynchronizes(lines in prop::collection::vec(line(), 0..24)) {
        let mut input = serde_json::to_vec(&initialize(0, "1.0", "1.0", &["read", "edit"])).unwrap();
        input.push(b'\n');
        let mut intact = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let bytes = match line {
                Line::Garbage(bytes) => bytes.iter().copied().filter(|b| *b != b'\n').collect(),
                Line::Valid(t) => {
                    intact.push(format!("u{i}"));
                    serde_json::to_vec(&templates(&format!("u{i}"))[*t]).unwrap()
                }
                Line::Mutated(t, at, byte, op) => {
                    let mut bytes = serde_json::to_vec(&templates(&format!("m{i}"))[*t]).unwrap();
                    let at = at % bytes.len();
                    match op {
                        0 => bytes[at] = *byte,
                        1 => { bytes.remove(at); }
                        _ => bytes.insert(at, *byte),
                    }
                    bytes.retain(|b| *b != b'\n');
                    bytes
                }
            };
            input.extend(bytes);
            input.push(b'\n');
        }
        input.extend(serde_json::to_vec(&request("sentinel", "arc.ping", json!({}))).unwrap());
        input.push(b'\n');
        let output = serve_bytes(input);
        let responses = schema(RESPONSES);
        let mut ids = BTreeMap::<String, usize>::new();
        for line in &output {
            let message: Value = serde_json::from_str(line).unwrap();
            prop_assert!(validate(&responses, &message).unwrap().is_empty(), "{}", line);
            if let Some(Value::String(id)) = message.get("id") {
                *ids.entry(id.clone()).or_default() += 1;
            }
        }
        prop_assert!(output.len() <= lines.len() + 2);
        for id in &intact {
            prop_assert_eq!(ids.get(id).copied(), Some(1), "{}", id);
        }
        let sentinel = output.iter().find(|l| l.contains("\"sentinel\"")).unwrap();
        prop_assert_eq!(
            serde_json::from_str::<Value>(sentinel).unwrap(),
            json!({"jsonrpc": "2.0", "id": "sentinel", "result": {}})
        );
    }
}
