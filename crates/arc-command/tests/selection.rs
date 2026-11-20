#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]
//! The session's selection (CORE-CMD-036): a read-only command sets it, the previous selection
//! is kept, the keywords `selection`, `previous` and `last` stand for identifier lists, and the
//! state is bounded and holds no document change.

mod common;

use std::collections::BTreeMap;

use arc_base::{Limits, ObjectId};
use arc_command::{
    Accept, CommandLine, CommandSpec, IdParam, Invocation, Invoker, Permission, PromptSequence,
    PromptStep, Registry, Session, Submitted, Surface, Target, UndoBehavior,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Ids {
    ids: Vec<IdParam>,
}

/// Sets the selection to `ids` from a read-only command.
fn set_command(r: &mut Registry) {
    r.register(
        CommandSpec::new(
            "test.select.set",
            "test-select-set",
            "test-select-set-description",
            |context, p: Ids| -> arc_base::Result<()> {
                context.set_selection(p.ids.into_iter().map(|i| i.0).collect())
            },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read]),
    )
    .unwrap();
}

/// Returns the identifiers it receives, so a test sees what the keywords expanded to.
fn echo_command(r: &mut Registry) {
    r.register(
        CommandSpec::new(
            "test.select.echo",
            "test-select-echo",
            "test-select-echo-description",
            |_context, p: Ids| -> arc_base::Result<Vec<IdParam>> { Ok(p.ids) },
        )
        .with_undo(UndoBehavior::NotUndoable)
        .with_permissions([Permission::Read])
        .with_prompts(PromptSequence::new().step(PromptStep::new(
            "ids",
            "test-select-ids",
            Accept::Selection {
                min: 1,
                max: None,
                types: Vec::new(),
            },
        ))),
    )
    .unwrap();
}

fn registry() -> Registry {
    let mut r = common::registry();
    set_command(&mut r);
    echo_command(&mut r);
    r
}

fn session(surface: Surface) -> Session {
    Session::new(surface, [Permission::Read, Permission::Edit])
}

/// Creates a named object through the common create command and returns its identifier.
fn create(
    invoker: &Invoker<'_>,
    doc: &arc_doc::Document,
    session: &Session,
    name: &str,
) -> ObjectId {
    let outcome = invoker
        .invoke(
            doc,
            session,
            Invocation::new("test.item.create", json!({"name": name, "length": 1.0})),
        )
        .unwrap();
    serde_json::from_value::<IdParam>(outcome.value["id"].clone())
        .unwrap()
        .0
}

fn ids_json(ids: &[ObjectId]) -> serde_json::Value {
    json!(ids.iter().map(ToString::to_string).collect::<Vec<_>>())
}

#[test]
fn a_read_only_command_sets_the_selection_without_an_undo_step_or_a_revision_change() {
    let r = registry();
    let doc = common::document();
    let session = session(Surface::Automation);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &session, "a");
    let b = create(&invoker, &doc, &session, "b");
    let revision = doc.snapshot().revision();
    let undo = doc.undo_label();
    let outcome = invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.select.set", json!({"ids": ids_json(&[a, b])})),
        )
        .unwrap();
    assert_eq!(outcome.selection, Some(vec![a, b]));
    assert!(outcome.txn_id.is_none());
    assert_eq!(doc.snapshot().revision(), revision, "a revision changed");
    assert_eq!(doc.undo_label(), undo, "an undo step was added");
    assert_eq!(session.current_selection(&doc.snapshot()), vec![a, b]);
}

#[test]
fn the_previous_selection_is_the_one_before_the_current() {
    let r = registry();
    let doc = common::document();
    let session = session(Surface::Automation);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &session, "a");
    let b = create(&invoker, &doc, &session, "b");
    for ids in [vec![a], vec![b]] {
        invoker
            .invoke(
                &doc,
                &session,
                Invocation::new("test.select.set", json!({"ids": ids_json(&ids)})),
            )
            .unwrap();
    }
    let snapshot = doc.snapshot();
    assert_eq!(session.current_selection(&snapshot), vec![b]);
    assert_eq!(session.previous_selection(&snapshot), vec![a]);
}

