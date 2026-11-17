//! Rules of the dependency graph (CORE-DOC-029, CORE-DOC-030, CORE-DOC-034 and the stale-result
//! rule of CORE-DOC-032): recorded reads, dirty marking, early cutoff, node states, cycles,
//! node removal, change sets of commits and forced interleavings of commits with jobs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId, Result, SessionId, TransactionId};
use arc_doc::graph::{
    ChangeSet, Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Finish, Graph, Input,
    NodeKey, NodeState, StateKind, content_hash,
};
use arc_doc::{
    FieldDecl, FieldPath, Header, Properties, Schema, SchemaVersion, Snapshot, Txn, TxnLimits,
    TxnMeta, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

fn num(value: &DerivedValue) -> i64 {
    value.downcast_ref::<Number>().unwrap().0
}

fn ok(n: i64) -> Result<DerivedValue> {
    Ok(Arc::new(Number(n)))
}

fn registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("a", ValueType::I64).in_aspect("geometry"),
            FieldDecl::new("b", ValueType::I64).in_aspect("appearance"),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.cell").unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn key(name: &str) -> DerivedKey {
    DerivedKey::new(name).unwrap()
}

fn cell() -> TypeKey {
    TypeKey::new("test.cell").unwrap()
}

fn node(object: u128, name: &str) -> NodeKey {
    NodeKey::object(id(object), key(name))
}

fn field(ctx: &EvalContext<'_>, object: ObjectId, name: &str) -> i64 {
    match ctx.field(object, name) {
        Ok(Some(Value::I64(n))) => *n,
        _ => -1,
    }
}

/// A document: the latest snapshot and its graph.
struct Doc {
    snapshot: Snapshot,
    graph: Graph,
}

impl Doc {
    fn new(evaluators: Evaluators) -> Self {
        Self::with_depth(evaluators, arc_doc::graph::DEFAULT_DEPTH_LIMIT)
    }

    fn with_depth(evaluators: Evaluators, depth: usize) -> Self {
        let snapshot = Snapshot::empty(registry());
        let graph = Graph::with_depth_limit(Arc::new(evaluators), snapshot.clone(), depth);
        let mut doc = Self { snapshot, graph };
        doc.commit(|txn| {
            for n in 1..=2 {
                txn.create("test.cell", id(n), Header::default(), values(1, 1))
                    .unwrap();
            }
        });
        doc
    }

    fn commit(&mut self, body: impl FnOnce(&mut Txn)) -> ChangeSet {
        let meta = TxnMeta::new(TransactionId::from_u128(1), SessionId::from_u128(1));
        let mut txn = Txn::new(&self.snapshot, meta, TxnLimits::default());
        body(&mut txn);
        let parts = txn.into_parts();
        let changes = ChangeSet::from_commit(&parts.set, &parts.ops);
        self.snapshot = parts.set.current().clone();
        self.graph.apply(&changes, self.snapshot.clone());
        changes
    }

    fn set(&mut self, object: u128, name: &str, value: i64) {
        self.commit(|txn| {
            txn.modify(id(object), FieldPath::new(name), Some(Value::I64(value)))
                .unwrap();
        });
    }

    fn value(&self, key: &NodeKey) -> i64 {
        let view = self.graph.evaluate_now(key).unwrap();
        assert!(matches!(view.state, NodeState::Clean), "{view:?}");
        num(view.result.as_ref().unwrap())
    }

    fn kind(&self, key: &NodeKey) -> Option<StateKind> {
        self.graph.node(key).map(|view| view.state.kind())
    }

    fn non_clean(&self) -> BTreeSet<(NodeKey, StateKind)> {
        self.graph
            .non_clean()
            .map(|view| (view.key, view.state.kind()))
            .collect()
    }
}

fn values(a: i64, b: i64) -> Vec<Option<Value>> {
    vec![Some(Value::I64(a)), Some(Value::I64(b))]
}

fn counter() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

fn count(counter: &AtomicUsize) -> usize {
    counter.load(Ordering::SeqCst)
}

