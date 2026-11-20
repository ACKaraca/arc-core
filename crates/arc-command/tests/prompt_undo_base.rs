//! The base point and the direction lock of a point session are restored by `back()`: a relative
//! point typed after an undo measures from the point that held before the undone answer.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

mod common;

use arc_base::Unit;
use arc_command::{CommandLine, PromptInput, Submitted, Target};
use common::{document, registry, session};

struct Env {
    registry: arc_command::Registry,
    document: arc_doc::Document,
    session: arc_command::Session,
    localized: std::collections::BTreeMap<String, String>,
}

impl Env {
    fn new() -> Self {
        let mut session = session();
        session.units = vec![
            *Unit::by_name("meter").unwrap(),
            *Unit::by_name("degree").unwrap(),
        ];
        Self {
            registry: registry(),
            document: document(),
            session,
            localized: std::collections::BTreeMap::new(),
        }
    }

    fn target(&self) -> Target<'_> {
        Target {
            registry: &self.registry,
            document: &self.document,
            session: &self.session,
            selection: &[],
            localized: &self.localized,
            seed: 7,
        }
    }
}

fn answered_points(line: &CommandLine) -> Vec<(f64, f64)> {
    line.active()
        .unwrap()
        .answers()
        .iter()
        .filter_map(|a| match &a.input {
            PromptInput::Point(p) => Some((p.x, p.y)),
            _ => None,
        })
        .collect()
}

fn key(submitted: &Submitted) -> &str {
    match submitted {
        Submitted::Prompt(prompt) => prompt.key.as_str(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn relative_point_after_back_measures_from_the_point_before_the_undone_one() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    assert_eq!(key(&line.submit(&t, "1,2").unwrap()), "end");
    assert_eq!(key(&line.submit(&t, "5,6").unwrap()), "name");
    // Undo the answer after the second point: the session asks the second point again.
    assert_eq!(key(&line.back(&t).unwrap()), "end");
    assert_eq!(answered_points(&line), vec![(1.0, 2.0)]);
    // `@1,0` is relative to the first point (1, 2), not to the undone one (5, 6).
    assert_eq!(key(&line.submit(&t, "@1,0").unwrap()), "name");
    assert_eq!(answered_points(&line), vec![(1.0, 2.0), (2.0, 2.0)]);
}

#[test]
fn back_restores_the_direction_lock_that_an_accepted_point_released() {
    let env = Env::new();
    let t = env.target();
    let mut line = CommandLine::new();
    line.submit(&t, "test.shape.draw").unwrap();
    assert_eq!(key(&line.submit(&t, "1,2").unwrap()), "end");
    // `<90` locks a direction for the next point; the point answer that follows releases it.
    assert_eq!(key(&line.submit(&t, "<90").unwrap()), "end");
    let locked = line.active().unwrap().point_entry().locked_direction();
    assert!(locked.is_some());
    assert_eq!(key(&line.submit(&t, "5,6").unwrap()), "name");
    assert_eq!(
        line.active().unwrap().point_entry().locked_direction(),
        None
    );
    // Undo the second point: its lock is in force again, and the base is the first point.
    assert_eq!(key(&line.back(&t).unwrap()), "end");
    let restored = line.active().unwrap().point_entry().locked_direction();
    assert_eq!(restored, locked);
}
