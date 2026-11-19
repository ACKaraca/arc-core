//! `core.draft.entity.transform` for leaders, multileaders, hatches, dimensions, tables, arrays
//! and external references: the stored data of each kind under translation, rotation, scale and
//! mirror, the associative rule (every referenced object must be selected) and the refusals.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result, Tolerance};
use arc_doc::link::{self, LinkSpec, LoadMode};
use arc_doc::{Txn, Value};
use arc_draft::array::{Params, items};
use arc_draft::display::DisplayOptions;
use arc_geom::encoding::GeomCodec;
use arc_geom::{Arc2, Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::{Value as Json, json};
use support::{Fixture, curve_json};

const TRANSFORM: &str = "core.draft.entity.transform";
const ROT: f64 = 0.4;

fn code(result: Result<Json>) -> &'static str {
    result.unwrap_err().code()
}

fn id(n: u128) -> ObjectId {
    ObjectId::from_u128(n)
}

fn field(f: &Fixture, id: ObjectId, name: &str) -> Option<Value> {
    let snapshot = f.document.snapshot();
    let handle = snapshot.resolve(id).unwrap();
    let key = snapshot.type_key(handle).unwrap();
    let index = snapshot
        .registry()
        .get(key.as_str())
        .unwrap()
        .schema
        .fields
        .iter()
        .position(|d| d.name == name)?;
    snapshot
        .get(handle)
        .unwrap()
        .values
        .get(index)
        .cloned()
        .flatten()
}

fn number(f: &Fixture, id: ObjectId, name: &str) -> f64 {
    match field(f, id, name) {
        Some(Value::F64(v)) => v,
        other => panic!("no number {name}: {other:?}"),
    }
}

fn point(f: &Fixture, id: ObjectId, name: &str) -> [f64; 2] {
    match field(f, id, name) {
        Some(Value::Point2(p)) => [p.x, p.y],
        other => panic!("no point {name}: {other:?}"),
    }
}

/// The points of a list field; a dimension's definition points are records whose first field
/// is the stored position.
fn points(f: &Fixture, id: ObjectId, name: &str) -> Vec<[f64; 2]> {
    match field(f, id, name) {
        Some(Value::List(items)) => items
            .iter()
            .map(|v| match v {
                Value::Point2(p) => [p.x, p.y],
                Value::Record(fields) => match fields.first() {
                    Some(Some(Value::Point2(p))) => [p.x, p.y],
                    other => panic!("no position in record: {other:?}"),
                },
                other => panic!("not a point: {other:?}"),
            })
            .collect(),
        other => panic!("no point list {name}: {other:?}"),
    }
}

/// The points of the first list of a list-of-lists field, such as a multileader's leaders.
fn first_list_points(f: &Fixture, id: ObjectId, name: &str) -> Vec<[f64; 2]> {
    match field(f, id, name) {
        Some(Value::List(lists)) => match lists.first() {
            Some(Value::List(items)) => items
                .iter()
                .map(|v| match v {
                    Value::Point2(p) => [p.x, p.y],
                    other => panic!("not a point: {other:?}"),
                })
                .collect(),
            other => panic!("not a list: {other:?}"),
        },
        other => panic!("no list field {name}: {other:?}"),
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn assert_point(actual: [f64; 2], expected: [f64; 2]) {
    assert!(
        near(actual[0], expected[0]) && near(actual[1], expected[1]),
        "{actual:?} != {expected:?}"
    );
}

fn transform(f: &mut Fixture, objects: &[ObjectId], transform: Json) -> Result<Json> {
    let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
    f.run(TRANSFORM, json!({"objects": ids, "transform": transform}))
}

/// The map the tests apply: mirror across the y axis (`x ↦ −x`), then scale `s`, rotation
/// `ROT`, translation `t`.
fn map(p: [f64; 2], s: f64, t: [f64; 2]) -> [f64; 2] {
    let (x, y) = (-p[0] * s, p[1] * s);
    let (sin, cos) = ROT.sin_cos();
    [cos * x - sin * y + t[0], sin * x + cos * y + t[1]]
}

/// The linear part of the test map, `R · S · M`, as rows.
fn linear(s: f64) -> [[f64; 2]; 2] {
    let (sin, cos) = ROT.sin_cos();
    [[-cos * s, -sin * s], [-sin * s, cos * s]]
}

fn mirror_rotate_scale() -> Json {
    json!({"mirror": [0.0, 1.0], "rotation": ROT, "scale": 1.5, "translation": [3.0, -1.0]})
}

fn map_mirror_scale(p: [f64; 2]) -> [f64; 2] {
    map(p, 1.5, [3.0, -1.0])
}

fn square(f: &mut Fixture, space: ObjectId, lo: f64, hi: f64) -> ObjectId {
    let vertices = [(lo, lo), (hi, lo), (hi, hi), (lo, hi)]
        .iter()
        .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
        .collect();
    let curve = Curve2::Polyline(Polyline2::new(vertices, true).unwrap());
    f.entity(space, curve_json(&curve))
}

#[test]
fn leader_points_move_under_a_rigid_motion_and_scale() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let leader = f.create(
        "core.draft.leader.create",
        json!({"owner": space.to_string(), "points": [[0.0, 0.0], [5.0, 5.0]], "arrow": "dot"}),
    );
    transform(&mut f, &[leader], mirror_rotate_scale()).unwrap();
    let got = points(&f, leader, "points");
    assert_point(got[0], map_mirror_scale([0.0, 0.0]));
    assert_point(got[1], map_mirror_scale([5.0, 5.0]));
}

#[test]
fn multileader_mirror_is_refused_only_when_it_has_content() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let text = f.create(
        "core.draft.multileader.create",
        json!({"owner": space.to_string(), "leaders": [[[0.0, 0.0]]], "position": [20.0, 5.0],
               "text": "AB"}),
    );
    assert_eq!(
        code(transform(&mut f, &[text], json!({"mirror": [0.0, 1.0]}))),
        "draft.entity.transform_unsupported"
    );
    assert_point(point(&f, text, "position"), [20.0, 5.0]);

    let bare = f.create(
        "core.draft.multileader.create",
        json!({"owner": space.to_string(), "leaders": [[[0.0, 0.0]]], "position": [20.0, 5.0],
               "content": "none"}),
    );
    transform(
        &mut f,
        &[bare],
        json!({"mirror": [0.0, 1.0], "scale": 2.0, "translation": [1.0, 0.0]}),
    )
    .unwrap();
    assert_point(point(&f, bare, "position"), [-39.0, 10.0]);
    assert_point(first_list_points(&f, bare, "leaders")[0], [1.0, 0.0]);
    // Block scale follows the uniform scale; the default block scale is 1.
    assert!(near(number(&f, bare, "block_scale"), 2.0));
}