#[test]
fn keywords_expand_from_the_session_in_invocations_and_typed_lines() {
    let r = registry();
    let doc = common::document();
    let session = session(Surface::CommandLine);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &session, "a");
    let b = create(&invoker, &doc, &session, "b");
    invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.select.set", json!({"ids": ids_json(&[a])})),
        )
        .unwrap();
    invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.select.set", json!({"ids": ids_json(&[b])})),
        )
        .unwrap();
    let echo = |params| {
        invoker
            .invoke(&doc, &session, Invocation::new("test.select.echo", params))
            .unwrap()
            .value
    };
    assert_eq!(echo(json!({"ids": "selection"})), ids_json(&[b]));
    assert_eq!(echo(json!({"ids": ["previous"]})), ids_json(&[a]));

    let localized = BTreeMap::new();
    let target = Target {
        registry: &r,
        document: &doc,
        session: &session,
        selection: &[],
        localized: &localized,
        seed: 0,
    };
    let mut line = CommandLine::new();
    let Submitted::Ran(outcome) = line.submit(&target, "test.select.echo previous").unwrap() else {
        panic!("the typed line did not run");
    };
    assert_eq!(outcome.value, ids_json(&[a]));
    let Submitted::Ran(outcome) = line
        .submit(&target, "test.select.echo ids=selection")
        .unwrap()
    else {
        panic!("the typed line did not run");
    };
    assert_eq!(outcome.value, ids_json(&[b]));
}

#[test]
fn keywords_answer_a_selection_prompt() {
    let r = registry();
    let doc = common::document();
    let session = session(Surface::CommandLine);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &session, "a");
    invoker
        .invoke(
            &doc,
            &session,
            Invocation::new("test.select.set", json!({"ids": ids_json(&[a])})),
        )
        .unwrap();
    let localized = BTreeMap::new();
    let target = Target {
        registry: &r,
        document: &doc,
        session: &session,
        selection: &[],
        localized: &localized,
        seed: 0,
    };
    let mut line = CommandLine::new();
    assert!(matches!(
        line.submit(&target, "test.select.echo").unwrap(),
        Submitted::Prompt(_)
    ));
    let Submitted::Ran(outcome) = line.submit(&target, "selection").unwrap() else {
        panic!("the prompt answer did not run the command");
    };
    assert_eq!(outcome.value, ids_json(&[a]));
}

#[test]
fn last_is_the_products_hook_and_fails_without_one() {
    let r = registry();
    let doc = common::document();
    let plain = session(Surface::Automation);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &plain, "a");
    let error = invoker
        .invoke(
            &doc,
            &plain,
            Invocation::new("test.select.echo", json!({"ids": "last"})),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.selection.unavailable");

    let hooked = session(Surface::Automation).with_last_objects(move |_| vec![a]);
    let outcome = invoker
        .invoke(
            &doc,
            &hooked,
            Invocation::new("test.select.echo", json!({"ids": "last"})),
        )
        .unwrap();
    assert_eq!(outcome.value, ids_json(&[a]));
}

#[test]
fn identifiers_of_objects_that_no_longer_exist_are_dropped_when_read() {
    let r = registry();
    let doc = common::document();
    let session = session(Surface::Automation);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &session, "a");
    let gone: ObjectId = arc_base::id::IdGenerator::seeded(7).generate();
    let snapshot = doc.snapshot();
    session.record_selection(&[a, gone], &snapshot).unwrap();
    assert_eq!(session.current_selection(&snapshot), vec![a]);
}

#[test]
fn the_selection_is_bounded_by_the_session_limit() {
    let r = registry();
    let doc = common::document();
    let plain = session(Surface::Automation);
    let invoker = Invoker::new(&r);
    let a = create(&invoker, &doc, &plain, "a");
    let b = create(&invoker, &doc, &plain, "b");
    let mut limited = session(Surface::Automation);
    limited.limits = Limits {
        array_length: 1,
        ..Limits::DEFAULT
    };
    let snapshot = doc.snapshot();
    let refused = limited.record_selection(&[a, b], &snapshot).unwrap_err();
    assert_eq!(refused.code(), "cmd.params.limit_exceeded");
    let refused = invoker
        .invoke(
            &doc,
            &limited,
            Invocation::new("test.select.set", json!({"ids": ids_json(&[a, b])})),
        )
        .unwrap_err();
    assert_eq!(refused.code(), "cmd.params.limit_exceeded");
    assert!(limited.current_selection(&snapshot).is_empty());
}
