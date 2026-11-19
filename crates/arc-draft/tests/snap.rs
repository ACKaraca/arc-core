//! Object snaps over documents: one fixture per snap kind, ranking, apertures, block references,
//! layer states, bounds and the constraint functions on real geometry.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

mod support;

use std::f64::consts::{FRAC_PI_2, PI};

use arc_base::math::{Point2, Vector2};
use arc_base::{Limits, ObjectId, Tolerance};
use arc_draft::snap::{SnapCandidate, SnapKind, SnapModes, SnapQuery, snap_candidates};
use arc_geom::{Arc2, Circle2, Curve2, Ellipse2, Nurbs2, Polyline2, PolylineVertex, Segment2};
use proptest::prelude::*;
use serde_json::json;
use support::{Fixture, curve_json};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn segment(a: Point2, b: Point2) -> Curve2 {
    Curve2::Segment(Segment2::new(a, b).unwrap())
}

fn query(cursor: Point2, aperture: f64) -> SnapQuery {
    SnapQuery::new(cursor, aperture, SnapModes::ALL, Tolerance::DEFAULT)
}

fn run(f: &Fixture, space: ObjectId, query: &SnapQuery) -> Vec<SnapCandidate> {
    snap_candidates(&f.document.snapshot(), space, query).unwrap()
}

fn of_kind(out: &[SnapCandidate], kind: SnapKind) -> Vec<&SnapCandidate> {
    out.iter().filter(|c| c.kind == kind).collect()
}

fn at(point: Point2, tolerance: f64) -> impl Fn(&&SnapCandidate) -> bool {
    move |c: &&SnapCandidate| (c.point - point).norm() <= tolerance
}

fn has(out: &[SnapCandidate], kind: SnapKind, point: Point2) -> bool {
    of_kind(out, kind)
        .iter()
        .any(|c| (c.point - point).norm() < 1e-6)
}

#[test]
fn line_endpoint_midpoint_and_nearest() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(10.0, 0.0))));

    let out = run(&f, space, &query(p(5.0, 0.2), 0.5));
    assert!(has(&out, SnapKind::Midpoint, p(5.0, 0.0)));
    assert!(has(&out, SnapKind::Nearest, p(5.0, 0.0)));
    assert!(of_kind(&out, SnapKind::Endpoint).is_empty());

    let out = run(&f, space, &query(p(0.1, 0.1), 0.5));
    assert_eq!(out[0].kind, SnapKind::Endpoint);
    assert!(has(&out, SnapKind::Endpoint, p(0.0, 0.0)));
    // The cursor is 0.1 beyond the start of the line along -x, so the extension path offers it.
    assert!(of_kind(&out, SnapKind::Extension).is_empty());
    let out = run(&f, space, &query(p(-0.1, 0.05), 0.5));
    assert!(has(&out, SnapKind::Extension, p(-0.1, 0.0)));
}

#[test]
fn arc_center_midpoint_quadrant_and_endpoints() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let arc = Arc2::new(p(0.0, 0.0), 2.0, 0.0, FRAC_PI_2).unwrap();
    f.entity(space, curve_json(&Curve2::Arc(arc)));

    let out = run(&f, space, &query(p(0.0, 2.0), 0.1));
    assert!(has(&out, SnapKind::Endpoint, p(0.0, 2.0)));
    assert!(has(&out, SnapKind::Quadrant, p(0.0, 2.0)));
    let out = run(&f, space, &query(p(1.4, 1.4), 0.1));
    let mid = 2.0 * (PI / 4.0).cos();
    assert!(has(&out, SnapKind::Midpoint, p(mid, mid)));
    let out = run(&f, space, &query(p(0.1, 0.1), 0.3));
    assert!(has(&out, SnapKind::Center, p(0.0, 0.0)));
    // (2, 0) is the start of the arc: an endpoint, not a quadrant on the other side of the turn.
    let out = run(&f, space, &query(p(-2.0, 0.0), 0.1));
    assert!(of_kind(&out, SnapKind::Quadrant).is_empty());
}

