//! Undo and redo through `Document` (CORE-DOC-019 to CORE-DOC-023 wired into the executor):
//! random sequences of commands, undos, redos, nested groups with failing owners and gestures
//! with coalesce keys run as jobs of a document whose regeneration scheduler runs, and are
//! compared after every step with a reference model that keeps, per undo step, the states
//! before and after it. The document state, `can_undo`, `can_redo`, the open group and the
//! journal length must equal the model's.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::graph::Evaluators;
use arc_doc::{
    Document, FieldDecl, FieldPath, Header, Schema, SchemaVersion, Snapshot, SubmitOptions,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};
use proptest::prelude::*;

const KEY: &str = "test.n";

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    registry
        .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

type State = BTreeMap<u128, i64>;

fn state(snapshot: &Snapshot) -> State {
    snapshot
        .iter()
        .map(|(_, object)| {
            let n = match object.values.first() {
                Some(Some(Value::I64(n))) => *n,
                _ => i64::MIN,
            };
            (object.id.as_u128(), n)
        })
        .collect()
}

#[derive(Debug, Clone)]
enum Edit {
    Create(i64),
    Set(usize, i64),
    Delete(usize),
}

#[derive(Debug, Clone)]
enum Action {
    Commit(Vec<Edit>, Option<u8>),
    Undo,
    Redo,
    BeginGroup,
    EndGroup,
    FailGroup,
    BeginGesture,
    EndGesture,
}

fn action() -> impl Strategy<Value = Action> {
    let edit = prop_oneof![
        (-3..3_i64).prop_map(Edit::Create),
        (any::<usize>(), -3..3_i64).prop_map(|(p, n)| Edit::Set(p, n)),
        any::<usize>().prop_map(Edit::Delete),
    ];
    prop_oneof![
        6 => (prop::collection::vec(edit, 0..4), prop::option::of(0..2_u8))
            .prop_map(|(edits, key)| Action::Commit(edits, key)),
        4 => Just(Action::Undo),
        3 => Just(Action::Redo),
        1 => Just(Action::BeginGroup),
        1 => Just(Action::EndGroup),
        1 => Just(Action::FailGroup),
        1 => Just(Action::BeginGesture),
        1 => Just(Action::EndGesture),
    ]
}

#[derive(Debug, Clone)]
struct Step {
    before: State,
    after: State,
    coalesce: Option<(u64, u8)>,
}

#[derive(Debug, Default)]
struct Model {
    state: State,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Per open group: the members of the open step when it began and the state then.
    groups: Vec<(usize, State)>,
    open: Option<(Step, usize)>,
    gesture: Option<u64>,
    gestures: u64,
}

impl Model {
    fn committed(&mut self, before: State, key: Option<u8>) {
        self.redo.clear();
        if let Some((open, members)) = &mut self.open {
            open.after = self.state.clone();
            *members += 1;
            return;
        }
        let coalesce = self.gesture.zip(key);
        if let Some(top) = self.undo.last_mut()
            && coalesce.is_some()
            && top.coalesce == coalesce
        {
            top.after = self.state.clone();
            return;
        }
        self.undo.push(Step {
            before,
            after: self.state.clone(),
            coalesce,
        });
    }
}

/// The concrete operations of `edits` on `state`, applied to it.
fn plan(state: &mut State, next_id: &mut u128, edits: &[Edit]) -> Vec<(u128, Option<i64>, bool)> {
    let mut ops = Vec::new();
    for edit in edits {
        let pick = |state: &State, p: usize| {
            let ids: Vec<u128> = state.keys().copied().collect();
            (!ids.is_empty()).then(|| ids[p % ids.len()])
        };
        match *edit {
            Edit::Create(n) => {
                *next_id += 1;
                state.insert(*next_id, n);
                ops.push((*next_id, Some(n), true));
            }
            Edit::Set(p, n) => {
                if let Some(id) = pick(state, p) {
                    state.insert(id, n);
                    ops.push((id, Some(n), false));
                }
            }
            Edit::Delete(p) => {
                if let Some(id) = pick(state, p) {
                    state.remove(&id);
                    ops.push((id, None, false));
                }
            }
        }
    }
    ops
}

