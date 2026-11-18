//! Guarded execution: the guard test, rollback of failures, contained faults in every phase,
//! checks that refuse disabled, unpermitted and unexposed commands from every entry point,
//! transaction metadata, quantities, cancellation and progress.

#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use arc_base::id::Identifier;
use arc_base::{Category, ObjectId};
use arc_command::{Invocation, Invoker, PROGRESS_DELAY_MILLIS, Permission, Session, Surface};
use arc_doc::Value;
use common::{Reports, SteppingClock, document, invoke, registry, session};
use serde_json::json;

fn created(outcome: &arc_command::Outcome) -> String {
    outcome.value["id"].as_str().unwrap().to_owned()
}

#[test]
fn guard_a_command_that_unwinds_restores_the_last_snapshot_and_the_next_command_runs() {
    let (r, doc, s) = (registry(), document(), session());
    invoke(
        &r,
        &doc,
        &s,
        "test.item.create",
        json!({"name": "a", "length": 1}),
    )
    .unwrap();
    let before = doc.snapshot();

    let error = invoke(&r, &doc, &s, "test.fault.panic", json!({})).unwrap_err();
    assert_eq!(error.code(), "cmd.fault");
    assert_eq!(error.category(), Category::Internal);
    assert_eq!(
        error.details().get("panic"),
        Some(&arc_base::error::Value::Text("guard test fault".into()))
    );
    assert!(doc.snapshot().ptr_eq(&before));

    let next = invoke(
        &r,
        &doc,
        &s,
        "test.item.create",
        json!({"name": "b", "length": 2}),
    );
    assert_eq!(next.unwrap().revision, before.revision() + 1);
}

#[test]
fn faults_in_decoding_predicates_and_result_encoding_are_contained() {
    let (r, doc, s) = (registry(), document(), session());
    let before = doc.snapshot();
    for (command, params) in [
        ("test.fault.decode", json!({"boom": null})),
        ("test.fault.encode", json!({})),
        ("test.fault.predicate", json!({})),
    ] {
        let error = invoke(&r, &doc, &s, command, params).unwrap_err();
        assert_eq!(error.code(), "cmd.fault", "{command}");
        assert!(doc.snapshot().ptr_eq(&before));
    }
}

#[test]
fn a_failing_command_rolls_back_and_keeps_its_own_code() {
    let (r, doc, s) = (registry(), document(), session());
    let before = doc.snapshot();
    let error = invoke(&r, &doc, &s, "test.fail.always", json!({})).unwrap_err();
    assert_eq!(error.code(), "doc.object.not_found");
    assert!(doc.snapshot().ptr_eq(&before));
}

#[test]
fn a_disabled_command_is_refused_on_every_surface() {
    let r = registry();
    let doc = document();
    let params = json!({"id": ObjectId::from_u128(1).to_string(), "length": 1});
    for surface in Surface::ALL {
        let mut s = session();
        s.surface = surface;
        let error = invoke(&r, &doc, &s, "test.item.set_length", params.clone()).unwrap_err();
        assert_eq!(error.code(), "cmd.disabled");
        assert_eq!(
            error.details().get("reason"),
            Some(&arc_base::error::Value::Text(
                "test-nothing-to-change".into()
            ))
        );
        assert_eq!(
            error.details().get("count"),
            Some(&arc_base::error::Value::Int(0))
        );
    }
}

#[test]
fn a_missing_permission_class_is_refused_for_edits_and_queries() {
    let (r, doc) = (registry(), document());
    let read_only = Session::new(Surface::Mcp, [Permission::Read]);
    for (command, params) in [
        ("test.item.create", json!({"name": "a", "length": 1})),
        ("test.file.touch", json!({})),
    ] {
        let error = invoke(&r, &doc, &read_only, command, params).unwrap_err();
        assert_eq!(error.code(), "cmd.permission_denied", "{command}");
        assert_eq!(error.category(), Category::PermissionDenied);
    }
    let error = invoke(&r, &doc, &read_only, "test.item.random", json!({})).unwrap_err();
    assert_eq!(error.code(), "cmd.not_exposed");
    assert!(invoke(&r, &doc, &read_only, "test.item.summary", json!({})).is_ok());
    assert_eq!(doc.snapshot().revision(), 0);
}

#[test]
fn invalid_parameters_are_refused_with_pointers_before_running() {
    let (r, doc, s) = (registry(), document(), session());
    let error = invoke(
        &r,
        &doc,
        &s,
        "test.item.create",
        json!({"name": 3, "lenght": 1}),
    )
    .unwrap_err();
    assert_eq!(error.code(), "cmd.params.invalid");
    let violations: serde_json::Value =
        serde_json::from_str(match error.details().get("violations").unwrap() {
            arc_base::error::Value::Text(text) => text,
            other => panic!("{other:?}"),
        })
        .unwrap();
    assert_eq!(
        violations,
        json!([
            {"pointer": "/lenght", "kind": "additional_property"},
            {"pointer": "/length", "kind": "required"},
            {"pointer": "/name", "kind": "type"},
        ])
    );
    let error = invoke(&r, &doc, &s, "test.item.delete", json!({"id": "not-an-id"})).unwrap_err();
    assert_eq!(error.code(), "cmd.params.invalid");
    assert_eq!(doc.snapshot().revision(), 0);
}

