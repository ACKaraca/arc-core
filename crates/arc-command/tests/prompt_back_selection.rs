//! Stepping back over a selection answer of a prompt sequence whose selection takes no objects
//! (`min` 0), with objects typed as text, as the command line types them.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use std::sync::Arc;

use arc_command::prompt::{Accept, Feedback, PromptSequence, PromptSession, PromptStep};
use arc_command::spec::CommandSpec;
use arc_doc::Document;
use common::{document, invoke, registry, session};
use serde_json::{Map, json};

#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
struct Trim {
    edges: Vec<String>,
    length: f64,
}

fn create(doc: &Document, name: &str) -> String {
    let registry = registry();
    let outcome = invoke(
        &registry,
        doc,
        &session(),
        "test.item.create",
        json!({"name": name, "length": 1.0}),
    )
    .unwrap();
    outcome.value["id"].as_str().unwrap().to_owned()
}

fn trim_spec() -> CommandSpec {
    CommandSpec::new("test.trim.run", "l", "d", |_, _: Trim| Ok(())).with_prompts(
        PromptSequence::new()
            .step(PromptStep::new(
                "edges",
                "m",
                Accept::Selection {
                    min: 0,
                    max: None,
                    types: Vec::new(),
                },
            ))
            .step(PromptStep::new("length", "m", Accept::distance())),
    )
}

fn asked(feedback: &Feedback) -> Option<&str> {
    match feedback {
        Feedback::Ask(prompt) => Some(prompt.key.as_str()),
        _ => None,
    }
}

#[test]
fn back_over_a_typed_selection_asks_the_selection_again_and_accepts_it_again() {
    let doc = document();
    let edge = create(&doc, "edge");
    let snapshot = doc.snapshot();
    let session = session();
    let (mut prompts, first) =
        PromptSession::start(Arc::new(trim_spec()), Map::new(), &session, &snapshot);
    assert_eq!(asked(&first), Some("edges"));

    let answered = prompts.answer_text(&edge, &snapshot);
    assert_eq!(asked(&answered), Some("length"), "{answered:?}");

    let back = prompts.back(&snapshot);
    assert_eq!(asked(&back), Some("edges"), "{back:?}");

    let again = prompts.answer_text(&edge, &snapshot);
    assert_eq!(asked(&again), Some("length"), "{again:?}");

    // Back from the first prompt asks it again rather than failing.
    assert_eq!(asked(&prompts.back(&snapshot)), Some("edges"));
    assert_eq!(asked(&prompts.back(&snapshot)), Some("edges"));
}
