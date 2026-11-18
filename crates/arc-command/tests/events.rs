//! Event subscriptions and selection (CORE-CMD-024, CORE-CMD-025): sessions over in-memory
//! pipes, document changes in order, changes of other sessions, hidden previews, bounded queues
//! with the drop notice, progress under the same rules and the selection a command sees.

#![allow(clippy::unwrap_used, clippy::too_many_lines, reason = "tests")]

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use arc_command::protocol::{EVENT_QUEUE, PipeReader, PipeWriter, pipe};
use arc_command::schema::{check_schema, validate};
use arc_command::{CommandSpec, NoLog, Permission, Registry, Server, ServerConfig, UndoBehavior};
use arc_doc::Document;
use common::{Nothing, SteppingClock, document};
use serde_json::{Value, json};

const RESPONSES: &str = include_str!("../../../schemas/automation/responses.json");

fn registry() -> Registry {
    let mut r = common::registry();
    r.register(
        CommandSpec::new(
            "test.selection.read",
            "test-selection",
            "test-selection-description",
            |ctx, _: Nothing| -> arc_base::Result<Vec<String>> {
                Ok(ctx.selection().iter().map(ToString::to_string).collect())
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r
}

fn config(permissions: &[Permission]) -> ServerConfig {
    let mut config = ServerConfig::new("t", "0", permissions.iter().copied());
    config.clock = Arc::new(SteppingClock {
        now: AtomicU64::new(0),
        step: 100,
    });
    config
}

fn documents() -> BTreeMap<String, Document> {
    BTreeMap::from([("main".to_owned(), document())])
}

/// One client of a session.
struct Peer {
    to: PipeWriter,
    from: PipeReader,
    schema: Value,
    next: u64,
}

impl Peer {
    fn send(&mut self, method: &str, params: &Value) -> u64 {
        self.next += 1;
        let line = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        writeln!(self.to, "{line}").unwrap();
        self.next
    }

    /// The next line, which must be a valid server message.
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.from.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "the session ended");
        let message: Value = serde_json::from_str(&line).unwrap();
        let violations = validate(&self.schema, &message).unwrap();
        assert!(violations.is_empty(), "{violations:?} in {line}");
        message
    }

    /// The response to `method` and the notifications that came before it.
    fn call(&mut self, method: &str, params: &Value) -> (Vec<Value>, Value) {
        let id = self.send(method, params);
        let mut notes = Vec::new();
        loop {
            let message = self.recv();
            if message["id"] == id {
                return (notes, message);
            }
            assert_eq!(message["method"], "event.notify", "{message}");
            notes.push(message["params"].clone());
        }
    }

    fn init(&mut self, permissions: &[&str]) {
        let (_, r) = self.call(
            "arc.initialize",
            &json!({"clientName": "t", "protocolVersions": {"min": "1.0", "max": "1.0"}, "permissions": permissions}),
        );
        assert!(r["result"].is_object(), "{r}");
    }

    /// The next notification, which must arrive within the pipe's timeout.
    fn note(&mut self) -> Value {
        let message = self.recv();
        assert_eq!(message["method"], "event.notify", "{message}");
        message["params"].clone()
    }
}

/// Runs `script` with `n` connected sessions of one server and ends them afterwards.
fn sessions<R>(
    n: usize,
    config: ServerConfig,
    script: impl FnOnce(Vec<Peer>, &BTreeMap<String, Document>) -> R,
) -> R {
    let (registry, documents) = (registry(), documents());
    let server = Server::new(&registry, &documents, config, &NoLog);
    let mut schema: Value = serde_json::from_str(RESPONSES).unwrap();
    schema.as_object_mut().unwrap().remove("$id");
    check_schema(&schema).unwrap();
    std::thread::scope(|scope| {
        let mut peers = Vec::new();
        for _ in 0..n {
            let (to, server_in) = pipe();
            let (server_out, from) = pipe();
            scope.spawn(|| server.serve(server_in, server_out).unwrap());
            peers.push(Peer {
                to,
                from: from.with_timeout(Duration::from_secs(20)),
                schema: schema.clone(),
                next: 0,
            });
        }
        script(peers, &documents)
    })
}

fn create(peer: &mut Peer, name: &str) -> (Vec<Value>, Value) {
    peer.call(
        "command.run",
        &json!({"document": "main", "command": "test.item.create", "seed": 1,
                "params": {"name": name, "length": 1}}),
    )
}

#[test]
fn document_events_follow_the_commits_and_precede_their_response() {
    sessions(
        1,
        config(&[Permission::Read, Permission::Edit]),
        |mut peers, _| {
            let mut c = peers.remove(0);
            c.init(&["read", "edit"]);
            let (_, r) = c.call("event.subscribe", &json!({"events": ["document.changed"]}));
            assert_eq!(
                r["result"]["subscriptions"],
                json!([{"event": "command.progress"}, {"event": "document.changed", "document": "main"}])
            );
            let (notes, made) = create(&mut c, "a");
            let id = made["result"]["value"]["id"].clone();
            assert_eq!(notes.len(), 1, "the change comes before the response");
            assert_eq!(notes[0]["event"], "document.changed");
            assert_eq!(notes[0]["document"], "main");
            assert_eq!(notes[0]["revision"], made["result"]["revision"]);
            assert_eq!(notes[0]["changes"]["created"], json!([id]));
            assert_eq!(notes[0]["commits"], 1);
            let (notes, _) = c.call(
            "command.run",
            &json!({"document": "main", "command": "test.item.rename", "params": {"ids": [id], "name": "b"}}),
        );
            assert_eq!(notes[0]["changes"]["modified"], json!([id]));
            assert_eq!(notes[0]["revision"], 2);
            let (notes, _) = c.call(
                "command.run",
                &json!({"document": "main", "command": "test.item.delete", "params": {"id": id}}),
            );
            assert_eq!(notes[0]["changes"]["deleted"], json!([id]));
            assert_eq!(notes[0]["revision"], 3);
            // A read-only run changes nothing, so it is not an event.
            let (notes, _) = c.call(
                "command.run",
                &json!({"document": "main", "command": "test.item.summary"}),
            );
            assert!(notes.is_empty());
            // Unsubscribing ends them.
            let (_, r) = c.call(
                "event.unsubscribe",
                &json!({"events": ["document.changed"]}),
            );
            assert_eq!(
                r["result"]["subscriptions"],
                json!([{"event": "command.progress"}])
            );
            let (notes, _) = create(&mut c, "c");
            assert!(notes.is_empty());
        },
    );
}

#[test]
fn other_sessions_changes_arrive_and_previews_stay_hidden() {
    sessions(
        2,
        config(&[Permission::Read, Permission::Edit]),
        |mut peers, docs| {
            let mut b = peers.pop().unwrap();
            let mut a = peers.pop().unwrap();
            a.init(&["read"]);
            b.init(&["read", "edit"]);
            a.call(
                "event.subscribe",
                &json!({"events": ["document.changed"], "document": "main"}),
            );
            // B previews and then makes a real change: A sees only the real one, whose revision
            // shows that the preview left the revision alone.
            let (_, preview) = b.call(
                "command.run",
                &json!({"document": "main", "command": "test.item.create", "preview": true,
                    "params": {"name": "p", "length": 1}}),
            );
            assert_eq!(preview["result"]["revision"], 0);
            let (_, real) = create(&mut b, "r");
            let note = a.note();
            assert_eq!(note["revision"], 1);
            assert_eq!(
                note["changes"]["created"],
                json!([real["result"]["value"]["id"]])
            );
            assert_eq!(docs["main"].snapshot().len(), 1);
            // A holds no `edit`, but its own queries still work, and no further event is pending.
            let (notes, r) = a.call("query.objects", &json!({"document": "main"}));
            assert!(notes.is_empty());
            assert_eq!(r["result"]["objects"].as_array().unwrap().len(), 1);
        },
    );
}

#[test]
fn progress_follows_the_subscription() {
    let long = json!({"document": "main", "command": "test.long.run", "params": {"steps": 12}});
    sessions(
        2,
        config(&[Permission::Read, Permission::Edit]),
        |mut peers, _| {
            let mut quiet = peers.pop().unwrap();
            let mut loud = peers.pop().unwrap();
            loud.init(&["read", "edit"]);
            quiet.init(&["read", "edit"]);
            // Without any subscription call, progress of the session's own runs is delivered.
            let (notes, _) = loud.call("command.run", &long);
            assert!(
                notes.iter().any(|n| n["event"] == "command.progress"),
                "{notes:?}"
            );
            let (_, r) = quiet.call(
                "event.unsubscribe",
                &json!({"events": ["command.progress"]}),
            );
            assert_eq!(r["result"]["subscriptions"], json!([]));
            let (notes, response) = quiet.call("command.run", &long);
            assert!(notes.is_empty(), "{notes:?}");
            assert_eq!(response["result"]["value"], 12);
            let (_, r) = quiet.call("event.subscribe", &json!({"events": ["command.progress"]}));
            assert_eq!(
                r["result"]["subscriptions"],
                json!([{"event": "command.progress"}])
            );
            let (notes, _) = quiet.call("command.run", &long);
            assert!(notes.iter().any(|n| n["event"] == "command.progress"));
        },
    );
}

#[test]
fn selection_is_validated_and_seen_by_commands() {
    sessions(
        1,
        config(&[Permission::Read, Permission::Edit]),
        |mut peers, _| {
            let mut c = peers.remove(0);
            c.init(&["read", "edit"]);
            c.call("event.subscribe", &json!({"events": ["selection.changed"]}));
            let (notes, a) = create(&mut c, "a");
            let (b_notes, b) = create(&mut c, "b");
            let (a, b) = (
                a["result"]["value"]["id"].clone(),
                b["result"]["value"]["id"].clone(),
            );
            // A command's selection after it is the session's selection and is announced.
            for (notes, id) in [(&notes, &a), (&b_notes, &b)] {
                assert_eq!(notes.len(), 1, "{notes:?}");
                assert_eq!(notes[0]["event"], "selection.changed");
                assert_eq!(notes[0]["source"], "command");
                assert_eq!(notes[0]["objects"], json!([{"id": id}]));
            }
            let (_, r) = c.call("selection.get", &json!({"document": "main"}));
            assert_eq!(r["result"]["objects"], json!([{"id": b}]));
            // Setting: ids and sub-object keys, replacing, reported to the subscriber.
            let (notes, r) = c.call(
            "selection.set",
            &json!({"document": "main", "objects": [{"id": a, "subObjects": ["e1"]}, {"id": b}]}),
        );
            assert_eq!(
                r["result"]["objects"],
                json!([{"id": a, "subObjects": ["e1"]}, {"id": b}])
            );
            assert_eq!(notes.len(), 1);
            assert_eq!(
                (notes[0]["source"].clone(), notes[0]["objects"].clone()),
                (json!("client"), r["result"]["objects"].clone())
            );
            // Commands of the session see exactly that selection.
            let read = json!({"document": "main", "command": "test.selection.read"});
            let (_, r) = c.call("command.run", &read);
            assert_eq!(r["result"]["value"], json!([a, b]));
            // Unknown identifiers are refused with the list, and the selection is unchanged.
            let ghost = "0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c";
            let (_, r) = c.call(
                "selection.set",
                &json!({"document": "main", "objects": [{"id": a}, {"id": ghost}]}),
            );
            assert_eq!(r["error"]["data"]["code"], "cmd.selection.unknown");
            assert_eq!(r["error"]["data"]["details"]["unknown"], json!([ghost]));
            assert_eq!(r["error"]["data"]["arguments"]["count"], 1);
            let (_, r) = c.call("command.run", &read);
            assert_eq!(r["result"]["value"], json!([a, b]));
            // A selected object that is deleted leaves the selection.
            c.call(
                "command.run",
                &json!({"document": "main", "command": "test.item.delete", "params": {"id": a}}),
            );
            let (_, r) = c.call("command.run", &read);
            assert_eq!(r["result"]["value"], json!([b]));
            let (_, r) = c.call("selection.get", &json!({"document": "main"}));
            assert_eq!(r["result"]["objects"], json!([{"id": b}]));
            // Clearing.
            let (_, r) = c.call("selection.set", &json!({"document": "main", "objects": []}));
            assert_eq!(r["result"]["objects"], json!([]));
            let (_, r) = c.call("selection.get", &json!({"document": "nope"}));
            assert_eq!(r["error"]["code"], -32005);
        },
    );
}

#[test]
fn queries_events_and_selection_need_the_read_class() {
    sessions(1, config(&[Permission::Read]), |mut peers, _| {
        let mut c = peers.remove(0);
        c.init(&["edit"]);
        for (method, params) in [
            ("event.subscribe", json!({"events": ["document.changed"]})),
            ("selection.get", json!({"document": "main"})),
            ("query.objects", json!({"document": "main"})),
        ] {
            let (_, r) = c.call(method, &params);
            assert_eq!(
                r["error"]["data"]["code"], "cmd.permission_denied",
                "{method}"
            );
            assert_eq!(r["error"]["data"]["details"]["permission"], "read");
        }
    });
}

/// A writer that holds everything back until it is opened, like a client that stopped reading.
#[derive(Clone)]
struct Stalled {
    gate: Arc<(Mutex<bool>, Condvar)>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for Stalled {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let (open, wake) = &*self.gate;
        let mut open = open.lock().unwrap();
        while !*open {
            open = wake.wait(open).unwrap();
        }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_stalled_client_gets_bounded_ordered_events_and_every_loss_is_told() {
    let (registry, documents) = (registry(), documents());
    let server = Server::new(
        &registry,
        &documents,
        config(&[Permission::Read, Permission::Edit]),
        &NoLog,
    );
    let stalled = Stalled {
        gate: Arc::new((Mutex::new(false), Condvar::new())),
        bytes: Arc::default(),
    };
    let (mut to, input) = pipe();
    let bursts = 2000;
    std::thread::scope(|scope| {
        let output = stalled.clone();
        let session = &server;
        scope.spawn(move || session.serve(input, output).unwrap());
        for (i, (method, params)) in [
            ("arc.initialize", json!({"clientName": "slow", "protocolVersions": {"min": "1.0", "max": "1.0"}, "permissions": ["read"]})),
            ("event.subscribe", json!({"events": ["document.changed"]})),
        ]
        .into_iter()
        .enumerate()
        {
            writeln!(to, "{}", json!({"jsonrpc": "2.0", "id": i, "method": method, "params": params})).unwrap();
        }
        // Wait for the subscription: the session answers, but the stalled writer takes nothing.
        let subscribed = || documents["main"].events().subscribers() > 0;
        let start = std::time::Instant::now();
        while !subscribed() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
        // Other clients change the document while the stalled one reads nothing.
        for i in 0..bursts {
            server
                .call(
                    "command.run",
                    json!({"document": "main", "command": "test.item.create", "seed": i,
                           "params": {"name": "x", "length": 1}}),
                )
                .unwrap();
        }
        let final_revision = documents["main"].snapshot().revision();
        // No wait for the pump: whatever it has not moved yet is moved by the flush that
        // precedes the shutdown, which is after the gate opens.
        {
            let (open, wake) = &*stalled.gate;
            *open.lock().unwrap() = true;
            wake.notify_all();
        }
        drop(to);
        // The scope joins the session, which flushes before it ends.
        assert_eq!(final_revision, u64::try_from(bursts).unwrap());
    });
    let text = String::from_utf8(stalled.bytes.lock().unwrap().clone()).unwrap();
    let messages: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let events: Vec<&Value> = messages
        .iter()
        .filter(|m| m["method"] == "event.notify")
        .map(|m| &m["params"])
        .collect();
    let dropped: Vec<u64> = events
        .iter()
        .filter(|e| e["event"] == "events.dropped")
        .map(|e| e["dropped"].as_u64().unwrap())
        .collect();
    let changes: Vec<&&Value> = events
        .iter()
        .filter(|e| e["event"] == "document.changed")
        .collect();
    // How many events reach the session's queue depends on how fast its pump thread runs: a
    // pump that falls behind lets the document's own subscription merge commits into one event
    // (its `commits`), which is also a bounded outcome. So the drop notice is required exactly
    // when commits are unaccounted for.
    let covered: u64 = changes.iter().map(|e| e["commits"].as_u64().unwrap()).sum();
    if dropped.is_empty() {
        assert_eq!(
            covered,
            u64::try_from(bursts).unwrap(),
            "commits lost silently"
        );
    }
    // The session queue, the output queue, the line in the writer and the document
    // subscription (64 events and one merged) are the most that can be waiting.
    assert!(
        changes.len() <= 2 * EVENT_QUEUE + 64 + 8,
        "{} events delivered",
        changes.len()
    );
    let revisions: Vec<u64> = changes
        .iter()
        .map(|e| e["revision"].as_u64().unwrap())
        .collect();
    assert!(
        revisions.windows(2).all(|w| w[0] < w[1]),
        "events keep their order"
    );
    // The newest survive: the last event is the last commit, the lost ones were the oldest.
    assert_eq!(
        revisions.last().copied(),
        Some(u64::try_from(bursts).unwrap())
    );
    assert!(covered <= u64::try_from(bursts).unwrap());
    // A notice precedes the events that follow the loss.
    if let Some(first_notice) = events.iter().position(|e| e["event"] == "events.dropped") {
        assert!(first_notice < events.len() - 1);
        assert!(dropped.iter().all(|n| *n > 0));
    }
}
