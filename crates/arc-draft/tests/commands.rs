//! The drafting commands through the invoker: entity creation, modification, the relationship
//! and deletion rules, draw order against a list model, block nesting and cycles, and appearance
//! of nested references read from the document.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use std::collections::BTreeMap;

use arc_base::ObjectId;
use arc_draft::appearance::{Source, resolve_in};
use arc_draft::order::sorted;
use arc_draft::types::{self, entity};
use proptest::prelude::*;
use serde_json::json;
use support::{Fixture, curve_json, every_curve};

fn code(result: arc_base::Result<serde_json::Value>) -> &'static str {
    result.unwrap_err().code()
}

#[test]
fn every_curve_kind_and_a_point_become_entities_of_their_type() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let keys = [
        types::LINE,
        types::RAY,
        types::CONSTRUCTION_LINE,
        types::CIRCLE,
        types::ARC,
        types::ELLIPSE,
        types::ELLIPTICAL_ARC,
        types::POLYLINE,
    ];
    for (curve, key) in every_curve().iter().zip(keys) {
        let id = f.entity(space, curve_json(curve));
        let snapshot = f.document.snapshot();
        assert_eq!(
            snapshot
                .type_key(snapshot.resolve(id).unwrap())
                .unwrap()
                .as_str(),
            key
        );
    }
    let point = f.entity(space, json!({"kind": "point", "position": [1.0, 2.0]}));
    assert!(f.values(point)[entity::KIND].is_some());
    // Bad geometry and a non-container owner are refused.
    let bad = json!({"owner": space.to_string(), "geometry": {"kind": "circle"}});
    assert_eq!(
        code(f.run("core.draft.entity.create", bad)),
        "draft.field.invalid"
    );
    let line = f.line(space);
    let in_line = json!({"owner": line.to_string(), "geometry": curve_json(&every_curve()[0])});
    assert_eq!(
        code(f.run("core.draft.entity.create", in_line)),
        "draft.object.wrong_type"
    );
}

#[test]
fn modify_sets_fields_and_validators_reject_bad_values() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let line = f.line(space);
    let set = |field: &str, value: serde_json::Value| json!({"objects": [line.to_string()], "field": field, "value": value});
    f.run("core.draft.entity.modify", set("elevation", json!(3.5)))
        .unwrap();
    f.run("core.draft.entity.modify", set("thickness", json!(-1.0)))
        .unwrap();
    f.run("core.draft.entity.modify", set("color_by", json!("object")))
        .unwrap();
    f.run(
        "core.draft.entity.modify",
        set("color", json!([1.0, 0.0, 0.0, 1.0])),
    )
    .unwrap();
    assert_eq!(types::f64_at(&f.values(line), entity::ELEVATION), Some(3.5));
    assert_eq!(
        code(f.run(
            "core.draft.entity.modify",
            set("extrusion", json!([0.0, 0.0, 0.0]))
        )),
        "draft.extrusion.zero"
    );
    let circle = curve_json(&every_curve()[3]);
    assert_eq!(
        code(f.run("core.draft.entity.modify", set("geometry", circle))),
        "draft.geometry.invalid"
    );
    assert_eq!(
        code(f.run("core.draft.entity.modify", set("nope", json!(1)))),
        "draft.field.invalid"
    );
    assert_eq!(
        code(f.run("core.draft.layer.modify", set("name", json!("x")))),
        "draft.object.wrong_type"
    );
    let layout = json!({"name": "sheet", "kind": "layout", "unit": "unitless"});
    assert_eq!(
        code(f.run("core.draft.space.create", layout)),
        "draft.space.layout_unitless"
    );
    let empty = json!({"name": "e", "elements": [{"kind": "dot"}]});
    assert_eq!(
        code(f.run("core.draft.linetype.load", empty)),
        "draft.linetype.pattern_empty"
    );
}

