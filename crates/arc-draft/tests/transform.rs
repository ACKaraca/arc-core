//! `core.draft.entity.transform`: translation of every supported kind, rotation and mirror, scale
//! of text and block references, refusals with the stable code, and undo restoring every field.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId};
use arc_doc::Value;
use arc_draft::display::{DisplayOptions, Lists, space_list};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Arc2, Circle2, Curve2, Polyline2, PolylineVertex};
use serde_json::{Value as Json, json};
use support::Fixture;
use support::curve_json;

const TRANSFORM: &str = "core.draft.entity.transform";

fn code(result: arc_base::Result<Json>) -> &'static str {
    result.unwrap_err().code()
}

/// The stored value of field `name` of `id`.
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

fn curve(f: &Fixture, id: ObjectId) -> Curve2 {
    match field(f, id, "geometry") {
        Some(Value::Geometry(bytes)) => Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap(),
        other => panic!("no geometry: {other:?}"),
    }
}

fn point(f: &Fixture, id: ObjectId, name: &str) -> [f64; 2] {
    match field(f, id, name) {
        Some(Value::Point2(p)) => [p.x, p.y],
        other => panic!("no point {name}: {other:?}"),
    }
}

fn number(f: &Fixture, id: ObjectId, name: &str) -> f64 {
    match field(f, id, name) {
        Some(Value::F64(v)) => v,
        other => panic!("no number {name}: {other:?}"),
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

fn transform(f: &mut Fixture, objects: &[ObjectId], transform: Json) -> arc_base::Result<Json> {
    let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
    f.run(TRANSFORM, json!({"objects": ids, "transform": transform}))
}

/// A polyline of `(x, y, bulge)` vertices.
fn polyline(points: &[(f64, f64, f64)], closed: bool) -> Polyline2 {
    let vertices = points
        .iter()
        .map(|&(x, y, bulge)| PolylineVertex::new(Point2::new(x, y), bulge))
        .collect();
    Polyline2::new(vertices, closed).unwrap()
}

fn line(f: &mut Fixture, space: ObjectId, x0: f64, y0: f64, x1: f64, y1: f64) -> ObjectId {
    let geometry = json!({"kind": "segment", "start": [x0, y0], "end": [x1, y1]});
    f.entity(space, geometry)
}

/// A drawing with one entity of every supported kind, each at the origin region.
struct Drawing {
    point: ObjectId,
    segment: ObjectId,
    circle: ObjectId,
    arc: ObjectId,
    polyline: ObjectId,
    spline: ObjectId,
    wipeout: ObjectId,
    text: ObjectId,
    block: ObjectId,
}

fn drawing(f: &mut Fixture) -> Drawing {
    let space = f.space("model");
    let point = f.entity(space, json!({"kind": "point", "position": [1.0, 2.0]}));
    let segment = line(f, space, 0.0, 0.0, 4.0, 0.0);
    let circle = f.entity(
        space,
        curve_json(&Curve2::Circle(
            Circle2::new(Point2::new(10.0, 0.0), 2.0).unwrap(),
        )),
    );
    let arc = f.entity(
        space,
        curve_json(&Curve2::Arc(
            Arc2::new(Point2::new(0.0, 10.0), 3.0, 0.0, 1.0).unwrap(),
        )),
    );
    let open_line = f.entity(
        space,
        curve_json(&Curve2::Polyline(polyline(
            &[(0.0, 20.0, 0.5), (5.0, 20.0, 0.0)],
            false,
        ))),
    );
    let spline = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "fit_points": [[20.0, 0.0], [22.0, 3.0], [25.0, 1.0], [27.0, 2.0]]}),
    );
    let wipeout = f.create(
        "core.draft.wipeout.create",
        json!({"owner": space.to_string(), "boundary": curve_json(&Curve2::Polyline(
            polyline(&[(30.0, 0.0, 0.0), (32.0, 0.0, 0.0), (32.0, 2.0, 0.0)], true)))}),
    );
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "A", "position": [40.0, 0.0],
               "rotation": 0.25, "height": 2.0}),
    );
    let definition = f.create(
        "core.draft.block.define",
        json!({"name": "door", "objects": []}),
    );
    let block = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": definition.to_string(),
               "position": [50.0, 0.0], "scale": [2.0, 1.0, 1.0], "rotation": 0.0}),
    );
    Drawing {
        point,
        segment,
        circle,
        arc,
        polyline: open_line,
        spline,
        wipeout,
        text,
        block,
    }
}

