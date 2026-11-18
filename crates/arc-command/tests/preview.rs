//! Preview runs (CORE-CMD-006) and the change summary of every run: a preview leaves the
//! document's objects and undo stack as they were and reports what a real run changes.

#![allow(clippy::unwrap_used, clippy::too_many_lines, reason = "tests")]

mod common;

use std::collections::BTreeMap;

use arc_command::{CommandSpec, NoLog, Permission, Registry, Server, ServerConfig, UndoBehavior};
use arc_doc::Document;
use arc_doc::undo::differences;
use common::{Nothing, document, registry};
use proptest::prelude::*;
use serde_json::{Value, json};

fn config() -> ServerConfig {
    ServerConfig::new("t", "0", [Permission::Read, Permission::Edit])
}

fn registry_with_undo_control() -> Registry {
    let mut r = registry();
    r.register(
        CommandSpec::new(
            "test.undo.control",
            "test-undo-control",
            "test-undo-control-description",
            |_, _: Nothing| -> arc_base::Result<()> { Ok(()) },
        )
        .with_undo(UndoBehavior::UndoControl)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
    r
}

fn run(server: &Server<'_>, command: &str, params: &Value, preview: bool) -> Result<Value, Value> {
    server.call(
        "command.run",
        json!({"document": "d", "command": command, "params": params, "seed": 3, "preview": preview}),
    )
}

/// What identifies a document for the property: its objects, undo steps and open group.
fn state(document: &Document) -> (usize, usize, Option<String>) {
    document.with_undo(|undo| {
        (
            undo.undo_steps().count(),
            undo.redo_steps().count(),
            undo.group().map(str::to_owned),
        )
    })
}

#[derive(Debug, Clone)]
enum Step {
    Create(f64),
    SetLength(usize, f64),
    Rename(Vec<usize>, String),
    Delete(usize),
    Draw(u32),
    Long(u32),
    Summary,
    Fail,
    Fault,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => (0.0..1e5_f64).prop_map(Step::Create),
        2 => (any::<usize>(), 0.0..1e5_f64).prop_map(|(i, l)| Step::SetLength(i, l)),
        2 => (prop::collection::vec(any::<usize>(), 1..4), "[a-z]{1,5}").prop_map(|(i, n)| Step::Rename(i, n)),
        2 => any::<usize>().prop_map(Step::Delete),
        1 => (1..5_u32).prop_map(Step::Draw),
        1 => (0..4_u32).prop_map(Step::Long),
        1 => Just(Step::Summary),
        1 => Just(Step::Fail),
        1 => Just(Step::Fault),
    ]
}

