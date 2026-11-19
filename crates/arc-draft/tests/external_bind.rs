//! Binding a placed external reference copies every entity type of the linked space (CORE-DRAFT-025):
//! references are remapped to the copies, names collide under a prefix, clips keep or drop whole
//! entities, budgets refuse before anything is created, and one undo restores the host.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result};
use arc_doc::link::{self, LinkSpec, Links, LoadMode, Opened};
use arc_doc::{Header, Snapshot, Txn, Value, ValueType};
use arc_draft::display::{DisplayList, DisplayOptions, Lists, Shape, space_list_with_links};
use arc_draft::external::WorkBudget;
use arc_draft::external_commands::bind_command_with_budget;
use arc_draft::{style, types};
use arc_geom::{Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;
use support::{Fixture, curve_json, every_curve};

const HOST: u128 = 1;
const LINKED: u128 = 2;
const LINK: u128 = 10;
/// The seed of the linked drawing's identifiers: none of them is an identifier of the host.
const LINKED_SEED: u64 = 1_000_000;

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

/// A drafting document for the linked drawing, with identifiers from `LINKED_SEED`.
fn linked_fixture() -> Fixture {
    Fixture::drafting_seeded(LINKED_SEED)
}

fn s(object: ObjectId) -> String {
    object.to_string()
}

fn segment(a: (f64, f64), b: (f64, f64)) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)).unwrap())
}

fn host(document: ObjectId) -> (Fixture, ObjectId) {
    host_at(document, "linked.arc")
}

/// The host, with a link to `path` as the placed reference's drawing.
fn host_at(document: ObjectId, path: &'static str) -> (Fixture, ObjectId) {
    let mut f = Fixture::drafting_with_links();
    let space = f.space("Host");
    let spec = LinkSpec {
        document,
        path,
        hub_resource: None,
        mode: LoadMode::Full,
    };
    f.document
        .submit(move |txn: &mut Txn| link::create(txn, id(LINK), &spec))
        .wait()
        .unwrap();
    (f, space)
}

fn opening(document: ObjectId, snapshot: Snapshot) -> impl Fn(&str, &str) -> Result<Opened> {
    move |_from: &str, path: &str| {
        Ok(Opened {
            location: path.to_owned(),
            document,
            content: [7; 32],
            snapshot: snapshot.clone(),
        })
    }
}

fn load(f: &Fixture, resolver: &dyn link::LinkResolver) -> Links {
    link::load(
        &f.document.snapshot(),
        id(HOST),
        "host.arc",
        resolver,
        &Limits::DEFAULT,
    )
}

fn place(f: &mut Fixture, space: ObjectId, position: [f64; 2]) -> ObjectId {
    f.create(
        "core.draft.external_ref.create",
        json!({"owner": s(space), "link": id(LINK).to_string(), "position": position}),
    )
}

fn list(snapshot: &Snapshot, space: ObjectId, links: Option<&Links>) -> DisplayList {
    space_list_with_links(
        snapshot,
        Lists::Direct,
        links,
        space,
        &DisplayOptions::default(),
    )
    .unwrap()
}

fn bind_json(placed: ObjectId) -> Json {
    json!({"reference": placed.to_string()})
}

fn bind_with(f: &mut Fixture, links: &Links, budget: WorkBudget) {
    f.commands
        .register(bind_command_with_budget(links.clone(), budget))
        .unwrap();
}

