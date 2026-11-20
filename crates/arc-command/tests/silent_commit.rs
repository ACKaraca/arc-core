//! A command that commits silently: bookkeeping that is journaled but is not an undo step and
//! leaves both stacks alone (`Context::commit_silently`).

#![allow(clippy::unwrap_used, reason = "tests")]

mod common;

use arc_command::{CommandSpec, Context, Invocation, Registry, UndoBehavior};
use arc_doc::{StoredForeign, TxnKind};
use common::{document, invoke, registry, session};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Note {
    text: String,
}

fn note(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    ctx.txn()?.add_foreign(StoredForeign {
        source_format: "test".to_owned(),
        record_kind: "note".to_owned(),
        data: p.text.into_bytes(),
        ..StoredForeign::default()
    })?;
    Ok(())
}

/// Silent, but also creates an object, which a silent commit must refuse.
fn sneak(ctx: &mut Context<'_>, _: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    ctx.after_commit(|_, _| {
        AFTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })?;
    let id = ctx.new_id();
    ctx.txn()?
        .create(
            common::ITEM,
            id,
            arc_doc::Header::default(),
            vec![
                Some(arc_doc::Value::Quantity(arc_base::Quantity::new(
                    1.0,
                    arc_base::Dimension::LENGTH,
                ))),
                Some(arc_doc::Value::String("x".into())),
            ],
        )
        .map(drop)
}

/// A command without a transaction asking for a silent commit.
fn read_only(ctx: &mut Context<'_>, _: Note) -> arc_base::Result<()> {
    ctx.commit_silently()
}

/// A command without a transaction asking for an after-commit action.
fn read_only_action(ctx: &mut Context<'_>, _: Note) -> arc_base::Result<()> {
    ctx.after_commit(|_, _| Ok(()))
}

fn row(text: &str) -> StoredForeign {
    StoredForeign {
        source_format: "test".to_owned(),
        record_kind: "note".to_owned(),
        data: text.as_bytes().to_vec(),
        ..StoredForeign::default()
    }
}

/// A normal (undoable step) command that adds a row, like a setting or a coordinate system.
fn add(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<()> {
    ctx.txn()?.add_foreign(row(&p.text)).map(drop)
}

/// A normal command that removes the row with the highest key.
fn drop_last(ctx: &mut Context<'_>, _: Note) -> arc_base::Result<()> {
    let last = ctx.snapshot().foreign_rows().last().map(|r| r.id);
    match last {
        Some(id) => ctx.txn()?.remove_foreign(id),
        None => Ok(()),
    }
}

/// Silent, but rewrites the lowest row.
fn rewrite(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    let first = ctx.snapshot().foreign_rows().next().map(|r| r.id);
    ctx.txn()?.set_foreign(StoredForeign {
        id: first.unwrap_or(1),
        ..row(&p.text)
    })
}

/// Silent, but removes the highest row.
fn remove_silently(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    drop_last(ctx, p)
}

/// Silent, and takes key 1, which an undone step named.
fn low_key(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<()> {
    ctx.commit_silently()?;
    ctx.txn()?.set_foreign(StoredForeign {
        id: 1,
        ..row(&p.text)
    })
}

static AFTER: AtomicUsize = AtomicUsize::new(0);

#[derive(serde::Serialize, JsonSchema)]
struct Done {
    log: Vec<String>,
}

/// Silent; its after-commit action counts and amends the result, or fails when asked to.
fn finish(ctx: &mut Context<'_>, p: Note) -> arc_base::Result<Done> {
    ctx.commit_silently()?;
    ctx.txn()?.add_foreign(row(&p.text))?;
    let fail = p.text == "fail";
    ctx.after_commit(move |_, value| {
        AFTER.fetch_add(1, Ordering::SeqCst);
        if fail {
            return Err(arc_base::Error::new(arc_command::error::codes::PATH_IO));
        }
        value["log"] = json!(["after"]);
        Ok(())
    })?;
    Ok(Done { log: Vec::new() })
}

fn registry_with_notes() -> Registry {
    let mut r = registry();
    for (id, run) in [
        ("test.row.add", add as fn(&mut Context<'_>, Note) -> _),
        ("test.row.drop", drop_last),
        ("test.row.rewrite", rewrite),
        ("test.row.remove", remove_silently),
        ("test.row.low", low_key),
    ] {
        r.register(CommandSpec::new(id, "l", "d", run)).unwrap();
    }
    r.register(
        CommandSpec::new("test.row.peek_action", "l", "d", read_only_action)
            .with_undo(UndoBehavior::NotUndoable),
    )
    .unwrap();
    r.register(CommandSpec::new("test.row.finish", "l", "d", finish))
        .unwrap();
    r.register(CommandSpec::new("test.row.note", "l", "d", note))
        .unwrap();
    r.register(CommandSpec::new("test.row.sneak", "l", "d", sneak))
        .unwrap();
    r.register(
        CommandSpec::new("test.row.peek", "l", "d", read_only).with_undo(UndoBehavior::NotUndoable),
    )
    .unwrap();
    r
}

fn notes(doc: &arc_doc::Document) -> usize {
    doc.snapshot()
        .foreign_rows()
        .filter(|row| row.record_kind == "note")
        .count()
}

fn create(r: &Registry, doc: &arc_doc::Document, s: &arc_command::Session, name: &str) {
    invoke(
        r,
        doc,
        s,
        "test.item.create",
        json!({"name": name, "length": 1}),
    )
    .unwrap();
}

#[test]
fn a_silent_commit_is_journaled_but_no_step_and_both_stacks_survive_it() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    create(&r, &doc, &s, "a");
    create(&r, &doc, &s, "b");
    assert!(doc.undo().wait().unwrap().value);
    let (undo_before, redo_before) = (doc.undo_step_list(10).len(), doc.redo_step_list(10).len());
    assert_eq!((undo_before, redo_before), (1, 1));
    let journal = doc.with_journal(arc_doc::Journal::len);

    let outcome = invoke(&r, &doc, &s, "test.row.note", json!({"text": "one"})).unwrap();

    assert!(outcome.txn_id.is_some());
    assert_eq!(notes(&doc), 1);
    assert_eq!(doc.with_journal(arc_doc::Journal::len), journal + 1);
    let kind = doc.with_journal(|j| j.iter().last().map(|e| e.record().kind));
    assert_eq!(kind, Some(TxnKind::Merge));
    assert_eq!(doc.undo_step_list(10).len(), undo_before);
    assert_eq!(doc.redo_step_list(10).len(), redo_before);
}

#[test]
fn steps_recorded_before_a_silent_commit_still_undo_and_redo_around_it() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    create(&r, &doc, &s, "a");
    create(&r, &doc, &s, "b");
    invoke(&r, &doc, &s, "test.row.note", json!({"text": "one"})).unwrap();
    let with_both = doc.snapshot();

    // The undo takes the user's last edit, not the bookkeeping, and the row stays.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), 1);
    assert_eq!(notes(&doc), 1);
    // A second silent commit does not clear the redo stack.
    invoke(&r, &doc, &s, "test.row.note", json!({"text": "two"})).unwrap();
    assert_eq!(doc.redo_step_list(10).len(), 1);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), 0);
    assert!(doc.redo().wait().unwrap().value);
    assert!(doc.redo().wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), with_both.iter().count());
    assert_eq!(notes(&doc), 2);
    assert!(!doc.can_redo());
}