#[test]
fn solid_hatch_loops_equal_a_direct_rebuild_of_the_moved_boundary() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let outline = square(&mut f, space, 0.0, 10.0);
    let hatch = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "solid", "objects": [outline.to_string()]}),
    );
    // Moves the boundary and the hatch together: the hatch's stored loops must match a hatch
    // built on the moved square.
    let direct = square(&mut f, space, 2.0, 22.0);
    let moved = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "solid", "objects": [direct.to_string()]}),
    );
    transform(
        &mut f,
        &[outline, hatch],
        json!({"scale": 2.0, "translation": [2.0, 2.0]}),
    )
    .unwrap();
    assert_eq!(
        format!("{:?}", field(&f, hatch, "loops")),
        format!("{:?}", field(&f, moved, "loops"))
    );
}

#[test]
fn hatch_with_an_unselected_boundary_is_refused_as_associated() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let outline = square(&mut f, space, 0.0, 10.0);
    let hatch = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "solid", "objects": [outline.to_string()]}),
    );
    let before = format!("{:?}", field(&f, hatch, "loops"));
    assert_eq!(
        code(transform(
            &mut f,
            &[hatch],
            json!({"translation": [1.0, 0.0]})
        )),
        "draft.entity.transform_unsupported"
    );
    assert_eq!(before, format!("{:?}", field(&f, hatch, "loops")));
}

#[test]
fn pattern_hatch_turns_and_scales_its_pattern_and_refuses_mirror() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let pattern = f
        .run(
            "core.draft.hatch_pattern.load",
            json!({"names": ["brick-bond"]}),
        )
        .unwrap();
    let pattern: ObjectId = pattern["ids"][0].as_str().unwrap().parse().unwrap();
    let boundary = square(&mut f, space, 0.0, 10.0);
    let hatch = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "pattern", "objects": [boundary.to_string()],
               "pattern": pattern.to_string(), "scale": 1.0}),
    );
    assert_eq!(
        code(transform(
            &mut f,
            &[boundary, hatch],
            json!({"mirror": [1.0, 0.0]})
        )),
        "draft.entity.transform_unsupported"
    );
    transform(
        &mut f,
        &[boundary, hatch],
        json!({"rotation": 0.5, "scale": 2.0}),
    )
    .unwrap();
    assert!(near(number(&f, hatch, "angle"), 0.5));
    assert!(near(number(&f, hatch, "scale"), 2.0));
}

fn dimension_on(f: &mut Fixture, space: ObjectId, line: ObjectId) -> ObjectId {
    f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"object": line.to_string(), "key": "start"},
                          {"object": line.to_string(), "key": "end"}],
               "line_position": [0.0, -5.0]}),
    )
}

