//! Batch undo and redo, step lists, marks and restoring the last erased objects, through
//! `Document`: each batch is one transaction, one journal record and one event.

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
    Cardinality, Document, FieldDecl, FieldPath, Header, OnCopy, OnDelete, ReferenceSpec,
    RelationshipRule, Schema, SchemaVersion, Snapshot, Strength, TxnKind, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType,
};

const KEY: &str = "test.item";

fn registry() -> Arc<TypeRegistry> {
    let link = ValueType::Reference(ReferenceSpec {
        targets: vec![TypeKey::new(KEY).unwrap()],
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
            FieldDecl::new("n", ValueType::I64),
            FieldDecl::new("link", link).optional(),
        ],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new(KEY).unwrap(), schema))
        .unwrap();
    Arc::new(registry)
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn document() -> Document {
    Document::new(registry(), Arc::new(Evaluators::new())).unwrap()
}

fn state(snapshot: &Snapshot) -> BTreeMap<u128, i64> {
    snapshot
        .iter()
        .map(|(_, o)| match o.values.first() {
            Some(Some(Value::I64(n))) => (o.id.as_u128(), *n),
            _ => (o.id.as_u128(), i64::MIN),
        })
        .collect()
}

fn command(name: &str) -> String {
    format!("{{\"command\":\"{name}\"}}")
}

fn create(doc: &Document, n: u128, value: i64, link: Option<u128>, name: &str) {
    let name = command(name);
    doc.submit(move |txn| {
        txn.meta_mut().command = name.clone();
        txn.meta_mut().label = name;
        let values = vec![
            Some(Value::I64(value)),
            link.map(|target| Value::Reference(id(target))),
        ];
        txn.create(KEY, id(n), Header::default(), values).map(drop)
    })
    .wait()
    .unwrap();
}

fn delete(doc: &Document, n: u128, name: &str) {
    let name = command(name);
    doc.submit(move |txn| {
        txn.meta_mut().command = name;
        txn.delete(id(n))
    })
    .wait()
    .unwrap();
}

fn set(doc: &Document, n: u128, value: i64) {
    doc.submit(move |txn| txn.modify(id(n), FieldPath::new("n"), Some(Value::I64(value))))
        .wait()
        .unwrap();
}

fn code<T: std::fmt::Debug>(result: arc_base::Result<T>) -> &'static str {
    result.unwrap_err().code()
}

#[test]
fn batches_are_atomic_bounded_and_one_commit() {
    let doc = document();
    for n in 1..=3 {
        create(&doc, n, 10 * i64::try_from(n).unwrap(), None, "make");
    }
    let full = state(&doc.snapshot());
    let events = doc.subscribe();
    let journal = doc.with_journal(arc_doc::Journal::len);

    // Out-of-range counts change nothing.
    assert_eq!(code(doc.undo_many(0).wait()), "doc.undo.sequence_invalid");
    assert_eq!(code(doc.undo_many(4).wait()), "doc.undo.sequence_invalid");
    assert_eq!(code(doc.redo_many(1).wait()), "doc.undo.sequence_invalid");
    assert_eq!(doc.with_journal(arc_doc::Journal::len), journal);

    assert!(doc.undo_many(2).wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 10)]));
    assert_eq!(doc.with_journal(arc_doc::Journal::len), journal + 1);
    assert!(events.try_recv().is_some());
    assert!(events.try_recv().is_none(), "one notification");
    assert_eq!(
        (doc.undo_step_list(10).len(), doc.redo_step_list(10).len()),
        (1, 2)
    );

    // The redo stack keeps one step per undone step: redo one, then the rest at once.
    assert!(doc.redo().wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 10), (2, 20)]));
    assert!(doc.redo_many(1).wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), full);

    // Undo everything, redo everything as one step, and undo that step alone.
    assert!(doc.undo_many(3).wait().unwrap().value);
    assert!(state(&doc.snapshot()).is_empty());
    assert!(doc.redo_many(3).wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), full);
    assert_eq!(doc.undo_step_list(10).len(), 1);
    assert!(doc.undo().wait().unwrap().value);
    assert!(state(&doc.snapshot()).is_empty());
    doc.with_journal(|journal| {
        let kinds: Vec<_> = journal.iter().map(|e| e.record().kind).collect();
        assert!(matches!(kinds.last(), Some(TxnKind::UndoOf(_))));
    });

    // A batch is refused inside an open group.
    doc.begin_group("\"g\"").wait().unwrap();
    assert_eq!(code(doc.undo_many(1).wait()), "doc.undo.sequence_invalid");
    doc.end_group().wait().unwrap();
}