/// Every reference of every host object names a host object, the reference fields name a
/// target of the type they require, and no reference is an identifier of the linked drawing.
fn assert_references_resolve(snapshot: &Snapshot, linked: &Snapshot) {
    fn walk(value: &Value, out: &mut Vec<ObjectId>) {
        match value {
            Value::Reference(id) => out.push(*id),
            Value::List(items) => items.iter().for_each(|v| walk(v, out)),
            Value::Record(fields) => fields.iter().flatten().for_each(|v| walk(v, out)),
            Value::Map(entries) => entries.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    fn typed(snapshot: &Snapshot, ty: &ValueType, value: &Value, owner: ObjectId) {
        match (ty, value) {
            (ValueType::Reference(spec), Value::Reference(target)) => {
                let handle = snapshot.resolve(*target).unwrap_or_else(|e| {
                    panic!("{owner} refers to {target}, not in the host: {e:?}")
                });
                let key = snapshot.type_key(handle).unwrap().as_str();
                assert!(
                    spec.targets.is_empty() || spec.targets.iter().any(|t| t.as_str() == key),
                    "{owner} refers to a {key}, which its field does not accept"
                );
            }
            (ValueType::List(inner), Value::List(items)) => {
                items.iter().for_each(|v| typed(snapshot, inner, v, owner));
            }
            (ValueType::Map(inner), Value::Map(entries)) => {
                entries
                    .values()
                    .for_each(|v| typed(snapshot, inner, v, owner));
            }
            (ValueType::Record(fields), Value::Record(values)) => {
                for (field, value) in fields.iter().zip(values) {
                    if let Some(value) = value {
                        typed(snapshot, &field.value_type, value, owner);
                    }
                }
            }
            _ => {}
        }
    }
    let linked_ids: BTreeSet<ObjectId> = linked.iter().map(|(_, o)| o.id).collect();
    for (handle, object) in snapshot.iter() {
        let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
        let mut found = Vec::new();
        object
            .values
            .iter()
            .flatten()
            .for_each(|v| walk(v, &mut found));
        for target in &found {
            assert!(
                !linked_ids.contains(target),
                "{} refers to {target}, an identifier of the linked drawing",
                object.id
            );
            assert!(
                snapshot.resolve(*target).is_ok(),
                "{} refers to {target} which is not in the host",
                object.id
            );
        }
        let decl = snapshot.registry().get(&key).unwrap();
        for (field, value) in decl.schema.fields.iter().zip(&object.values) {
            if let Some(value) = value {
                typed(snapshot, &field.value_type, value, object.id);
            }
        }
    }
}

/// The objects of the host whose owner is `owner`, by type key.
fn owned_by(snapshot: &Snapshot, owner: ObjectId) -> Vec<(String, Vec<Option<Value>>)> {
    snapshot
        .iter()
        .filter(|(_, object)| object.header.owner == Some(owner))
        .map(|(handle, object)| {
            let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
            (key, object.values.clone())
        })
        .collect()
}

/// The linked drawing with one of every entity type the crate defines. The block `inner` holds a
/// line, and `outer` places `inner`; the space places `outer`.
#[expect(clippy::too_many_lines, reason = "one fixture with every entity type")]
fn every_kind() -> Fixture {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let layer = d.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [1.0, 0.0, 0.0, 1.0]}),
    );
    let style = d.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "Std"}),
    );
    d.set("core.draft.style.modify", &[style], "height", json!(5.0));
    let dim_style = d.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "Dim"}),
    );
    d.set(
        "core.draft.style.modify",
        &[dim_style],
        "text_height",
        json!(3.0),
    );
    let line = d.create(
        "core.draft.entity.create",
        json!({"owner": s(space), "geometry": curve_json(&segment((0.0, 0.0), (10.0, 0.0))), "layer": s(layer)}),
    );
    d.entity(
        space,
        curve_json(&Curve2::Circle(
            arc_geom::Circle2::new(Point2::new(20.0, 0.0), 3.0).unwrap(),
        )),
    );
    d.entity(space, json!({"kind": "point", "position": [1.0, 1.0]}));
    d.entity(
        space,
        curve_json(&Curve2::Polyline(
            Polyline2::new(
                vec![
                    PolylineVertex::new(Point2::new(30.0, 0.0), 0.0),
                    PolylineVertex::new(Point2::new(35.0, 5.0), 0.0),
                    PolylineVertex::new(Point2::new(40.0, 0.0), 0.0),
                ],
                false,
            )
            .unwrap(),
        )),
    );
    d.create(
        "core.draft.spline.create",
        json!({"owner": s(space), "degree": 3,
               "control_points": [[0.0, 0.0], [1.0, 2.0], [3.0, 2.0], [4.0, 0.0], [5.0, 1.0]],
               "weights": [1.0, 2.0, 1.0, 1.0, 0.5]}),
    );
    d.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "Std text", "position": [50.0, 0.0], "style": s(style)}),
    );
    d.create(
        "core.draft.rich_text.create",
        json!({"owner": s(space), "text": "one\ntwo", "position": [50.0, 10.0], "width": 40.0}),
    );
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "linear", "style": s(dim_style),
               "points": [{"position": [0.0, -5.0]}, {"position": [10.0, -5.0]}], "line_position": [0.0, -8.0]}),
    );
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "aligned",
               "points": [{"position": [0.0, -15.0]}, {"position": [10.0, -12.0]}], "line_position": [0.0, -18.0]}),
    );
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "radial",
               "points": [{"object": s(line), "key": "start"}, {"position": [30.0, 0.0]}],
               "line_position": [60.0, 0.0]}),
    );
    let pattern = d
        .run(
            "core.draft.hatch_pattern.load",
            json!({"names": ["brick-bond"]}),
        )
        .unwrap();
    let pattern: ObjectId = pattern["ids"][0].as_str().unwrap().parse().unwrap();
    let square = curve_json(&Curve2::Polyline(
        Polyline2::new(
            vec![
                PolylineVertex::new(Point2::new(0.0, 60.0), 0.0),
                PolylineVertex::new(Point2::new(10.0, 60.0), 0.0),
                PolylineVertex::new(Point2::new(10.0, 70.0), 0.0),
                PolylineVertex::new(Point2::new(0.0, 70.0), 0.0),
            ],
            true,
        )
        .unwrap(),
    ));
    d.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "pattern", "boundary": [square], "pattern": s(pattern), "scale": 1.0}),
    );
    d.create(
        "core.draft.table.create",
        json!({"owner": s(space), "columns": [10.0, 20.0], "rows": [{"kind": "data", "height": 5.0}],
               "cells": [{"cell": "A1", "content": {"kind": "text", "text": "T"}}]}),
    );
    d.create(
        "core.draft.leader.create",
        json!({"owner": s(space), "points": [[0.0, 80.0], [5.0, 85.0]], "arrow": "dot"}),
    );
    d.create(
        "core.draft.multileader.create",
        json!({"owner": s(space), "leaders": [[[0.0, 90.0]]], "position": [20.0, 95.0], "text": "AB"}),
    );
    d.create(
        "core.draft.wipeout.create",
        json!({"owner": s(space), "frame": true,
               "boundary": closed(&[(100.0, 0.0), (110.0, 0.0), (110.0, 10.0), (100.0, 10.0)])}),
    );
    let inner = d.create("core.draft.block.define", json!({"name": "inner"}));
    d.create(
        "core.draft.entity.create",
        json!({"owner": s(inner), "geometry": curve_json(&segment((0.0, 0.0), (2.0, 0.0)))}),
    );
    let outer = d.create("core.draft.block.define", json!({"name": "outer"}));
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(outer), "block": s(inner), "position": [5.0, 0.0]}),
    );
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(space), "block": s(outer), "position": [120.0, 0.0]}),
    );
    d.create(
        "core.draft.array.create",
        json!({"owner": s(space), "block": s(inner), "position": [0.0, 120.0],
               "array": {"kind": "rectangular", "rows": 2, "columns": 2, "row_spacing": 5.0, "column_spacing": 5.0}}),
    );
    d
}