#[test]
fn free_dimension_moves_its_points_and_dimension_line() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"position": [0.0, 0.0]}, {"position": [4.0, 0.0]}],
               "line_position": [0.0, 3.0]}),
    );
    transform(&mut f, &[dimension], json!({"translation": [1.0, 2.0]})).unwrap();
    let got = points(&f, dimension, "points");
    assert_point(got[0], [1.0, 2.0]);
    assert_point(got[1], [5.0, 2.0]);
    assert_point(point(&f, dimension, "line_position"), [1.0, 5.0]);
    assert_eq!(
        code(transform(
            &mut f,
            &[dimension],
            json!({"mirror": [0.0, 1.0]})
        )),
        "draft.entity.transform_unsupported"
    );
}

#[test]
fn associated_dimension_follows_when_its_object_is_selected() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)).unwrap(),
        )),
    );
    let dimension = dimension_on(&mut f, space, line);
    let before = {
        let snapshot = f.document.snapshot();
        arc_draft::dimension::resolve(&snapshot, dimension, &DisplayOptions::default())
            .unwrap()
            .points
    };
    transform(
        &mut f,
        &[line, dimension],
        json!({"rotation": std::f64::consts::FRAC_PI_2, "translation": [1.0, 1.0]}),
    )
    .unwrap();
    let snapshot = f.document.snapshot();
    let after =
        arc_draft::dimension::resolve(&snapshot, dimension, &DisplayOptions::default()).unwrap();
    for (a, b) in after.points.iter().zip(&before) {
        // A quarter turn about the origin, then the translation.
        assert_point([a.x, a.y], [-b.y + 1.0, b.x + 1.0]);
    }
}

#[test]
fn associated_dimension_with_an_unselected_object_is_refused() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(10.0, 0.0)).unwrap(),
        )),
    );
    let dimension = dimension_on(&mut f, space, line);
    assert_eq!(
        code(transform(
            &mut f,
            &[dimension],
            json!({"translation": [1.0, 0.0]})
        )),
        "draft.entity.transform_unsupported"
    );
    assert_point(point(&f, dimension, "line_position"), [0.0, -5.0]);
}

#[test]
fn table_moves_and_scales_its_geometry_and_refuses_mirror() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let table = f.create(
        "core.draft.table.create",
        json!({"owner": space.to_string(), "columns": [10.0, 20.0],
               "rows": [{"kind": "data", "height": 5.0}],
               "cells": [{"cell": "A1", "content": {"kind": "text", "text": "T"}}]}),
    );
    assert_eq!(
        code(transform(&mut f, &[table], json!({"mirror": [1.0, 0.0]}))),
        "draft.entity.transform_unsupported"
    );
    transform(
        &mut f,
        &[table],
        json!({"scale": 2.0, "translation": [1.0, 0.0]}),
    )
    .unwrap();
    assert_point(point(&f, table, "position"), [1.0, 0.0]);
    let Some(Value::List(widths)) = field(&f, table, "column_widths") else {
        panic!("no widths")
    };
    let widths: Vec<f64> = widths
        .iter()
        .map(|w| match w {
            Value::F64(v) => *v,
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(near(widths[0], 20.0) && near(widths[1], 40.0), "{widths:?}");
}

/// The block frame `R(θ) diag(sx, sy)` of an array, as columns.
fn frame(rotation: f64, sx: f64, sy: f64) -> [[f64; 2]; 2] {
    let (sin, cos) = rotation.sin_cos();
    [[cos * sx, -sin * sy], [sin * sx, cos * sy]]
}

fn product(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    [
        [
            a[0][0] * b[0][0] + a[0][1] * b[1][0],
            a[0][0] * b[0][1] + a[0][1] * b[1][1],
        ],
        [
            a[1][0] * b[0][0] + a[1][1] * b[1][0],
            a[1][0] * b[0][1] + a[1][1] * b[1][1],
        ],
    ]
}

fn assert_matrix(actual: [[f64; 2]; 2], expected: [[f64; 2]; 2]) {
    for r in 0..2 {
        for c in 0..2 {
            assert!(
                near(actual[r][c], expected[r][c]),
                "{actual:?} != {expected:?}"
            );
        }
    }
}

/// The stored parameters and frame of an array.
fn array_params(f: &Fixture, array: ObjectId) -> (Params, f64, f64) {
    let snapshot = f.document.snapshot();
    let handle = snapshot.resolve(array).unwrap();
    let values = snapshot.get(handle).unwrap().values.clone();
    let params = Params::from_values(&values);
    let scale = match field(f, array, "scale") {
        Some(Value::Vector3(v)) => (v.x, v.y),
        other => panic!("{other:?}"),
    };
    (params, scale.0, scale.1)
}

/// The items of an array must be the mapped items of the array before the transform, both in
/// position and in the block frame each item carries.
fn check_items_map(f: &Fixture, array: ObjectId, before: &[(Point2, f64)], curve: Option<&Curve2>) {
    let (params, sx, sy) = array_params(f, array);
    let tol = Tolerance::DEFAULT;
    let after = items(&params, curve, &tol).unwrap();
    assert_eq!(after.len(), before.len());
    let linear = linear(1.5);
    for (item, (old, old_rotation)) in after.iter().zip(before) {
        assert_point(
            [item.position.x, item.position.y],
            map_mirror_scale([old.x, old.y]),
        );
        // Each item's frame is the mapped frame of the old item: L · R(old) · diag(1, 1).
        let expected = product(linear, frame(*old_rotation, 1.0, 1.0));
        let actual = frame(item.rotation, sx, sy);
        assert_matrix(actual, expected);
    }
}

fn before_items(f: &Fixture, array: ObjectId, curve: Option<&Curve2>) -> Vec<(Point2, f64)> {
    let (params, _, _) = array_params(f, array);
    items(&params, curve, &Tolerance::DEFAULT)
        .unwrap()
        .into_iter()
        .map(|item| (item.position, item.rotation))
        .collect()
}

fn block_definition(f: &mut Fixture) -> ObjectId {
    f.create(
        "core.draft.block.define",
        json!({"name": "unit", "objects": []}),
    )
}

#[test]
fn rectangular_array_moves_as_a_frame_and_its_grid_follows_a_mirror() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let block = block_definition(&mut f);
    let array = f.create(
        "core.draft.array.create",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [1.0, 2.0],
               "array": {"kind": "rectangular", "rows": 2, "columns": 3,
                         "row_spacing": 4.0, "column_spacing": 5.0, "angle": 0.3}}),
    );
    let before = before_items(&f, array, None);
    transform(&mut f, &[array], mirror_rotate_scale()).unwrap();
    check_items_map(&f, array, &before, None);
}

