//! History queries (CORE-DOC-025), state comparison (CORE-DOC-026) and the source map
//! (CORE-DOC-052).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::graph::Evaluators;
use arc_doc::undo::differences;
use arc_doc::{
    ChangeKind, Comparison, Document, DocumentConfig, FieldDecl, FieldPath, Header, HistoryQuery,
    Schema, SchemaVersion, Snapshot, SourceMap, TypeDecl, TypeKey, TypeRegistry, Value, ValueType,
    compare, compare_ops,
};
use proptest::prelude::*;

const A: &str = "test.a";
const B: &str = "test.b";

fn registry() -> Arc<TypeRegistry> {
    let mut registry = TypeRegistry::new();
    for key in [A, B] {
        let schema = Schema::new(
            SchemaVersion::new(1, 0),
            vec![
                FieldDecl::new("n", ValueType::I64),
                FieldDecl::new("tags", ValueType::List(Box::new(ValueType::String))).optional(),
            ],
        );
        registry
            .register(TypeDecl::new(TypeKey::new(key).unwrap(), schema))
            .unwrap();
    }
    Arc::new(registry)
}

fn document() -> Document {
    let config = DocumentConfig {
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

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn create(doc: &Document, key: &'static str, n: u128, author: &'static str, time: u64) {
    doc.submit(move |txn| {
        let meta = txn.meta_mut();
        meta.author = author.to_owned();
        meta.time_unix_millis = time;
        txn.create(
            key,
            id(n),
            Header::default(),
            vec![Some(Value::I64(0)), None],
        )
        .map(drop)
    })
    .wait()
    .unwrap();
}

fn set_n(doc: &Document, n: u128, value: i64, author: &'static str, time: u64) {
    doc.submit(move |txn| {
        let meta = txn.meta_mut();
        meta.author = author.to_owned();
        meta.time_unix_millis = time;
        txn.modify(id(n), FieldPath::new("n"), Some(Value::I64(value)))
    })
    .wait()
    .unwrap();
}

#[test]
fn history_filters_by_object_type_author_and_time() {
    let doc = document();
    create(&doc, A, 1, "ann", 100);
    create(&doc, B, 2, "bob", 200);
    set_n(&doc, 1, 5, "bob", 300);
    set_n(&doc, 2, 6, "ann", 400);
    let labels = |q: &HistoryQuery| -> Vec<(String, u64)> {
        doc.history(q)
            .iter()
            .map(|r| (r.author.clone(), r.time_unix_millis))
            .collect()
    };
    assert_eq!(labels(&HistoryQuery::default()).len(), 4);
    let touched = labels(&HistoryQuery::touching(id(1)));
    assert_eq!(touched, [("ann".into(), 100), ("bob".into(), 300)]);
    let by_type = HistoryQuery {
        type_key: Some(TypeKey::new(B).unwrap()),
        ..HistoryQuery::default()
    };
    assert_eq!(labels(&by_type), [("bob".into(), 200), ("ann".into(), 400)]);
    let by_author = HistoryQuery {
        author: Some("bob".into()),
        ..HistoryQuery::default()
    };
    assert_eq!(labels(&by_author).len(), 2);
    let ranged = HistoryQuery {
        from_millis: Some(200),
        until_millis: Some(400),
        ..HistoryQuery::default()
    };
    assert_eq!(labels(&ranged), [("bob".into(), 200), ("bob".into(), 300)]);
    let combined = HistoryQuery {
        object: Some(id(2)),
        author: Some("ann".into()),
        ..HistoryQuery::default()
    };
    assert_eq!(labels(&combined), [("ann".into(), 400)]);
    // Object and type apply to the same operation.
    let mismatched = HistoryQuery {
        object: Some(id(1)),
        type_key: Some(TypeKey::new(B).unwrap()),
        ..HistoryQuery::default()
    };
    assert!(labels(&mismatched).is_empty());
    assert_eq!(doc.with_journal(|j| j.query(&by_type).count()), 2);
}

#[test]
fn comparison_lists_created_deleted_modified_and_relinked() {
    let doc = document();
    create(&doc, A, 1, "x", 1);
    create(&doc, A, 2, "x", 1);
    create(&doc, A, 3, "x", 1);
    let before = doc.snapshot();
    doc.submit(|txn| {
        txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(7)))?;
        txn.modify(
            id(1),
            FieldPath::new("tags"),
            Some(Value::List(vec![Value::String("t".into())])),
        )?;
        txn.delete(id(2))?;
        txn.relink(id(3), Some(id(1)))?;
        txn.create(B, id(4), Header::default(), vec![Some(Value::I64(1)), None])
            .map(drop)
    })
    .wait()
    .unwrap();
    let after = doc.snapshot();
    let cmp = compare(&before, &after);
    let kinds: Vec<_> = cmp
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.kind))
        .collect();
    assert_eq!(kinds.len(), 4);
    assert_eq!(kinds[0].1, ChangeKind::Modified);
    assert_eq!(kinds[1].1, ChangeKind::Deleted);
    assert_eq!(kinds[2].1, ChangeKind::Relinked);
    assert_eq!(kinds[3].1, ChangeKind::Created);
    let first = &cmp.objects[0];
    let paths: Vec<_> = first.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["n", "tags"]);
    assert_eq!(first.changes[0].before, Some(Value::I64(0)));
    assert_eq!(first.changes[0].after, Some(Value::I64(7)));
    assert_eq!(first.changes[1].before, None);
    assert_eq!(cmp.objects[2].changes[0].path, "(owner)");
    // The canonical JSON is stable and mirrors.
    let json = cmp.to_json().unwrap();
    assert_eq!(json, compare(&before, &after).to_json().unwrap());
    assert!(json.contains("\"kind\": \"relinked\""), "{json}");
    assert!(json.contains("\"path\": \"n\""), "{json}");
    // A change set (the operations of the commit) compares like the state it produces.
    let ops = doc.with_journal(|j| j.last().unwrap().record().ops.clone());
    assert_eq!(compare_ops(&before, &ops).unwrap(), cmp);
    // Undo returns a state whose comparison with the original is empty.
    assert!(doc.undo().wait().unwrap().value);
    assert!(compare(&before, &doc.snapshot()).is_empty());
}

