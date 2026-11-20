#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
//! Session state that is not document state: the settings a product keeps per session, and the
//! `last` hook a product configures for every session it serves.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::time::Duration;

use arc_base::ObjectId;
use arc_command::protocol::pipe;
use arc_command::selection::LastHook;
use arc_command::settings::{MAX_SETTING_BYTES, MAX_SETTINGS};
use arc_command::{
    CommandSpec, IdParam, Invocation, Invoker, NoLog, Permission, Registry, Server, ServerConfig,
    Session, Surface, UndoBehavior,
};
use arc_doc::{Document, Snapshot};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Setting {
    key: String,
    value: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Key {
    key: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Ids {
    ids: Vec<IdParam>,
}

/// The common registry plus a command that writes a setting, one that reads it, and one that
/// returns the identifiers it receives (so a test sees what `last` expanded to).
fn registry() -> Registry {
    let mut r = common::registry();
    r.register(
        CommandSpec::new(
            "test.settings.set",
            "test-settings-set",
            "test-settings-set-description",
            |context, p: Setting| -> arc_base::Result<()> { context.set_setting(&p.key, p.value) },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.settings.get",
            "test-settings-get",
            "test-settings-get-description",
            |context, p: Key| -> arc_base::Result<Option<Value>> { Ok(context.setting(&p.key)) },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.settings.write_then_fail",
            "test-settings-write-then-fail",
            "test-settings-write-then-fail-description",
            |context, p: Setting| -> arc_base::Result<()> {
                context.set_setting(&p.key, p.value)?;
                Err(arc_base::Error::new(
                    arc_command::error::codes::PARAMS_INVALID,
                ))
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.settings.write_and_read",
            "test-settings-write-and-read",
            "test-settings-write-and-read-description",
            |context, p: Setting| -> arc_base::Result<Option<Value>> {
                context.set_setting(&p.key, p.value)?;
                Ok(context.setting(&p.key))
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r.register(
        CommandSpec::new(
            "test.select.echo",
            "test-select-echo",
            "test-select-echo-description",
            |_context, p: Ids| -> arc_base::Result<Vec<IdParam>> { Ok(p.ids) },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r
}

fn session() -> Session {
    Session::new(Surface::Automation, [Permission::Read, Permission::Edit])
}

fn code_of<T: std::fmt::Debug>(result: arc_base::Result<T>) -> &'static str {
    result.unwrap_err().code()
}

/// Every live object of the test item type, the `last` hook of the tests.
fn live_items(snapshot: &Snapshot) -> Vec<ObjectId> {
    snapshot
        .iter_type(common::ITEM)
        .map(|items| items.map(|(_, o)| o.id).collect())
        .unwrap_or_default()
}

#[test]
fn two_sessions_do_not_share_settings_and_clones_of_one_do() {
    let first = session();
    let second = session();
    first.set_setting("acme.object_snap", json!(true)).unwrap();
    assert_eq!(first.setting("acme.object_snap"), Some(json!(true)));
    assert_eq!(second.setting("acme.object_snap"), None);

    let clone = first.clone();
    clone.set_setting("acme.echo", json!("on")).unwrap();
    assert_eq!(first.setting("acme.echo"), Some(json!("on")));
    assert_eq!(first.remove_setting("acme.object_snap"), Some(json!(true)));
    assert_eq!(clone.setting("acme.object_snap"), None);
}

#[test]
fn a_command_writes_and_reads_a_setting_without_a_revision_or_an_undo_step() {
    let r = registry();
    let doc = common::document();
    let session = session();
    let invoker = Invoker::new(&r);
    // An undoable change first, so the undo history is not empty.
    invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.item.create", json!({"name": "a", "length": 1.0})),
        )
        .unwrap();
    let revision = doc.snapshot().revision();
    let undo = doc.undo_label();

    let written = invoker
        .invoke(
            &doc,
            &session,
            Invocation::new(
                "test.settings.set",
                json!({"key": "acme.object_snap", "value": true}),
            ),
        )
        .unwrap();
    assert!(written.txn_id.is_none());
    assert_eq!(doc.snapshot().revision(), revision, "a revision changed");
    assert_eq!(doc.undo_label(), undo, "an undo step was added");

    let read = invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.settings.get", json!({"key": "acme.object_snap"})),
        )
        .unwrap();
    assert_eq!(read.value, json!(true));
    assert_eq!(session.setting("acme.object_snap"), Some(json!(true)));
}

#[test]
fn keys_values_and_the_number_of_settings_are_bounded() {
    let s = session();
    for key in ["nodot", "acme.", ".name", "acme..x", "acme.a b", ""] {
        assert_eq!(
            code_of(s.set_setting(key, json!(1))),
            "cmd.params.invalid",
            "{key}"
        );
    }
    let long = "a.".to_owned() + &"b".repeat(200);
    assert_eq!(
        code_of(s.set_setting(&long, json!(1))),
        "cmd.params.invalid"
    );

    let big = json!("x".repeat(MAX_SETTING_BYTES));
    assert_eq!(
        code_of(s.set_setting("acme.big", big)),
        "cmd.params.limit_exceeded"
    );

    for i in 0..MAX_SETTINGS {
        s.set_setting(&format!("acme.k{i}"), json!(i)).unwrap();
    }
    assert_eq!(
        code_of(s.set_setting("acme.extra", json!(0))),
        "cmd.params.limit_exceeded"
    );
    // Replacing a value keeps the count; removing one frees a place.
    s.set_setting("acme.k0", json!("again")).unwrap();
    assert_eq!(s.setting("acme.k0"), Some(json!("again")));
    assert!(s.remove_setting("acme.k1").is_some());
    s.set_setting("acme.extra", json!(0)).unwrap();
}

/// Sends each request, waits for its response line and returns the response lines. The stream
/// stays open until the last answer, as a client that waits for each answer keeps it.
fn exchange(server: &Server<'_>, requests: &[Value]) -> Vec<String> {
    let (mut to_server, server_in) = pipe();
    let (server_out, from_server) = pipe();
    let mut from_server = from_server.with_timeout(Duration::from_secs(30));
    let mut lines = Vec::new();
    std::thread::scope(|scope| {
        let served = scope.spawn(|| server.serve(server_in, server_out));
        for request in requests {
            to_server
                .write_all(format!("{request}\n").as_bytes())
                .unwrap();
            let mut line = String::new();
            from_server.read_line(&mut line).unwrap();
            lines.push(line.trim_end().to_owned());
        }
        drop(to_server);
        served.join().unwrap().unwrap();
    });
    lines
}

#[test]
fn last_reaches_every_protocol_session_when_the_product_configured_it() {
    let documents: BTreeMap<String, Document> =
        BTreeMap::from([("main".to_owned(), common::document())]);
    let registry = registry();
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "arc.initialize",
        "params": {"clientName": "test", "protocolVersions": {"min": "1.0", "max": "1.0"},
                   "permissions": ["read", "edit"]}});
    let create = run(2, "test.item.create", json!({"name": "a", "length": 1.0}));
    let echo = run(3, "test.select.echo", json!({"ids": "last"}));

    let mut config = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
    let hook: LastHook = Arc::new(live_items);
    config.last = Some(hook);
    let server = Server::new(&registry, &documents, config, &NoLog);
    let lines = exchange(&server, &[initialize.clone(), create.clone(), echo.clone()]);
    let created = live_items(&documents["main"].snapshot());
    assert_eq!(created.len(), 1);
    assert!(lines[2].contains(&created[0].to_string()), "{}", lines[2]);

    // The same requests to a server without the hook fail with the crate's code.
    let plain = ServerConfig::new("arc-test", "0.1.0", [Permission::Read, Permission::Edit]);
    let server = Server::new(&registry, &documents, plain, &NoLog);
    let lines = exchange(&server, &[initialize, echo]);
    assert!(
        lines[1].contains("cmd.selection.unavailable"),
        "{}",
        lines[1]
    );
}

fn run(id: u64, command: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "command.run",
           "params": {"document": "main", "command": command, "params": params}})
}

#[test]
fn a_failed_command_and_a_dry_run_keep_no_setting_and_a_command_reads_its_own_write() {
    let r = registry();
    let doc = common::document();
    let session = session();
    let invoker = Invoker::new(&r);
    let write =
        |command: &str| Invocation::new(command, json!({"key": "acme.snap", "value": "on"}));

    // A command that writes and then fails leaves the setting as it was.
    assert!(
        invoker
            .invoke(&doc, &session, write("test.settings.write_then_fail"))
            .is_err()
    );
    assert_eq!(
        session.setting("acme.snap"),
        None,
        "a failed command kept its write"
    );

    // A dry run writes nothing.
    invoker
        .dry_run(&doc, &session, write("test.settings.set"))
        .unwrap();
    assert_eq!(
        session.setting("acme.snap"),
        None,
        "a dry run kept its write"
    );

    // A command sees the value it has just written, and the write persists when it succeeds.
    let outcome = invoker
        .invoke(&doc, &session, write("test.settings.write_and_read"))
        .unwrap();
    assert_eq!(outcome.value, json!("on"));
    assert_eq!(session.setting("acme.snap"), Some(json!("on")));
}
