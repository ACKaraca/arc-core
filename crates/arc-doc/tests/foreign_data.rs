//! Foreign data rows as document state (CORE-DOC-040): the transaction operations that set,
//! replace and remove `arc_foreign` rows, their journal records, undo and redo, their bounds and
//! their isolation in snapshots. A model-based test runs random commits, undos and redos
//! against a plain map and checks every retained journal record round-trips.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::id::Identifier;
use arc_base::{Limits, ObjectId};
use arc_doc::graph::Evaluators;
use arc_doc::record::TxnRecord;
use arc_doc::{
    Document, DocumentConfig, FieldDecl, Header, Op, Schema, SchemaVersion, Snapshot,
    StoredForeign, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
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

fn document_with(limits: Limits) -> Document {
    let config = DocumentConfig {
        limits,
        workers: 0,
        ..DocumentConfig::default()
    };
    Document::open(
        Snapshot::empty(registry()),
        Arc::new(Evaluators::new()),
        config,
    )
    .unwrap()
}

fn document() -> Document {
    document_with(Limits::DEFAULT)
}

fn row(id: i64, owner: Option<u128>, data: &[u8]) -> StoredForeign {
    StoredForeign {
        id,
        source_format: "dxf".to_owned(),
        source_version: Some("AC1032".to_owned()),
        source_id: Some("s".to_owned()),
        object: owner.map(|owner| owner.to_be_bytes().to_vec()),
        record_kind: "entity".to_owned(),
        data: data.to_vec(),
        asset: None,
    }
}

fn rows(snapshot: &Snapshot) -> Vec<StoredForeign> {
    snapshot.foreign_rows().map(|row| (**row).clone()).collect()
}

#[test]
fn rows_are_set_replaced_removed_and_undone() {
    let doc = document();
    let owner = ObjectId::from_u128(7);
    doc.submit(move |txn| {
        txn.create(KEY, owner, Header::default(), vec![Some(Value::I64(1))])?;
        txn.set_foreign(row(5, Some(7), b"a"))?;
        let added = txn.add_foreign(row(0, Some(7), b"b"))?;
        assert_eq!(added, 6, "one more than the highest key");
        Ok(())
    })
    .wait()
    .unwrap();
    let first = doc.snapshot();
    assert_eq!(rows(&first), [row(5, Some(7), b"a"), row(6, Some(7), b"b")]);
    assert_eq!(first.foreign_of(owner).count(), 2);

    doc.submit(|txn| {
        txn.set_foreign(row(5, None, b"a2"))?;
        txn.remove_foreign(6)
    })
    .wait()
    .unwrap();
    assert_eq!(rows(&doc.snapshot()), [row(5, None, b"a2")]);
    assert_eq!(rows(&first).len(), 2, "a held snapshot keeps its rows");

    // Deleting the object a row names leaves the row.
    doc.submit(move |txn| txn.delete(owner)).wait().unwrap();
    assert_eq!(rows(&doc.snapshot()), [row(5, None, b"a2")]);
    doc.submit(|txn| {
        txn.set_foreign(row(9, Some(7), b"orphan"))?;
        Ok(())
    })
    .wait()
    .unwrap();
    doc.submit(move |txn| {
        txn.create(KEY, owner, Header::default(), vec![Some(Value::I64(2))])
            .map(drop)
    })
    .wait()
    .unwrap();
    doc.submit(move |txn| txn.delete(owner)).wait().unwrap();
    assert_eq!(doc.snapshot().foreign_len(), 2);

    for _ in 0..3 {
        assert!(doc.undo().wait().unwrap().value);
    }
    assert_eq!(rows(&doc.snapshot()), [row(5, None, b"a2")]);
    // Undoing the delete of the owner touches no row.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(rows(&doc.snapshot()), [row(5, None, b"a2")]);
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(
        rows(&doc.snapshot()),
        [row(5, Some(7), b"a"), row(6, Some(7), b"b")]
    );
    assert!(doc.undo().wait().unwrap().value);
    assert!(doc.snapshot().foreign_len() == 0 && doc.snapshot().is_empty());
    assert!(!doc.undo().wait().unwrap().value);
    for _ in 0..6 {
        assert!(doc.redo().wait().unwrap().value);
    }
    assert_eq!(doc.snapshot().foreign_len(), 2);
    assert_eq!(
        doc.snapshot().foreign(5).map(|r| r.data.clone()),
        Some(b"a2".to_vec())
    );
}

#[test]
fn malformed_rows_and_missing_rows_fail_and_a_failed_transaction_changes_nothing() {
    let doc = document();
    doc.submit(|txn| txn.set_foreign(row(1, None, b"x")))
        .wait()
        .unwrap();
    let before = doc.snapshot();
    let bad = |edit: fn(&mut StoredForeign)| {
        let mut r = row(2, None, b"y");
        edit(&mut r);
        r
    };
    for (r, reason) in [
        (bad(|r| r.object = Some(vec![1; 15])), "object"),
        (bad(|r| r.asset = Some(vec![1; 31])), "asset"),
        (bad(|r| r.source_format.clear()), "source_format"),
        (bad(|r| r.record_kind.clear()), "record_kind"),
    ] {
        let error = doc
            .submit(move |txn| txn.set_foreign(r))
            .wait()
            .unwrap_err();
        assert_eq!(error.code(), "doc.foreign.invalid");
        assert_eq!(
            error.arguments().get("reason"),
            Some(&arc_base::error::Value::from(reason))
        );
    }
    let error = doc.submit(|txn| txn.remove_foreign(99)).wait().unwrap_err();
    assert_eq!(error.code(), "doc.foreign.not_found");
    // A good write followed by a failing one: nothing is published.
    let error = doc
        .submit(|txn| {
            txn.set_foreign(row(3, None, b"z"))?;
            txn.remove_foreign(99)
        })
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.foreign.not_found");
    assert!(doc.snapshot().ptr_eq(&before));
    assert_eq!(rows(&doc.snapshot()), [row(1, None, b"x")]);
}

#[test]
fn rows_are_bounded_by_the_limits_at_commit() {
    let limits = Limits {
        record_count: 2,
        string_bytes: 8,
        input_bytes: 4,
        ..Limits::DEFAULT
    };
    let doc = document_with(limits);
    let over = |doc: &Document, edit: fn(&mut StoredForeign), name: &str| {
        let mut r = row(1, None, b"ok");
        edit(&mut r);
        let error = doc
            .submit(move |txn| txn.set_foreign(r))
            .wait()
            .unwrap_err();
        assert_eq!(error.code(), "doc.txn.limit_exceeded");
        assert_eq!(
            error.arguments().get("limit"),
            Some(&arc_base::error::Value::from(name))
        );
    };
    over(&doc, |r| r.data = vec![0; 5], "input_bytes");
    over(&doc, |r| r.record_kind = "k".repeat(9), "string_bytes");
    doc.submit(|txn| {
        txn.add_foreign(row(0, None, b"a"))?;
        txn.add_foreign(row(0, None, b"b")).map(drop)
    })
    .wait()
    .unwrap();
    let before = doc.snapshot();
    let error = doc
        .submit(|txn| txn.add_foreign(row(0, None, b"c")).map(drop))
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.txn.limit_exceeded");
    assert!(doc.snapshot().ptr_eq(&before));
    // Replacing and removing stay possible at the bound.
    doc.submit(|txn| {
        txn.set_foreign(row(1, None, b"r"))?;
        txn.remove_foreign(2)
    })
    .wait()
    .unwrap();
}

#[test]
fn foreign_operations_are_self_inverting_records_without_type_keys() {
    let doc = document();
    doc.submit(|txn| {
        txn.create(
            KEY,
            ObjectId::from_u128(1),
            Header::default(),
            vec![Some(Value::I64(1))],
        )?;
        txn.set_foreign(row(-4, Some(1), &[0xFF; 3]))?;
        txn.set_foreign(row(i64::MAX, None, b""))
    })
    .wait()
    .unwrap();
    doc.submit(|txn| txn.remove_foreign(-4)).wait().unwrap();
    let registry = registry();
    doc.with_journal(|journal| {
        assert_eq!(journal.len(), 2);
        for entry in journal.iter() {
            let decoded = TxnRecord::decode(entry.bytes(), &registry, &Limits::DEFAULT).unwrap();
            assert_eq!(&decoded, entry.record());
        }
        let first = journal.iter().next().unwrap().record();
        assert_eq!(first.ops.len(), 3);
        assert_eq!(
            first.type_keys().len(),
            1,
            "foreign operations name no type"
        );
        let last = journal.iter().next_back().unwrap().record();
        assert!(last.type_keys().is_empty());
        let Op::ModifyForeign { id, before, after } = &last.ops[0] else {
            panic!("{:?}", last.ops);
        };
        assert_eq!((*id, before.is_some(), after.is_none()), (-4, true, true));
        assert_eq!(last.ops[0].inverse().inverse(), last.ops[0]);
        // The decoder is strict: a flipped bit in the tag area never decodes to another record.
        let mut bytes = journal.iter().next_back().unwrap().bytes().to_vec();
        bytes.push(0);
        assert!(TxnRecord::decode(&bytes, &registry, &Limits::DEFAULT).is_err());
    });
}

#[derive(Debug, Clone)]
enum Edit {
    Set(i64, Vec<u8>),
    Add(Vec<u8>),
    Remove(i64),
    /// Fails the whole commit.
    Fail,
}

#[derive(Debug, Clone)]
enum Action {
    Commit(Vec<Edit>),
    Undo,
    Redo,
}

fn action() -> impl Strategy<Value = Action> {
    let edit = prop_oneof![
        3 => (1..6_i64, prop::collection::vec(any::<u8>(), 0..4)).prop_map(|(id, d)| Edit::Set(id, d)),
        3 => prop::collection::vec(any::<u8>(), 0..4).prop_map(Edit::Add),
        3 => (1..8_i64).prop_map(Edit::Remove),
        1 => Just(Edit::Fail),
    ];
    prop_oneof![
        6 => prop::collection::vec(edit, 0..4).prop_map(Action::Commit),
        3 => Just(Action::Undo),
        2 => Just(Action::Redo),
    ]
}

type Model = BTreeMap<i64, Vec<u8>>;

fn model_rows(model: &Model) -> Vec<StoredForeign> {
    model
        .iter()
        .map(|(id, data)| row(*id, None, data))
        .collect()
}

/// Applies `edits` to a copy of `model`; `None` when the commit must fail.
fn plan(model: &Model, edits: &[Edit]) -> Option<Model> {
    let mut next = model.clone();
    for edit in edits {
        match edit {
            Edit::Set(id, data) => {
                next.insert(*id, data.clone());
            }
            Edit::Add(data) => {
                let id = next.keys().next_back().map_or(1, |last| (last + 1).max(1));
                next.insert(id, data.clone());
            }
            Edit::Remove(id) => {
                next.remove(id)?;
            }
            Edit::Fail => return None,
        }
    }
    Some(next)
}

fn run(actions: &[Action]) {
    let doc = document();
    let registry = registry();
    let mut model = Model::new();
    let mut undo: Vec<(Model, Model)> = Vec::new();
    let mut redo: Vec<(Model, Model)> = Vec::new();
    for action in actions {
        match action {
            Action::Commit(edits) => {
                let before = doc.snapshot();
                let planned = plan(&model, edits);
                let list = edits.clone();
                let result = doc
                    .submit(move |txn| {
                        for edit in list {
                            match edit {
                                Edit::Set(id, data) => txn.set_foreign(row(id, None, &data))?,
                                Edit::Add(data) => {
                                    txn.add_foreign(row(0, None, &data))?;
                                }
                                Edit::Remove(id) => txn.remove_foreign(id)?,
                                Edit::Fail => {
                                    return Err(arc_base::Error::new(
                                        arc_doc::error::codes::OP_MISMATCH,
                                    ));
                                }
                            }
                        }
                        Ok(())
                    })
                    .wait();
                match (planned, result) {
                    (Some(next), Ok(done)) => {
                        if done.txn_id.is_some() {
                            undo.push((model.clone(), next.clone()));
                            redo.clear();
                        }
                        model = next;
                    }
                    (None, Err(_)) => assert!(doc.snapshot().ptr_eq(&before)),
                    (planned, result) => panic!("{planned:?} against {result:?}"),
                }
            }
            Action::Undo => {
                let done = doc.undo().wait().unwrap();
                match undo.pop() {
                    Some((before, after)) => {
                        assert!(done.value);
                        model = before.clone();
                        redo.push((before, after));
                    }
                    None => assert!(!done.value),
                }
            }
            Action::Redo => {
                let done = doc.redo().wait().unwrap();
                match redo.pop() {
                    Some((before, after)) => {
                        assert!(done.value);
                        model = after.clone();
                        undo.push((before, after));
                    }
                    None => assert!(!done.value),
                }
            }
        }
        assert_eq!(rows(&doc.snapshot()), model_rows(&model));
    }
    doc.with_journal(|journal| {
        for entry in journal.iter() {
            let decoded = TxnRecord::decode(entry.bytes(), &registry, &Limits::DEFAULT).unwrap();
            assert_eq!(&decoded, entry.record());
        }
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn foreign_rows_follow_a_map_through_commits_undo_and_redo(
        actions in prop::collection::vec(action(), 1..30)
    ) {
        run(&actions);
    }
}
