//! Model-based test of transactions and savepoints (CORE-DOC-013, CORE-DOC-014): random
//! operation sequences with savepoints, rollbacks, releases and an operation limit are compared
//! against a reference model after every step; at the end the recorded operations replayed on
//! the base reproduce the candidate, and their inverses replayed in reverse on the candidate
//! reproduce the base.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{ObjectId, SessionId, TransactionId};
use arc_doc::{
    FieldDecl, FieldPath, Header, Op, Properties, Schema, SchemaVersion, Snapshot, Txn, TxnLimits,
    TxnMeta, TxnSavepoint, TypeDecl, TypeKey, TypeRegistry, Value, ValueType, WorkingSet,
};
use proptest::prelude::*;

const KEYS: [&str; 2] = ["test.a", "test.b"];

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for key in KEYS {
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![
                FieldDecl::new("n", ValueType::I64),
                FieldDecl::new("tags", ValueType::Map(Box::new(ValueType::I64))).optional(),
                FieldDecl::new("items", ValueType::List(Box::new(ValueType::I64))),
            ],
        );
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema))
            .unwrap();
    }
    Arc::new(registry)
}

/// The state of a document by identifier: type key, header and values.
type State = BTreeMap<ObjectId, (String, Header, Vec<Option<Value>>)>;

fn state(snapshot: &Snapshot) -> State {
    snapshot
        .iter()
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().to_string();
            (
                object.id,
                (key, object.header.clone(), object.values.clone()),
            )
        })
        .collect()
}

fn values(n: i64, tags: bool, items: u8) -> Vec<Option<Value>> {
    vec![
        Some(Value::I64(n)),
        tags.then(|| Value::Map(BTreeMap::new())),
        Some(Value::List((0..i64::from(items)).map(Value::I64).collect())),
    ]
}

#[derive(Debug, Clone)]
enum Step {
    Create {
        id: Option<usize>,
        kind: usize,
        n: i64,
        tags: bool,
        items: u8,
    },
    SetN {
        pick: usize,
        n: i64,
    },
    SetTags {
        pick: usize,
        present: bool,
    },
    SetTag {
        pick: usize,
        key: u8,
        value: Option<i64>,
    },
    SetItem {
        pick: usize,
        index: u8,
        n: i64,
    },
    Rename {
        pick: usize,
        name: Option<u8>,
    },
    Relink {
        pick: usize,
        owner: Option<usize>,
    },
    Delete {
        pick: usize,
    },
    Savepoint,
    Rollback {
        pick: usize,
    },
    Release {
        pick: usize,
    },
}

fn step() -> impl Strategy<Value = Step> {
    let pick = any::<usize>();
    prop_oneof![
        4 => (prop::option::weighted(0.2, any::<usize>()), 0..KEYS.len(), any::<i64>(), any::<bool>(), 0..3_u8)
            .prop_map(|(id, kind, n, tags, items)| Step::Create { id, kind, n, tags, items }),
        2 => (pick, any::<i64>()).prop_map(|(pick, n)| Step::SetN { pick, n }),
        1 => (pick, any::<bool>()).prop_map(|(pick, present)| Step::SetTags { pick, present }),
        2 => (pick, 0..3_u8, prop::option::of(any::<i64>()))
            .prop_map(|(pick, key, value)| Step::SetTag { pick, key, value }),
        2 => (pick, 0..3_u8, any::<i64>()).prop_map(|(pick, index, n)| Step::SetItem { pick, index, n }),
        1 => (pick, prop::option::of(any::<u8>())).prop_map(|(pick, name)| Step::Rename { pick, name }),
        1 => (pick, prop::option::of(any::<usize>())).prop_map(|(pick, owner)| Step::Relink { pick, owner }),
        2 => pick.prop_map(|pick| Step::Delete { pick }),
        2 => Just(Step::Savepoint),
        1 => pick.prop_map(|pick| Step::Rollback { pick }),
        1 => pick.prop_map(|pick| Step::Release { pick }),
    ]
}

fn nth(model: &State, pick: usize) -> Option<ObjectId> {
    model.keys().nth(pick.checked_rem(model.len())?).copied()
}

