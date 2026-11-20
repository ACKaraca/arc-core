//! The snap index: candidates from an index equal the direct query on the same revision, a query
//! against another revision or with other build bounds is refused, and the index is bounded.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

mod support;

use arc_base::math::{Point2, Vector2};
use arc_base::{Limits, ObjectId, Tolerance};
use arc_draft::snap::{
    SnapCandidate, SnapIndex, SnapKind, SnapModes, SnapQuery, snap_candidates,
    snap_candidates_indexed,
};
use arc_geom::{Arc2, Circle2, Curve2, Polyline2, PolylineVertex, Segment2};
use serde_json::json;
use support::{Fixture, curve_json};

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn query(cursor: Point2) -> SnapQuery {
    SnapQuery::new(cursor, 0.75, SnapModes::ALL, Tolerance::DEFAULT)
}

/// A drawing with lines, an arc, a circle, a polyline, a point and an entity on a hidden layer.
fn drawing() -> (Fixture, ObjectId) {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(p(0.0, 0.0), p(10.0, 0.0)).unwrap(),
        )),
    );
    f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(p(10.0, 0.0), p(10.0, 8.0)).unwrap(),
        )),
    );
    f.entity(
        space,
        curve_json(&Curve2::Arc(
            Arc2::new(p(4.0, 4.0), 3.0, 0.0, std::f64::consts::FRAC_PI_2).unwrap(),
        )),
    );
    f.entity(
        space,
        curve_json(&Curve2::Circle(Circle2::new(p(-3.0, -3.0), 2.0).unwrap())),
    );
    let vertices = vec![
        PolylineVertex::new(p(-6.0, 2.0), 0.0),
        PolylineVertex::new(p(-2.0, 2.0), 0.5),
        PolylineVertex::new(p(-2.0, 6.0), 0.0),
    ];
    f.entity(
        space,
        curve_json(&Curve2::Polyline(Polyline2::new(vertices, false).unwrap())),
    );
    f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": {"kind": "point", "position": [7.0, 7.0]}}),
    );
    let hidden = f.create("core.draft.layer.create", json!({"name": "hidden"}));
    f.set("core.draft.layer.modify", &[hidden], "on", json!(false));
    f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "layer": hidden.to_string(),
               "geometry": curve_json(&Curve2::Segment(Segment2::new(p(-10.0, 9.0), p(10.0, 9.0)).unwrap()))}),
    );
    (f, space)
}

fn same(a: &[SnapCandidate], b: &[SnapCandidate]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.kind == y.kind
                && x.extended == y.extended
                && x.sources == y.sources
                && x.point == y.point
                && x.distance == y.distance
        })
}

fn cursors() -> Vec<Point2> {
    let mut out = Vec::new();
    for i in -12..=12 {
        for j in -10..=10 {
            out.push(p(f64::from(i) * 0.9, f64::from(j) * 0.9));
        }
    }
    out
}

#[test]
fn indexed_candidates_equal_the_direct_query_at_every_cursor_position() {
    let (f, space) = drawing();
    let snapshot = f.document.snapshot();
    let index = SnapIndex::build(&snapshot, space, &query(p(0.0, 0.0))).unwrap();
    assert_eq!(index.revision(), snapshot.revision());
    assert_eq!(index.space(), space);
    assert!(!index.is_empty());
    let mut found = 0;
    for cursor in cursors() {
        let mut q = query(cursor);
        // A previous point and a UCS exercise the perpendicular, tangent and parallel snaps.
        q.from = Some(p(-7.0, -7.0));
        q.ucs_x = Vector2::new(1.0, 0.5);
        let direct = snap_candidates(&snapshot, space, &q).unwrap();
        let indexed = snap_candidates_indexed(&snapshot, &index, &q).unwrap();
        assert!(
            same(&direct, &indexed),
            "cursor {cursor:?}: {direct:?} != {indexed:?}"
        );
        found += direct.len();
    }
    assert!(found > 0);
    // The hidden layer's line is not a candidate in either path.
    let hidden = snap_candidates_indexed(&snapshot, &index, &query(p(0.0, 9.0))).unwrap();
    assert!(hidden.iter().all(|c| (c.point.y - 9.0).abs() > 1e-9));
}

#[test]
fn indexed_query_with_modes_and_aperture_matches_the_direct_query() {
    let (f, space) = drawing();
    let snapshot = f.document.snapshot();
    let index = SnapIndex::build(&snapshot, space, &query(p(0.0, 0.0))).unwrap();
    let modes = SnapModes::of(&[SnapKind::Endpoint, SnapKind::Intersection]);
    for aperture in [0.0, 0.5, 3.0] {
        let q = SnapQuery::new(p(10.0, 0.0), aperture, modes, Tolerance::DEFAULT);
        let direct = snap_candidates(&snapshot, space, &q).unwrap();
        let indexed = snap_candidates_indexed(&snapshot, &index, &q).unwrap();
        assert!(same(&direct, &indexed));
    }
}

#[test]
fn a_query_against_another_revision_is_refused_and_a_rebuilt_index_answers_it() {
    let (mut f, space) = drawing();
    let built = f.document.snapshot();
    let index = SnapIndex::build(&built, space, &query(p(0.0, 0.0))).unwrap();
    f.entity(
        space,
        curve_json(&Curve2::Segment(
            Segment2::new(p(20.0, 0.0), p(20.0, 5.0)).unwrap(),
        )),
    );
    let current = f.document.snapshot();
    assert_ne!(current.revision(), index.revision());

    let q = query(p(20.0, 0.0));
    let error = snap_candidates_indexed(&current, &index, &q).unwrap_err();
    assert_eq!(error.code(), "draft.snap.index_stale");
    // The index still answers for the revision it was built from.
    assert!(snap_candidates_indexed(&built, &index, &q).is_ok());

    let rebuilt = SnapIndex::build(&current, space, &q).unwrap();
    let direct = snap_candidates(&current, space, &q).unwrap();
    let indexed = snap_candidates_indexed(&current, &rebuilt, &q).unwrap();
    assert!(same(&direct, &indexed));
    assert!(
        direct
            .iter()
            .any(|c| c.kind == SnapKind::Endpoint && c.point == p(20.0, 0.0))
    );
}

#[test]
fn a_query_with_other_build_bounds_is_refused() {
    let (f, space) = drawing();
    let snapshot = f.document.snapshot();
    let index = SnapIndex::build(&snapshot, space, &query(p(0.0, 0.0))).unwrap();
    let mut tolerance = query(p(0.0, 0.0));
    tolerance.tolerance = Tolerance {
        linear: 1e-3,
        ..Tolerance::DEFAULT
    };
    assert_eq!(
        snap_candidates_indexed(&snapshot, &index, &tolerance)
            .unwrap_err()
            .code(),
        "draft.snap.index_mismatch"
    );
    let mut depth = query(p(0.0, 0.0));
    depth.max_depth += 1;
    assert_eq!(
        snap_candidates_indexed(&snapshot, &index, &depth)
            .unwrap_err()
            .code(),
        "draft.snap.index_mismatch"
    );
}

#[test]
fn the_index_is_bounded_by_the_record_count() {
    let (f, space) = drawing();
    let snapshot = f.document.snapshot();
    let mut small = query(p(0.0, 0.0));
    small.limits = Limits {
        record_count: 3,
        ..Limits::DEFAULT
    };
    assert_eq!(
        SnapIndex::build(&snapshot, space, &small)
            .unwrap_err()
            .code(),
        "draft.snap.limit_exceeded"
    );
}