/// A closed polyline of the corners, as the crate's clip and wipeout boundaries are stored.
fn closed(corners: &[(f64, f64)]) -> Json {
    let vertices = corners
        .iter()
        .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
        .collect();
    curve_json(&Curve2::Polyline(Polyline2::new(vertices, true).unwrap()))
}

#[test]
fn binding_every_entity_type_keeps_the_display_and_remaps_every_reference() {
    let linked = every_kind().document.snapshot();
    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [200.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let before = list(&f.document.snapshot(), space, Some(&links));
    assert!(before.problems.is_empty(), "{:?}", before.problems);
    let objects_before = f.document.snapshot().iter().count();

    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();

    // The space holds 17 entities; the block of `inner` holds one line and `outer` one block reference.
    assert_eq!(result["entities"], json!(19), "{result}");
    assert_eq!(result["blocks"], json!(2), "inner and outer");
    assert_eq!(result["problems"], json!([]), "{result}");
    assert_references_resolve(&snapshot, &linked);

    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    let kinds: Vec<String> = owned_by(&snapshot, block)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    for key in [
        "core.draft.line",
        "core.draft.circle",
        "core.draft.point",
        "core.draft.polyline",
        "core.draft.spline",
        "core.draft.text",
        "core.draft.rich_text",
        "core.draft.dimension",
        "core.draft.hatch",
        "core.draft.table",
        "core.draft.leader",
        "core.draft.multileader",
        "core.draft.wipeout",
        "core.draft.block_ref",
        "core.draft.array",
    ] {
        assert!(
            kinds.iter().any(|k| k == key),
            "{key} missing from {kinds:?}"
        );
    }

    let after = list(&snapshot, space, None);
    assert_eq!(after.primitives.len(), before.primitives.len());
    for (a, b) in before.primitives.iter().zip(&after.primitives) {
        assert_eq!(a.shape, b.shape);
    }

    // One undo restores the reference and removes every created object.
    f.document.undo().wait().unwrap();
    let restored = f.document.snapshot();
    assert!(restored.resolve(placed).is_ok());
    assert_eq!(restored.iter().count(), objects_before);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one bind, its names and its appearance"
)]
fn copies_of_colliding_names_are_prefixed_and_keep_the_linked_appearance() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let layer = d.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [1.0, 0.0, 0.0, 1.0]}),
    );
    let style = d.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "Std"}),
    );
    d.set("core.draft.style.modify", &[style], "height", json!(5.0));
    let dim_style = d.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "Dim"}),
    );
    d.set(
        "core.draft.style.modify",
        &[dim_style],
        "text_height",
        json!(3.0),
    );
    d.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "Hi", "position": [1.0, 1.0], "style": s(style), "layer": s(layer)}),
    );
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "linear", "style": s(dim_style),
               "points": [{"position": [0.0, -5.0]}, {"position": [10.0, -5.0]}], "line_position": [0.0, -8.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let host_style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "Std"}),
    );
    f.set(
        "core.draft.style.modify",
        &[host_style],
        "height",
        json!(1.0),
    );
    let host_dim = f.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "Dim"}),
    );
    f.set(
        "core.draft.style.modify",
        &[host_dim],
        "text_height",
        json!(9.0),
    );
    f.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [0.0, 0.0, 1.0, 1.0]}),
    );
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let before = list(&f.document.snapshot(), space, Some(&links));

    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);

    let names: Vec<String> = [
        "core.draft.text_style",
        "core.draft.dim_style",
        "core.draft.layer",
    ]
    .iter()
    .flat_map(|key| {
        snapshot
            .iter_type(key)
            .unwrap()
            .filter_map(|(_, o)| match o.values.first() {
                Some(Some(Value::String(name))) => Some(name.clone()),
                _ => None,
            })
    })
    .collect();
    for name in ["linked-Std", "linked-Dim", "linked-Walls"] {
        assert!(
            names.iter().any(|n| n == name),
            "{name} missing from {names:?}"
        );
    }

    // The copied text uses the copied style, so its appearance is the linked one.
    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    let text_values = owned_by(&snapshot, block)
        .into_iter()
        .find(|(k, _)| k == "core.draft.text")
        .unwrap()
        .1;
    let style_ref = match &text_values[arc_draft::text::field::STYLE] {
        Some(Value::Reference(id)) => *id,
        other => panic!("no style reference: {other:?}"),
    };
    let copy = snapshot.get(snapshot.resolve(style_ref).unwrap()).unwrap();
    assert_eq!(copy.values[0], Some(Value::String("linked-Std".into())));
    assert_eq!(
        copy.values[arc_draft::text::style_field::HEIGHT],
        Some(Value::F64(5.0))
    );

    let after = list(&snapshot, space, None);
    assert_eq!(after.primitives.len(), before.primitives.len());
    for (a, b) in before.primitives.iter().zip(&after.primitives) {
        assert_eq!(a.shape, b.shape, "the copies keep the linked shapes");
    }
}

