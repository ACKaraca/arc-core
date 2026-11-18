//! Spline entities: the create and set commands, the stored form (encoding version 2), the
//! display contribution and the package round trip.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_base::{Limits, ObjectId};
use arc_doc::{Value, ValueType};
use arc_draft::display::{DisplayOptions, Shape, entity_list};
use arc_draft::types::{SPLINE, entity};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Nurbs2};
use serde_json::json;
use support::{Fixture, curve_json};

const CONTROL: [[f64; 2]; 5] = [[0.0, 0.0], [1.0, 3.0], [3.0, 3.0], [4.0, 0.0], [6.0, 1.0]];

fn nurbs_of(f: &Fixture, id: ObjectId) -> Nurbs2 {
    let Some(Some(Value::Geometry(bytes))) = f.values(id).get(entity::KIND).cloned() else {
        panic!("no geometry");
    };
    Nurbs2::decode(&bytes.0, &Limits::DEFAULT).unwrap()
}

#[test]
fn control_point_splines_store_the_curve_in_encoding_version_two() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let id = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "degree": 3, "control_points": CONTROL,
               "weights": [1.0, 2.0, 1.0, 1.0, 0.5]}),
    );
    let curve = nurbs_of(&f, id);
    assert_eq!(curve.degree(), 3);
    assert_eq!(curve.control_points().len(), 5);
    assert_eq!(curve.weights(), [1.0, 2.0, 1.0, 1.0, 0.5]);
    // The field descriptor names version 2, which is what the codec reports for NURBS.
    let snapshot = f.document.snapshot();
    let decl = snapshot.registry().get(SPLINE).unwrap();
    let Some(ValueType::Geometry(encoding)) = decl
        .schema
        .fields
        .iter()
        .find(|f| f.name == "geometry")
        .map(|f| f.value_type.clone())
    else {
        panic!("geometry is not a geometry field");
    };
    assert_eq!(encoding.name, curve.encoding().name);
    assert_eq!(encoding.version, u32::from(curve.encoding().version));
    assert_eq!(encoding.version, 2);
    assert!(arc_draft::spline::fit_points(&f.values(id)).is_empty());
}

#[test]
fn fit_point_splines_interpolate_and_keep_the_fit_points() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let fit = [[0.0, 0.0], [1.0, 2.0], [3.0, 3.0], [5.0, 1.0], [6.0, 0.0]];
    let id = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "fit_points": fit}),
    );
    let curve = nurbs_of(&f, id);
    let (lo, hi) = curve.parameter_range();
    assert_eq!(curve.point_at(lo), Point2::new(0.0, 0.0));
    assert!((curve.point_at(hi) - Point2::new(6.0, 0.0)).norm() < 1e-9);
    let stored = arc_draft::spline::fit_points(&f.values(id));
    assert_eq!(stored.len(), 5);
    assert_eq!(stored[2], Point2::new(3.0, 3.0));
    // Each fit point lies on the curve: the interpolation passes through it.
    let near = |p: &Point2| {
        (0..=2000)
            .map(|i| curve.point_at(lo + (hi - lo) * f64::from(i) / 2000.0))
            .map(|q| (q - p).norm())
            .fold(f64::INFINITY, f64::min)
    };
    assert!(stored.iter().all(|p| near(p) < 1e-2));
}

#[test]
fn invalid_descriptions_are_refused_with_their_codes() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let owner = space.to_string();
    let both = f.run(
        "core.draft.spline.create",
        json!({"owner": owner, "control_points": CONTROL, "fit_points": CONTROL}),
    );
    assert_eq!(both.unwrap_err().code(), "draft.field.invalid");
    let neither = f.run("core.draft.spline.create", json!({"owner": owner}));
    assert_eq!(neither.unwrap_err().code(), "draft.field.invalid");
    let weights_with_fit = f.run(
        "core.draft.spline.create",
        json!({"owner": owner, "fit_points": CONTROL, "weights": [1.0, 1.0, 1.0, 1.0, 1.0]}),
    );
    assert_eq!(weights_with_fit.unwrap_err().code(), "draft.field.invalid");
    let degree = f.run(
        "core.draft.spline.create",
        json!({"owner": owner, "control_points": CONTROL[..3], "degree": 3}),
    );
    assert_eq!(degree.unwrap_err().code(), "geom.nurbs.degree_invalid");
    let coincident = f.run(
        "core.draft.spline.create",
        json!({"owner": owner, "fit_points": [[0.0, 0.0], [0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]}),
    );
    assert_eq!(coincident.unwrap_err().code(), "geom.fit.points_invalid");
    // Nothing was created by any of them.
    assert_eq!(f.document.snapshot().len(), 1);
}

