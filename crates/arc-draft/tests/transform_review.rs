//! Review fixes of the transform: quadrant and parameter keys on circles and arcs, ordinate
//! dimensions under rotation, nested block paths of dimension points, the stored curve of a path
//! array, overflow inside records, and hatches whose boundary is partly gone.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result, Tolerance};
use arc_doc::Value;
use arc_draft::array::{Params, items};
use arc_draft::display::DisplayOptions;
use arc_geom::encoding::GeomCodec;
use arc_geom::{Arc2, Circle2, Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::{Value as Json, json};
use support::{Fixture, curve_json};

const TRANSFORM: &str = "core.draft.entity.transform";
const QUARTER: f64 = std::f64::consts::FRAC_PI_2;

fn code(result: Result<Json>) -> &'static str {
    result.unwrap_err().code()
}

fn transform(f: &mut Fixture, objects: &[ObjectId], transform: Json) -> Result<Json> {
    let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
    f.run(TRANSFORM, json!({"objects": ids, "transform": transform}))
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

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn assert_point(actual: [f64; 2], expected: [f64; 2]) {
    assert!(
        near(actual[0], expected[0]) && near(actual[1], expected[1]),
        "{actual:?} != {expected:?}"
    );
}

/// Rotation by `a` about the origin.
fn rotate(p: [f64; 2], a: f64) -> [f64; 2] {
    let (sin, cos) = a.sin_cos();
    [cos * p[0] - sin * p[1], sin * p[0] + cos * p[1]]
}

fn resolved(f: &Fixture, dimension: ObjectId) -> Vec<Point2> {
    let snapshot = f.document.snapshot();
    arc_draft::dimension::resolve(&snapshot, dimension, &DisplayOptions::default())
        .unwrap()
        .points
}

fn circle(f: &mut Fixture, space: ObjectId, center: [f64; 2], radius: f64) -> ObjectId {
    f.entity(
        space,
        curve_json(&Curve2::Circle(
            Circle2::new(Point2::new(center[0], center[1]), radius).unwrap(),
        )),
    )
}

#[test]
fn radial_dimension_on_a_circle_quadrant_follows_a_quarter_turn_and_refuses_thirty_degrees() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let c = circle(&mut f, space, [10.0, 0.0], 2.0);
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "radial",
               "points": [{"object": c.to_string(), "key": "quadrant", "quadrant": 0},
                          {"position": [30.0, 0.0]}],
               "line_position": [60.0, 0.0]}),
    );
    let before = resolved(&f, dimension)[0];
    assert_point([before.x, before.y], [12.0, 0.0]);

    // A quarter turn lands on another quadrant: the key advances and the point follows.
    transform(&mut f, &[c, dimension], json!({"rotation": QUARTER})).unwrap();
    let after = resolved(&f, dimension)[0];
    assert_point([after.x, after.y], rotate([before.x, before.y], QUARTER));

    // Thirty degrees does not land on a quadrant: the entity is refused and nothing changes.
    let refused = transform(
        &mut f,
        &[c, dimension],
        json!({"rotation": std::f64::consts::PI / 6.0}),
    );
    assert_eq!(code(refused), "draft.entity.transform_unsupported");
    let still = resolved(&f, dimension)[0];
    assert_point([still.x, still.y], [after.x, after.y]);
}

#[test]
fn radial_dimension_on_a_circle_parameter_follows_any_rotation() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let c = circle(&mut f, space, [10.0, 0.0], 2.0);
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "radial",
               "points": [{"object": c.to_string(), "key": "parameter", "parameter": 0.5},
                          {"position": [30.0, 0.0]}],
               "line_position": [60.0, 0.0]}),
    );
    let before = resolved(&f, dimension)[0];
    let angle = std::f64::consts::PI / 6.0;
    transform(&mut f, &[c, dimension], json!({"rotation": angle})).unwrap();
    let after = resolved(&f, dimension)[0];
    let expected = rotate([before.x, before.y], angle);
    assert_point([after.x, after.y], expected);
}

#[test]
fn radial_dimension_on_an_arc_parameter_follows_a_rotation_within_the_arc() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let arc = f.entity(
        space,
        curve_json(&Curve2::Arc(
            Arc2::new(Point2::new(0.0, 0.0), 5.0, 0.0, 1.5).unwrap(),
        )),
    );
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "radial",
               "points": [{"object": arc.to_string(), "key": "parameter", "parameter": 0.5},
                          {"position": [30.0, 0.0]}],
               "line_position": [60.0, 0.0]}),
    );
    let before = resolved(&f, dimension)[0];
    transform(&mut f, &[arc, dimension], json!({"rotation": 0.3})).unwrap();
    let after = resolved(&f, dimension)[0];
    assert_point([after.x, after.y], rotate([before.x, before.y], 0.3));
}