#[test]
fn layers_and_linetypes_are_blocked_from_deletion_while_used_and_names_are_unique() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 1.0}, {"kind": "gap", "length": 0.5}]}),
    );
    let layer = f.create(
        "core.draft.layer.create",
        json!({"name": "walls", "linetype": dashed.to_string(), "color": [0.0, 1.0, 0.0, 1.0]}),
    );
    assert_eq!(
        code(f.run("core.draft.layer.create", json!({"name": "walls"}))),
        "draft.name.taken"
    );
    let line = f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": curve_json(&every_curve()[0]), "layer": layer.to_string()}),
    );
    let delete_layer = json!({"objects": [layer.to_string()]});
    assert_eq!(
        code(f.run("core.draft.layer.delete", delete_layer.clone())),
        "doc.reference.delete_blocked"
    );
    f.run(
        "core.draft.entity.delete",
        json!({"objects": [line.to_string()]}),
    )
    .unwrap();
    f.run("core.draft.layer.delete", delete_layer).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

    #[test]
    fn draw_order_commands_match_a_list_model(
        count in 2_usize..6,
        ops in prop::collection::vec((prop::collection::vec(any::<prop::sample::Index>(), 1..3), 0_u8..4, any::<prop::sample::Index>()), 1..12),
    ) {
        let mut f = Fixture::new();
        let space = f.space("model");
        let mut list: Vec<ObjectId> = (0..count).map(|_| f.line(space)).collect();
        for (picks, kind, reference) in ops {
            let mut moved: Vec<ObjectId> = picks.iter().map(|p| list[p.index(list.len())]).collect();
            moved.sort();
            moved.dedup();
            let reference = list[reference.index(list.len())];
            let ids: Vec<String> = moved.iter().map(ToString::to_string).collect();
            let (command, params) = match kind {
                0 => ("core.draft.order.front", json!({"objects": ids})),
                1 => ("core.draft.order.back", json!({"objects": ids})),
                2 => ("core.draft.order.above", json!({"objects": ids, "reference": reference.to_string()})),
                _ => ("core.draft.order.below", json!({"objects": ids, "reference": reference.to_string()})),
            };
            let result = f.run(command, params);
            if kind >= 2 && moved.contains(&reference) {
                prop_assert_eq!(code(result), "draft.order.reference_moved");
                continue;
            }
            result.unwrap();
            let block: Vec<_> = list.iter().copied().filter(|i| moved.contains(i)).collect();
            let mut rest: Vec<_> = list.iter().copied().filter(|i| !moved.contains(i)).collect();
            let at = match kind {
                0 => rest.len(),
                1 => 0,
                2 => rest.iter().position(|i| *i == reference).unwrap() + 1,
                _ => rest.iter().position(|i| *i == reference).unwrap(),
            };
            rest.splice(at..at, block);
            list = rest;
            let keyed: Vec<_> = list
                .iter()
                .map(|id| match f.values(*id)[entity::DRAW_ORDER] {
                    Some(arc_doc::Value::I64(k)) => (*id, k),
                    _ => panic!("no key"),
                })
                .collect();
            let order: Vec<_> = sorted(&keyed).into_iter().map(|(id, _)| id).collect();
            prop_assert_eq!(&order, &list);
        }
    }
}

#[test]
fn draw_order_refuses_entities_of_different_containers() {
    let mut f = Fixture::new();
    let a = f.space("a");
    let b = f.space("b");
    let in_a = f.line(a);
    let in_b = f.line(b);
    let params = json!({"objects": [in_a.to_string()], "reference": in_b.to_string()});
    assert_eq!(
        code(f.run("core.draft.order.above", params)),
        "draft.order.container_mismatch"
    );
}

fn define(f: &mut Fixture, name: &str) -> ObjectId {
    f.create(
        "core.draft.block.define",
        json!({"name": name, "attributes": [{"tag": "NO", "default": "1"}]}),
    )
}

fn insert(
    f: &mut Fixture,
    owner: ObjectId,
    block: ObjectId,
) -> arc_base::Result<serde_json::Value> {
    f.run(
        "core.draft.block.insert",
        json!({"owner": owner.to_string(), "block": block.to_string(), "position": [0.0, 0.0]}),
    )
}