#[test]
fn circle_quadrant_center_tangent_and_perpendicular() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(
        space,
        curve_json(&Curve2::Circle(Circle2::new(p(0.0, 0.0), 2.0).unwrap())),
    );

    let out = run(&f, space, &query(p(2.0, 0.05), 0.2));
    assert!(has(&out, SnapKind::Quadrant, p(2.0, 0.0)));
    assert!(of_kind(&out, SnapKind::Center).is_empty());
    let out = run(&f, space, &query(p(0.05, 0.05), 0.3));
    assert!(has(&out, SnapKind::Center, p(0.0, 0.0)));

    // From (4, 0) the tangent points are at 60 degrees: (1, sqrt 3) and (1, -sqrt 3).
    let mut q = query(p(1.0, 1.7), 0.5);
    q.from = Some(p(4.0, 0.0));
    let out = run(&f, space, &q);
    let root = 3.0_f64.sqrt();
    assert!(has(&out, SnapKind::Tangent, p(1.0, root)));

    // Without a previous point the deferred snaps yield nothing.
    let out = run(&f, space, &query(p(1.0, 1.7), 0.5));
    assert!(of_kind(&out, SnapKind::Tangent).is_empty());
    assert!(of_kind(&out, SnapKind::Perpendicular).is_empty());
}

#[test]
fn circle_perpendicular_from_the_previous_point() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(
        space,
        curve_json(&Curve2::Circle(Circle2::new(p(0.0, 0.0), 2.0).unwrap())),
    );
    let mut q = query(p(2.0, 0.1), 0.3);
    q.from = Some(p(5.0, 0.0));
    let out = run(&f, space, &q);
    assert!(has(&out, SnapKind::Perpendicular, p(2.0, 0.0)));
}

#[test]
fn ellipse_quadrants_follow_the_axes_and_need_alignment() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let ellipse = Ellipse2::new(p(0.0, 0.0), Vector2::new(4.0, 0.0), 0.5).unwrap();
    f.entity(space, curve_json(&Curve2::Ellipse(ellipse)));
    let out = run(&f, space, &query(p(0.0, 2.0), 0.1));
    assert!(has(&out, SnapKind::Quadrant, p(0.0, 2.0)));
    let out = run(&f, space, &query(p(-4.0, 0.0), 0.1));
    assert!(has(&out, SnapKind::Quadrant, p(-4.0, 0.0)));

    // A UCS turned 45 degrees has no quadrant points on an axis-aligned ellipse.
    let mut q = query(p(0.0, 2.0), 0.1);
    q.ucs_x = Vector2::new(1.0, 1.0);
    assert!(of_kind(&run(&f, space, &q), SnapKind::Quadrant).is_empty());
}

#[test]
fn polyline_arc_segment_has_a_center_and_closed_polyline_a_centroid() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let arc_chain = Polyline2::new(
        vec![
            PolylineVertex::new(p(0.0, 0.0), 1.0),
            PolylineVertex::new(p(2.0, 0.0), 0.0),
        ],
        false,
    )
    .unwrap();
    f.entity(space, curve_json(&Curve2::Polyline(arc_chain)));
    let out = run(&f, space, &query(p(1.0, 0.05), 0.3));
    assert!(has(&out, SnapKind::Center, p(1.0, 0.0)));
    let out = run(&f, space, &query(p(0.05, 0.0), 0.3));
    assert!(has(&out, SnapKind::Endpoint, p(0.0, 0.0)));

    let square = Polyline2::new(
        vec![
            PolylineVertex::new(p(0.0, 0.0), 0.0),
            PolylineVertex::new(p(2.0, 0.0), 0.0),
            PolylineVertex::new(p(2.0, 2.0), 0.0),
            PolylineVertex::new(p(0.0, 2.0), 0.0),
        ],
        true,
    )
    .unwrap();
    f.entity(space, curve_json(&Curve2::Polyline(square)));
    let out = run(&f, space, &query(p(1.0, 1.05), 0.2));
    assert!(has(&out, SnapKind::GeometricCenter, p(1.0, 1.0)));
    let out = run(&f, space, &query(p(1.0, 0.05), 0.2));
    assert!(has(&out, SnapKind::Midpoint, p(1.0, 0.0)));
}

#[test]
fn spline_has_ends_but_no_midpoint() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let spline = Nurbs2::interpolate(
        &[p(0.0, 0.0), p(5.0, 5.0), p(10.0, 0.0)],
        2,
        &Tolerance::DEFAULT,
    )
    .unwrap();
    f.entity(space, curve_json(&Curve2::Nurbs(spline)));
    let out = run(&f, space, &query(p(0.0, 0.05), 0.2));
    assert!(has(&out, SnapKind::Endpoint, p(0.0, 0.0)));
    let out = run(&f, space, &query(p(5.0, 5.0), 0.2));
    assert!(of_kind(&out, SnapKind::Midpoint).is_empty());
    assert!(of_kind(&out, SnapKind::Endpoint).is_empty());
}

