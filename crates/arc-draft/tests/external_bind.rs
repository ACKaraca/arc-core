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
use arc_doc::{Snapshot, Txn, Value};
use arc_draft::display::{DisplayList, DisplayOptions, Lists, space_list_with_links};
use arc_draft::external::WorkBudget;
use arc_draft::external_commands::bind_command_with_budget;
use arc_geom::{Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::{Value as Json, json};
use support::{Fixture, curve_json};

const HOST: u128 = 1;
const LINKED: u128 = 2;
const LINK: u128 = 10;

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn s(object: ObjectId) -> String {
    object.to_string()
}

fn segment(a: (f64, f64), b: (f64, f64)) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)).unwrap())
}

fn host(document: ObjectId) -> (Fixture, ObjectId) {
    let mut f = Fixture::drafting_with_links();
    let space = f.space("Host");
    let spec = LinkSpec {
        document,
        path: "linked.arc",
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

/// Every reference of every host object names an object of the host snapshot.
fn assert_references_resolve(snapshot: &Snapshot) {
    fn walk(value: &Value, out: &mut Vec<ObjectId>) {
        match value {
            Value::Reference(id) => out.push(*id),
            Value::List(items) => items.iter().for_each(|v| walk(v, out)),
            Value::Record(fields) => fields.iter().flatten().for_each(|v| walk(v, out)),
            Value::Map(entries) => entries.values().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    for (_, object) in snapshot.iter() {
        let mut found = Vec::new();
        object
            .values
            .iter()
            .flatten()
            .for_each(|v| walk(v, &mut found));
        for target in found {
            assert!(
                snapshot.resolve(target).is_ok(),
                "{} refers to {target} which is not in the host",
                object.id
            );
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
    let mut d = Fixture::drafting();
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
    assert_references_resolve(&snapshot);

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
    let mut d = Fixture::drafting();
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
    assert_references_resolve(&snapshot);

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
    let mut d = Fixture::drafting();
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
    assert_references_resolve(&snapshot);
    assert_eq!(result["problems"], json!([]), "{result}");

    let block: ObjectId = result["block"].as_str().unwrap().parse().unwrap();
    assert_eq!(dimension_target_owner(&snapshot, block), Some(block));
}

#[test]
fn associative_references_are_cleared_when_their_target_is_cut_away() {
    let mut d = Fixture::drafting();
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
    assert_references_resolve(&snapshot);

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
    let mut d = Fixture::drafting();
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
    assert_references_resolve(&f.document.snapshot());
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