/// The command and parameters of `step` over the objects `ids`.
fn command(step: &Step, ids: &[String]) -> (&'static str, Value) {
    let pick = |i: usize| ids.get(i % ids.len().max(1)).cloned();
    match (step, ids.is_empty()) {
        (Step::SetLength(i, l), false) => {
            ("test.item.set_length", json!({"id": pick(*i), "length": l}))
        }
        (Step::Rename(is, n), false) => (
            "test.item.rename",
            json!({"ids": is.iter().filter_map(|i| pick(*i)).collect::<Vec<_>>(), "name": n}),
        ),
        (Step::Delete(i), false) => ("test.item.delete", json!({"id": pick(*i)})),
        (Step::Draw(c), _) => (
            "test.shape.draw",
            json!({"name": "s", "length": 1.5, "copies": c}),
        ),
        (Step::Long(n), _) => ("test.long.run", json!({"steps": n})),
        (Step::Summary, _) => ("test.item.summary", json!({})),
        (Step::Fail, _) => ("test.fail.always", json!({})),
        (Step::Fault, _) => ("test.fault.panic", json!({})),
        (Step::Create(l), _) | (Step::SetLength(_, l), true) => {
            ("test.item.create", json!({"name": "n", "length": l}))
        }
        (_, true) => ("test.item.create", json!({"name": "n", "length": 1})),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 40, ..ProptestConfig::default() })]

    /// A preview changes nothing visible and reports what the same run changes on an identical
    /// document: objects, undo stack and open group as before, and the summary, the value and
    /// the errors equal those of the real run.
    #[test]
    fn a_preview_leaves_the_document_unchanged_and_reports_the_real_run(
        setup in prop::collection::vec(step(), 0..8),
        probe in step(),
    ) {
        let registry = registry();
        let documents = BTreeMap::from([("d".to_owned(), document())]);
        let twins = BTreeMap::from([("d".to_owned(), document())]);
        let (previewed, real) = (
            Server::new(&registry, &documents, config(), &NoLog),
            Server::new(&registry, &twins, config(), &NoLog),
        );
        // The same steps on both documents: equal identifiers, since the seed and revisions are.
        let mut ids: Vec<String> = Vec::new();
        for step in &setup {
            let (name, params) = command(step, &ids);
            let a = run(&previewed, name, &params, false);
            let b = run(&real, name, &params, false);
            prop_assert_eq!(a.is_ok(), b.is_ok());
            if let (Ok(a), Ok(b)) = (a, b) {
                prop_assert_eq!(&a["summary"], &b["summary"]);
                ids.extend(a["summary"]["created"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()));
                for gone in a["summary"]["deleted"].as_array().unwrap() {
                    ids.retain(|id| id != gone.as_str().unwrap());
                }
            }
        }
        let (name, params) = command(&probe, &ids);
        let (doc, twin) = (&documents["d"], &twins["d"]);
        let before = doc.snapshot();
        let steps = state(doc);
        let preview = run(&previewed, name, &params, true);
        let actual = run(&real, name, &params, false);
        let after = doc.snapshot();
        prop_assert_eq!(differences(&before, &after), vec![], "objects changed");
        prop_assert_eq!(state(doc), steps.clone());
        prop_assert!(after.revision() >= before.revision());
        match (preview, actual) {
            (Ok(p), Ok(a)) => {
                prop_assert_eq!(&p["preview"], &json!(true));
                prop_assert_eq!(&a["preview"], &json!(false));
                prop_assert_eq!(&p["summary"], &a["summary"]);
                prop_assert_eq!(&p["value"], &a["value"]);
                prop_assert_eq!(&p["command"], &a["command"]);
                prop_assert_eq!(&p["baseRevision"], &json!(before.revision()));
                prop_assert_eq!(&p["revision"], &json!(after.revision()));
                // The summary is what the real run changed.
                let changed = differences(&before, &twin.snapshot());
                let mut named: Vec<String> = ["created", "modified", "deleted"]
                    .iter()
                    .flat_map(|k| a["summary"][*k].as_array().unwrap().clone())
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                let mut changed: Vec<String> = changed.iter().map(ToString::to_string).collect();
                named.sort();
                changed.sort();
                prop_assert_eq!(named, changed);
            }
            (Err(p), Err(a)) => prop_assert_eq!(&p["data"]["code"], &a["data"]["code"]),
            (p, a) => prop_assert!(false, "preview {p:?} but run {a:?}"),
        }
        // The undo stack still undoes the last real step, not the preview.
        if steps.0 > 0 && steps.2.is_none() {
            prop_assert!(doc.undo().wait().unwrap().value);
        }
    }
}

#[test]
fn undo_control_commands_cannot_be_previewed_and_leave_no_group_open() {
    let registry = registry_with_undo_control();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let error = run(&server, "test.undo.control", &json!({}), true).unwrap_err();
    assert_eq!(error["data"]["code"], "cmd.preview.unsupported");
    assert_eq!(error["code"], -32001);
    assert_eq!(state(&documents["d"]), (0, 0, None));
    let real = run(&server, "test.undo.control", &json!({}), false).unwrap();
    assert_eq!(
        real["summary"]["counts"],
        json!({"created": 0, "modified": 0, "deleted": 0})
    );
}

#[test]
fn a_preview_inside_a_clients_group_keeps_the_group_and_its_members() {
    let registry = registry();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let doc = &documents["d"];
    let create = json!({"name": "a", "length": 1});
    doc.begin_group("client").wait().unwrap();
    run(&server, "test.item.create", &create, false).unwrap();
    let preview = run(&server, "test.item.create", &create, true).unwrap();
    assert_eq!(preview["summary"]["counts"]["created"], 1);
    assert_eq!(state(doc).2.as_deref(), Some("client"));
    run(&server, "test.item.create", &create, false).unwrap();
    doc.end_group().wait().unwrap();
    // One step with the two real runs; the preview left nothing in it.
    assert_eq!(state(doc), (1, 0, None));
    assert_eq!(doc.snapshot().len(), 2);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.snapshot().len(), 0);
}