fn base() -> Snapshot {
    let snapshot = Snapshot::empty(registry());
    let mut set = WorkingSet::new(&snapshot);
    for n in 0..3_u8 {
        set.create(
            "test.a",
            arc_doc::Object::new(
                ObjectId::from_u128(u128::from(n) + 1),
                SchemaVersion::new(1, 0),
                Header::default(),
                values(i64::from(n), n % 2 == 0, n),
            ),
        )
        .unwrap();
    }
    set.current().clone()
}

/// The transaction under test and the reference model.
struct Harness {
    txn: Txn,
    model: State,
    /// Every identifier used so far.
    ids: Vec<ObjectId>,
    ops: usize,
    limit: u64,
    /// Every savepoint ever taken.
    taken: Vec<TxnSavepoint>,
    /// The live savepoints: index into `taken`, model and operation count.
    live: Vec<(usize, State, usize)>,
}

impl Harness {
    fn step(&mut self, step: Step) {
        let full = u64::try_from(self.ops).unwrap() >= self.limit;
        // The result and the expected model after the step, or `None` when it must fail.
        let (result, expected) = match step {
            Step::Create {
                id,
                kind,
                n,
                tags,
                items,
            } => self.create(id, kind, values(n, tags, items)),
            Step::Savepoint => {
                self.taken.push(self.txn.savepoint());
                self.live
                    .push((self.taken.len() - 1, self.model.clone(), self.ops));
                return;
            }
            Step::Rollback { pick } => return self.savepoint_step(pick, true),
            Step::Release { pick } => return self.savepoint_step(pick, false),
            other => match self.object_step(other) {
                Some(outcome) => outcome,
                None => return,
            },
        };
        match expected {
            Some(next) if !full => {
                result.unwrap();
                self.model = next;
                self.ops += 1;
            }
            Some(_) => assert_eq!(result.unwrap_err().code(), "doc.txn.limit_exceeded"),
            None => assert!(result.is_err()),
        }
        check(&self.txn, &self.model, self.ops);
    }

    fn create(
        &mut self,
        id: Option<usize>,
        kind: usize,
        values: Vec<Option<Value>>,
    ) -> (arc_base::Result<()>, Option<State>) {
        let id = match id {
            Some(pick) => self.ids[pick % self.ids.len()],
            None => {
                let id = ObjectId::from_u128(1_000 + self.ids.len() as u128);
                self.ids.push(id);
                id
            }
        };
        let result = self
            .txn
            .create(KEYS[kind], id, Header::default(), values.clone())
            .map(drop);
        let mut next = self.model.clone();
        let fresh = next
            .insert(id, (KEYS[kind].into(), Header::default(), values))
            .is_none();
        (result, fresh.then_some(next))
    }

    fn savepoint_step(&mut self, pick: usize, rollback: bool) {
        if self.taken.is_empty() {
            return;
        }
        let index = pick % self.taken.len();
        let savepoint = &self.taken[index];
        let result = if rollback {
            self.txn.rollback_to(savepoint)
        } else {
            self.txn.release(savepoint)
        };
        match self.live.iter().position(|entry| entry.0 == index) {
            Some(at) if rollback => {
                result.unwrap();
                self.model = self.live[at].1.clone();
                self.ops = self.live[at].2;
                self.live.truncate(at + 1);
            }
            Some(at) => {
                result.unwrap();
                self.live.truncate(at);
            }
            None => assert_eq!(result.unwrap_err().code(), "doc.savepoint.invalid"),
        }
        check(&self.txn, &self.model, self.ops);
    }