#[test]
fn associative_references_follow_their_copies() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let line = d.entity(space, curve_json(&segment((0.0, 0.0), (10.0, 0.0))));
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "radial",
               "points": [{"object": s(line), "key": "start"}, {"position": [30.0, 0.0]}],
               "line_position": [60.0, 0.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);
    assert_eq!(result["problems"], json!([]), "{result}");

    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    assert_eq!(dimension_target_owner(&snapshot, block), Some(block));
}

#[test]
fn associative_references_are_cleared_when_their_target_is_cut_away() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let line = d.entity(space, curve_json(&segment((-5.0, 5.0), (20.0, 5.0))));
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "radial",
               "points": [{"object": s(line), "key": "start"}, {"position": [5.0, 8.0]}],
               "line_position": [5.0, 8.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let clip = closed(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let placed = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": s(space), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": clip}),
        )
        .unwrap();
    let placed: ObjectId = placed["id"].as_str().unwrap().parse().unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);

    // The line is cut into a piece, which no reference names: the dimension becomes free.
    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    assert_eq!(dimension_target_owner(&snapshot, block), None);
    assert_eq!(
        result["problems"],
        json!([{"code": "draft.external_ref.bind_reference_cleared", "count": 1}]),
        "{result}"
    );
}

/// The owner of the object that the first point of the copied dimension in `block` names.
fn dimension_target_owner(snapshot: &Snapshot, block: ObjectId) -> Option<ObjectId> {
    let values = owned_by(snapshot, block)
        .into_iter()
        .find(|(k, _)| k == "core.draft.dimension")
        .expect("a copied dimension")
        .1;
    let points = match &values[arc_draft::dimension::field::POINTS] {
        Some(Value::List(items)) => items,
        other => panic!("points: {other:?}"),
    };
    // A point record holds its position and, when associative, the object it refers to.
    let target = match points.first() {
        Some(Value::Record(fields)) => fields.iter().flatten().find_map(|v| match v {
            Value::Reference(target) => Some(*target),
            _ => None,
        }),
        _ => None,
    };
    target.and_then(|target| {
        snapshot
            .get(snapshot.resolve(target).unwrap())
            .unwrap()
            .header
            .owner
    })
}