#[test]
fn a_preview_honors_the_expected_revision_and_failures_change_nothing() {
    let registry = registry();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let stale = server
        .call(
            "command.run",
            json!({"document": "d", "command": "test.item.create", "preview": true,
                   "params": {"name": "a", "length": 1}, "expectedRevision": 5}),
        )
        .unwrap_err();
    assert_eq!(stale["data"]["code"], "cmd.revision.mismatch");
    let failed = run(&server, "test.fail.always", &json!({}), true).unwrap_err();
    assert_eq!(failed["data"]["code"], "doc.object.not_found");
    let fault = run(&server, "test.fault.panic", &json!({}), true).unwrap_err();
    assert_eq!(fault["data"]["code"], "cmd.fault");
    assert_eq!(
        (documents["d"].snapshot().len(), state(&documents["d"])),
        (0, (0, 0, None))
    );
    // A preview of a read-only command reports no change and the unchanged revision.
    let read = run(&server, "test.item.summary", &json!({}), true).unwrap();
    assert_eq!(read["preview"], true);
    assert_eq!(read["revision"], read["baseRevision"]);
}

#[test]
fn every_run_reports_what_it_created_modified_and_deleted() {
    let registry = registry();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let made = run(
        &server,
        "test.item.create",
        &json!({"name": "a", "length": 1}),
        false,
    )
    .unwrap();
    let id = made["value"]["id"].clone();
    assert_eq!(made["summary"]["created"], json!([id]));
    assert_eq!(made["summary"]["truncated"], false);
    let renamed = run(
        &server,
        "test.item.rename",
        &json!({"ids": [id], "name": "b"}),
        false,
    )
    .unwrap();
    assert_eq!(renamed["summary"]["modified"], json!([id]));
    // Created and deleted in one transaction: no net change.
    let doomed = run(&server, "test.fail.always", &json!({}), false);
    assert!(doomed.is_err());
    let gone = run(&server, "test.item.delete", &json!({"id": id}), false).unwrap();
    assert_eq!(gone["summary"]["deleted"], json!([id]));
    assert_eq!(
        gone["summary"]["counts"],
        json!({"created": 0, "modified": 0, "deleted": 1})
    );
    let history = server
        .call("query.history", json!({"document": "d"}))
        .unwrap();
    let kinds: Vec<&str> = history["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["command"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["test.item.create", "test.item.rename", "test.item.delete"]
    );
}

#[test]
fn a_preview_that_changes_the_document_clears_the_redo_stack() {
    // The group mechanism records the previewed transaction like any change, and a change
    // clears redo; the documentation names this limit.
    let registry = registry();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let server = Server::new(&registry, &documents, config(), &NoLog);
    let doc = &documents["d"];
    let create = json!({"name": "a", "length": 1});
    run(&server, "test.item.create", &create, false).unwrap();
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(state(doc), (0, 1, None));
    run(&server, "test.item.summary", &json!({}), true).unwrap();
    assert_eq!(state(doc), (0, 1, None), "a read-only preview keeps redo");
    run(&server, "test.item.create", &create, true).unwrap();
    assert_eq!(state(doc), (0, 0, None));
}

#[test]
fn previews_are_not_recorded_in_macros() {
    let registry = registry();
    let documents = BTreeMap::from([("d".to_owned(), document())]);
    let recorder = arc_command::Recorder::new("t", "0", &arc_base::Limits::DEFAULT);
    let server = Server::new(&registry, &documents, config(), &NoLog).with_recorder(&recorder);
    let create = json!({"name": "a", "length": 1});
    run(&server, "test.item.create", &create, true).unwrap();
    assert_eq!(recorder.len(), 0);
    run(&server, "test.item.create", &create, false).unwrap();
    assert_eq!(recorder.len(), 1);
}