    /// Applies a step on a live object to the transaction and to a copy of the model; `None`
    /// when no object is live.
    fn object_step(&mut self, step: Step) -> Option<(arc_base::Result<()>, Option<State>)> {
        let pick = match &step {
            Step::SetN { pick, .. }
            | Step::SetTags { pick, .. }
            | Step::SetTag { pick, .. }
            | Step::SetItem { pick, .. }
            | Step::Rename { pick, .. }
            | Step::Relink { pick, .. }
            | Step::Delete { pick } => *pick,
            _ => return None,
        };
        let id = nth(&self.model, pick)?;
        let mut next = self.model.clone();
        let entry = next.get_mut(&id)?;
        let txn = &mut self.txn;
        let (result, ok) = match step {
            Step::SetN { n, .. } => {
                entry.2[0] = Some(Value::I64(n));
                let result = txn.modify(id, FieldPath::new("n"), Some(Value::I64(n)));
                (result, true)
            }
            Step::SetTags { present, .. } => {
                let value = present.then(|| Value::Map(BTreeMap::new()));
                entry.2[1] = value.clone();
                (txn.modify(id, FieldPath::new("tags"), value), true)
            }
            Step::SetTag { key, value, .. } => {
                let key = key.to_string();
                let ok = match &mut entry.2[1] {
                    Some(Value::Map(map)) => {
                        match value {
                            Some(v) => map.insert(key.clone(), Value::I64(v)),
                            None => map.remove(&key),
                        };
                        true
                    }
                    _ => false,
                };
                let path = FieldPath::new("tags").key(key);
                (txn.modify(id, path, value.map(Value::I64)), ok)
            }
            Step::SetItem { index, n, .. } => {
                let item = match &mut entry.2[2] {
                    Some(Value::List(items)) => items.get_mut(usize::from(index)),
                    _ => None,
                };
                let ok = item.map(|item| *item = Value::I64(n)).is_some();
                let path = FieldPath::new("items").index(u64::from(index));
                (txn.modify(id, path, Some(Value::I64(n))), ok)
            }
            Step::Rename { name, .. } => {
                let name = name.map(|n| n.to_string());
                entry.1.name = name.clone();
                let properties = Properties {
                    name,
                    ..Properties::of(&entry.1)
                };
                (txn.set_properties(id, properties), true)
            }
            Step::Relink { owner, .. } => {
                let owner = owner.map(|pick| self.ids[pick % self.ids.len()]);
                entry.1.owner = owner;
                (txn.relink(id, owner), true)
            }
            _ => {
                next.remove(&id);
                (txn.delete(id), true)
            }
        };
        Some((result, ok.then_some(next)))
    }
}

fn run(steps: &[Step], limit: u64) {
    let base = base();
    let meta = TxnMeta::new(TransactionId::from_u128(1), SessionId::from_u128(1));
    let limits = TxnLimits {
        operations: limit,
        memory_bytes: None,
    };
    let model = state(&base);
    let mut harness = Harness {
        txn: Txn::new(&base, meta, limits),
        ids: model.keys().copied().collect(),
        model,
        ops: 0,
        limit,
        taken: Vec::new(),
        live: Vec::new(),
    };
    for step in steps {
        harness.step(step.clone());
    }
    let Harness { txn, ops, .. } = harness;

    // Replaying the operations on the base reproduces the candidate, and the inverses in
    // reverse order on the candidate reproduce the base.
    let candidate = txn.snapshot().clone();
    let mut forward = WorkingSet::new(&base);
    for op in txn.ops() {
        op.apply(&mut forward).unwrap();
    }
    assert_eq!(state(forward.current()), state(&candidate));
    assert_eq!(forward.touched(), txn.touched());
    let mut backward = WorkingSet::new(&candidate);
    for op in txn.ops().iter().rev().map(Op::inverse) {
        op.apply(&mut backward).unwrap();
    }
    assert_eq!(state(backward.current()), state(&base));
    let parts = txn.into_parts();
    assert_eq!(parts.ops.len(), ops);
    assert_eq!(state(parts.set.current()), state(&candidate));
}

fn check(txn: &Txn, model: &State, ops: usize) {
    assert_eq!(&state(txn.snapshot()), model);
    assert_eq!(txn.ops().len(), ops);
    for id in model.keys() {
        assert_eq!(txn.get(*id).unwrap().id, *id);
    }
}

proptest! {
    #[test]
    fn transactions_match_the_model_and_their_operations_replay(
        steps in prop::collection::vec(step(), 0..80),
        limit in prop_oneof![Just(u64::MAX), 1..40_u64],
    ) {
        run(&steps, limit);
    }
}