#[test]
fn crossing_lines_intersect_and_extensions_meet_extended() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 5.0), p(10.0, 5.0))));
    f.entity(space, curve_json(&segment(p(5.0, 0.0), p(5.0, 10.0))));
    let out = run(&f, space, &query(p(5.05, 5.05), 0.2));
    let crossing = of_kind(&out, SnapKind::Intersection);
    assert!(
        crossing
            .iter()
            .any(|c| !c.extended && (c.point - p(5.0, 5.0)).norm() < 1e-6)
    );
    assert_eq!(
        crossing.iter().find(|c| !c.extended).unwrap().sources.len(),
        2
    );

    let mut g = Fixture::drafting();
    let space = g.space("model");
    g.entity(space, curve_json(&segment(p(0.0, 0.0), p(4.0, 0.0))));
    g.entity(space, curve_json(&segment(p(6.0, -2.0), p(6.0, 2.0))));
    let out = run(&g, space, &query(p(6.0, 0.05), 0.2));
    let extended: Vec<_> = of_kind(&out, SnapKind::Intersection)
        .into_iter()
        .filter(|c| c.extended)
        .collect();
    assert!(extended.iter().any(at(p(6.0, 0.0), 1e-6)));
    // The segments themselves do not reach (6, 0): no actual intersection there.
    assert!(
        of_kind(&out, SnapKind::Intersection)
            .iter()
            .all(|c| c.extended)
    );
}

#[test]
fn parallel_path_through_the_previous_point() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(10.0, 0.0))));
    // The path through (2, 5) parallel to the line passes (6, 5); the cursor is 0.1 from it.
    let mut q = query(p(6.0, 5.1), 0.3);
    q.from = Some(p(2.0, 5.0));
    let out = run(&f, space, &q);
    assert!(has(&out, SnapKind::Parallel, p(6.0, 5.0)));
    // Far from every parallel path, nothing is offered.
    let mut far = query(p(6.0, 8.0), 0.3);
    far.from = Some(p(2.0, 5.0));
    assert!(of_kind(&run(&f, space, &far), SnapKind::Parallel).is_empty());
}

#[test]
fn ranking_is_by_kind_then_distance() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(10.0, 0.0))));
    f.entity(space, curve_json(&segment(p(0.0, 0.3), p(0.0, 10.0))));
    let out = run(&f, space, &query(p(0.1, 0.05), 1.0));
    let kinds: Vec<SnapKind> = out.iter().map(|c| c.kind).collect();
    let mut sorted = kinds.clone();
    sorted.sort();
    assert_eq!(kinds, sorted);
    let endpoints = of_kind(&out, SnapKind::Endpoint);
    assert!(endpoints.windows(2).all(|w| w[0].distance <= w[1].distance));
    assert_eq!(out, run(&f, space, &query(p(0.1, 0.05), 1.0)));
}

#[test]
fn modes_and_aperture_restrict_the_output() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(10.0, 0.0))));
    let only_mid = SnapQuery::new(
        p(0.1, 0.0),
        0.5,
        SnapModes::of(&[SnapKind::Midpoint]),
        Tolerance::DEFAULT,
    );
    assert!(run(&f, space, &only_mid).is_empty());
    let zero = query(p(5.0, 0.0), 0.0);
    assert!(run(&f, space, &zero).is_empty());
    let far = query(p(5.0, 3.0), 0.5);
    assert!(run(&f, space, &far).is_empty());
    assert!(
        SnapModes::ALL
            .without(SnapKind::Nearest)
            .contains(SnapKind::Endpoint)
    );
    assert!(
        !SnapModes::ALL
            .without(SnapKind::Nearest)
            .contains(SnapKind::Nearest)
    );
}

#[test]
fn refuses_non_finite_input() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(1.0, 0.0))));
    let snapshot = f.document.snapshot();
    let bad = query(p(f64::NAN, 0.0), 0.5);
    assert!(snap_candidates(&snapshot, space, &bad).is_err());
    let mut zero_axis = query(p(0.0, 0.0), 0.5);
    zero_axis.ucs_x = Vector2::zeros();
    assert!(snap_candidates(&snapshot, space, &zero_axis).is_err());
    let mut bad_tolerance = query(p(0.0, 0.0), 0.5);
    bad_tolerance.tolerance.linear = 0.0;
    assert!(snap_candidates(&snapshot, space, &bad_tolerance).is_err());
}