/// `test.parity` is the parity of `a`; `test.down` is ten times the parity; both count runs.
fn parity_evaluators(parity: &Arc<AtomicUsize>, down: &Arc<AtomicUsize>) -> Evaluators {
    let mut evaluators = Evaluators::new();
    let runs = parity.clone();
    evaluators
        .register_object(cell(), key("test.parity"), move |ctx| {
            runs.fetch_add(1, Ordering::SeqCst);
            ok(field(ctx, ctx.object_id().unwrap(), "a").rem_euclid(2))
        })
        .unwrap();
    let runs = down.clone();
    evaluators
        .register_object(cell(), key("test.down"), move |ctx| {
            runs.fetch_add(1, Ordering::SeqCst);
            let me = ctx.object_id().unwrap();
            let parity = ctx.node(&NodeKey::object(me, key("test.parity")))?;
            ok(num(&parity) * 10)
        })
        .unwrap();
    evaluators
}

#[test]
fn reads_are_recorded_and_changes_dirty_exactly_the_readers_and_their_dependents() {
    let (parity, down) = (counter(), counter());
    let mut doc = Doc::new(parity_evaluators(&parity, &down));
    assert_eq!(doc.value(&node(1, "test.down")), 10);
    assert_eq!(doc.value(&node(2, "test.parity")), 1);
    doc.set(1, "b", 5);
    assert!(doc.non_clean().is_empty(), "b is in another aspect");
    doc.set(1, "a", 2);
    assert_eq!(
        doc.non_clean(),
        BTreeSet::from([
            (node(1, "test.down"), StateKind::Dirty),
            (node(1, "test.parity"), StateKind::Dirty),
        ])
    );
    assert_eq!(doc.value(&node(1, "test.down")), 0);
    assert_eq!((count(&parity), count(&down)), (3, 2));
}

#[test]
fn an_unchanged_result_hash_stops_propagation() {
    let (parity, down) = (counter(), counter());
    let mut doc = Doc::new(parity_evaluators(&parity, &down));
    assert_eq!(doc.value(&node(1, "test.down")), 10);
    assert_eq!((count(&parity), count(&down)), (1, 1));
    doc.set(1, "a", 3);
    doc.graph.evaluate_all_dirty().unwrap();
    assert_eq!(
        (count(&parity), count(&down)),
        (2, 1),
        "the parity is unchanged, so its dependent is not recomputed"
    );
    assert!(doc.non_clean().is_empty());
    assert_eq!(doc.value(&node(1, "test.down")), 10);
    doc.set(1, "a", 4);
    assert_eq!(doc.value(&node(1, "test.down")), 0);
    assert_eq!((count(&parity), count(&down)), (3, 2));
}

#[test]
fn failures_keep_the_last_good_result_block_dependents_and_clear_on_success() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(cell(), key("test.checked"), |ctx| {
            let a = field(ctx, ctx.object_id().unwrap(), "a");
            if a < 0 {
                Err(Error::new(arc_doc::error::codes::VALUE_OUT_OF_RANGE).with_arg("field", "a"))
            } else {
                ok(a)
            }
        })
        .unwrap();
    evaluators
        .register_object(cell(), key("test.next"), |ctx| {
            let me = ctx.object_id().unwrap();
            // Swallowing the error does not hide the failed input.
            let value = ctx.node(&NodeKey::object(me, key("test.checked")));
            ok(value.map_or(-100, |value| num(&value) + 1))
        })
        .unwrap();
    let mut doc = Doc::new(evaluators);
    let (checked, next) = (node(1, "test.checked"), node(1, "test.next"));
    assert_eq!(doc.value(&next), 2);
    doc.graph.drain_transitions();
    doc.set(1, "a", -5);
    doc.graph.evaluate_now(&next).unwrap();
    let view = doc.graph.node(&checked).unwrap();
    let NodeState::Failed(error) = &view.state else {
        panic!("{view:?}");
    };
    assert_eq!(error.code(), "doc.value.out_of_range");
    assert_eq!(num(view.result.as_ref().unwrap()), 1, "last good result");
    let view = doc.graph.node(&next).unwrap();
    let NodeState::Blocked(upstream) = &view.state else {
        panic!("{view:?}");
    };
    assert_eq!(upstream.to_vec(), vec![checked.clone()]);
    assert_eq!(num(view.result.as_ref().unwrap()), 2, "last good result");
    assert_eq!(
        doc.non_clean(),
        BTreeSet::from([
            (checked.clone(), StateKind::Failed),
            (next.clone(), StateKind::Blocked),
        ])
    );
    let transitions = doc.graph.drain_transitions();
    assert_eq!(transitions.len(), 2);
    assert!(transitions.iter().any(|t| t.key == checked
        && t.before == Some(StateKind::Clean)
        && t.after == Some(StateKind::Failed)));
    doc.set(1, "a", 7);
    assert_eq!(doc.value(&next), 8);
    assert_eq!(doc.kind(&checked), Some(StateKind::Clean));
    assert!(doc.non_clean().is_empty());
    assert!(
        doc.graph
            .drain_transitions()
            .iter()
            .all(|t| t.after == Some(StateKind::Clean))
    );
}