#[test]
fn a_clip_cuts_curves_and_keeps_or_drops_whole_entities_by_their_anchor() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    d.entity(space, curve_json(&segment((-5.0, 5.0), (20.0, 5.0))));
    d.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "inside", "position": [5.0, 5.0]}),
    );
    d.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "outside", "position": [50.0, 50.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let clip = closed(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let placed = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": s(space), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": clip}),
        )
        .unwrap();
    let placed: ObjectId = placed["id"].as_str().unwrap().parse().unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let before = list(&f.document.snapshot(), space, Some(&links));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    assert_eq!(result["clip_kept_whole"], json!(1), "{result}");
    assert_eq!(result["clip_dropped"], json!(1), "{result}");
    assert_eq!(result["clip_cut"], json!(1), "{result}");
    assert_references_resolve(&f.document.snapshot(), &linked);
    // The text inside is kept whole, so its strokes are not cut as the linked display cuts them.
    let after = list(&f.document.snapshot(), space, None);
    assert!(!after.primitives.is_empty());
    assert!(before.primitives.len() <= after.primitives.len());
}

#[test]
fn a_space_over_the_budget_is_refused_and_nothing_is_created() {
    let linked = every_kind().document.snapshot();
    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked));
    let objects_before = f.document.snapshot().iter().count();
    bind_with(
        &mut f,
        &links,
        WorkBudget {
            expansions: 10_000,
            primitives: 4,
            work: 1_000_000,
        },
    );
    let error = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap_err();
    assert_eq!(error.code(), "draft.external_ref.budget_exceeded");
    let snapshot = f.document.snapshot();
    assert!(snapshot.resolve(placed).is_ok());
    assert_eq!(snapshot.iter().count(), objects_before);
}

/// The objects of the host below `owner`'s block that name `key`, as values.
fn copies_of(snapshot: &Snapshot, block: ObjectId, key: &str) -> Vec<Vec<Option<Value>>> {
    owned_by(snapshot, block)
        .into_iter()
        .filter(|(k, _)| k == key)
        .map(|(_, values)| values)
        .collect()
}