#[test]
fn translation_moves_every_supported_kind() {
    let mut f = Fixture::drafting();
    let d = drawing(&mut f);
    let offset = json!({"translation": [1.0, -2.0]});
    let all = [
        d.point, d.segment, d.circle, d.arc, d.polyline, d.spline, d.wipeout, d.text, d.block,
    ];
    transform(&mut f, &all, offset).unwrap();

    assert_point(point(&f, d.point, "position"), [2.0, 0.0]);
    let Curve2::Segment(s) = curve(&f, d.segment) else {
        panic!("segment kind changed")
    };
    assert_point([s.start().x, s.start().y], [1.0, -2.0]);
    assert_point([s.end().x, s.end().y], [5.0, -2.0]);
    assert!(
        matches!(curve(&f, d.circle), Curve2::Circle(c) if near(c.center().x, 11.0) && near(c.center().y, -2.0))
    );
    assert!(
        matches!(curve(&f, d.arc), Curve2::Arc(a) if near(a.center().x, 1.0) && near(a.center().y, 8.0))
    );
    assert!(matches!(curve(&f, d.polyline), Curve2::Polyline(_)));
    assert!(matches!(curve(&f, d.wipeout), Curve2::Polyline(_)));
    assert_point(point(&f, d.text, "position"), [41.0, -2.0]);
    assert_point(point(&f, d.block, "position"), [51.0, -2.0]);
    let fit = match field(&f, d.spline, "fit_points") {
        Some(Value::List(items)) => items,
        other => panic!("{other:?}"),
    };
    let Some(Value::Point2(first)) = fit.first() else {
        panic!("no fit point")
    };
    assert_point([first.x, first.y], [21.0, -2.0]);
}

#[test]
fn rotation_turns_a_line_text_and_block_reference() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let segment = line(&mut f, space, 1.0, 0.0, 2.0, 0.0);
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "B", "position": [0.0, 1.0], "rotation": 0.25}),
    );
    let definition = f.create(
        "core.draft.block.define",
        json!({"name": "b", "objects": []}),
    );
    let block = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": definition.to_string(), "position": [3.0, 0.0],
               "scale": [1.0, 1.0, 1.0], "rotation": 0.5}),
    );
    transform(
        &mut f,
        &[segment, text, block],
        json!({"rotation": std::f64::consts::FRAC_PI_2}),
    )
    .unwrap();

    let Curve2::Segment(s) = curve(&f, segment) else {
        panic!()
    };
    assert_point([s.start().x, s.start().y], [0.0, 1.0]);
    assert_point([s.end().x, s.end().y], [0.0, 2.0]);
    assert_point(point(&f, text, "position"), [-1.0, 0.0]);
    assert!(near(
        number(&f, text, "rotation"),
        0.25 + std::f64::consts::FRAC_PI_2
    ));
    assert_point(point(&f, block, "position"), [0.0, 3.0]);
    assert!(near(
        number(&f, block, "rotation"),
        0.5 + std::f64::consts::FRAC_PI_2
    ));
}

#[test]
fn mirror_reflects_an_arc_and_a_block_reference_with_uniform_scale() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    // Mirror across the y axis: x ↦ −x.
    let arc = f.entity(
        space,
        curve_json(&Curve2::Arc(
            Arc2::new(Point2::new(2.0, 0.0), 1.0, 0.0, 1.0).unwrap(),
        )),
    );
    let before = match curve(&f, arc) {
        Curve2::Arc(a) => a.bounding_box(),
        other => panic!("{other:?}"),
    };
    transform(&mut f, &[arc], json!({"mirror": [0.0, 1.0]})).unwrap();
    let Curve2::Arc(a) = curve(&f, arc) else {
        panic!("arc kind changed")
    };
    assert!(near(a.center().x, -2.0) && near(a.center().y, 0.0));
    let after = a.bounding_box();
    assert!(near(after.min().x, -before.max().x) && near(after.max().x, -before.min().x));

    let definition = f.create(
        "core.draft.block.define",
        json!({"name": "m", "objects": []}),
    );
    let block = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": definition.to_string(), "position": [5.0, 1.0],
               "scale": [2.0, 2.0, 1.0], "rotation": 0.0}),
    );
    transform(&mut f, &[block], json!({"mirror": [0.0, 1.0]})).unwrap();
    assert_point(point(&f, block, "position"), [-5.0, 1.0]);
    // The mirror of an axis-aligned uniform scale is a negative x scale, which keeps the rotation.
    let scale = match field(&f, block, "scale") {
        Some(Value::Vector3(v)) => v,
        other => panic!("{other:?}"),
    };
    assert!(near(scale.x.abs(), 2.0) && near(scale.y.abs(), 2.0));
    assert!(
        near(scale.x * scale.y, -4.0),
        "mirror flips the orientation"
    );
}