#[test]
fn transaction_metadata_records_command_label_group_time_and_selection() {
    let (r, doc) = (registry(), document());
    let mut s = session();
    s.clock = Arc::new(arc_command::FixedClock(1_700_000_000_000));
    let journal = Arc::new(Records::default());
    doc.set_journal(journal.clone());
    let mut call = Invocation::new("test.item.create", json!({"length": "1200", "name": "a"}));
    call.group = Some("g1".into());
    let outcome = Invoker::new(&r).invoke(&doc, &s, call).unwrap();
    let id = created(&outcome);
    let records = journal.0.lock().unwrap();
    let meta = &records[0];
    assert_eq!(
        meta.command,
        r#"{"command":"test.item.create","params":{"length":"1200","name":"a"}}"#
    );
    assert_eq!(meta.label, r#"{"message_id":"test-create"}"#);
    assert_eq!(meta.group.as_deref(), Some("g1"));
    assert_eq!(meta.time_unix_millis, 1_700_000_000_000);
    assert_eq!(meta.selection_after, Some(format!("[\"{id}\"]")));
    // "1200" took the session's millimeters.
    let snapshot = doc.snapshot();
    let object = snapshot
        .get(snapshot.resolve(id.parse().unwrap()).unwrap())
        .unwrap();
    let Some(Value::Quantity(length)) = &object.values[0] else {
        panic!("no length")
    };
    assert!((length.value() - 1.2).abs() < 1e-12);
}

#[derive(Default)]
struct Records(std::sync::Mutex<Vec<arc_doc::TxnMeta>>);

impl arc_doc::JournalSink for Records {
    fn record(&self, committed: &arc_doc::Committed) {
        self.0.lock().unwrap().push(committed.meta.clone());
    }
}

#[test]
fn coalesce_keys_and_deprecated_aliases_are_reported() {
    let (r, doc, s) = (registry(), document(), session());
    let outcome = invoke(
        &r,
        &doc,
        &s,
        "test.item.make",
        json!({"name": "a", "length": 1}),
    )
    .unwrap();
    assert_eq!(outcome.command, "test.item.create");
    assert_eq!(outcome.diagnostics[0].code(), "cmd.alias.deprecated");
    let id = created(&outcome);
    let set = invoke(
        &r,
        &doc,
        &s,
        "test.item.set_length",
        json!({"id": id, "length": "2 m"}),
    )
    .unwrap();
    assert_eq!(set.coalesce.as_deref(), Some("test.length"));
    assert!(set.txn_id.is_some());
}

#[test]
fn quantities_with_bad_units_or_dimensions_fail_without_changes() {
    let (r, doc, s) = (registry(), document(), session());
    for length in ["3 kg", "NaN", "1e999 mm", "", "inf"] {
        let error = invoke(
            &r,
            &doc,
            &s,
            "test.item.create",
            json!({"name": "a", "length": length}),
        );
        assert!(error.unwrap_err().code().starts_with("solve."), "{length}");
    }
    let error = invoke(
        &r,
        &doc,
        &s,
        "test.item.create",
        json!({"name": "a", "length": -1}),
    );
    assert_eq!(error.unwrap_err().code(), "doc.value.out_of_range");
    assert_eq!(doc.snapshot().revision(), 0);
}

#[test]
fn progress_is_forwarded_after_the_delay_and_cancellation_rolls_back() {
    let (r, doc) = (registry(), document());
    let mut s = session();
    s.clock = Arc::new(SteppingClock {
        now: AtomicU64::new(0),
        step: 100,
    });
    let reports = Arc::new(Reports::default());
    let mut call = Invocation::new("test.long.run", json!({"steps": 10}));
    call.progress = Some(reports.clone());
    let outcome = Invoker::new(&r).invoke(&doc, &s, call).unwrap();
    assert_eq!(outcome.value, 10);
    let seen = reports.0.lock().unwrap().clone();
    // The clock reads 0 at the start and advances 100 ms per read: the first report at or past
    // the delay is the third step's.
    assert_eq!(PROGRESS_DELAY_MILLIS, 300);
    assert_eq!(seen.first().map(|r| r.0), Some(0.3));
    assert_eq!(seen.last().map(|r| r.0), Some(1.0));
    assert!(seen.iter().all(|r| r.1 == "test-long-step"));

    let before = doc.snapshot();
    let cancelling = Arc::new(Cancelling(arc_base::Cancel::new()));
    let mut call = Invocation::new("test.long.run", json!({"steps": 10}));
    call.cancel = cancelling.0.clone();
    call.progress = Some(cancelling);
    let error = Invoker::new(&r).invoke(&doc, &s, call).unwrap_err();
    assert_eq!(error.code(), "cmd.cancelled");
    assert!(doc.snapshot().ptr_eq(&before));
}

/// Cancels at the first report, as a user would.
struct Cancelling(arc_base::Cancel);

impl arc_command::ProgressSink for Cancelling {
    fn progress(&self, _: f64, _: &str) {
        self.0.cancel();
    }
}

#[test]
fn a_cancelled_query_returns_cmd_cancelled() {
    let (r, doc, s) = (registry(), document(), session());
    let call = Invocation::new("test.item.summary", json!({}));
    call.cancel.cancel();
    let error = Invoker::new(&r).invoke(&doc, &s, call).unwrap_err();
    assert_eq!(error.code(), "cmd.cancelled");
}