#[test]
fn cycles_are_reported_with_their_members_and_clear_when_broken() {
    let mut evaluators = Evaluators::new();
    // Local node i reads local node (i + 1) % 3 while the a of object 1 is 1.
    evaluators
        .register_local(key("test.ring"), |ctx| {
            let arc_doc::graph::Subject::Local(i) = ctx.key().subject else {
                panic!("a local node")
            };
            if field(ctx, id(1), "a") == 1 {
                let next = ctx.node(&NodeKey::local((i + 1) % 3, key("test.ring")))?;
                ok(num(&next) + 1)
            } else {
                ok(i64::try_from(i).unwrap())
            }
        })
        .unwrap();
    evaluators
        .register_local(key("test.outside"), |ctx| {
            ok(num(&ctx.node(&NodeKey::local(0, key("test.ring")))?) * 2)
        })
        .unwrap();
    let mut doc = Doc::new(evaluators);
    let ring = |i| NodeKey::local(i, key("test.ring"));
    let outside = NodeKey::local(9, key("test.outside"));
    let view = doc.graph.evaluate_now(&outside).unwrap();
    let NodeState::Blocked(upstream) = &view.state else {
        panic!("{view:?}");
    };
    assert_eq!(upstream.to_vec(), vec![ring(0)]);
    for i in 0..3 {
        let view = doc.graph.node(&ring(i)).unwrap();
        let NodeState::Cyclic(members) = &view.state else {
            panic!("{view:?}");
        };
        assert_eq!(
            members.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([ring(0), ring(1), ring(2)])
        );
    }
    doc.set(1, "a", 0);
    assert_eq!(doc.value(&outside), 0);
    assert_eq!(doc.value(&ring(2)), 2);
    assert_eq!(
        doc.kind(&ring(1)),
        Some(StateKind::Dirty),
        "not demanded yet"
    );
    doc.graph.evaluate_all_dirty().unwrap();
    assert!(doc.non_clean().is_empty());
}

#[test]
fn a_node_reading_itself_is_cyclic() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(cell(), key("test.me"), |ctx| {
            let me = ctx.key().clone();
            ctx.node(&me)
        })
        .unwrap();
    let doc = Doc::new(evaluators);
    let view = doc.graph.evaluate_now(&node(1, "test.me")).unwrap();
    let NodeState::Cyclic(members) = &view.state else {
        panic!("{view:?}");
    };
    assert_eq!(members.to_vec(), vec![node(1, "test.me")]);
}