#[test]
fn scale_and_rotation_compose_about_the_origin() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "C", "position": [1.0, 0.0], "height": 2.0}),
    );
    transform(
        &mut f,
        &[text],
        json!({"scale": 3.0, "translation": [0.0, 1.0]}),
    )
    .unwrap();
    assert_point(point(&f, text, "position"), [3.0, 1.0]);
    assert!(near(number(&f, text, "height"), 6.0));
}

#[test]
fn refused_kinds_and_mirrored_text_name_the_kind_and_change_nothing() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let segment = line(&mut f, space, 0.0, 0.0, 1.0, 0.0);
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "D", "position": [0.0, 0.0]}),
    );
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"position": [0.0, 0.0]}, {"position": [4.0, 0.0]}],
               "line_position": [0.0, 3.0]}),
    );
    let refused = transform(
        &mut f,
        &[segment, dimension],
        json!({"translation": [1.0, 0.0]}),
    );
    assert_eq!(code(refused), "draft.entity.transform_unsupported");
    // The segment moved nowhere: the command refuses the whole list.
    let Curve2::Segment(s) = curve(&f, segment) else {
        panic!()
    };
    assert_point([s.start().x, s.start().y], [0.0, 0.0]);

    assert_eq!(
        code(transform(&mut f, &[text], json!({"mirror": [1.0, 0.0]}))),
        "draft.entity.transform_unsupported"
    );
    assert_point(point(&f, text, "position"), [0.0, 0.0]);
}

#[test]
fn invalid_identifiers_and_parameters_are_refused() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let segment = line(&mut f, space, 0.0, 0.0, 1.0, 0.0);
    assert!(transform(&mut f, &[ObjectId::from_u128(99)], json!({})).is_err());
    assert!(
        transform(&mut f, &[space], json!({})).is_err(),
        "a space is not an entity"
    );
    assert_eq!(
        code(transform(&mut f, &[segment], json!({"scale": 0.0}))),
        "draft.field.invalid"
    );
    assert_eq!(
        code(transform(&mut f, &[segment], json!({"mirror": [0.0, 0.0]}))),
        "draft.field.invalid"
    );
    assert!(transform(&mut f, &[segment], json!({"unknown": 1})).is_err());
}

#[test]
fn undo_restores_every_field_of_every_moved_entity() {
    let mut f = Fixture::drafting();
    let d = drawing(&mut f);
    let all = [
        d.point, d.segment, d.circle, d.arc, d.polyline, d.spline, d.wipeout, d.text, d.block,
    ];
    let snapshot_of = |f: &Fixture| -> Vec<Vec<Option<Value>>> {
        all.iter()
            .map(|id| {
                let snapshot = f.document.snapshot();
                snapshot
                    .get(snapshot.resolve(*id).unwrap())
                    .unwrap()
                    .values
                    .clone()
            })
            .collect()
    };
    let before = snapshot_of(&f);
    transform(
        &mut f,
        &all,
        json!({"scale": 2.0, "rotation": 0.3, "translation": [4.0, 5.0]}),
    )
    .unwrap();
    assert_ne!(snapshot_of(&f), before);
    assert!(f.document.undo().wait().unwrap().value);
    assert_eq!(snapshot_of(&f), before);
}