/// The names of the objects of type `key`, in identifier order.
fn names_of_type(snapshot: &Snapshot, key: &str) -> Vec<String> {
    snapshot
        .iter_type(key)
        .unwrap()
        .filter_map(|(_, o)| match o.values.first() {
            Some(Some(Value::String(name))) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Every primitive of `after` has the shape of the matching primitive of `before`.
fn assert_same_shapes(before: &DisplayList, after: &DisplayList) {
    assert_eq!(after.primitives.len(), before.primitives.len());
    for (a, b) in before.primitives.iter().zip(&after.primitives) {
        assert_eq!(a.shape, b.shape);
    }
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Json {
    closed(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

#[test]
fn a_hatch_with_a_boundary_curve_the_clip_removes_keeps_its_area_and_is_not_associated() {
    // A ring: two closed circles, the outer one wholly outside the clip box, the inner inside it.
    let mut d = linked_fixture();
    let space = d.space("Model");
    let circle = |r: f64| {
        curve_json(&Curve2::Circle(
            arc_geom::Circle2::new(Point2::new(0.0, 0.0), r).unwrap(),
        ))
    };
    d.entity(space, circle(10.0));
    d.entity(space, circle(5.0));
    d.create(
        "core.draft.hatch.pick",
        json!({"owner": s(space), "kind": "solid", "pick": [6.0, 0.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let clip = square(-6.0, -6.0, 7.0, 6.0);
    let placed = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": s(space), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": clip}),
        )
        .unwrap();
    let placed: ObjectId = placed["id"].as_str().unwrap().parse().unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let before = list(&f.document.snapshot(), space, Some(&links));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);
    assert_eq!(result["clip_dropped"], json!(1), "{result}");
    assert_eq!(
        result["problems"],
        json!([{"code": "draft.external_ref.bind_reference_cleared", "count": 1}]),
        "{result}"
    );
    // Without the outer curve the hatch falls back to its stored loops, which are the ring: the
    // same region, in another representation of its loops.
    let after = list(&snapshot, space, None);
    assert_eq!(after.primitives.len(), before.primitives.len());
    let (area_before, area_after) = (region_area(&before), region_area(&after));
    let ring = std::f64::consts::PI * (100.0 - 25.0);
    assert!((area_before - ring).abs() < 1e-6, "{area_before}");
    assert!(
        (area_after - area_before).abs() < 1e-6,
        "{area_after} != {area_before}"
    );
}

/// The area of the filled regions of `list`: each part's outer loop less its holes.
fn region_area(list: &DisplayList) -> f64 {
    list.primitives
        .iter()
        .filter_map(|p| match &p.shape {
            Shape::Region(region) => Some(region),
            _ => None,
        })
        .flat_map(|region| region.parts())
        .map(|part| {
            part.outer().signed_area().abs()
                - part
                    .holes()
                    .iter()
                    .map(|h| h.signed_area().abs())
                    .sum::<f64>()
        })
        .sum()
}

#[test]
fn a_curve_the_clip_keeps_whole_stays_the_original_for_its_references() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let line = d.entity(space, curve_json(&segment((2.0, 5.0), (8.0, 5.0))));
    d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "radial",
               "points": [{"object": s(line), "key": "start"}, {"position": [8.0, 8.0]}],
               "line_position": [5.0, 8.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let clip = closed(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]);
    let placed = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": s(space), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": clip}),
        )
        .unwrap();
    let placed: ObjectId = placed["id"].as_str().unwrap().parse().unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);
    assert_eq!(result["clip_cut"], json!(0), "{result}");
    assert_eq!(result["clip_dropped"], json!(0), "{result}");
    assert_eq!(result["clip_kept_whole"], json!(2), "{result}");
    assert_eq!(result["problems"], json!([]), "{result}");
    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    assert_eq!(dimension_target_owner(&snapshot, block), Some(block));
}

#[test]
fn linked_defaults_map_to_the_host_defaults_and_other_collisions_are_prefixed() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let layer0 = d.create("core.draft.layer.create", json!({"name": "0"}));
    let walls = d.create("core.draft.layer.create", json!({"name": "Walls"}));
    let created = d.run("core.draft.style.defaults", json!({})).unwrap();
    let text_default: ObjectId = created["created"][0].as_str().unwrap().parse().unwrap();
    d.set(
        "core.draft.style.modify",
        &[text_default],
        "height",
        json!(5.0),
    );
    d.create(
        "core.draft.entity.create",
        json!({"owner": s(space), "geometry": curve_json(&segment((0.0, 0.0), (10.0, 0.0))), "layer": s(layer0)}),
    );
    d.create(
        "core.draft.entity.create",
        json!({"owner": s(space), "geometry": curve_json(&segment((0.0, 2.0), (10.0, 2.0))), "layer": s(walls)}),
    );
    d.create(
        "core.draft.text.create",
        json!({"owner": s(space), "content": "Hi", "position": [1.0, 1.0], "style": s(text_default)}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let host_layer0 = f.create(
        "core.draft.layer.create",
        json!({"name": "0", "color": [0.0, 0.0, 1.0, 1.0]}),
    );
    let host_created = f.run("core.draft.style.defaults", json!({})).unwrap();
    let host_text: ObjectId = host_created["created"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    f.set(
        "core.draft.style.modify",
        &[host_text],
        "height",
        json!(1.0),
    );
    f.create("core.draft.layer.create", json!({"name": "Walls"}));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);

    // No copy of the default layer or the default style exists; the other collision is prefixed.
    let layers = names_of_type(&snapshot, types::LAYER);
    assert!(!layers.iter().any(|n| n == "linked-0"), "{layers:?}");
    assert!(layers.iter().any(|n| n == "linked-Walls"), "{layers:?}");
    let styles = names_of_type(&snapshot, style::TEXT_STYLE);
    assert_eq!(
        styles.iter().filter(|n| *n == "default").count(),
        1,
        "{styles:?}"
    );
    assert!(!styles.iter().any(|n| n == "linked-default"), "{styles:?}");

    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    let lines = copies_of(&snapshot, block, "core.draft.line");
    let on_default = lines.iter().find(|v| {
        v.iter()
            .flatten()
            .any(|x| *x == Value::Reference(host_layer0))
    });
    assert!(
        on_default.is_some(),
        "no line on the host's layer 0: {lines:?}"
    );
    let texts = copies_of(&snapshot, block, "core.draft.text");
    assert_eq!(
        texts[0][arc_draft::text::field::STYLE],
        Some(Value::Reference(host_text))
    );
}

#[test]
fn a_budget_below_the_reach_refuses_before_any_copy_and_changes_nothing() {
    let linked = every_kind().document.snapshot();
    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked));
    let objects_before = f.document.snapshot().iter().count();
    bind_with(
        &mut f,
        &links,
        WorkBudget {
            expansions: 10_000,
            primitives: 1,
            work: 1_000_000,
        },
    );
    let error = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap_err();
    assert_eq!(error.code(), "draft.external_ref.budget_exceeded");
    let snapshot = f.document.snapshot();
    assert!(snapshot.resolve(placed).is_ok());
    assert_eq!(snapshot.iter().count(), objects_before);
}