#[test]
fn entity_limit_is_named_and_enforced() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(space, curve_json(&segment(p(0.0, 0.0), p(1.0, 0.0))));
    f.entity(space, curve_json(&segment(p(0.0, 1.0), p(1.0, 1.0))));
    let mut q = query(p(0.0, 0.0), 0.5);
    q.limits = Limits {
        record_count: 1,
        ..Limits::DEFAULT
    };
    let error = snap_candidates(&f.document.snapshot(), space, &q).unwrap_err();
    assert_eq!(error.code(), "draft.snap.limit_exceeded");
}

#[test]
fn block_reference_reaches_its_members_through_the_transform() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let block = f.create("core.draft.block.define", json!({"name": "b"}));
    f.entity(block, curve_json(&segment(p(0.0, 0.0), p(4.0, 0.0))));
    let insert = f.create(
        "core.draft.block.insert",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [10.0, 0.0]}),
    );
    let out = run(&f, space, &query(p(14.0, 0.05), 0.2));
    let endpoint = of_kind(&out, SnapKind::Endpoint)
        .into_iter()
        .find(|c| (c.point - p(14.0, 0.0)).norm() < 1e-6)
        .expect("the member endpoint is offered at its transformed position");
    assert_eq!(endpoint.sources[0].block_path, vec![insert]);
    assert_ne!(endpoint.sources[0].entity, insert);
}

#[test]
fn layer_states_exclude_frozen_and_off_and_keep_locked() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let layer = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let line = f.entity(space, curve_json(&segment(p(0.0, 0.0), p(10.0, 0.0))));
    f.set(
        "core.draft.entity.modify",
        &[line],
        "layer",
        json!(layer.to_string()),
    );
    let count = |f: &Fixture| run(f, space, &query(p(0.0, 0.05), 0.5)).len();
    assert!(count(&f) > 0);

    f.set("core.draft.layer.modify", &[layer], "locked", json!(true));
    assert!(count(&f) > 0);

    f.set("core.draft.layer.modify", &[layer], "locked", json!(false));
    f.set("core.draft.layer.modify", &[layer], "frozen", json!(true));
    assert_eq!(count(&f), 0);

    f.set("core.draft.layer.modify", &[layer], "frozen", json!(false));
    f.set("core.draft.layer.modify", &[layer], "on", json!(false));
    assert_eq!(count(&f), 0);
}

#[test]
fn insertion_point_of_text_and_block_references() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "A", "position": [1.0, 2.0]}),
    );
    let out = run(&f, space, &query(p(1.0, 2.05), 0.2));
    assert!(has(&out, SnapKind::Insertion, p(1.0, 2.0)));
}

#[test]
fn intersection_pairs_are_bounded() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    for i in 0..300 {
        let y = f64::from(i) * 0.001;
        f.entity(space, curve_json(&segment(p(0.0, y), p(10.0, y + 0.0005))));
    }
    // 300 curves near the cursor form 44850 pairs, under the bound: the query succeeds.
    let out = snap_candidates(&f.document.snapshot(), space, &query(p(5.0, 0.1), 0.5));
    assert!(out.is_ok());
}

proptest! {
    #[test]
    fn nearest_point_is_on_the_segment_and_no_farther_than_its_ends(
        ax in -50.0..50.0_f64, ay in -50.0..50.0_f64,
        bx in -50.0..50.0_f64, by in -50.0..50.0_f64,
        cx in -50.0..50.0_f64, cy in -50.0..50.0_f64,
    ) {
        prop_assume!((p(ax, ay) - p(bx, by)).norm() > 1e-3);
        let mut f = Fixture::drafting();
        let space = f.space("model");
        let seg = segment(p(ax, ay), p(bx, by));
        f.entity(space, curve_json(&seg));
        let cursor = p(cx, cy);
        let mut q = SnapQuery::new(cursor, 1e6, SnapModes::of(&[SnapKind::Nearest]), Tolerance::DEFAULT);
        q.from = None;
        let out = run(&f, space, &q);
        prop_assert_eq!(out.len(), 1);
        let hit = out[0].point;
        let on_segment = seg.distance_to(hit);
        prop_assert!(on_segment <= 1e-6);
        let best_end = (p(ax, ay) - cursor).norm().min((p(bx, by) - cursor).norm());
        prop_assert!((hit - cursor).norm() <= best_end + 1e-6);
    }
}

trait DistanceTo {
    fn distance_to(&self, point: Point2) -> f64;
}

impl DistanceTo for Curve2 {
    fn distance_to(&self, point: Point2) -> f64 {
        use arc_geom::closest::CurveProject;
        self.closest_point(point, &Tolerance::DEFAULT)
            .unwrap()
            .distance
    }
}
