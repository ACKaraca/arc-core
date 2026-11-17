//! Model-based tests of the dependency graph (CORE-DOC-029, CORE-DOC-030, CORE-DOC-032):
//! random evaluators whose reads depend on object values, random commits, and after every commit
//! a comparison of the dirty nodes with the transitive readers of the changed inputs in a
//! reference model, and of every node's result with a from-scratch recomputation. A
//! multi-threaded stress test runs jobs on several threads while another thread commits, and
//! checks that no clean node holds a result of a superseded revision.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use arc_base::id::Identifier;
use arc_base::{ObjectId, Result, SessionId, TransactionId};
use arc_doc::graph::{
    ChangeSet, Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Finish, Graph, NodeKey,
    NodeState, StateKind, content_hash,
};
use arc_doc::{
    FieldDecl, FieldPath, Header, Schema, SchemaVersion, Snapshot, Txn, TxnLimits, TxnMeta,
    TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};
use proptest::prelude::*;

/// Object slots and derived keys.
const OBJECTS: usize = 5;
const KEYS: usize = 3;

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

/// One read of a program, relative to the evaluated object.
#[derive(Debug, Clone)]
enum Leaf {
    Const(i64),
    /// The `a` (aspect `geometry`) of the object at this offset.
    A(usize),
    /// The `b` (aspect `appearance`) of the object at this offset.
    B(usize),
    /// A node of a lower key of the object at this offset.
    Node(usize, usize),
}

#[derive(Debug, Clone)]
enum Step {
    Leaf(Leaf),
    /// The first leaf when the evaluated object's `a` is even, else the second.
    Branch(Leaf, Leaf),
}

/// One program per derived key; key `j` reads only keys below `j`.
type Programs = Vec<Vec<Step>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Read {
    A(usize),
    B(usize),
    Node(usize, usize),
}

fn eval_leaf(leaf: &Leaf, key: usize, me: usize, read: &mut dyn FnMut(Read) -> i64) -> i64 {
    match *leaf {
        Leaf::Const(n) => n,
        Leaf::A(offset) => read(Read::A((me + offset) % OBJECTS)),
        Leaf::B(offset) => read(Read::B((me + offset) % OBJECTS)),
        Leaf::Node(offset, lower) => read(Read::Node((me + offset) % OBJECTS, lower % key.max(1))),
    }
}