#[test]
fn a_block_placed_shallowly_and_deeply_is_checked_at_its_deepest_placement() {
    // The space places a chain of 63 blocks and, last, `deep` on its own. The chain places `deep`
    // at depth 64, which is allowed; `deep` places `nested`, which then sits at depth 65 and is
    // refused, whichever placement the walk reads first.
    let mut d = Fixture::nesting_seeded(200, LINKED_SEED);
    let space = d.space("Model");
    let nested = d.create("core.draft.block.define", json!({"name": "nested"}));
    d.create(
        "core.draft.entity.create",
        json!({"owner": s(nested), "geometry": curve_json(&segment((0.0, 0.0), (1.0, 0.0)))}),
    );
    let deep = d.create("core.draft.block.define", json!({"name": "deep"}));
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(deep), "block": s(nested), "position": [0.0, 0.0]}),
    );
    let mut chain = Vec::new();
    for k in 1..=63 {
        chain.push(d.create("core.draft.block.define", json!({"name": format!("c{k}")})));
    }
    for pair in chain.windows(2) {
        d.create(
            "core.draft.block.insert",
            json!({"owner": s(pair[0]), "block": s(pair[1]), "position": [0.0, 0.0]}),
        );
    }
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(chain[62]), "block": s(deep), "position": [0.0, 0.0]}),
    );
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(space), "block": s(chain[0]), "position": [0.0, 0.0]}),
    );
    d.create(
        "core.draft.block.insert",
        json!({"owner": s(space), "block": s(deep), "position": [0.0, 0.0]}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked));
    let objects_before = f.document.snapshot().iter().count();
    bind_with(&mut f, &links, WorkBudget::default());
    let error = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap_err();
    assert_eq!(error.code(), "draft.block.nesting_too_deep");
    assert_eq!(f.document.snapshot().iter().count(), objects_before);
}

#[test]
fn a_long_prefixed_name_is_shortened_to_the_name_limit() {
    let stem = "s".repeat(150);
    let long = "L".repeat(200);
    let mut d = linked_fixture();
    let space = d.space("Model");
    let layer = d.create("core.draft.layer.create", json!({"name": long}));
    d.create(
        "core.draft.entity.create",
        json!({"owner": s(space), "geometry": curve_json(&segment((0.0, 0.0), (10.0, 0.0))), "layer": s(layer)}),
    );
    let linked = d.document.snapshot();

    let path: &'static str = Box::leak(format!("{stem}.arc").into_boxed_str());
    let (mut f, space) = host_at(id(LINKED), path);
    f.create("core.draft.layer.create", json!({"name": long}));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    assert_eq!(result["problems"], json!([]), "{result}");
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);
    let copies: Vec<String> = names_of_type(&snapshot, types::LAYER)
        .into_iter()
        .filter(|n| n.ends_with(&long) && n.len() > long.len())
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(copies[0].len(), 255);
    assert!(copies[0].starts_with('s'), "{}", copies[0]);
    assert!(copies[0].ends_with(&format!("-{long}")), "{}", copies[0]);
}

#[test]
fn the_header_name_partition_and_flags_survive_the_bind() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    let line = d.entity(space, curve_json(&segment((0.0, 0.0), (10.0, 0.0))));
    let values = d.values(line);
    let flags = arc_doc::Flags {
        system: true,
        ..arc_doc::Flags::default()
    };
    d.document
        .submit(move |txn: &mut Txn| {
            txn.create(
                "core.draft.line",
                id(900),
                Header {
                    owner: Some(space),
                    name: Some("tag".into()),
                    partition: Some("part".into()),
                    flags,
                },
                values,
            )
            .map(|_| ())
        })
        .wait()
        .unwrap();
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let placed_values = f.values(placed);
    f.document
        .submit(move |txn: &mut Txn| {
            txn.create(
                "core.draft.external_ref",
                id(901),
                Header {
                    owner: Some(space),
                    name: Some("placed".into()),
                    ..Header::default()
                },
                placed_values,
            )
            .map(|_| ())
        })
        .wait()
        .unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(id(901)))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);

    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    let copied: Vec<_> = snapshot
        .iter()
        .filter(|(_, o)| o.header.owner == Some(block) && o.header.name.as_deref() == Some("tag"))
        .map(|(_, o)| (o.header.partition.clone(), o.header.flags))
        .collect();
    assert_eq!(copied, vec![(Some("part".to_owned()), flags)]);
    let inserted: ObjectId = result["id"].as_str().unwrap().parse().unwrap();
    let block_ref = snapshot.get(snapshot.resolve(inserted).unwrap()).unwrap();
    assert_eq!(block_ref.header.name.as_deref(), Some("placed"));
}