#[test]
fn step_lists_name_the_commands_newest_first() {
    let doc = document();
    create(&doc, 1, 1, None, "first");
    create(&doc, 2, 2, None, "second");
    doc.begin_group("\"g\"").wait().unwrap();
    set(&doc, 1, 5);
    set(&doc, 2, 6);
    doc.end_group().wait().unwrap();
    let steps = doc.undo_step_list(10);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].transactions, 2, "a group is one step");
    assert_eq!(steps[1].command, command("second"));
    assert_eq!(steps[2].label, command("first"));
    assert_eq!(doc.undo_step_list(1).len(), 1);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.redo_step_list(10).len(), 1);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 1), (2, 2)]));
}

#[test]
fn marks_survive_commits_and_are_invalidated_by_rewriting() {
    let doc = document();
    create(&doc, 1, 1, None, "a");
    doc.set_mark("m").wait().unwrap();
    set(&doc, 1, 2);
    set(&doc, 1, 3);
    create(&doc, 2, 9, None, "b");
    assert_eq!(doc.marks(), vec!["m".to_owned()]);
    assert!(doc.undo_to_mark("m").wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 1)]));
    // Already at the mark: nothing to undo; the mark is still valid.
    assert!(!doc.undo_to_mark("m").wait().unwrap().value);
    // The undo to the mark left its three steps on the redo stack: redo them all.
    assert!(doc.redo_many(3).wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 3), (2, 9)]));
    // Undoing past the mark invalidates it.
    assert!(doc.undo_many(2).wait().unwrap().value);
    assert_eq!(
        code(doc.undo_to_mark("m").wait()),
        "doc.undo.sequence_invalid"
    );
    assert!(doc.marks().is_empty());

    // A mark on an empty stack, a new commit after undo, and name limits.
    let doc = document();
    doc.set_mark("start").wait().unwrap();
    create(&doc, 1, 1, None, "a");
    assert!(doc.undo_to_mark("start").wait().unwrap().value);
    assert!(doc.marks().contains(&"start".to_owned()));
    doc.set_mark("m").wait().unwrap();
    create(&doc, 2, 2, None, "b");
    assert!(doc.clear_mark("m").wait().unwrap().value);
    assert!(!doc.clear_mark("m").wait().unwrap().value);
    assert_eq!(code(doc.set_mark("").wait()), "doc.undo.sequence_invalid");
    assert_eq!(
        code(
            doc.set_mark("x".repeat(arc_doc::MAX_MARK_NAME_BYTES + 1))
                .wait()
        ),
        "doc.undo.sequence_invalid"
    );
    for n in 0..arc_doc::MAX_MARKS - 1 {
        doc.set_mark(format!("k{n}")).wait().unwrap();
    }
    assert_eq!(
        code(doc.set_mark("over").wait()),
        "doc.undo.sequence_invalid"
    );
    doc.begin_group("\"g\"").wait().unwrap();
    assert_eq!(code(doc.set_mark("k0").wait()), "doc.undo.sequence_invalid");
    doc.end_group().wait().unwrap();
}

#[test]
fn restore_erased_recreates_objects_without_undoing_later_commands() {
    let doc = document();
    create(&doc, 1, 10, None, "make");
    create(&doc, 2, 20, Some(1), "make");
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        0
    );
    delete(&doc, 2, "erase");
    set(&doc, 1, 11);
    let before = doc.with_journal(arc_doc::Journal::len);

    let done = doc
        .restore_erased(command("oops"), command("oops"))
        .wait()
        .unwrap();
    assert_eq!(done.value, 1);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 11), (2, 20)]));
    assert_eq!(doc.with_journal(arc_doc::Journal::len), before + 1);
    assert_eq!(doc.undo_step_list(1)[0].command, command("oops"));
    // The restored object is live with its identifier and its reference.
    let snapshot = doc.snapshot();
    let object = snapshot.get(snapshot.resolve(id(2)).unwrap()).unwrap();
    assert_eq!(object.values.get(1), Some(&Some(Value::Reference(id(1)))));
    // Nothing left to restore: the erase's objects are live.
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        0
    );

    // The restore undoes and redoes like any step.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 11)]));
    assert!(doc.redo().wait().unwrap().value);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 11), (2, 20)]));

    // Undone erases are not restorable: their objects are live.
    delete(&doc, 2, "erase");
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        0
    );
}