#[test]
fn comparison_skips_shared_chunks_in_a_large_store() {
    let doc = document();
    doc.submit(|txn| {
        for n in 0..3000_u128 {
            txn.create(
                A,
                id(n + 1),
                Header::default(),
                vec![Some(Value::I64(0)), None],
            )?;
        }
        Ok(())
    })
    .wait()
    .unwrap();
    let before = doc.snapshot();
    set_n(&doc, 2500, 1, "x", 1);
    let cmp = compare(&before, &doc.snapshot());
    assert_eq!(cmp.objects.len(), 1);
    assert_eq!(cmp.objects[0].id, id(2500).to_string());
}

fn mirror(c: &Comparison) -> Comparison {
    Comparison {
        objects: c
            .objects
            .iter()
            .map(|o| {
                let mut m = o.clone();
                m.kind = match o.kind {
                    ChangeKind::Created => ChangeKind::Deleted,
                    ChangeKind::Deleted => ChangeKind::Created,
                    other => other,
                };
                m.before_type = o.after_type.clone();
                m.after_type = o.before_type.clone();
                for change in &mut m.changes {
                    std::mem::swap(&mut change.before, &mut change.after);
                }
                m
            })
            .collect(),
    }
}

#[derive(Debug, Clone)]
enum Edit {
    Create(u8, bool),
    Modify(u8, i64),
    Tag(u8, Option<String>),
    Delete(u8),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0..12_u8, any::<bool>()).prop_map(|(a, b)| Edit::Create(a, b)),
        (0..12_u8, -3..3_i64).prop_map(|(a, b)| Edit::Modify(a, b)),
        (0..12_u8, prop::option::of("[a-c]{0,2}")).prop_map(|(a, b)| Edit::Tag(a, b)),
        (0..12_u8).prop_map(Edit::Delete),
    ]
}