#[test]
fn inline_demand_depth_is_bounded() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_local(key("test.chain"), |ctx| {
            let arc_doc::graph::Subject::Local(i) = ctx.key().subject else {
                panic!("a local node")
            };
            if i == 0 {
                return ok(0);
            }
            ok(num(&ctx.node(&NodeKey::local(i - 1, key("test.chain")))?) + 1)
        })
        .unwrap();
    let doc = Doc::with_depth(evaluators, 4);
    let chain = |i| NodeKey::local(i, key("test.chain"));
    let view = doc.graph.evaluate_now(&chain(10)).unwrap();
    assert!(matches!(view.state, NodeState::Blocked(_)), "{view:?}");
    let view = doc.graph.node(&chain(7)).unwrap();
    let NodeState::Failed(error) = &view.state else {
        panic!("{view:?}");
    };
    assert_eq!(error.code(), "doc.graph.depth_exceeded");
    // Evaluated from the bottom up, every step stays within the limit.
    let fresh = {
        let mut evaluators = Evaluators::new();
        evaluators
            .register_local(key("test.chain"), |ctx| {
                let arc_doc::graph::Subject::Local(i) = ctx.key().subject else {
                    panic!("a local node")
                };
                if i == 0 {
                    return ok(0);
                }
                ok(num(&ctx.node(&NodeKey::local(i - 1, key("test.chain")))?) + 1)
            })
            .unwrap();
        Doc::with_depth(evaluators, 4)
    };
    for i in 0..=10 {
        assert_eq!(fresh.value(&chain(i)), i64::try_from(i).unwrap());
    }
}

#[test]
fn absent_objects_are_dependencies_and_deleted_objects_lose_their_nodes() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_local(key("test.watch"), |ctx| ok(field(ctx, id(3), "a")))
        .unwrap();
    evaluators
        .register_object(cell(), key("test.own"), |ctx| {
            ok(field(ctx, ctx.object_id().unwrap(), "a"))
        })
        .unwrap();
    evaluators
        .register_local(key("test.via"), |ctx| {
            ok(ctx
                .node(&node(2, "test.own"))
                .map_or(-1, |value| num(&value)))
        })
        .unwrap();
    let mut doc = Doc::new(evaluators);
    let watch = NodeKey::local(0, key("test.watch"));
    let via = NodeKey::local(0, key("test.via"));
    assert_eq!(doc.value(&watch), -1);
    assert_eq!(doc.value(&via), 1);
    doc.commit(|txn| {
        txn.create("test.cell", id(3), Header::default(), values(4, 0))
            .unwrap();
    });
    assert_eq!(doc.kind(&watch), Some(StateKind::Dirty));
    assert_eq!(doc.value(&watch), 4);
    doc.commit(|txn| txn.delete(id(2)).unwrap());
    assert!(doc.graph.node(&node(2, "test.own")).is_none());
    assert_eq!(doc.kind(&via), Some(StateKind::Dirty));
    assert_eq!(doc.value(&via), -1);
    assert_eq!(
        doc.graph
            .evaluate_now(&node(2, "test.own"))
            .unwrap_err()
            .code(),
        "doc.object.not_found"
    );
    doc.commit(|txn| {
        txn.create("test.cell", id(2), Header::default(), values(9, 0))
            .unwrap();
    });
    assert_eq!(doc.value(&via), 9, "the node's object reappeared");
    assert_eq!(doc.graph.len(), 3);
}

#[test]
fn referrer_sets_are_dependencies() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(cell(), key("test.users"), |ctx| {
            ok(i64::try_from(ctx.referrers(ctx.object_id().unwrap()).count()).unwrap())
        })
        .unwrap();
    let mut doc = Doc::new(evaluators);
    let users = node(1, "test.users");
    assert_eq!(doc.value(&users), 0);
    let header = Header {
        owner: Some(id(1)),
        ..Header::default()
    };
    doc.commit(|txn| {
        txn.create("test.cell", id(3), header, values(0, 0))
            .unwrap();
    });
    assert_eq!(doc.value(&users), 1);
    doc.set(3, "a", 5);
    assert_eq!(doc.kind(&users), Some(StateKind::Clean), "same references");
    doc.commit(|txn| txn.relink(id(3), None).unwrap());
    assert_eq!(doc.value(&users), 0);
}

