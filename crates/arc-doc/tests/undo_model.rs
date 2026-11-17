//! Model-based test of undo and redo (CORE-DOC-020 to CORE-DOC-023): random sequences of edits,
//! undos, redos, nested groups with failing owners, gestures with coalesce keys and changing
//! limits are compared after every step with a reference model that keeps, per undo step, the
//! states before and after it. The document state must equal the model's state, and the undo
//! and redo stacks must have the model's shape: the number of transactions and the memory of
//! each step. The journal must keep one record per commit.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId, Result, SessionId, TransactionId};
use arc_doc::{
    FieldDecl, FieldPath, Header, Journal, Properties, Schema, SchemaVersion, Snapshot,
    SnapshotCell, Txn, TxnLimits, TxnMeta, TypeDecl, TypeKey, TypeRegistry, UndoLimits,
    UndoManager, Value, ValueType, check_references,
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

/// The state of the document: `n` and name per object.
type State = BTreeMap<u128, (i64, Option<String>)>;

fn state(snapshot: &Snapshot) -> State {
    snapshot
        .iter()
        .map(|(_, object)| {
            let n = match object.values.first() {
                Some(Some(Value::I64(n))) => *n,
                _ => i64::MIN,
            };
            (object.id.as_u128(), (n, object.header.name.clone()))
        })
        .collect()
}

/// The document under test: published snapshot, journal and undo manager.
struct Doc {
    cell: SnapshotCell,
    journal: Journal,
    undo: UndoManager,
    next: u128,
}

impl Doc {
    fn new(limits: UndoLimits) -> Self {
        Self {
            cell: SnapshotCell::new(Snapshot::empty(registry())),
            journal: Journal::default(),
            undo: UndoManager::new(limits),
            next: 1,
        }
    }

    fn meta(&mut self) -> TxnMeta {
        let mut meta = TxnMeta::new(TransactionId::from_u128(self.next), SessionId::from_u128(1));
        self.next += 1;
        meta.parent = self.journal.last().map(|entry| entry.record().txn_id);
        meta.group = self.undo.group().map(str::to_owned);
        meta
    }

    /// Commits `txn` the way the commit unit does and reports it.
    fn commit(&mut self, mut txn: Txn, coalesce: Option<&str>) -> Result<u64> {
        let before = txn.base().clone();
        txn.apply_delete_rules(&Limits::DEFAULT)?;
        let parts = txn.into_parts();
        check_references(&parts.set)?;
        self.cell.publish(&parts.set)?;
        let after = self.cell.load();
        let id = parts.meta.id;
        let bytes = self
            .journal
            .append(parts.meta, parts.ops, &before, &after)?
            .memory_bytes();
        self.undo.committed(&self.journal, id, coalesce)?;
        Ok(bytes)
    }
}

#[derive(Debug, Clone)]
enum Edit {
    Create(i64),
    SetN(usize, i64),
    Rename(usize, Option<u8>),
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
    Limits(usize, u64),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (-3..3_i64).prop_map(Edit::Create),
        (any::<usize>(), -3..3_i64).prop_map(|(p, n)| Edit::SetN(p, n)),
        (any::<usize>(), prop::option::of(0..3_u8)).prop_map(|(p, n)| Edit::Rename(p, n)),
        any::<usize>().prop_map(Edit::Delete),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        6 => (prop::collection::vec(edit(), 0..4), prop::option::of(0..2_u8))
            .prop_map(|(edits, key)| Action::Commit(edits, key)),
        4 => Just(Action::Undo),
        3 => Just(Action::Redo),
        1 => Just(Action::BeginGroup),
        1 => Just(Action::EndGroup),
        1 => Just(Action::FailGroup),
        1 => Just(Action::BeginGesture),
        1 => Just(Action::EndGesture),
        1 => (1..6_usize, prop_oneof![Just(u64::MAX), 300..3_000_u64])
            .prop_map(|(steps, bytes)| Action::Limits(steps, bytes)),
    ]
}

/// One step of the model's stacks.
#[derive(Debug, Clone)]
struct ModelStep {
    before: State,
    after: State,
    bytes: Vec<u64>,
    coalesce: Option<(u64, u8)>,
}

#[derive(Debug)]
struct Model {
    state: State,
    undo: VecDeque<ModelStep>,
    redo: Vec<ModelStep>,
    limits: UndoLimits,
    /// Per open group: transactions of the open step and the state when it began.
    groups: Vec<(usize, State)>,
    open: Option<ModelStep>,
    gesture: Option<u64>,
    gestures: u64,
}