proptest! {
    /// Comparing a state with itself is empty, `compare(a, b)` mirrors `compare(b, a)`, and
    /// the objects compared as different are those `undo::differences` finds.
    #[test]
    fn comparison_is_empty_on_itself_and_mirrors(
        first in prop::collection::vec(edit(), 0..12),
        second in prop::collection::vec(edit(), 0..12),
    ) {
        let doc = document();
        let run = |edits: Vec<Edit>| {
            for e in edits {
                let _ = doc.submit(move |txn| match e {
                    Edit::Create(n, b) => txn
                        .create(if b { A } else { B }, id(u128::from(n) + 1), Header::default(), vec![Some(Value::I64(0)), None])
                        .map(drop),
                    Edit::Modify(n, v) => txn.modify(id(u128::from(n) + 1), FieldPath::new("n"), Some(Value::I64(v))),
                    Edit::Tag(n, t) => txn.modify(
                        id(u128::from(n) + 1),
                        FieldPath::new("tags"),
                        t.map(|t| Value::List(vec![Value::String(t)])),
                    ),
                    Edit::Delete(n) => txn.delete(id(u128::from(n) + 1)),
                }).wait();
            }
        };
        run(first);
        let a = doc.snapshot();
        run(second);
        let b = doc.snapshot();
        prop_assert!(compare(&a, &a).is_empty());
        prop_assert!(compare(&b, &b).is_empty());
        let ab = compare(&a, &b);
        prop_assert_eq!(&mirror(&compare(&b, &a)), &ab);
        let ids: BTreeSet<String> = ab.objects.iter().map(|o| o.id.clone()).collect();
        let expected: BTreeSet<String> = differences(&a, &b).iter().map(ToString::to_string).collect();
        prop_assert_eq!(ids, expected);
    }
}

// ---- source map ---------------------------------------------------------------------

/// Imports `sources` the way an importer does: resolve each source entity or create an object
/// for it and record the source identifier. Returns the object of each source.
fn import(
    doc: &Document,
    sources: &[(String, String, String)],
    next: u128,
) -> BTreeMap<(String, String, String), ObjectId> {
    let sources = sources.to_vec();
    doc.submit(move |txn| {
        let mut map = SourceMap::of(txn.snapshot());
        let mut out = BTreeMap::new();
        let mut next = next;
        for (format, document, source_id) in sources {
            let object = match map.resolve(&format, &document, &source_id) {
                Some(object) => {
                    txn.modify(object, FieldPath::new("n"), Some(Value::I64(1)))?;
                    object
                }
                None => {
                    next += 1;
                    let object = id(next);
                    txn.create(
                        A,
                        object,
                        Header::default(),
                        vec![Some(Value::I64(0)), None],
                    )?;
                    map.bind(txn, &format, &document, &source_id, object)?;
                    object
                }
            };
            out.insert((format, document, source_id), object);
        }
        Ok(out)
    })
    .wait()
    .unwrap()
    .value
}

proptest! {
    /// A re-import of the same sources resolves to the same objects and creates none.
    #[test]
    fn a_reimport_resolves_to_the_same_objects(
        sources in prop::collection::vec(("[xy]", "[pq]{0,2}", "[a-d]{1,2}"), 1..12),
    ) {
        let doc = document();
        let first = import(&doc, &sources, 1000);
        let count = doc.snapshot().len();
        let second = import(&doc, &sources, 5000);
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(doc.snapshot().len(), count);
        let distinct: BTreeSet<_> = sources.iter().collect();
        prop_assert_eq!(count, distinct.len());
    }
}

#[test]
fn a_deleted_object_does_not_resolve_and_a_rebind_points_to_the_new_one() {
    let doc = document();
    let src = vec![("ifc".to_owned(), "m.ifc".to_owned(), "G1".to_owned())];
    let first = import(&doc, &src, 1000);
    let old = first.values().next().copied().unwrap();
    doc.submit(move |txn| txn.delete(old)).wait().unwrap();
    let map = SourceMap::of(&doc.snapshot());
    assert_eq!(map.resolve("ifc", "m.ifc", "G1"), None);
    assert_eq!(map.sources_of(old), [("ifc", "m.ifc", "G1")]);
    let second = import(&doc, &src, 2000);
    let new = second.values().next().copied().unwrap();
    assert_ne!(new, old);
    assert_eq!(doc.snapshot().foreign_len(), 1, "the record was re-pointed");
    assert_eq!(
        SourceMap::of(&doc.snapshot()).resolve("ifc", "m.ifc", "G1"),
        Some(new)
    );
    // Undo and redo carry the record with the object.
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(
        SourceMap::of(&doc.snapshot()).resolve("ifc", "m.ifc", "G1"),
        None
    );
    assert!(doc.undo().wait().unwrap().value);
    assert_eq!(
        SourceMap::of(&doc.snapshot()).resolve("ifc", "m.ifc", "G1"),
        Some(old)
    );
    // Another document or format is another entity.
    assert_eq!(
        SourceMap::of(&doc.snapshot()).resolve("ifc", "other.ifc", "G1"),
        None
    );
    let err = doc
        .submit(move |txn| SourceMap::default().bind(txn, "", "d", "i", old))
        .wait()
        .unwrap_err();
    assert_eq!(err.code(), "doc.foreign.invalid");
}