#[test]
fn a_block_with_non_uniform_scale_takes_an_oblique_mirror() {
    // Mirror across the line y = x of a block scaled (2, 1): the linear part is [[0, 1], [2, 0]],
    // which is the rotation by a quarter turn times diag(2, −1).
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let definition = f.create(
        "core.draft.block.define",
        json!({"name": "o", "objects": []}),
    );
    let block = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": definition.to_string(), "position": [5.0, 1.0],
               "scale": [2.0, 1.0, 1.0], "rotation": 0.0}),
    );
    transform(&mut f, &[block], json!({"mirror": [1.0, 1.0]})).unwrap();
    assert_point(point(&f, block, "position"), [1.0, 5.0]);
    assert!(near(
        number(&f, block, "rotation"),
        std::f64::consts::FRAC_PI_2
    ));
    let scale = match field(&f, block, "scale") {
        Some(Value::Vector3(v)) => v,
        other => panic!("{other:?}"),
    };
    assert!(near(scale.x, 2.0) && near(scale.y, -1.0));
}

#[test]
fn the_object_count_bound_is_checked_before_any_entity_is_read() {
    let mut f = Fixture::drafting();
    let objects: Vec<String> = (0..10_001u128)
        .map(|i| ObjectId::from_u128(i + 1).to_string())
        .collect();
    assert_eq!(
        code(f.run(TRANSFORM, json!({"objects": objects, "transform": {}}))),
        "cmd.params.invalid"
    );
}

#[test]
fn a_result_that_overflows_is_refused_and_nothing_changes() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let point = f.entity(space, json!({"kind": "point", "position": [1.0e10, 0.0]}));
    assert_eq!(
        code(transform(&mut f, &[point], json!({"scale": 1.0e300}))),
        "draft.field.invalid"
    );
    assert_point(point_of(&f, point), [1.0e10, 0.0]);
}

#[test]
fn a_spline_without_fit_points_stays_without_them() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let spline = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "control_points": [[0.0, 0.0], [1.0, 2.0], [3.0, 2.0], [4.0, 0.0]]}),
    );
    let before = field(&f, spline, "fit_points");
    transform(&mut f, &[spline], json!({"translation": [1.0, 1.0]})).unwrap();
    assert_eq!(
        field(&f, spline, "fit_points"),
        before,
        "no fit points were created or moved"
    );
}

/// Dependents that are not selected: a dimension on a transformed line and a hatch on a
/// transformed closed polyline. After the transform the graph-driven display list equals a direct
/// rebuild, so nothing is stale.
#[test]
fn unselected_dependents_follow_a_transformed_curve() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let line_id = line(&mut f, space, 0.0, 0.0, 10.0, 0.0);
    let dimension = f.create(
        "core.draft.dimension.create",
        json!({"owner": space.to_string(), "kind": "aligned",
               "points": [{"object": line_id.to_string(), "key": "start"},
                          {"object": line_id.to_string(), "key": "end"}],
               "line_position": [0.0, -5.0]}),
    );
    let square = f.entity(
        space,
        curve_json(&Curve2::Polyline(polyline(
            &[
                (0.0, 20.0, 0.0),
                (5.0, 20.0, 0.0),
                (5.0, 25.0, 0.0),
                (0.0, 25.0, 0.0),
            ],
            true,
        ))),
    );
    let hatch = f.create(
        "core.draft.hatch.create",
        json!({"owner": space.to_string(), "kind": "solid", "objects": [square.to_string()]}),
    );
    transform(&mut f, &[line_id], json!({"translation": [3.0, 0.0]})).unwrap();

    let snapshot = f.document.snapshot();
    let resolved =
        arc_draft::dimension::resolve(&snapshot, dimension, &DisplayOptions::default()).unwrap();
    assert_point([resolved.points[0].x, resolved.points[0].y], [3.0, 0.0]);
    assert_point([resolved.points[1].x, resolved.points[1].y], [13.0, 0.0]);

    let graph = space_list(
        &snapshot,
        Lists::Graph(f.document.graph()),
        space,
        &DisplayOptions::default(),
    )
    .unwrap();
    let direct = space_list(&snapshot, Lists::Direct, space, &DisplayOptions::default()).unwrap();
    assert_eq!(
        format!("{:?}", graph.primitives),
        format!("{:?}", direct.primitives),
        "stale dependents"
    );
    let _ = hatch;
}

fn point_of(f: &Fixture, id: ObjectId) -> [f64; 2] {
    point(f, id, "position")
}
