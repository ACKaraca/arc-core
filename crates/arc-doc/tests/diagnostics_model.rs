//! The document diagnostics list (CORE-DOC-035) against a reference model: after random
//! sequences of commits, the list holds exactly one diagnostic per failed, blocked or cyclic
//! node (built from the graph's states) and one validator warning per live object the validator
//! warns about; replaying its change events reproduces it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use arc_base::id::Identifier;
use arc_base::{Error, ObjectId};
use arc_doc::error::codes;
use arc_doc::events::DEFAULT_CAPACITY;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, NodeState, Subject,
    content_hash,
};
use arc_doc::{
    Diagnostic, Document, DocumentConfig, Event, FieldDecl, FieldPath, Finding, Header, Origin,
    Priority, Schema, SchemaVersion, Severity, Snapshot, TypeDecl, TypeKey, TypeRegistry, Value,
    ValueType,
};
use proptest::prelude::*;

const WAIT: Duration = Duration::from_secs(60);
const OBJECTS: u128 = 5;

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

/// A value of 0 is a warning.
fn validator(values: &[Option<Value>]) -> Vec<Finding> {
    match values.first() {
        Some(Some(Value::I64(0))) => vec![Finding {
            severity: Severity::Warning,
            error: Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "v"),
        }],
        _ => Vec::new(),
    }
}

fn registry() -> Arc<TypeRegistry> {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("v", ValueType::I64).in_aspect("value"),
            FieldDecl::new("link", ValueType::I64).in_aspect("link"),
        ],
    );
    let mut decl = TypeDecl::new(TypeKey::new("test.item").unwrap(), schema);
    decl.validators.push(validator);
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn check(n: u128) -> NodeKey {
    NodeKey::object(id(n), DerivedKey::new("test.check").unwrap())
}

fn follow(n: u128) -> NodeKey {
    NodeKey::object(id(n), DerivedKey::new("test.follow").unwrap())
}

fn int(ctx: &EvalContext<'_>, object: ObjectId, name: &str) -> i64 {
    match ctx.field(object, name) {
        Ok(Some(Value::I64(n))) => *n,
        _ => 0,
    }
}

/// `check` fails for a negative value; `follow` reads the `follow` node of the object its link
/// names (failing when that object is missing), or its own `check` for link 0: chains that
/// end at a failed check are blocked, loops are cyclic.
fn evaluators() -> Evaluators {
    let item = || TypeKey::new("test.item").unwrap();
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(
            item(),
            DerivedKey::new("test.check").unwrap(),
            |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let v = int(ctx, ctx.object_id().unwrap(), "v");
                if v < 0 {
                    return Err(Error::new(codes::VALUE_OUT_OF_RANGE).with_arg("field", "v"));
                }
                Ok(Arc::new(Number(v)))
            },
        )
        .unwrap();
    evaluators
        .register_object(
            item(),
            DerivedKey::new("test.follow").unwrap(),
            |ctx: &EvalContext<'_>| -> arc_base::Result<DerivedValue> {
                let me = ctx.object_id().unwrap();
                let link = int(ctx, me, "link");
                let next = if link == 0 {
                    check(me.as_u128())
                } else {
                    follow(u128::try_from(link).unwrap_or(0))
                };
                let value = ctx.node(&next)?;
                Ok(Arc::new(Number(
                    value.downcast_ref::<Number>().map_or(0, |n| n.0),
                )))
            },
        )
        .unwrap();
    evaluators
}

#[derive(Debug, Clone)]
enum Step {
    Create(u128, i64, i64),
    Set(u128, i64),
    Link(u128, i64),
    Delete(u128),
}

fn step() -> impl Strategy<Value = Step> {
    let n = 1..=OBJECTS;
    let link = 0..=i64::try_from(OBJECTS).unwrap() + 1;
    prop_oneof![
        (n.clone(), -1..=2_i64, link.clone()).prop_map(|(n, v, l)| Step::Create(n, v, l)),
        (n.clone(), -1..=2_i64).prop_map(|(n, v)| Step::Set(n, v)),
        (n.clone(), link).prop_map(|(n, l)| Step::Link(n, l)),
        n.prop_map(Step::Delete),
    ]
}

fn run(doc: &Document, step: &Step) {
    let step = step.clone();
    // Steps on missing or existing objects fail and change nothing; that is part of the model.
    drop(
        doc.submit(move |txn| match step {
            Step::Create(n, v, l) => txn
                .create(
                    "test.item",
                    id(n),
                    Header::default(),
                    vec![Some(Value::I64(v)), Some(Value::I64(l))],
                )
                .map(drop),
            Step::Set(n, v) => txn.modify(id(n), FieldPath::new("v"), Some(Value::I64(v))),
            Step::Link(n, l) => txn.modify(id(n), FieldPath::new("link"), Some(Value::I64(l))),
            Step::Delete(n) => txn.delete(id(n)).map(drop),
        })
        .wait(),
    );
}