fn eval_program(
    programs: &Programs,
    key: usize,
    me: usize,
    read: &mut dyn FnMut(Read) -> i64,
) -> i64 {
    let mut sum = 0;
    for step in &programs[key] {
        sum += match step {
            Step::Leaf(leaf) => eval_leaf(leaf, key, me, read),
            Step::Branch(even, odd) => {
                if read(Read::A(me)).rem_euclid(2) == 0 {
                    eval_leaf(even, key, me, read)
                } else {
                    eval_leaf(odd, key, me, read)
                }
            }
        };
    }
    sum.rem_euclid(5)
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

fn id(slot: usize) -> ObjectId {
    ObjectId::from_u128(u128::try_from(slot).unwrap() + 1)
}

fn derived(key: usize) -> DerivedKey {
    DerivedKey::new(&format!("test.k{key}")).unwrap()
}

fn node(slot: usize, key: usize) -> NodeKey {
    NodeKey::object(id(slot), derived(key))
}

fn field(ctx: &EvalContext<'_>, slot: usize, name: &str) -> i64 {
    match ctx.field(id(slot), name) {
        Ok(Some(Value::I64(n))) => *n,
        _ => -1,
    }
}

fn evaluators(programs: &Arc<Programs>) -> Evaluators {
    let mut evaluators = Evaluators::new();
    for key in 0..KEYS {
        let programs = programs.clone();
        evaluators
            .register_object(
                TypeKey::new("test.cell").unwrap(),
                derived(key),
                move |ctx| -> Result<DerivedValue> {
                    let me = usize::try_from(ctx.object_id().unwrap().as_u128() - 1).unwrap();
                    let value = eval_program(&programs, key, me, &mut |read| match read {
                        Read::A(slot) => field(ctx, slot, "a"),
                        Read::B(slot) => field(ctx, slot, "b"),
                        Read::Node(slot, lower) => ctx
                            .node(&node(slot, lower))
                            .map_or(-1, |value| value.downcast_ref::<Number>().unwrap().0),
                    });
                    Ok(Arc::new(Number(value)))
                },
            )
            .unwrap();
    }
    evaluators
}

/// What a from-scratch evaluation of one node read.
#[derive(Debug, Default, Clone)]
struct Deps {
    /// Object inputs: (slot, 0 for existence, 1 for `a`, 2 for `b`).
    inputs: BTreeSet<(usize, u8)>,
    nodes: BTreeSet<(usize, usize)>,
}

fn live_values(snapshot: &Snapshot, slot: usize) -> Option<(i64, i64)> {
    let object = snapshot.get(snapshot.resolve(id(slot)).ok()?).ok()?;
    match object.values.as_slice() {
        [Some(Value::I64(a)), Some(Value::I64(b))] => Some((*a, *b)),
        _ => None,
    }
}

/// Recomputes every node of every live object from scratch, with what each read.
fn reference(snapshot: &Snapshot, programs: &Programs) -> BTreeMap<(usize, usize), (i64, Deps)> {
    fn compute(
        snapshot: &Snapshot,
        programs: &Programs,
        slot: usize,
        key: usize,
        done: &mut BTreeMap<(usize, usize), (i64, Deps)>,
    ) -> i64 {
        if let Some((value, _)) = done.get(&(slot, key)) {
            return *value;
        }
        let mut deps = Deps::default();
        let value = eval_program(programs, key, slot, &mut |read| match read {
            Read::A(other) | Read::B(other) => match live_values(snapshot, other) {
                Some((a, b)) => {
                    let is_a = matches!(read, Read::A(_));
                    deps.inputs.insert((other, if is_a { 1 } else { 2 }));
                    if is_a { a } else { b }
                }
                None => {
                    deps.inputs.insert((other, 0));
                    -1
                }
            },
            Read::Node(other, lower) => {
                // A node of an object depends on the object's existence: deleting the object
                // removes the node.
                deps.inputs.insert((other, 0));
                if live_values(snapshot, other).is_none() {
                    return -1;
                }
                deps.nodes.insert((other, lower));
                compute(snapshot, programs, other, lower, done)
            }
        });
        done.insert((slot, key), (value, deps));
        value
    }
    let mut done = BTreeMap::new();
    for slot in 0..OBJECTS {
        if live_values(snapshot, slot).is_some() {
            for key in 0..KEYS {
                compute(snapshot, programs, slot, key, &mut done);
            }
        }
    }
    done
}

#[derive(Debug, Clone)]
enum Mutation {
    SetA(usize, i64),
    SetB(usize, i64),
    /// Deletes a live object or creates a missing one.
    Toggle(usize, i64),
}

fn commit(snapshot: &Snapshot, graph: &Graph, mutations: &[Mutation]) -> Snapshot {
    let meta = TxnMeta::new(TransactionId::from_u128(1), SessionId::from_u128(1));
    let mut txn = Txn::new(snapshot, meta, TxnLimits::default());
    for mutation in mutations {
        let live = |txn: &Txn, slot| txn.get(id(slot)).is_ok();
        match *mutation {
            Mutation::SetA(slot, v) | Mutation::SetB(slot, v) if live(&txn, slot) => {
                let name = if matches!(mutation, Mutation::SetA(..)) {
                    "a"
                } else {
                    "b"
                };
                txn.modify(id(slot), FieldPath::new(name), Some(Value::I64(v)))
                    .unwrap();
            }
            Mutation::Toggle(slot, _) if live(&txn, slot) => txn.delete(id(slot)).unwrap(),
            Mutation::Toggle(slot, v) => {
                txn.create(
                    "test.cell",
                    id(slot),
                    Header::default(),
                    vec![Some(Value::I64(v)), Some(Value::I64(v))],
                )
                .unwrap();
            }
            _ => {}
        }
    }
    let parts = txn.into_parts();
    let changes = ChangeSet::from_commit(&parts.set, &parts.ops);
    let next = parts.set.current().clone();
    graph.apply(&changes, next.clone());
    next
}

/// The object inputs a commit changed, in the reference's terms, and the whole-changed slots.
fn changed_inputs(
    before: &Snapshot,
    after: &Snapshot,
    mutations: &[Mutation],
) -> (BTreeSet<(usize, u8)>, BTreeSet<usize>) {
    let mut inputs = BTreeSet::new();
    let mut whole = BTreeSet::new();
    let mut live: BTreeMap<usize, bool> = (0..OBJECTS)
        .map(|slot| (slot, live_values(before, slot).is_some()))
        .collect();
    let mut toggled = BTreeSet::new();
    for mutation in mutations {
        match *mutation {
            Mutation::SetA(slot, _) if live[&slot] => {
                inputs.insert((slot, 1));
            }
            Mutation::SetB(slot, _) if live[&slot] => {
                inputs.insert((slot, 2));
            }
            Mutation::Toggle(slot, _) => {
                toggled.insert(slot);
                live.insert(slot, !live[&slot]);
            }
            _ => {}
        }
    }
    for slot in toggled {
        if live_values(before, slot).is_some() || live_values(after, slot).is_some() {
            whole.insert(slot);
            inputs.extend([(slot, 0), (slot, 1), (slot, 2)]);
        }
    }
    (inputs, whole)
}

fn expected_dirty(
    deps: &BTreeMap<(usize, usize), (i64, Deps)>,
    inputs: &BTreeSet<(usize, u8)>,
    whole: &BTreeSet<usize>,
) -> BTreeSet<(usize, usize)> {
    let mut dirty: BTreeSet<(usize, usize)> = deps
        .iter()
        .filter(|(_, (_, deps))| deps.inputs.iter().any(|input| inputs.contains(input)))
        .map(|(node, _)| *node)
        .collect();
    loop {
        let more: Vec<(usize, usize)> = deps
            .iter()
            .filter(|(node, (_, deps))| {
                !dirty.contains(*node) && deps.nodes.iter().any(|dep| dirty.contains(dep))
            })
            .map(|(node, _)| *node)
            .collect();
        if more.is_empty() {
            break;
        }
        dirty.extend(more);
    }
    dirty.retain(|(slot, _)| !whole.contains(slot));
    dirty
}

fn graph_value(graph: &Graph, slot: usize, key: usize) -> i64 {
    let view = graph.evaluate_now(&node(slot, key)).unwrap();
    assert!(matches!(view.state, NodeState::Clean), "{view:?}");
    view.result.unwrap().downcast_ref::<Number>().unwrap().0
}

fn check_all(graph: &Graph, snapshot: &Snapshot, programs: &Programs) {
    for ((slot, key), (value, _)) in reference(snapshot, programs) {
        assert_eq!(graph_value(graph, slot, key), value, "node {slot}/{key}");
    }
    assert!(graph.non_clean().next().is_none());
}

fn leaf(key: usize) -> BoxedStrategy<Leaf> {
    let offset = 0..OBJECTS;
    if key == 0 {
        prop_oneof![
            (0i64..3).prop_map(Leaf::Const),
            offset.clone().prop_map(Leaf::A),
            offset.prop_map(Leaf::B),
        ]
        .boxed()
    } else {
        prop_oneof![
            (0i64..3).prop_map(Leaf::Const),
            offset.clone().prop_map(Leaf::A),
            offset.clone().prop_map(Leaf::B),
            (offset.clone(), 0..KEYS).prop_map(|(o, k)| Leaf::Node(o, k)),
            (offset, 0..KEYS).prop_map(|(o, k)| Leaf::Node(o, k)),
        ]
        .boxed()
    }
}

fn program(key: usize) -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        leaf(key).prop_map(Step::Leaf),
        (leaf(key), leaf(key)).prop_map(|(even, odd)| Step::Branch(even, odd)),
    ];
    prop::collection::vec(step, 1..4)
}