#[test]
fn restore_erased_brings_back_a_cascade_in_owner_first_order() {
    let doc = document();
    create(&doc, 1, 1, None, "make");
    doc.submit(|txn| {
        let header = Header {
            owner: Some(id(1)),
            ..Header::default()
        };
        txn.create(KEY, id(2), header, vec![Some(Value::I64(2)), None])
            .map(drop)
    })
    .wait()
    .unwrap();
    // The record deletes the owner and, by the deletion rules, the owned object; both return.
    let deleted = doc.submit(|txn| txn.delete(id(1))).wait().unwrap();
    assert!(deleted.txn_id.is_some());
    assert!(state(&doc.snapshot()).is_empty());
    let restored = doc
        .restore_erased("{}".into(), "{}".into())
        .wait()
        .unwrap()
        .value;
    assert_eq!(restored, 2);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 1), (2, 2)]));
}

#[test]
fn restore_erased_considers_only_the_newest_erase() {
    let doc = document();
    create(&doc, 1, 1, None, "make");
    create(&doc, 2, 2, None, "make");
    delete(&doc, 1, "erase-a");
    delete(&doc, 2, "erase-b");
    assert!(doc.undo().wait().unwrap().value);
    // The newest erase (B) was undone: nothing to restore, and A stays erased.
    for _ in 0..2 {
        assert_eq!(
            doc.restore_erased("{}".into(), "{}".into())
                .wait()
                .unwrap()
                .value,
            0
        );
        assert_eq!(state(&doc.snapshot()), BTreeMap::from([(2, 2)]));
    }
    // Restoring a real erase, then asking again, finds it live and restores nothing more.
    delete(&doc, 2, "erase-c");
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        1
    );
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        0
    );
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(2, 2)]));
    // A restore clears the redo stack like any commit.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(doc.redo_step_list(10).len(), 1);
    assert_eq!(state(&doc.snapshot()), BTreeMap::new());
    assert_eq!(
        doc.restore_erased("{}".into(), "{}".into())
            .wait()
            .unwrap()
            .value,
        1
    );
    assert!(doc.redo_step_list(10).is_empty());
}

#[test]
fn restore_erased_refuses_an_object_whose_reference_target_is_gone() {
    let doc = document();
    create(&doc, 1, 1, None, "make");
    create(&doc, 2, 2, Some(1), "make");
    delete(&doc, 2, "erase-2");
    // The target goes away in a merge transaction, which is no erase and no undo step.
    doc.submit(|txn| {
        txn.meta_mut().kind = TxnKind::Merge;
        txn.delete(id(1))
    })
    .wait()
    .unwrap();
    let (snapshot, journal, steps) = (
        doc.snapshot(),
        doc.with_journal(arc_doc::Journal::len),
        doc.undo_step_list(10).len(),
    );
    let error = doc
        .restore_erased("{}".into(), "{}".into())
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.reference.dangling");
    assert!(doc.snapshot().ptr_eq(&snapshot));
    assert_eq!(doc.with_journal(arc_doc::Journal::len), journal);
    assert_eq!(doc.undo_step_list(10).len(), steps);
}

#[test]
fn restore_erased_skips_objects_the_erase_created_and_deleted_itself() {
    let doc = document();
    create(&doc, 2, 20, None, "make");
    // One transaction creates a temporary object, deletes it and deletes B.
    let name = command("erase-with-temporary");
    doc.submit(move |txn| {
        txn.meta_mut().command = name.clone();
        txn.meta_mut().label = name;
        txn.create(
            KEY,
            id(1),
            Header::default(),
            vec![Some(Value::I64(1)), None],
        )
        .map(drop)?;
        txn.delete(id(1))?;
        txn.delete(id(2))
    })
    .wait()
    .unwrap();
    assert_eq!(state(&doc.snapshot()), BTreeMap::new());

    // Only B comes back: A never existed before that transaction.
    let done = doc
        .restore_erased(command("oops"), command("oops"))
        .wait()
        .unwrap();
    assert_eq!(done.value, 1);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(2, 20)]));
}

#[test]
fn restore_erased_looks_past_a_record_that_only_removed_its_own_temporaries() {
    let doc = document();
    create(&doc, 1, 1, None, "make");
    delete(&doc, 1, "erase-a");
    // A later record creates a temporary object and deletes it: it erased nothing that existed.
    let name = command("temporary");
    doc.submit(move |txn| {
        txn.meta_mut().command = name.clone();
        txn.meta_mut().label = name;
        txn.create(
            KEY,
            id(2),
            Header::default(),
            vec![Some(Value::I64(2)), None],
        )
        .map(drop)?;
        txn.delete(id(2))
    })
    .wait()
    .unwrap();
    assert_eq!(state(&doc.snapshot()), BTreeMap::new());

    // The search continues past it to the real erase of A.
    let done = doc
        .restore_erased(command("oops"), command("oops"))
        .wait()
        .unwrap();
    assert_eq!(done.value, 1);
    assert_eq!(state(&doc.snapshot()), BTreeMap::from([(1, 1)]));
}