#[test]
fn a_silent_commit_may_write_only_foreign_rows() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    let before = doc.snapshot();
    let ran = AFTER.load(Ordering::SeqCst);
    let error = invoke(&r, &doc, &s, "test.row.sneak", json!({"text": "x"})).unwrap_err();
    assert_eq!(error.code(), "cmd.silent_scope");
    assert_eq!(
        AFTER.load(Ordering::SeqCst),
        ran,
        "a refused commit runs no action"
    );
    assert!(doc.snapshot().ptr_eq(&before));
    assert_eq!(doc.with_journal(arc_doc::Journal::len), 0);
}

#[test]
fn a_command_without_a_transaction_cannot_commit_silently() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    let error = invoke(&r, &doc, &s, "test.row.peek", json!({"text": "x"})).unwrap_err();
    assert_eq!(error.code(), "cmd.no_transaction");
}

fn keys(doc: &arc_doc::Document) -> Vec<i64> {
    doc.snapshot().foreign_rows().map(|r| r.id).collect()
}

fn run(r: &Registry, doc: &arc_doc::Document, s: &arc_command::Session, command: &str) {
    invoke(r, doc, s, command, json!({"text": "t"})).unwrap();
}

#[test]
fn a_silent_row_never_takes_a_key_an_undone_step_names() {
    // A: a step adds the highest row, is undone, a silent record follows, the redo still applies.
    let (r, doc, s) = (registry_with_notes(), document(), session());
    run(&r, &doc, &s, "test.row.add");
    run(&r, &doc, &s, "test.row.add");
    assert!(doc.undo().wait().unwrap().value);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(keys(&doc), Vec::<i64>::new());
    run(&r, &doc, &s, "test.row.note");
    assert_eq!(keys(&doc), vec![3]);
    assert!(doc.redo().wait().unwrap().value);
    assert!(doc.redo().wait().unwrap().value);
    assert_eq!(keys(&doc), vec![1, 2, 3]);
    // And back again: the undo of both steps, with the silent row in between, still applies.
    assert!(doc.undo().wait().unwrap().value);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(keys(&doc), vec![3]);
}

#[test]
fn a_silent_row_never_takes_the_key_a_removed_row_had() {
    // B: a step removes the highest row, a silent record follows, the undo of the removal applies.
    let (r, doc, s) = (registry_with_notes(), document(), session());
    run(&r, &doc, &s, "test.row.add");
    run(&r, &doc, &s, "test.row.drop");
    run(&r, &doc, &s, "test.row.note");
    assert_eq!(keys(&doc), vec![2]);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(keys(&doc), vec![1, 2]);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(keys(&doc), vec![2]);
}