fn programs() -> impl Strategy<Value = Programs> {
    (program(0), program(1), program(2)).prop_map(|(a, b, c)| vec![a, b, c])
}

fn mutation() -> impl Strategy<Value = Mutation> {
    let slot = 0..OBJECTS;
    prop_oneof![
        3 => (slot.clone(), 0i64..4).prop_map(|(s, v)| Mutation::SetA(s, v)),
        2 => (slot.clone(), 0i64..4).prop_map(|(s, v)| Mutation::SetB(s, v)),
        1 => (slot, 0i64..4).prop_map(|(s, v)| Mutation::Toggle(s, v)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn incremental_results_equal_a_recomputation_from_scratch(
        programs in programs(),
        commits in prop::collection::vec(prop::collection::vec(mutation(), 1..4), 1..12),
        drivers in prop::collection::vec(any::<bool>(), 12),
    ) {
        let programs = Arc::new(programs);
        let mut snapshot = Snapshot::empty(registry());
        let graph = Graph::new(Arc::new(evaluators(&programs)), snapshot.clone());
        let initial: Vec<Mutation> = (0..OBJECTS - 1).map(|slot| Mutation::Toggle(slot, 1)).collect();
        snapshot = commit(&snapshot, &graph, &initial);
        check_all(&graph, &snapshot, &programs);
        for (mutations, all_dirty_first) in commits.iter().zip(drivers) {
            let before = reference(&snapshot, &programs);
            let next = commit(&snapshot, &graph, mutations);
            let (inputs, whole) = changed_inputs(&snapshot, &next, mutations);
            let dirty: BTreeSet<(usize, usize)> = graph
                .non_clean()
                .map(|view| {
                    assert_eq!(view.state.kind(), StateKind::Dirty);
                    let slot = usize::try_from(match view.key.subject {
                        arc_doc::graph::Subject::Object(id) => id.as_u128() - 1,
                        arc_doc::graph::Subject::Local(_) => panic!("no local nodes"),
                    }).unwrap();
                    let key = (0..KEYS).find(|key| derived(*key) == view.key.key).unwrap();
                    (slot, key)
                })
                .collect();
            prop_assert_eq!(dirty, expected_dirty(&before, &inputs, &whole));
            for slot in &whole {
                for key in 0..KEYS {
                    prop_assert!(graph.node(&node(*slot, key)).is_none());
                }
            }
            snapshot = next;
            if all_dirty_first {
                graph.evaluate_all_dirty().unwrap();
            }
            check_all(&graph, &snapshot, &programs);
        }
    }
}

/// A small deterministic generator for the stress test.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap() % bound
    }
}

