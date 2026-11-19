//! Block references nested deeper than the drafting model allows: the deepest ones are dropped
//! with a loss item, and the rest of the file imports.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use arc_draft_dxf::plan::{Container, ImportOptions, Op, Shape, plan};
use arc_io_dxf::model::Insert;
use arc_io_dxf::{Drawing, Entity, EntityKind};
use serde_json::Value;
use support::{Env, bytes, drawing, line};

const REASON: &str = "draft.dxf.reference_depth_exceeded";

fn insert_of(name: &str) -> Entity {
    Entity::new(EntityKind::Insert(Insert {
        block: name.to_owned(),
        ..Insert::default()
    }))
}

/// A chain of `links` references: the model space inserts `B1`, `B1` inserts `B2` and so on up
/// to `B{links}`, which holds a line.
fn chain(links: usize) -> Drawing {
    let mut d = drawing(vec![insert_of("B1")], Some(4));
    for k in 1..links {
        d.blocks.push(support::block(
            &format!("B{k}"),
            vec![insert_of(&format!("B{}", k + 1))],
        ));
    }
    d.blocks.push(support::block(
        &format!("B{links}"),
        vec![line([0.0; 3], [1.0, 0.0, 0.0])],
    ));
    d
}

/// The inserts of the plan as (container, block) pairs, the container `None` for the model space.
fn inserts(plan: &arc_draft_dxf::Plan) -> Vec<(Option<String>, String)> {
    plan.ops
        .iter()
        .filter_map(|op| match op {
            Op::Entity { container, spec } => match &spec.shape {
                Shape::Insert(i) => Some((
                    match container {
                        Container::Space => None,
                        Container::Block(name) => Some(name.clone()),
                    },
                    i.block.clone(),
                )),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn depth_items(report: &arc_io_dxf::loss::LossReport) -> Vec<(u64, Option<String>)> {
    report
        .items
        .iter()
        .filter(|i| i.reason == REASON)
        .map(|i| {
            let count = match i.arguments.get("count") {
                Some(arc_io_dxf::loss::Argument::Int(n)) => u64::try_from(*n).unwrap_or(0),
                _ => 0,
            };
            (count, i.source_id.clone())
        })
        .collect()
}

#[test]
fn a_chain_of_65_references_drops_only_the_deepest() {
    let d = chain(65);
    let p = plan(&d, &ImportOptions::default());
    let kept = inserts(&p);
    assert_eq!(kept.len(), 64);
    assert!(!kept.contains(&(Some("B64".to_owned()), "B65".to_owned())));
    assert!(kept.contains(&(Some("B63".to_owned()), "B64".to_owned())));
    assert!(kept.contains(&(None, "B1".to_owned())));
    assert_eq!(depth_items(&p.report).len(), 1);
    assert_eq!(depth_items(&p.report)[0].0, 1);
    assert_eq!(p.report.summary.dropped, 1);
    // The definition itself is still created; only the reference is not.
    assert!(
        p.ops
            .iter()
            .any(|op| matches!(op, Op::Block(b) if b.name == "B65"))
    );
}

#[test]
fn a_chain_of_64_references_imports_without_loss() {
    let d = chain(64);
    let p = plan(&d, &ImportOptions::default());
    assert_eq!(inserts(&p).len(), 64);
    assert!(depth_items(&p.report).is_empty());
    assert_eq!(p.report.summary.dropped, 0);
}

#[test]
fn a_wide_graph_with_shared_sub_blocks_drops_only_the_reference_beyond_the_limit() {
    // C1 -> C2 -> ... -> C63 -> C64 -> D is 65 references deep, E -> D is 2 deep, and twenty
    // blocks W0..W19 each insert D at level 2. Only C64 -> D is above the limit.
    let mut d = drawing(
        vec![insert_of("C1"), insert_of("E")]
            .into_iter()
            .chain((0..20).map(|i| insert_of(&format!("W{i}"))))
            .collect(),
        Some(4),
    );
    for k in 1..64 {
        d.blocks.push(support::block(
            &format!("C{k}"),
            vec![insert_of(&format!("C{}", k + 1))],
        ));
    }
    d.blocks.push(support::block("C64", vec![insert_of("D")]));
    d.blocks.push(support::block("E", vec![insert_of("D")]));
    for i in 0..20 {
        d.blocks
            .push(support::block(&format!("W{i}"), vec![insert_of("D")]));
    }
    d.blocks
        .push(support::block("D", vec![line([0.0; 3], [1.0, 0.0, 0.0])]));
    let p = plan(&d, &ImportOptions::default());
    let kept = inserts(&p);
    assert!(!kept.contains(&(Some("C64".to_owned()), "D".to_owned())));
    assert!(kept.contains(&(Some("E".to_owned()), "D".to_owned())));
    assert_eq!(kept.iter().filter(|(_, b)| b == "D").count(), 21);
    assert!(kept.contains(&(Some("C63".to_owned()), "C64".to_owned())));
    assert_eq!(depth_items(&p.report).len(), 1);
    assert_eq!(p.report.summary.dropped, 1);
}

#[test]
fn the_import_command_completes_a_chain_of_65_references() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("deep.dxf", &bytes(&chain(65)));
    let result: Value = env.import(&path, space).unwrap().value;
    let items = result["report"]["items"].as_array().unwrap();
    let depth: Vec<&Value> = items.iter().filter(|i| i["reason"] == REASON).collect();
    assert_eq!(depth.len(), 1);
    assert_eq!(depth[0]["arguments"]["count"], 1);
    assert_eq!(depth[0]["arguments"]["block"], "B65");
    assert_eq!(result["report"]["summary"]["dropped"], 1);
    // The model space keeps its one reference; the definitions were created in the transaction.
    assert_eq!(env.exported(space).entities.len(), 1);
}

#[test]
fn the_import_command_has_no_depth_loss_for_a_chain_of_64_references() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let path = env.write("ok.dxf", &bytes(&chain(64)));
    let result: Value = env.import(&path, space).unwrap().value;
    let items = result["report"]["items"].as_array().unwrap();
    assert!(items.iter().all(|i| i["reason"] != REASON));
}

#[test]
fn a_short_kept_chain_keeps_the_blocks_a_dropped_chain_reaches() {
    // Space -> C1 -> ... -> C64 -> D is 65 references: only C64 -> D is dropped. Space -> E -> D
    // reaches D at level 2, so D -> X (level 3) must be kept.
    let mut d = drawing(vec![insert_of("C1"), insert_of("E")], Some(4));
    for k in 1..64 {
        d.blocks.push(support::block(
            &format!("C{k}"),
            vec![insert_of(&format!("C{}", k + 1))],
        ));
    }
    d.blocks.push(support::block("C64", vec![insert_of("D")]));
    d.blocks.push(support::block("E", vec![insert_of("D")]));
    d.blocks.push(support::block("D", vec![insert_of("X")]));
    d.blocks
        .push(support::block("X", vec![line([0.0; 3], [1.0, 0.0, 0.0])]));
    let p = plan(&d, &ImportOptions::default());
    let kept = inserts(&p);
    assert!(!kept.contains(&(Some("C64".to_owned()), "D".to_owned())));
    assert!(kept.contains(&(Some("E".to_owned()), "D".to_owned())));
    assert!(kept.contains(&(Some("D".to_owned()), "X".to_owned())));
    assert_eq!(depth_items(&p.report).len(), 1);
    assert_eq!(p.report.summary.dropped, 1);
}