#[allow(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "one match arm per action"
)]
fn run(actions: &[Action]) {
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    let mut model = Model::default();
    let mut commits = 0;
    let mut groups = 0;
    let mut next_id = 0;
    for action in actions {
        match action {
            Action::Commit(edits, key) => {
                let before = model.state.clone();
                let mut next = model.state.clone();
                let ops = plan(&mut next, &mut next_id, edits);
                let options = key.map_or_else(SubmitOptions::default, |k| {
                    SubmitOptions::coalesce(format!("k{k}"))
                });
                let done = document
                    .submit_with(options, move |txn| {
                        for (id, n, create) in ops {
                            let id = ObjectId::from_u128(id);
                            match (n, create) {
                                (Some(n), true) => txn
                                    .create(KEY, id, Header::default(), vec![Some(Value::I64(n))])
                                    .map(drop)?,
                                (Some(n), false) => {
                                    txn.modify(id, FieldPath::new("n"), Some(Value::I64(n)))?;
                                }
                                (None, _) => txn.delete(id)?,
                            }
                        }
                        Ok(())
                    })
                    .wait()
                    .unwrap();
                if done.txn_id.is_some() {
                    commits += 1;
                    model.state = next;
                    model.committed(before, *key);
                }
            }
            Action::Undo => {
                let done = document.undo().wait();
                if !model.groups.is_empty() {
                    assert_eq!(done.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                let done = done.unwrap();
                let Some(mut step) = model.undo.pop() else {
                    assert!(!done.value);
                    continue;
                };
                assert!(done.value && done.txn_id.is_some());
                commits += 1;
                model.state = step.before.clone();
                step.coalesce = None;
                model.redo.push(step);
                if let Some(top) = model.undo.last_mut() {
                    top.coalesce = None;
                }
            }
            Action::Redo => {
                let done = document.redo().wait();
                if !model.groups.is_empty() {
                    assert_eq!(done.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                let done = done.unwrap();
                let Some(step) = model.redo.pop() else {
                    assert!(!done.value);
                    continue;
                };
                assert!(done.value);
                commits += 1;
                model.state = step.after.clone();
                model.undo.push(Step {
                    coalesce: None,
                    ..step
                });
            }
            Action::BeginGroup => {
                groups += 1;
                document
                    .begin_group(format!("\"g{groups}\""))
                    .wait()
                    .unwrap();
                let start = model.open.as_ref().map_or(0, |(_, members)| *members);
                if model.open.is_none() {
                    let step = Step {
                        before: model.state.clone(),
                        after: model.state.clone(),
                        coalesce: None,
                    };
                    model.open = Some((step, 0));
                }
                model.groups.push((start, model.state.clone()));
            }
            Action::EndGroup => {
                let result = document.end_group().wait();
                if model.groups.pop().is_none() {
                    assert_eq!(result.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                result.unwrap();
                if model.groups.is_empty() {
                    let (open, members) = model.open.take().unwrap();
                    if members > 0 {
                        model.undo.push(open);
                    }
                }
            }
            Action::FailGroup => {
                let result = document.fail_group().wait();
                let Some((start, at_begin)) = model.groups.pop() else {
                    assert_eq!(result.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                };
                let (open, members) = model.open.as_mut().unwrap();
                let rolled_back = *members - start;
                *members = start;
                open.after = at_begin.clone();
                assert_eq!(result.unwrap().value, rolled_back > 0);
                if rolled_back > 0 {
                    commits += 1;
                }
                model.state = at_begin;
                if model.groups.is_empty() {
                    model.open = None;
                }
            }
            Action::BeginGesture => {
                document.begin_gesture().wait().unwrap();
                model.gesture = Some(model.gestures);
                model.gestures += 1;
            }
            Action::EndGesture => {
                document.end_gesture().wait().unwrap();
                model.gesture = None;
            }
        }
        assert_eq!(state(&document.snapshot()), model.state);
        let outside = model.groups.is_empty();
        assert_eq!(document.can_undo(), outside && !model.undo.is_empty());
        assert_eq!(document.can_redo(), outside && !model.redo.is_empty());
        assert_eq!(document.with_journal(arc_doc::Journal::len), commits);
        let (undo, redo) =
            document.with_undo(|undo| (undo.undo_steps().count(), undo.redo_steps().count()));
        assert_eq!((undo, redo), (model.undo.len(), model.redo.len()));
        assert_eq!(document.with_undo(|undo| undo.group().is_some()), !outside);
    }
}

proptest! {
    #[test]
    fn document_undo_redo_groups_and_gestures_match_the_model(
        actions in prop::collection::vec(action(), 0..40)
    ) {
        run(&actions);
    }
}

#[test]
fn labels_and_the_journal_follow_the_commits() {
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    assert!(!document.can_undo() && document.undo_label().is_none());
    document
        .submit(|txn| {
            txn.meta_mut().label = "\"create\"".into();
            txn.create(
                KEY,
                ObjectId::from_u128(1),
                Header::default(),
                vec![Some(Value::I64(1))],
            )
            .map(drop)
        })
        .wait()
        .unwrap();
    assert_eq!(document.undo_label().as_deref(), Some("\"create\""));
    assert!(document.undo().wait().unwrap().value);
    assert_eq!(document.redo_label().as_deref(), Some("\"create\""));
    assert!(document.undo_label().is_none());
    assert!(document.redo().wait().unwrap().value);
    assert_eq!(state(&document.snapshot()), State::from([(1, 1)]));
    document.with_journal(|journal| {
        let kinds: Vec<_> = journal.iter().map(|e| e.record().kind).collect();
        assert_eq!(kinds.len(), 3);
        assert!(matches!(kinds[1], arc_doc::TxnKind::UndoOf(_)));
        assert!(matches!(kinds[2], arc_doc::TxnKind::RedoOf(_)));
    });
    // A job naming another group than the open one fails before its commit.
    document.begin_group("\"outer\"").wait().unwrap();
    let error = document
        .submit(|txn| {
            txn.meta_mut().group = Some("\"other\"".into());
            txn.modify(
                ObjectId::from_u128(1),
                FieldPath::new("n"),
                Some(Value::I64(2)),
            )
        })
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.undo.sequence_invalid");
    assert_eq!(state(&document.snapshot()), State::from([(1, 1)]));
}

#[test]
fn a_coalesced_edit_moves_the_undo_position() {
    let document = Document::new(registry(), Arc::new(Evaluators::new())).unwrap();
    document.begin_gesture().wait().unwrap();
    let edit = |n: i64, create: bool| {
        document
            .submit_with(SubmitOptions::coalesce("k".to_owned()), move |txn| {
                let id = ObjectId::from_u128(1);
                if create {
                    txn.create(KEY, id, Header::default(), vec![Some(Value::I64(n))])
                        .map(drop)
                } else {
                    txn.modify(id, FieldPath::new("n"), Some(Value::I64(n)))
                }
            })
            .wait()
            .unwrap();
    };
    edit(1, true);
    let saved = document.undo_position();
    edit(2, false);
    // The second edit joined the first step; a save taken before it must not look current.
    assert_ne!(document.undo_position(), saved);
    document.end_gesture().wait().unwrap();
}