#[test]
fn concurrent_jobs_never_apply_results_of_superseded_revisions() {
    let programs: Arc<Programs> = Arc::new(vec![
        vec![Step::Leaf(Leaf::A(0)), Step::Leaf(Leaf::A(1))],
        vec![
            Step::Branch(Leaf::Node(0, 0), Leaf::B(2)),
            Step::Leaf(Leaf::Node(3, 0)),
        ],
        vec![Step::Leaf(Leaf::Node(1, 1)), Step::Leaf(Leaf::Node(0, 0))],
    ]);
    let mut snapshot = Snapshot::empty(registry());
    let graph = Graph::new(Arc::new(evaluators(&programs)), snapshot.clone());
    let initial: Vec<Mutation> = (0..OBJECTS).map(|slot| Mutation::Toggle(slot, 1)).collect();
    snapshot = commit(&snapshot, &graph, &initial);
    let stop = AtomicBool::new(false);
    let applied = AtomicUsize::new(0);
    let last = std::thread::scope(|scope| {
        for worker in 0..3u64 {
            let (graph, stop, applied) = (&graph, &stop, &applied);
            scope.spawn(move || {
                let mut random = Lcg(worker + 7);
                while !stop.load(Ordering::SeqCst) {
                    let key = node(random.next(OBJECTS), random.next(KEYS));
                    if random.next(4) == 0 {
                        let _ = graph.evaluate_now(&key);
                    } else if let Ok(job) = graph.begin(&key) {
                        let outcome = job.evaluate();
                        if graph.finish(job, outcome) == Finish::Applied {
                            applied.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
            });
        }
        let committer = scope.spawn(|| {
            let mut random = Lcg(1);
            for _ in 0..300 {
                let mutations: Vec<Mutation> = (0..1 + random.next(2))
                    .map(|_| {
                        let (slot, value) =
                            (random.next(OBJECTS), i64::try_from(random.next(4)).unwrap());
                        match random.next(8) {
                            0 => Mutation::Toggle(slot, value),
                            1..=2 => Mutation::SetB(slot, value),
                            _ => Mutation::SetA(slot, value),
                        }
                    })
                    .collect();
                snapshot = commit(&snapshot, &graph, &mutations);
                std::thread::yield_now();
            }
            snapshot
        });
        let last = committer.join().unwrap();
        stop.store(true, Ordering::SeqCst);
        last
    });
    let expected = reference(&last, &programs);
    for ((slot, key), (value, _)) in &expected {
        if let Some(view) = graph.node(&node(*slot, *key))
            && matches!(view.state, NodeState::Clean)
        {
            let result = view.result.unwrap().downcast_ref::<Number>().unwrap().0;
            assert_eq!(
                result, *value,
                "clean node {slot}/{key} holds a stale result"
            );
        }
    }
    assert!(
        applied.load(Ordering::SeqCst) > 0,
        "the workers applied results"
    );
    graph.evaluate_all_dirty().unwrap();
    check_all(&graph, &last, &programs);
}