/// What identifies a diagnostic for the comparison.
type Key = (String, &'static str, Vec<ObjectId>);

fn key_of(diagnostic: &Diagnostic) -> Key {
    let origin = match &diagnostic.origin {
        Origin::Node(key) => format!("node {key}"),
        other => format!("{other:?}"),
    };
    (origin, diagnostic.code(), diagnostic.objects.clone())
}

fn counted(diagnostics: impl IntoIterator<Item = Key>) -> BTreeMap<Key, usize> {
    let mut map = BTreeMap::new();
    for key in diagnostics {
        *map.entry(key).or_default() += 1;
    }
    map
}

/// The reference: one diagnostic per problem node, with the node's own object first; one
/// validator warning per live object whose value is 0.
fn reference(doc: &Document) -> BTreeMap<(String, &'static str), usize> {
    let mut expected = Vec::new();
    for view in doc.graph().non_clean() {
        let code = match &view.state {
            NodeState::Failed(error) => error.code(),
            NodeState::Blocked(_) => "doc.graph.blocked",
            NodeState::Cyclic(_) => "doc.graph.cycle",
            other => panic!("a non-clean node after idle: {other:?}"),
        };
        expected.push((format!("node {}", view.key), code));
    }
    let snapshot = doc.snapshot();
    for n in 1..=OBJECTS {
        let Ok(handle) = snapshot.resolve(id(n)) else {
            continue;
        };
        if matches!(
            snapshot.get(handle).unwrap().values.first(),
            Some(Some(Value::I64(0)))
        ) {
            expected.push(("Validator".to_owned(), "doc.value.out_of_range"));
        }
    }
    let mut map = BTreeMap::new();
    for key in expected {
        *map.entry(key).or_default() += 1;
    }
    map
}

proptest! {
    #[test]
    fn the_list_matches_graph_states_and_delivered_diagnostics(
        steps in prop::collection::vec(step(), 1..14),
    ) {
        let config = DocumentConfig { workers: 2, ..DocumentConfig::default() };
        let doc = Document::open(
            Snapshot::empty(registry()),
            Arc::new(evaluators()),
            config,
        ).unwrap();
        let sub = doc.events().subscribe_with_capacity(DEFAULT_CAPACITY * 64);
        let mut replay: BTreeMap<Key, usize> = BTreeMap::new();
        let mut version = 0;
        for step in &steps {
            run(&doc, step);
            doc.regenerate((1..=OBJECTS).flat_map(|n| [check(n), follow(n)]), Priority::Visible);
            prop_assert!(doc.wait_idle(WAIT));
            let snapshot = doc.diagnostics_snapshot();
            prop_assert!(snapshot.version >= version);
            version = snapshot.version;
            prop_assert_eq!(snapshot.dropped, 0);
            let listed = counted(snapshot.diagnostics.iter().map(key_of));
            let without_objects: BTreeMap<(String, &'static str), usize> = {
                let mut map = BTreeMap::new();
                for ((origin, code, _), count) in &listed {
                    *map.entry((origin.clone(), *code)).or_default() += count;
                }
                map
            };
            prop_assert_eq!(&without_objects, &reference(&doc));
            // Node diagnostics name their own object first; validator ones name the object.
            for diagnostic in snapshot.diagnostics.iter() {
                match &diagnostic.origin {
                    Origin::Node(key) => {
                        let Subject::Object(own) = key.subject else { panic!("object nodes") };
                        prop_assert_eq!(diagnostic.objects.first(), Some(&own));
                        let node = doc.diagnostics().for_origin(&diagnostic.origin);
                        prop_assert_eq!(node.len(), 1);
                    }
                    _ => prop_assert_eq!(diagnostic.objects.len(), 1),
                }
                for object in &diagnostic.objects {
                    prop_assert!(doc
                        .diagnostics()
                        .for_object(*object)
                        .iter()
                        .any(|d| key_of(d) == key_of(diagnostic)));
                }
            }
            // Replaying the change events gives the list.
            while let Some(event) = sub.try_recv() {
                if let Event::DiagnosticsChanged(change) = event {
                    prop_assert!(!change.overflow);
                    for removed in &change.removed {
                        let key = key_of(removed);
                        let count = replay.get_mut(&key);
                        prop_assert!(count.is_some(), "removed {:?} that was not listed", key);
                        if let Some(count) = count {
                            *count -= 1;
                            if *count == 0 {
                                replay.remove(&key);
                            }
                        }
                    }
                    for added in &change.added {
                        *replay.entry(key_of(added)).or_default() += 1;
                    }
                }
            }
            prop_assert_eq!(&replay, &listed);
        }
    }
}
