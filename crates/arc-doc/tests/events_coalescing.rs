//! Document events (CORE-DOC-036): every commit emits `committed`; a subscriber that does not
//! read receives one coalesced event and never blocks the executor; dropped subscribers are
//! removed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "helpers outside test functions build valid literal fixtures"
)]

use std::sync::Arc;
use std::time::Duration;

use arc_base::ObjectId;
use arc_base::id::Identifier;
use arc_doc::events::{Changes, CommittedEvent};
use arc_doc::graph::Evaluators;
use arc_doc::{
    Document, Event, FieldDecl, FieldPath, Header, Schema, SchemaVersion, TypeDecl, TypeKey,
    TypeRegistry, Value, ValueType,
};

fn document() -> Document {
    let schema = Schema::new(
        SchemaVersion::new(1, 0),
        vec![FieldDecl::new("n", ValueType::I64)],
    );
    let mut registry = TypeRegistry::new();
    registry
        .register(TypeDecl::new(TypeKey::new("test.item").unwrap(), schema))
        .unwrap();
    Document::new(Arc::new(registry), Arc::new(Evaluators::new())).unwrap()
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn committed(event: Option<Event>) -> CommittedEvent {
    match event {
        Some(Event::Committed(event)) => event,
        other => panic!("{other:?}"),
    }
}

fn listed(changes: &Changes) -> (Vec<ObjectId>, Vec<ObjectId>) {
    match changes {
        Changes::Listed(by_type) => {
            let groups = &by_type[&TypeKey::new("test.item").unwrap()];
            (
                groups.created.iter().copied().collect(),
                groups.modified.iter().copied().collect(),
            )
        }
        Changes::All => panic!("all"),
    }
}

#[test]
fn a_slow_subscriber_gets_one_coalesced_event_and_never_blocks_the_executor() {
    let doc = document();
    let slow = doc.events().subscribe_with_capacity(2);
    let reader = doc.subscribe();
    const COMMITS: u128 = 200;
    for n in 1..=COMMITS {
        let pending = doc.submit(move |txn| {
            txn.create(
                "test.item",
                id(n),
                Header::default(),
                vec![Some(Value::I64(0))],
            )
            .map(drop)
        });
        // Each commit completes although the slow subscriber reads nothing.
        assert!(pending.wait_timeout(Duration::from_secs(10)).is_some());
        let event = committed(reader.try_recv());
        assert_eq!(event.revision, u64::try_from(n).unwrap());
        assert!(event.snapshot.ptr_eq(&doc.snapshot()));
    }
    doc.submit(|txn| txn.modify(id(1), FieldPath::new("n"), Some(Value::I64(1))))
        .wait()
        .unwrap();

    let first = committed(slow.try_recv());
    let second = committed(slow.try_recv());
    let merged = committed(slow.try_recv());
    assert!(slow.try_recv().is_none());
    assert_eq!((first.revision, first.commits), (1, 1));
    assert_eq!((second.revision, second.commits), (2, 1));
    assert_eq!(merged.revision, 201);
    assert_eq!(merged.commits, 199);
    assert!(merged.snapshot.ptr_eq(&doc.snapshot()));
    let (created, modified) = listed(&merged.changes);
    assert_eq!(created, (3..=COMMITS).map(id).collect::<Vec<_>>());
    assert_eq!(modified, vec![id(1)]);
}

#[test]
fn dropped_subscribers_are_removed_and_closing_ends_subscriptions() {
    let doc = document();
    let kept = doc.subscribe();
    drop(doc.subscribe());
    doc.submit(|txn| {
        txn.create(
            "test.item",
            id(1),
            Header::default(),
            vec![Some(Value::I64(0))],
        )
        .map(drop)
    })
    .wait()
    .unwrap();
    assert_eq!(doc.events().subscribers(), 1);
    assert_eq!(committed(kept.recv()).revision, 1);
    drop(doc);
    assert!(kept.recv().is_none());
}

#[test]
fn failed_jobs_emit_nothing() {
    let doc = document();
    let sub = doc.subscribe();
    let error = doc
        .submit(|txn| txn.modify(id(9), FieldPath::new("n"), None))
        .wait()
        .unwrap_err();
    assert_eq!(error.code(), "doc.object.not_found");
    assert!(sub.recv_timeout(Duration::from_millis(20)).is_none());
}