#[test]
fn ordinate_dimension_keeps_translation_and_scale_and_refuses_rotation() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "ordinate_x",
               "points": [{"position": [1.0, 2.0]}, {"position": [5.0, 9.0]}],
               "line_position": [1.0, 20.0]}),
    );
    transform(&mut f, &[dimension], json!({"translation": [3.0, 0.0]})).unwrap();
    transform(&mut f, &[dimension], json!({"scale": 2.0})).unwrap();
    assert_eq!(
        code(transform(&mut f, &[dimension], json!({"rotation": 0.5}))),
        "draft.entity.transform_unsupported"
    );
}

#[test]
fn a_dimension_on_an_entity_inside_a_block_follows_the_block_reference_only() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line = f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)).unwrap(),
        )),
    );
    let definition = f.create(
        "core.draft.block.define",
        json!({"name": "unit", "objects": [line.to_string()]}),
    );
    let block_ref = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": definition.to_string(),
               "position": [0.0, 0.0]}),
    );
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"object": block_ref.to_string(), "path": [line.to_string()],
                           "key": "start"},
                          {"position": [20.0, 0.0]}],
               "line_position": [0.0, 5.0]}),
    );
    // Only the referenced block reference is selected: the path entry is not required.
    transform(
        &mut f,
        &[block_ref, dimension],
        json!({"translation": [5.0, 0.0]}),
    )
    .unwrap();
    assert_point(
        [resolved(&f, dimension)[0].x, resolved(&f, dimension)[0].y],
        [5.0, 0.0],
    );
    // Selecting the entry inside the shared definition would move the point twice: refused.
    assert_eq!(
        code(transform(
            &mut f,
            &[block_ref, line, dimension],
            json!({"translation": [1.0, 0.0]}),
        )),
        "draft.entity.transform_unsupported"
    );
}

#[test]
fn a_path_array_with_a_stored_curve_moves_its_items_with_the_curve() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "unit", "objects": []}),
    );
    let curve = Curve2::Arc(Arc2::new(Point2::new(0.0, 0.0), 6.0, 0.0, 1.5).unwrap());
    let array = f.create(
        "core.draft.array.create",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [6.0, 0.0],
               "array": {"kind": "path", "curve": curve_json(&curve), "count": 4}}),
    );
    let before: Vec<Point2> = {
        let params = params_of(&f, array);
        items(&params, Some(&curve), &Tolerance::DEFAULT)
            .unwrap()
            .into_iter()
            .map(|i| i.position)
            .collect()
    };
    transform(
        &mut f,
        &[array],
        json!({"rotation": 0.4, "scale": 1.5, "translation": [2.0, -1.0]}),
    )
    .unwrap();
    let Some(Value::Geometry(bytes)) = field(&f, array, "curve") else {
        panic!("no stored curve")
    };
    let moved = Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap();
    let after = items(&params_of(&f, array), Some(&moved), &Tolerance::DEFAULT).unwrap();
    assert_eq!(after.len(), before.len());
    for (item, old) in after.iter().zip(&before) {
        let r = rotate([old.x * 1.5, old.y * 1.5], 0.4);
        assert_point([item.position.x, item.position.y], [r[0] + 2.0, r[1] - 1.0]);
    }
}

fn params_of(f: &Fixture, id: ObjectId) -> Params {
    let snapshot = f.document.snapshot();
    let handle = snapshot.resolve(id).unwrap();
    Params::from_values(&snapshot.get(handle).unwrap().values)
}

#[test]
fn a_scale_that_overflows_a_table_row_or_a_dimension_point_is_refused() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let table = f.create(
        "core.draft.table.create",
        json!({"owner": space.to_string(), "columns": [10.0],
               "rows": [{"kind": "data", "height": 1e308}],
               "cells": [{"cell": "A1", "content": {"kind": "text", "text": "T"}}]}),
    );
    assert_eq!(
        code(transform(&mut f, &[table], json!({"scale": 10.0}))),
        "draft.field.invalid"
    );
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"position": [1e308, 0.0]}, {"position": [0.0, 0.0]}],
               "line_position": [0.0, 3.0]}),
    );
    assert_eq!(
        code(transform(&mut f, &[dimension], json!({"scale": 10.0}))),
        "draft.field.invalid"
    );
}

#[test]
fn a_hatch_whose_boundary_is_partly_gone_is_not_associated() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let square = |f: &mut Fixture, lo: f64| {
        let vertices = [
            (lo, lo),
            (lo + 5.0, lo),
            (lo + 5.0, lo + 5.0),
            (lo, lo + 5.0),
        ]
        .iter()
        .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
        .collect();
        let curve = Curve2::Polyline(Polyline2::new(vertices, true).unwrap());
        f.entity(space, curve_json(&curve))
    };
    let first = square(&mut f, 0.0);
    let second = square(&mut f, 20.0);
    let hatch = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "solid",
               "objects": [first.to_string(), second.to_string()]}),
    );
    f.run(
        "core.draft.entity.delete",
        json!({"objects": [second.to_string()]}),
    )
    .unwrap();
    // The hatch keeps its stored loops; it is not associated with the first square any more.
    transform(&mut f, &[hatch], json!({"translation": [1.0, 0.0]})).unwrap();
}