impl Model {
    fn push(&mut self, step: ModelStep) {
        self.undo.push_back(step);
        self.trim();
    }

    fn trim(&mut self) {
        let total =
            |undo: &VecDeque<ModelStep>| -> u64 { undo.iter().flat_map(|s| &s.bytes).sum() };
        while self.undo.len() > self.limits.steps || total(&self.undo) > self.limits.memory_bytes {
            self.undo.pop_front();
        }
    }

    fn committed(&mut self, before: State, bytes: u64, key: Option<u8>) {
        self.redo.clear();
        if let Some(open) = &mut self.open {
            open.bytes.push(bytes);
            open.after = self.state.clone();
            return;
        }
        let coalesce = self.gesture.zip(key);
        if let Some(top) = self.undo.back_mut()
            && coalesce.is_some()
            && top.coalesce == coalesce
        {
            top.bytes.push(bytes);
            top.after = self.state.clone();
            self.trim();
            return;
        }
        self.push(ModelStep {
            before,
            after: self.state.clone(),
            bytes: vec![bytes],
            coalesce,
        });
    }
}

/// The identifier of the `pick`-th live object of `state`.
fn pick(state: &State, pick: usize) -> Option<u128> {
    let ids: Vec<u128> = state.keys().copied().collect();
    (!ids.is_empty()).then(|| ids[pick % ids.len()])
}

fn apply_edits(doc: &mut Doc, model: &mut State, edits: &[Edit]) -> Txn {
    let meta = doc.meta();
    let mut txn = Txn::new(&doc.cell.load(), meta, TxnLimits::default());
    for edit in edits {
        match *edit {
            Edit::Create(n) => {
                let id = 1_000 + doc.next;
                doc.next += 1;
                txn.create(
                    KEY,
                    ObjectId::from_u128(id),
                    Header::default(),
                    vec![Some(Value::I64(n))],
                )
                .unwrap();
                model.insert(id, (n, None));
            }
            Edit::SetN(p, n) => {
                if let Some(id) = pick(model, p) {
                    txn.modify(
                        ObjectId::from_u128(id),
                        FieldPath::new("n"),
                        Some(Value::I64(n)),
                    )
                    .unwrap();
                    model.get_mut(&id).unwrap().0 = n;
                }
            }
            Edit::Rename(p, name) => {
                if let Some(id) = pick(model, p) {
                    let name = name.map(|n| format!("o{n}"));
                    let properties = Properties {
                        name: name.clone(),
                        ..Properties::default()
                    };
                    txn.set_properties(ObjectId::from_u128(id), properties)
                        .unwrap();
                    model.get_mut(&id).unwrap().1 = name;
                }
            }
            Edit::Delete(p) => {
                if let Some(id) = pick(model, p) {
                    txn.delete(ObjectId::from_u128(id)).unwrap();
                    model.remove(&id);
                }
            }
        }
    }
    txn
}

fn check(doc: &Doc, model: &Model, commits: usize) {
    assert_eq!(state(&doc.cell.load()), model.state);
    let shape = |steps: Vec<(usize, u64)>| steps;
    let undo: Vec<(usize, u64)> = doc
        .undo
        .undo_steps()
        .map(|s| (s.transactions().len(), s.memory_bytes()))
        .collect();
    let expected: Vec<(usize, u64)> = model
        .undo
        .iter()
        .map(|s| (s.bytes.len(), s.bytes.iter().sum()))
        .collect();
    assert_eq!(shape(undo), expected);
    let redo: Vec<(usize, u64)> = doc
        .undo
        .redo_steps()
        .map(|s| (s.transactions().len(), s.memory_bytes()))
        .collect();
    let expected: Vec<(usize, u64)> = model
        .redo
        .iter()
        .map(|s| (s.bytes.len(), s.bytes.iter().sum()))
        .collect();
    assert_eq!(redo, expected);
    assert_eq!(doc.journal.len(), commits);
    assert_eq!(doc.undo.group().is_some(), !model.groups.is_empty());
}

