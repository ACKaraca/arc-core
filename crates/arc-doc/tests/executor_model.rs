//! Model-based test of the executor and commit (CORE-DOC-015, CORE-DOC-016, CORE-DOC-018):
//! random jobs, some failing at each validation stage, some returning an error and some
//! unwinding, run through the executor; the document must equal a reference model that applies
//! only the successful jobs, every failure must carry the code of its stage, and after every
//! job the clean nodes of the dependency graph must equal a from-scratch evaluation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Once};

use arc_base::id::Identifier;
use arc_base::{Category, Error, ErrorCode, ObjectId, Result};
use arc_doc::error::codes;
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, Evaluators, NodeKey, NodeState, content_hash,
};
use arc_doc::{
    Bound, Cardinality, Document, DocumentValidator, FieldDecl, FieldPath, Finding, Header, OnCopy,
    OnDelete, ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Snapshot, Strength,
    Txn, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
};
use proptest::prelude::*;

const THIRTEEN: ErrorCode = ErrorCode {
    code: "test.item.thirteen",
    category: Category::InvalidInput,
    message_id: "test-item-thirteen",
};
const REJECTED: ErrorCode = ErrorCode {
    code: "test.document.rejected",
    category: Category::InvalidInput,
    message_id: "test-document-rejected",
};
const SLOTS: u128 = 5;
/// The identifier of the object a failing job adds; it never exists.
const FAULTY: u128 = 100;

fn id(slot: u128) -> ObjectId {
    ObjectId::from_u128(slot)
}

fn n_of(values: &[Option<Value>]) -> Option<i64> {
    match values.first() {
        Some(Some(Value::I64(n))) => Some(*n),
        _ => None,
    }
}

fn type_validator(values: &[Option<Value>]) -> Vec<Finding> {
    match n_of(values) {
        Some(13) => vec![Finding {
            severity: Severity::Error,
            error: Error::new(THIRTEEN),
        }],
        Some(7) => vec![Finding {
            severity: Severity::Warning,
            error: Error::new(THIRTEEN),
        }],
        _ => Vec::new(),
    }
}

/// Rejects a transaction that leaves a touched object with `n` 42.
fn document_validator() -> DocumentValidator {
    Arc::new(|_base: &Snapshot, candidate: &Snapshot, touched| {
        let rejected = touched.keys().any(|id: &ObjectId| {
            candidate
                .resolve(*id)
                .and_then(|handle| candidate.get(handle))
                .is_ok_and(|object| n_of(&object.values) == Some(42))
        });
        if rejected {
            vec![Finding {
                severity: Severity::Error,
                error: Error::new(REJECTED),
            }]
        } else {
            Vec::new()
        }
    })
}

#[derive(Debug)]
struct Number(i64);

impl Derived for Number {
    fn content_hash(&self) -> u128 {
        content_hash(&self.0)
    }
}

fn sum_key() -> DerivedKey {
    DerivedKey::new("test.sum").unwrap()
}

/// `n` of the object plus `n` of its link target, or minus one without a live target.
fn evaluators() -> Evaluators {
    let mut evaluators = Evaluators::new();
    evaluators
        .register_object(
            TypeKey::new("test.item").unwrap(),
            sum_key(),
            |ctx| -> Result<DerivedValue> {
                let me = ctx.object_id().unwrap();
                let n = |id| match ctx.field(id, "n") {
                    Ok(Some(Value::I64(n))) => *n,
                    _ => -1,
                };
                let linked = match ctx.field(me, "link")? {
                    Some(Value::Reference(target)) => n(*target),
                    _ => -1,
                };
                Ok(Arc::new(Number(n(me) + linked)))
            },
        )
        .unwrap();
    evaluators
}

fn document() -> Document {
    let link = ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new("test.item").unwrap()],
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality: Cardinality::Optional,
        },
    });
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![
            FieldDecl::new("n", ValueType::I64).with_bound(Bound::Int { min: 0, max: 100 }),
            FieldDecl::new("link", link).optional(),
        ],
    );
    let mut decl = TypeDecl::new(TypeKey::new("test.item").unwrap(), schema);
    decl.validators.push(type_validator);
    let mut registry = TypeRegistry::new();
    registry.register(decl).unwrap();
    let doc = Document::new(Arc::new(registry), Arc::new(evaluators())).unwrap();
    doc.add_validator(document_validator());
    doc
}

#[derive(Debug, Clone)]
enum Step {
    Create(u128, i64, Option<u128>),
    Set(u128, i64),
    Link(u128, Option<u128>),
    Delete(u128),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    Field,
    Reference,
    Type,
    Document,
    Panic,
    JobError,
}

fn step() -> impl Strategy<Value = Step> {
    let slot = 1..=SLOTS;
    let link = prop::option::of(1..=SLOTS);
    prop_oneof![
        (slot.clone(), 0..=10_i64, link.clone()).prop_map(|(s, n, l)| Step::Create(s, n, l)),
        (slot.clone(), 0..=10_i64).prop_map(|(s, n)| Step::Set(s, n)),
        (slot.clone(), link).prop_map(|(s, l)| Step::Link(s, l)),
        slot.prop_map(Step::Delete),
    ]
}

fn fault() -> impl Strategy<Value = Fault> {
    prop_oneof![
        4 => Just(Fault::None),
        1 => Just(Fault::Field),
        1 => Just(Fault::Reference),
        1 => Just(Fault::Type),
        1 => Just(Fault::Document),
        1 => Just(Fault::Panic),
        1 => Just(Fault::JobError),
    ]
}

fn create(txn: &mut Txn, slot: u128, n: i64, link: Option<u128>) -> Result<()> {
    let values = vec![Some(Value::I64(n)), link.map(|t| Value::Reference(id(t)))];
    txn.create("test.item", id(slot), Header::default(), values)
        .map(drop)
}