#[test]
fn a_silent_commit_only_adds_rows_above_the_floor() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    run(&r, &doc, &s, "test.row.add");
    for command in ["test.row.rewrite", "test.row.remove"] {
        let error = invoke(&r, &doc, &s, command, json!({"text": "other"})).unwrap_err();
        assert_eq!(error.code(), "cmd.silent_scope", "{command}");
    }
    // Key 1 is absent after the undo but the history names it.
    assert!(doc.undo().wait().unwrap().value);
    let error = invoke(&r, &doc, &s, "test.row.low", json!({"text": "other"})).unwrap_err();
    assert_eq!(error.code(), "cmd.silent_scope");
    assert_eq!(keys(&doc), Vec::<i64>::new());
}

#[test]
fn a_silent_commit_in_an_open_group_forms_no_step_and_the_group_is_one_step() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    doc.begin_group("g").wait().unwrap();
    for command in ["test.item.create", "test.row.note", "test.item.create"] {
        let mut invocation = Invocation::new(
            command,
            if command == "test.row.note" {
                json!({"text": "t"})
            } else {
                json!({"name": "a", "length": 1})
            },
        );
        invocation.group = Some("g".to_owned());
        arc_command::Invoker::new(&r)
            .invoke(&doc, &s, invocation)
            .unwrap();
    }
    doc.end_group().wait().unwrap();
    assert_eq!(doc.undo_step_list(10).len(), 1);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), 0);
    assert_eq!(keys(&doc).len(), 1);
}

#[test]
fn a_silent_commit_in_a_gesture_does_not_split_the_coalesced_step() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    let id: arc_base::ObjectId = invoke(
        &r,
        &doc,
        &s,
        "test.item.create",
        json!({"name": "a", "length": 1}),
    )
    .unwrap()
    .value["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let rename = |doc: &arc_doc::Document, name: &'static str| {
        doc.submit_with(arc_doc::SubmitOptions::coalesce("name"), move |txn| {
            txn.modify(
                id,
                arc_doc::FieldPath::new("name"),
                Some(arc_doc::Value::String(name.to_owned())),
            )
        })
        .wait()
        .unwrap();
    };
    doc.begin_gesture().wait().unwrap();
    rename(&doc, "b");
    run(&r, &doc, &s, "test.row.note");
    rename(&doc, "c");
    doc.end_gesture().wait().unwrap();
    let steps = doc.undo_step_list(10);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].transactions, 2, "{steps:?}");
}

#[test]
fn undo_many_counts_steps_and_skips_silent_records() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    for name in ["a", "b"] {
        create(&r, &doc, &s, name);
        run(&r, &doc, &s, "test.row.note");
    }
    create(&r, &doc, &s, "c");
    assert_eq!(doc.undo_step_list(10).len(), 3);
    assert!(doc.undo_many(3).wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), 0);
    assert_eq!(keys(&doc).len(), 2);
    assert!(doc.redo_many(3).wait().unwrap().value);
    assert_eq!(doc.snapshot().iter().count(), 3);
}

#[test]
fn a_dry_run_of_a_silent_command_commits_and_runs_nothing() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    create(&r, &doc, &s, "a");
    assert!(doc.undo().wait().unwrap().value);
    let before = (doc.snapshot(), doc.with_journal(arc_doc::Journal::len));
    let ran = AFTER.load(Ordering::SeqCst);
    let preview = arc_command::Invoker::new(&r)
        .dry_run(
            &doc,
            &s,
            Invocation::new("test.row.finish", json!({"text": "t"})),
        )
        .unwrap();
    assert!(preview.outcome.txn_id.is_none());
    assert!(doc.snapshot().ptr_eq(&before.0));
    assert_eq!(doc.with_journal(arc_doc::Journal::len), before.1);
    assert_eq!(doc.redo_step_list(10).len(), 1);
    assert_eq!(AFTER.load(Ordering::SeqCst), ran);
}

#[test]
fn an_after_commit_action_runs_once_the_transaction_committed_and_amends_the_result() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    let ran = AFTER.load(Ordering::SeqCst);
    let outcome = invoke(&r, &doc, &s, "test.row.finish", json!({"text": "t"})).unwrap();
    assert_eq!(outcome.value["log"], json!(["after"]));
    assert_eq!(AFTER.load(Ordering::SeqCst), ran + 1);
    assert_eq!(keys(&doc).len(), 1);
    // A failing action fails the command, but the transaction stays committed.
    let error = invoke(&r, &doc, &s, "test.row.finish", json!({"text": "fail"})).unwrap_err();
    assert_eq!(error.code(), "cmd.path.io");
    assert_eq!(keys(&doc).len(), 2);
}

#[test]
fn a_command_without_a_transaction_cannot_ask_for_an_after_commit_action() {
    let (r, doc, s) = (registry_with_notes(), document(), session());
    let error = invoke(&r, &doc, &s, "test.row.peek_action", json!({"text": "x"})).unwrap_err();
    assert_eq!(error.code(), "cmd.no_transaction");
}