#[test]
fn polar_array_turns_its_total_angle_under_a_mirror() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let block = block_definition(&mut f);
    let array = f.create(
        "core.draft.array.create",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [5.0, 0.0],
               "array": {"kind": "polar", "center": [0.0, 0.0], "count": 4,
                         "total_angle": 2.0}}),
    );
    let before = before_items(&f, array, None);
    transform(&mut f, &[array], mirror_rotate_scale()).unwrap();
    check_items_map(&f, array, &before, None);
}

#[test]
fn path_array_needs_its_path_selected_and_then_follows_the_mapped_path() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let block = block_definition(&mut f);
    let path = f.entity(
        space,
        curve_json(&Curve2::Arc(
            Arc2::new(Point2::new(0.0, 0.0), 6.0, 0.0, 1.5).unwrap(),
        )),
    );
    let array = f.create(
        "core.draft.array.create",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [6.0, 0.0],
               "array": {"kind": "path", "path": path.to_string(), "count": 4}}),
    );
    assert_eq!(
        code(transform(
            &mut f,
            &[array],
            json!({"translation": [1.0, 0.0]})
        )),
        "draft.entity.transform_unsupported"
    );
    let curve = match field(&f, path, "geometry") {
        Some(Value::Geometry(bytes)) => Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap(),
        other => panic!("{other:?}"),
    };
    let before = before_items(&f, array, Some(&curve));
    transform(&mut f, &[array, path], mirror_rotate_scale()).unwrap();
    let curve = match field(&f, path, "geometry") {
        Some(Value::Geometry(bytes)) => Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap(),
        other => panic!("{other:?}"),
    };
    check_items_map(&f, array, &before, Some(&curve));
}

#[test]
fn external_reference_moves_like_a_block_reference() {
    let mut f = Fixture::drafting_with_links();
    let space = f.space("Host");
    let link_id = id(10);
    let spec = LinkSpec {
        document: id(2),
        path: "linked.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    f.document
        .submit(move |txn: &mut Txn| link::create(txn, link_id, &spec))
        .wait()
        .unwrap();
    let reference = f.create(
        "core.draft.external_ref.create",
        json!({"owner": space.to_string(), "link": link_id.to_string(), "position": [1.0, 2.0]}),
    );
    transform(
        &mut f,
        &[reference],
        json!({"rotation": 0.5, "translation": [3.0, 0.0]}),
    )
    .unwrap();
    assert_point(point(&f, reference, "position"), {
        let (sin, cos) = 0.5f64.sin_cos();
        [cos * 1.0 - sin * 2.0 + 3.0, sin * 1.0 + cos * 2.0]
    });
    assert!(near(number(&f, reference, "rotation"), 0.5));
}