#[allow(
    clippy::too_many_lines,
    reason = "one match arm per action of the model"
)]
fn run(actions: &[Action]) {
    let limits = UndoLimits::default();
    let mut doc = Doc::new(limits);
    let mut model = Model {
        state: State::new(),
        undo: VecDeque::new(),
        redo: Vec::new(),
        limits,
        groups: Vec::new(),
        open: None,
        gesture: None,
        gestures: 0,
    };
    let mut commits = 0;
    let mut groups = 0;
    for action in actions {
        match action {
            Action::Commit(edits, key) => {
                let before = model.state.clone();
                let mut next = model.state.clone();
                let txn = apply_edits(&mut doc, &mut next, edits);
                let key_text = key.map(|k| format!("k{k}"));
                let bytes = doc.commit(txn, key_text.as_deref()).unwrap();
                commits += 1;
                model.state = next;
                model.committed(before, bytes, *key);
            }
            Action::Undo => {
                let meta = doc.meta();
                let base = doc.cell.load();
                let prepared = doc
                    .undo
                    .undo(&doc.journal, &base, meta, TxnLimits::default());
                if !model.groups.is_empty() {
                    assert_eq!(prepared.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                let Some(mut step) = model.undo.pop_back() else {
                    assert!(prepared.unwrap().is_none());
                    continue;
                };
                doc.commit(prepared.unwrap().unwrap(), None).unwrap();
                commits += 1;
                model.state = step.before.clone();
                step.coalesce = None;
                model.redo.push(step);
                if let Some(top) = model.undo.back_mut() {
                    top.coalesce = None;
                }
            }
            Action::Redo => {
                let meta = doc.meta();
                let base = doc.cell.load();
                let prepared = doc
                    .undo
                    .redo(&doc.journal, &base, meta, TxnLimits::default());
                if !model.groups.is_empty() {
                    assert_eq!(prepared.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                let Some(step) = model.redo.pop() else {
                    assert!(prepared.unwrap().is_none());
                    continue;
                };
                let bytes = doc.commit(prepared.unwrap().unwrap(), None).unwrap();
                commits += 1;
                model.state = step.after.clone();
                model.push(ModelStep {
                    bytes: vec![bytes],
                    coalesce: None,
                    ..step
                });
            }
            Action::BeginGroup => {
                groups += 1;
                doc.undo.begin_group(format!("\"g{groups}\""));
                let start = model.open.as_ref().map_or(0, |open| open.bytes.len());
                if model.open.is_none() {
                    model.open = Some(ModelStep {
                        before: model.state.clone(),
                        after: model.state.clone(),
                        bytes: Vec::new(),
                        coalesce: None,
                    });
                }
                model.groups.push((start, model.state.clone()));
            }
            Action::EndGroup => {
                let result = doc.undo.end_group();
                if model.groups.pop().is_none() {
                    assert_eq!(result.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                }
                result.unwrap();
                if model.groups.is_empty() {
                    let open = model.open.take().unwrap();
                    if !open.bytes.is_empty() {
                        model.push(open);
                    }
                }
            }
            Action::FailGroup => {
                let meta = doc.meta();
                let base = doc.cell.load();
                let prepared = doc
                    .undo
                    .fail_group(&doc.journal, &base, meta, TxnLimits::default());
                let Some((start, at_begin)) = model.groups.pop() else {
                    assert_eq!(prepared.unwrap_err().code(), "doc.undo.sequence_invalid");
                    continue;
                };
                let open = model.open.as_mut().unwrap();
                let members = open.bytes.len() - start;
                open.bytes.truncate(start);
                open.after = at_begin.clone();
                match prepared.unwrap() {
                    Some(txn) => {
                        assert!(members > 0);
                        doc.commit(txn, None).unwrap();
                        commits += 1;
                    }
                    None => assert_eq!(members, 0),
                }
                model.state = at_begin;
                if model.groups.is_empty() {
                    model.open = None;
                }
            }
            Action::BeginGesture => {
                doc.undo.begin_gesture();
                model.gesture = Some(model.gestures);
                model.gestures += 1;
            }
            Action::EndGesture => {
                doc.undo.end_gesture();
                model.gesture = None;
            }
            Action::Limits(steps, bytes) => {
                let limits = UndoLimits {
                    steps: *steps,
                    memory_bytes: *bytes,
                };
                doc.undo.set_limits(limits);
                model.limits = limits;
                model.trim();
            }
        }
        check(&doc, &model, commits);
    }
}

proptest! {
    #[test]
    fn undo_redo_groups_gestures_and_limits_match_the_model(
        actions in prop::collection::vec(action(), 0..40)
    ) {
        run(&actions);
    }
}