#[test]
fn a_cyclic_definition_is_rejected_with_the_cycle_listed() {
    let mut f = Fixture::new();
    let a = define(&mut f, "a");
    let b = define(&mut f, "b");
    let c = define(&mut f, "c");
    insert(&mut f, a, b).unwrap();
    insert(&mut f, b, c).unwrap();
    let error = insert(&mut f, c, a).unwrap_err();
    assert_eq!(error.code(), "draft.block.cycle");
    let expected = format!("{c} -> {a} -> {b} -> {c}");
    assert_eq!(
        error.details().get("cycle"),
        Some(&arc_base::error::Value::Text(expected))
    );
    let error = insert(&mut f, a, a).unwrap_err();
    assert_eq!(
        error.details().get("cycle"),
        Some(&arc_base::error::Value::Text(format!("{a} -> {a}")))
    );
    // The rejected transactions left nothing behind.
    assert_eq!(
        f.document
            .snapshot()
            .iter_type(types::BLOCK_REF)
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn nesting_deeper_than_the_limit_is_rejected_from_either_end() {
    let mut f = Fixture::with_nesting(3);
    let space = f.space("model");
    let blocks: Vec<_> = (0..4).map(|n| define(&mut f, &format!("b{n}"))).collect();
    insert(&mut f, blocks[0], blocks[1]).unwrap();
    insert(&mut f, blocks[1], blocks[2]).unwrap();
    // Space -> b0 -> b1 -> b2: three references.
    insert(&mut f, space, blocks[0]).unwrap();
    let error = insert(&mut f, blocks[2], blocks[3]).unwrap_err();
    assert_eq!(error.code(), "draft.block.nesting_too_deep");
    let error = insert(&mut f, space, blocks[0]);
    assert!(error.is_ok());
    // A definition that nobody inserts may nest up to the limit.
    let mut g = Fixture::with_nesting(3);
    let bs: Vec<_> = (0..4).map(|n| define(&mut g, &format!("b{n}"))).collect();
    insert(&mut g, bs[0], bs[1]).unwrap();
    insert(&mut g, bs[1], bs[2]).unwrap();
    insert(&mut g, bs[2], bs[3]).unwrap();
    let space = g.space("model");
    assert_eq!(
        insert(&mut g, space, bs[0]).unwrap_err().code(),
        "draft.block.nesting_too_deep"
    );
}

#[test]
fn block_insert_fills_attribute_defaults_and_rejects_unknown_tags() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let a = define(&mut f, "a");
    let r: ObjectId = insert(&mut f, space, a).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut expected = BTreeMap::new();
    expected.insert("NO".to_owned(), arc_doc::Value::String("1".into()));
    assert_eq!(
        f.values(r)[entity::REF_ATTRIBUTES],
        Some(arc_doc::Value::Map(expected))
    );
    let params = json!({"owner": space.to_string(), "block": a.to_string(), "position": [0.0, 0.0], "attributes": {"X": "1"}});
    assert_eq!(
        code(f.run("core.draft.block.insert", params)),
        "draft.block.attribute_unknown"
    );
    let zero = json!({"owner": space.to_string(), "block": a.to_string(), "position": [0.0, 0.0], "scale": [1.0, 0.0, 1.0]});
    assert_eq!(
        code(f.run("core.draft.block.insert", zero)),
        "draft.block.scale_zero"
    );
    // A used definition cannot be deleted; moving an entity into a new block works.
    let line = f.line(space);
    let b = f.create(
        "core.draft.block.define",
        json!({"name": "b", "objects": [line.to_string()]}),
    );
    let snapshot = f.document.snapshot();
    assert_eq!(
        snapshot
            .get(snapshot.resolve(line).unwrap())
            .unwrap()
            .header
            .owner,
        Some(b)
    );
}

#[test]
fn by_block_appearance_resolves_through_two_nested_references() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let outer_layer = f.create(
        "core.draft.layer.create",
        json!({"name": "outer", "color": [0.0, 0.0, 1.0, 1.0], "lineweight": 0.35}),
    );
    let inner = define(&mut f, "inner");
    let outer = define(&mut f, "outer");
    let line = f.line(inner);
    let inner_ref: ObjectId = insert(&mut f, outer, inner).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let outer_ref: ObjectId = f
        .run(
            "core.draft.block.insert",
            json!({"owner": space.to_string(), "block": outer.to_string(), "position": [1.0, 1.0], "layer": outer_layer.to_string()}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    for (id, mode) in [(line, "block"), (inner_ref, "block")] {
        for field in ["color_by", "lineweight_by"] {
            f.run(
                "core.draft.entity.modify",
                json!({"objects": [id.to_string()], "field": field, "value": mode}),
            )
            .unwrap();
        }
    }
    let resolved = resolve_in(
        &f.document.snapshot(),
        &[line, inner_ref, outer_ref],
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(resolved.color.source, Source::Layer(outer_layer));
    assert_eq!(
        resolved.color.value.rgba,
        arc_base::Color::new(0.0, 0.0, 1.0, 1.0)
    );
    assert_eq!(resolved.lineweight.value, Some(0.35));
    assert_eq!(resolved.linetype.source, Source::Default);
}