#[test]
fn binding_every_curve_and_dimension_kind_keeps_the_display() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    for curve in every_curve() {
        d.entity(space, curve_json(&curve));
    }
    let dimensions = [
        json!({"owner": s(space), "kind": "angular_lines", "line_position": [5.0, 5.0],
               "points": [{"position": [0.0, 0.0]}, {"position": [10.0, 0.0]},
                          {"position": [0.0, 0.0]}, {"position": [0.0, 10.0]}]}),
        json!({"owner": s(space), "kind": "angular_points", "line_position": [20.0, 5.0],
               "points": [{"position": [20.0, 0.0]}, {"position": [30.0, 0.0]}, {"position": [20.0, 10.0]}]}),
        json!({"owner": s(space), "kind": "angular_arc", "line_position": [40.0, 7.0],
               "points": [{"position": [40.0, 0.0]}, {"position": [50.0, 0.0]}, {"position": [40.0, 10.0]}]}),
        json!({"owner": s(space), "kind": "diameter", "line_position": [70.0, 5.0],
               "points": [{"position": [70.0, 0.0]}, {"position": [73.0, 0.0]}]}),
        json!({"owner": s(space), "kind": "arc_length", "line_position": [90.0, 7.0],
               "points": [{"position": [90.0, 0.0]}, {"position": [100.0, 0.0]}, {"position": [90.0, 10.0]}]}),
        json!({"owner": s(space), "kind": "ordinate_x", "line_position": [0.0, -20.0],
               "points": [{"position": [0.0, -30.0]}, {"position": [5.0, -25.0]}]}),
        json!({"owner": s(space), "kind": "ordinate_y", "line_position": [-20.0, 0.0],
               "points": [{"position": [-30.0, 0.0]}, {"position": [-25.0, 5.0]}]}),
    ];
    for dimension in dimensions {
        d.create("core.draft.dimension.create", dimension);
    }
    let first = d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "linear", "points": [{"position": [0.0, -50.0]}, {"position": [10.0, -50.0]}], "line_position": [0.0, -55.0]}),
    );
    let second = d.create(
        "core.draft.dimension.create",
        json!({"owner": s(space), "kind": "linear", "points": [{"position": [0.0, -60.0]}, {"position": [10.0, -60.0]}], "line_position": [0.0, -65.0]}),
    );
    d.create(
        "core.draft.chain.create",
        json!({"kind": "baseline", "members": [s(first), s(second)], "spacing": 10.0}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let placed = place(&mut f, space, [0.0, 0.0]);
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    let before = list(&f.document.snapshot(), space, Some(&links));
    assert!(before.problems.is_empty(), "{:?}", before.problems);
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    let snapshot = f.document.snapshot();
    assert_references_resolve(&snapshot, &linked);
    assert_eq!(result["entities"], json!(8 + 9), "{result}");
    assert_eq!(result["chains"], json!(1), "{result}");
    assert_eq!(result["problems"], json!([]), "{result}");
    assert_same_shapes(&before, &list(&snapshot, space, None));
}

#[test]
fn a_clip_keeps_or_drops_leaders_multileaders_hatches_and_wipeouts_by_their_anchor() {
    let mut d = linked_fixture();
    let space = d.space("Model");
    d.create(
        "core.draft.leader.create",
        json!({"owner": s(space), "points": [[5.0, 5.0], [6.0, 7.0]], "arrow": "dot"}),
    );
    d.create(
        "core.draft.leader.create",
        json!({"owner": s(space), "points": [[50.0, 50.0], [55.0, 55.0]], "arrow": "dot"}),
    );
    d.create(
        "core.draft.multileader.create",
        json!({"owner": s(space), "leaders": [[[5.0, 5.0]]], "position": [5.0, 5.0], "text": "IN"}),
    );
    d.create(
        "core.draft.multileader.create",
        json!({"owner": s(space), "leaders": [[[50.0, 50.0]]], "position": [50.0, 50.0], "text": "OUT"}),
    );
    d.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "solid", "boundary": [square(2.0, 2.0, 4.0, 4.0)]}),
    );
    d.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "solid", "boundary": [square(50.0, 50.0, 54.0, 54.0)]}),
    );
    d.create(
        "core.draft.wipeout.create",
        json!({"owner": s(space), "frame": true, "boundary": square(3.0, 3.0, 5.0, 5.0)}),
    );
    d.create(
        "core.draft.wipeout.create",
        json!({"owner": s(space), "frame": true, "boundary": square(60.0, 60.0, 62.0, 62.0)}),
    );
    let linked = d.document.snapshot();

    let (mut f, space) = host(id(LINKED));
    let clip = square(0.0, 0.0, 10.0, 10.0);
    let placed = f
        .run(
            "core.draft.external_ref.create",
            json!({"owner": s(space), "link": id(LINK).to_string(), "position": [0.0, 0.0], "clip": clip}),
        )
        .unwrap();
    let placed: ObjectId = placed["id"].as_str().unwrap().parse().unwrap();
    let links = load(&f, &opening(id(LINKED), linked.clone()));
    bind_with(&mut f, &links, WorkBudget::default());
    let result = f
        .run("core.draft.external_ref.bind", bind_json(placed))
        .unwrap();
    assert_references_resolve(&f.document.snapshot(), &linked);
    assert_eq!(result["clip_kept_whole"], json!(4), "{result}");
    assert_eq!(result["clip_dropped"], json!(4), "{result}");
    assert_eq!(result["clip_cut"], json!(0), "{result}");
}