#[test]
fn set_replaces_geometry_and_fit_points_together() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let id = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "fit_points": [[0.0, 0.0], [1.0, 1.0], [2.0, 0.0], [3.0, 1.0]]}),
    );
    assert_eq!(arc_draft::spline::fit_points(&f.values(id)).len(), 4);
    f.run(
        "core.draft.spline.set",
        json!({"objects": [id.to_string()], "control_points": CONTROL, "degree": 2}),
    )
    .unwrap();
    let curve = nurbs_of(&f, id);
    assert_eq!(curve.degree(), 2);
    assert_eq!(curve.control_points().len(), 5);
    assert!(arc_draft::spline::fit_points(&f.values(id)).is_empty());
    // A line is not a spline.
    let line = f.line(space);
    let wrong = f.run(
        "core.draft.spline.set",
        json!({"objects": [line.to_string()], "control_points": CONTROL}),
    );
    assert_eq!(wrong.unwrap_err().code(), "draft.object.wrong_type");
}

#[test]
fn entity_create_builds_a_spline_from_a_nurbs_curve_and_modify_edits_its_fields() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let nurbs = Nurbs2::from_control_points(
        3,
        CONTROL.iter().map(|[x, y]| Point2::new(*x, *y)).collect(),
        vec![1.0; 5],
    )
    .unwrap();
    let id = f.entity(space, curve_json(&Curve2::Nurbs(nurbs.clone())));
    assert_eq!(nurbs_of(&f, id), nurbs);
    f.set(
        "core.draft.entity.modify",
        &[id],
        "fit_points",
        json!([[0.0, 0.0], [1.0, 1.0]]),
    );
    assert_eq!(arc_draft::spline::fit_points(&f.values(id)).len(), 2);
    // The geometry field accepts only a NURBS curve.
    let line = curve_json(&support::every_curve()[0]);
    let err = f
        .run(
            "core.draft.entity.modify",
            json!({"objects": [id.to_string()], "field": "geometry", "value": line}),
        )
        .unwrap_err();
    assert!(!err.code().is_empty());
    assert!(matches!(nurbs_of(&f, id), n if n == nurbs));
}

#[test]
fn a_spline_contributes_one_stroke_with_its_exact_curve_and_dashes() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let id = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "control_points": CONTROL}),
    );
    let curve = nurbs_of(&f, id);
    let snapshot = f.document.snapshot();
    let list = entity_list(&snapshot, id, &DisplayOptions::default()).unwrap();
    assert_eq!(list.primitives.len(), 1);
    let Shape::Stroke {
        curve: shown,
        drawn,
    } = &list.primitives[0].shape
    else {
        panic!("not a stroke");
    };
    assert_eq!(shown, &Curve2::Nurbs(curve.clone()));
    assert!(drawn.is_none());

    // A dashed linetype generates pieces on the exact curve.
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 1.0}, {"kind": "gap", "length": 1.0}]}),
    );
    f.set(
        "core.draft.entity.modify",
        &[id],
        "linetype_by",
        json!("object"),
    );
    f.set(
        "core.draft.entity.modify",
        &[id],
        "linetype",
        json!(dashed.to_string()),
    );
    let snapshot = f.document.snapshot();
    let list = entity_list(&snapshot, id, &DisplayOptions::default()).unwrap();
    let Shape::Stroke {
        drawn: Some(pieces),
        ..
    } = &list.primitives[0].shape
    else {
        panic!("no dashes");
    };
    assert!(pieces.len() >= 3);
}

#[test]
fn a_spline_with_a_non_nurbs_geometry_fails_validation() {
    use arc_doc::GeometryBytes;
    let mut f = Fixture::new();
    let space = f.space("model");
    let id = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "control_points": CONTROL}),
    );
    // A circle's bytes decode as a curve but not as a NURBS curve.
    let circle = support::every_curve()[3].encode();
    let err = f
        .document
        .snapshot()
        .registry()
        .get(SPLINE)
        .map(|d| {
            (d.validators[0])(&{
                let mut v = f.values(id);
                v[entity::KIND] = Some(Value::Geometry(GeometryBytes(circle)));
                v
            })
        })
        .unwrap();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].error.code(), "draft.geometry.invalid");
}

#[test]
fn splines_survive_save_and_reopen() {
    let mut f = Fixture::new();
    let space = f.space("model");
    f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "control_points": CONTROL, "weights": [1.0, 2.0, 1.0, 1.0, 1.0]}),
    );
    f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "fit_points": [[0.0, 0.0], [1.0, 2.0], [3.0, 3.0], [5.0, 1.0]], "degree": 2}),
    );
    support::round_trip(&f);
}