#[test]
fn change_sets_name_aspects_headers_whole_objects_and_referrers() {
    let mut doc = Doc::new(Evaluators::new());
    let changes = doc.commit(|txn| {
        txn.modify(id(1), FieldPath::new("a"), Some(Value::I64(3)))
            .unwrap();
    });
    assert!(changes.contains(&Input::Aspect(id(1), Arc::from("geometry"))));
    assert!(!changes.contains(&Input::Aspect(id(1), Arc::from("appearance"))));
    assert!(!changes.contains(&Input::Header(id(1))));
    assert!(!changes.contains(&Input::Exists(id(1))));
    let changes = doc.commit(|txn| {
        txn.set_properties(
            id(1),
            Properties {
                name: Some("x".to_owned()),
                ..Properties::default()
            },
        )
        .unwrap();
        txn.relink(id(2), Some(id(1))).unwrap();
    });
    assert!(changes.contains(&Input::Header(id(1))));
    assert!(changes.contains(&Input::Header(id(2))));
    assert!(changes.contains(&Input::Referrers(id(1))));
    assert!(!changes.contains(&Input::Aspect(id(1), Arc::from("geometry"))));
    let changes = doc.commit(|txn| {
        txn.delete(id(2)).unwrap();
        txn.create("test.cell", id(3), Header::default(), values(0, 0))
            .unwrap();
    });
    for object in [2, 3] {
        assert!(changes.contains(&Input::Exists(id(object))));
        assert!(changes.contains(&Input::Aspect(id(object), Arc::from("anything"))));
    }
    assert!(
        changes.contains(&Input::Referrers(id(1))),
        "the owned object left"
    );
    assert!(!changes.contains(&Input::Exists(id(1))));
}

#[test]
fn begin_fails_for_missing_objects_and_unknown_keys() {
    let doc = Doc::new(Evaluators::new());
    assert_eq!(
        doc.graph.begin(&node(1, "test.none")).unwrap_err().code(),
        "doc.graph.unknown_key"
    );
    assert_eq!(
        doc.graph.begin(&node(8, "test.none")).unwrap_err().code(),
        "doc.object.not_found"
    );
    assert!(doc.graph.is_empty());
}

#[test]
fn a_result_whose_input_changed_after_its_revision_is_never_applied() {
    let (parity, down) = (counter(), counter());
    let mut doc = Doc::new(parity_evaluators(&parity, &down));
    let target = node(1, "test.down");
    assert_eq!(doc.value(&target), 10);
    doc.set(1, "a", 2);

    // Commit between begin and evaluate: the job reads its own, older snapshot.
    let job = doc.graph.begin(&target).unwrap();
    assert_eq!(doc.kind(&target), Some(StateKind::Running));
    doc.set(1, "a", 3);
    let outcome = job.evaluate();
    assert_eq!(doc.graph.finish(job, outcome), Finish::Stale);
    assert_eq!(doc.kind(&target), Some(StateKind::Dirty));

    // Commit between evaluate and finish.
    let job = doc.graph.begin(&target).unwrap();
    let outcome = job.evaluate();
    doc.set(1, "a", 4);
    assert_eq!(doc.graph.finish(job, outcome), Finish::Stale);
    assert_eq!(doc.value(&target), 0);

    // A commit of an input the job did not read leaves its result valid.
    doc.set(1, "a", 5);
    let job = doc.graph.begin(&target).unwrap();
    let outcome = job.evaluate();
    doc.set(1, "b", 9);
    doc.set(2, "a", 9);
    assert_eq!(doc.graph.finish(job, outcome), Finish::Applied);
    assert_eq!(doc.kind(&target), Some(StateKind::Clean));
    assert_eq!(doc.value(&target), 10);
}

#[test]
fn cancelled_and_dropped_jobs_leave_the_node_dirty() {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(cell(), key("test.slow"), |ctx| {
            ctx.yield_point()?;
            ok(1)
        })
        .unwrap();
    let doc = Doc::new(evaluators);
    let slow = node(1, "test.slow");
    let job = doc.graph.begin(&slow).unwrap();
    job.cancel().cancel();
    let outcome = job.evaluate();
    assert_eq!(doc.graph.finish(job, outcome), Finish::Stale);
    assert_eq!(doc.kind(&slow), Some(StateKind::Dirty));
    let job = doc.graph.begin(&slow).unwrap();
    assert_eq!(doc.kind(&slow), Some(StateKind::Running));
    drop(job);
    assert_eq!(doc.kind(&slow), Some(StateKind::Dirty));
    assert_eq!(doc.value(&slow), 1);
}
