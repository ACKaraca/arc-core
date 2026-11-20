//! A command stores the length it ran with as a session setting, and the next prompt of the same
//! session offers it as the default (`PromptStep::with_remembered_default`).

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::sync::Arc;

use arc_command::prompt::{
    Accept, Feedback, PromptInput, PromptSequence, PromptSession, PromptStep,
};
use arc_command::spec::CommandSpec;
use arc_command::{Invocation, Invoker, Registry};
use common::{document, session};
use serde_json::{Map, json};

const KEY: &str = "test.trim.remembered_length";

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Trim {
    length: f64,
}

fn trim() -> CommandSpec {
    CommandSpec::new("test.trim.run", "l", "d", |ctx, p: Trim| {
        ctx.set_setting(KEY, json!(p.length))
    })
    .with_prompts(
        PromptSequence::new()
            .step(PromptStep::new("length", "m", Accept::distance()).with_remembered_default(KEY)),
    )
}

#[test]
fn a_command_stores_its_length_and_the_next_prompt_offers_it() {
    let doc = document();
    let snapshot = doc.snapshot();
    let session = session();
    let mut registry = Registry::new();
    registry.register(trim()).unwrap();

    let (mut first, asked) =
        PromptSession::start(Arc::new(trim()), Map::new(), &session, &snapshot);
    assert!(
        matches!(&asked, Feedback::Ask(p) if p.default.is_none()),
        "{asked:?}"
    );
    let Feedback::Done(invocation) = first.answer(PromptInput::Distance(2.5), &snapshot) else {
        panic!("the length prompt did not finish")
    };
    Invoker::new(&registry)
        .invoke(
            &doc,
            &session,
            Invocation::new("test.trim.run", invocation.params),
        )
        .unwrap();
    assert_eq!(session.setting(KEY), Some(json!(2.5)));

    // The next session of the same host offers the stored length.
    let (mut next, asked) = PromptSession::start(Arc::new(trim()), Map::new(), &session, &snapshot);
    let Feedback::Ask(prompt) = &asked else {
        panic!("{asked:?}")
    };
    assert!(matches!(prompt.default, Some(PromptInput::Distance(v)) if (v - 2.5).abs() < 1e-12));
    assert!(matches!(next.accept_default(&snapshot), Feedback::Done(_)));
}

#[test]
fn a_failed_command_stores_nothing() {
    let doc = document();
    let session = session();
    let mut registry = Registry::new();
    registry
        .register(
            CommandSpec::new("test.trim.fail", "l", "d", |ctx, p: Trim| {
                ctx.set_setting(KEY, json!(p.length))?;
                Err::<(), _>(arc_base::Error::new(
                    arc_command::error::codes::PROMPT_ENDED,
                ))
            })
            .with_prompts(PromptSequence::new().step(
                PromptStep::new("length", "m", Accept::distance()).with_remembered_default(KEY),
            )),
        )
        .unwrap();
    let _ = Invoker::new(&registry).invoke(
        &doc,
        &session,
        Invocation::new("test.trim.fail", json!({"length": 7.0})),
    );
    assert_eq!(session.setting(KEY), None);
}