fn run_job(txn: &mut Txn, steps: &[Step], fault: Fault) -> Result<()> {
    for step in steps {
        match *step {
            Step::Create(s, n, link) => create(txn, s, n, link)?,
            Step::Set(s, n) => txn.modify(id(s), FieldPath::new("n"), Some(Value::I64(n)))?,
            Step::Link(s, l) => {
                txn.modify(
                    id(s),
                    FieldPath::new("link"),
                    l.map(|t| Value::Reference(id(t))),
                )?;
            }
            Step::Delete(s) => txn.delete(id(s))?,
        }
    }
    match fault {
        Fault::None => Ok(()),
        Fault::Field => create(txn, FAULTY, 200, None),
        Fault::Reference => create(txn, FAULTY, 1, Some(999)),
        Fault::Type => create(txn, FAULTY, 13, None),
        Fault::Document => create(txn, FAULTY, 42, None),
        Fault::Panic => panic!("injected fault"),
        Fault::JobError => Err(Error::new(codes::PATH_INVALID)),
    }
}

/// Slot to `n` and link.
type Model = BTreeMap<u128, (i64, Option<u128>)>;

/// The model after `steps` and the code the job must fail with, if any (`None` inside: a step
/// fails, with whatever code it has).
fn predict(
    model: &Model,
    steps: &[Step],
    fault: Fault,
) -> std::result::Result<Model, Option<&'static str>> {
    let mut m = model.clone();
    for step in steps {
        let ok = match *step {
            Step::Create(s, n, l) => m.insert(s, (n, l)).is_none(),
            Step::Set(s, n) => m.get_mut(&s).map(|e| e.0 = n).is_some(),
            Step::Link(s, l) => m.get_mut(&s).map(|e| e.1 = l).is_some(),
            Step::Delete(s) => m.remove(&s).is_some(),
        };
        if !ok {
            return Err(None);
        }
    }
    // The deletion rules clear references to base objects the job deleted; a reference to
    // an object that never was live in the base dangles.
    let live: BTreeSet<u128> = m.keys().copied().collect();
    let mut dangling = false;
    for (_, link) in m.values_mut() {
        if let Some(t) = *link
            && !live.contains(&t)
        {
            if model.contains_key(&t) {
                *link = None;
            } else {
                dangling = true;
            }
        }
    }
    match fault {
        Fault::Panic => Err(Some("doc.executor.fault")),
        Fault::JobError => Err(Some("doc.path.invalid")),
        Fault::Field => Err(Some("doc.value.out_of_range")),
        _ if dangling || fault == Fault::Reference => Err(Some("doc.reference.dangling")),
        Fault::Type => Err(Some("test.item.thirteen")),
        Fault::Document => Err(Some("test.document.rejected")),
        _ => Ok(m),
    }
}

fn state(snapshot: &Snapshot) -> Model {
    snapshot
        .iter()
        .map(|(_, object)| {
            let link = match object.values.get(1) {
                Some(Some(Value::Reference(t))) => Some(t.as_u128()),
                _ => None,
            };
            (object.id.as_u128(), (n_of(&object.values).unwrap(), link))
        })
        .collect()
}

fn expected_sum(model: &Model, slot: u128) -> i64 {
    let (n, link) = model[&slot];
    n + link.and_then(|t| model.get(&t)).map_or(-1, |e| e.0)
}

fn sum_of(view_result: Option<&DerivedValue>) -> i64 {
    view_result.unwrap().downcast_ref::<Number>().unwrap().0
}

fn quiet_injected_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let injected = info
                .payload()
                .downcast_ref::<&str>()
                .is_some_and(|text| *text == "injected fault");
            if !injected {
                default(info);
            }
        }));
    });
}

proptest! {
    #[test]
    fn executor_matches_a_model_of_successful_jobs(
        jobs in prop::collection::vec((prop::collection::vec(step(), 0..5), fault()), 1..12),
    ) {
        quiet_injected_panics();
        let doc = document();
        let mut model = Model::new();
        for (steps, fault) in jobs {
            let graph = doc.graph();
            for &slot in model.keys() {
                graph.evaluate_now(&NodeKey::object(id(slot), sum_key())).unwrap();
            }
            let before = doc.snapshot();
            let expected = predict(&model, &steps, fault);
            let job_steps = steps.clone();
            let outcome = doc.submit(move |txn| run_job(txn, &job_steps, fault)).wait();
            match (expected, outcome) {
                (Ok(next), Ok(done)) => {
                    model = next;
                    prop_assert!(done.snapshot.ptr_eq(&doc.snapshot()));
                }
                (Err(code), Err(error)) => {
                    if let Some(code) = code {
                        prop_assert_eq!(error.code(), code);
                    }
                    prop_assert!(doc.snapshot().ptr_eq(&before));
                }
                (expected, outcome) => {
                    prop_assert!(false, "expected {:?}, got {:?}", expected, outcome.map(|d| d.txn_id));
                }
            }
            prop_assert_eq!(&state(&doc.snapshot()), &model);
            // Clean nodes, before any re-evaluation, equal a from-scratch evaluation.
            for &slot in model.keys() {
                if let Some(view) = graph.node(&NodeKey::object(id(slot), sum_key()))
                    && matches!(view.state, NodeState::Clean)
                {
                    prop_assert_eq!(sum_of(view.result.as_ref()), expected_sum(&model, slot));
                }
            }
            for &slot in model.keys() {
                let view = graph.evaluate_now(&NodeKey::object(id(slot), sum_key())).unwrap();
                prop_assert_eq!(sum_of(view.result.as_ref()), expected_sum(&model, slot));
            }
        }
    }
}
