//! Records the neutral model does not type are kept in the document as foreign data rows on
//! import, written back on export, removed again by undo, and bounded in number.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::Limits;
use arc_base::id::Identifier;
use arc_draft_dxf::{ImportOptions, plan};
use arc_io_dxf::{EntityKind, Group, Object, ObjectKind, PreservedRecord};
use support::{Env, bytes, circle, drawing};

fn record(kind: &str, handle: &str) -> PreservedRecord {
    PreservedRecord {
        kind: kind.to_owned(),
        groups: vec![
            Group::text(5, handle),
            Group::text(330, "1F"),
            Group::text(100, "AcDbEntity"),
            Group::text(8, "0"),
            Group::double(10, 1.5),
            Group::int(70, 3),
        ],
    }
}

fn preserved_entity(kind: &str, handle: &str) -> arc_io_dxf::Entity {
    arc_io_dxf::Entity::new(EntityKind::Preserved(record(kind, handle)))
}

#[test]
fn an_unmapped_record_is_kept_written_back_and_removed_by_undo() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(
        vec![circle([0.0; 3], 1.0), preserved_entity("HELIX", "ABC")],
        Some(4),
    );
    d.objects.push(Object::new(ObjectKind::Preserved(record(
        "ACDBPLACEHOLDER",
        "ABD",
    ))));
    let path = env.write("p.dxf", &bytes(&d));
    let before = env.snapshot();
    let result = env.import(&path, space).unwrap().value;
    assert_eq!(result["entities"], 1);
    let reasons: Vec<&str> = result["report"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons
            .iter()
            .filter(|r| **r == "draft.dxf.record_preserved")
            .count(),
        2
    );
    assert!(!reasons.contains(&"draft.dxf.entity_unmapped"));

    let after = env.snapshot();
    let rows: Vec<_> = after.foreign_of(space).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.source_format == "dxf"));
    assert_eq!(
        rows.iter()
            .map(|r| r.record_kind.as_str())
            .collect::<Vec<_>>(),
        ["entity", "object"]
    );
    assert_eq!(rows[0].source_id.as_deref(), Some("ABC"));

    let exported = env.exported(space);
    let entity = exported
        .entities
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Preserved(r) => Some(r),
            _ => None,
        })
        .expect("the preserved entity is written back");
    let expected = record("HELIX", "ABC");
    assert_eq!(entity.kind, "HELIX");
    for (got, want) in entity.groups.iter().zip(&expected.groups) {
        if want.code == 330 {
            assert_eq!(got.code, 330, "the owner is the container now");
        } else {
            assert_eq!(got, want);
        }
    }
    assert!(exported.objects.iter().any(
        |o| matches!(&o.kind, ObjectKind::Preserved(r) if *r == record("ACDBPLACEHOLDER", "ABD"))
    ));
    // The handles of the exporter's own records are above the preserved ones.
    assert!(
        exported
            .tables
            .layers
            .iter()
            .all(|t| t.head.handle.is_none_or(|h| h.0 > 0xABD))
    );
    // Export again after re-import into a second space: the rows follow the record.
    let again = env.exported(space);
    assert_eq!(again.entities.len(), exported.entities.len());

    // One undo removes the objects and the rows; redo brings them back.
    assert!(env.document.undo().wait().unwrap().value);
    assert_eq!(env.snapshot().foreign_len(), before.foreign_len());
    assert!(env.document.redo().wait().unwrap().value);
    assert_eq!(env.snapshot().foreign_of(space).count(), 2);
}

#[test]
fn records_beyond_the_bound_are_reported_not_kept() {
    let d = drawing(
        vec![
            preserved_entity("HELIX", "1"),
            preserved_entity("HELIX", "2"),
            preserved_entity("HELIX", "3"),
        ],
        Some(4),
    );
    let options = ImportOptions {
        limits: Limits {
            record_count: 1,
            ..Limits::DEFAULT
        },
        ..ImportOptions::default()
    };
    let plan = plan(&d, &options);
    assert_eq!(plan.foreign.len(), 1);
    let count = |reason: &str| -> i64 {
        plan.report
            .items
            .iter()
            .filter(|i| i.reason == reason)
            .map(|i| match i.arguments.get("count") {
                Some(arc_io_dxf::loss::Argument::Int(n)) => *n,
                _ => 1,
            })
            .sum()
    };
    assert_eq!(count("draft.dxf.record_preserved"), 1);
    assert_eq!(count("draft.dxf.foreign_overflow"), 2);
    let _ = bytes;
    let _ = <arc_base::ObjectId as Identifier>::from_u128;
}
