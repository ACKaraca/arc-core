use std::f64::consts::{FRAC_PI_2, PI, TAU};

use arc_base::math::{Point2, Vector2};
use proptest::prelude::*;

use super::*;
use crate::curve::tests::curve;
use crate::polyline::{Polyline2, PolylineVertex};
use crate::primitives::{Arc2, Circle2, Segment2};

const TOL: Tolerance = Tolerance::DEFAULT;

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap())
}

#[test]
fn invalid_tolerances_are_rejected() {
    let s = seg(0.0, 0.0, 1.0, 1.0);
    for (linear, angular) in [(0.0, 1e-9), (-1.0, 1e-9), (f64::NAN, 1e-9), (1e-6, -1.0)] {
        let tolerance = Tolerance {
            linear,
            angular,
            ..TOL
        };
        assert_eq!(
            intersect(&s, &s, &tolerance).unwrap_err().code(),
            "geom.tolerance.invalid"
        );
    }
}

#[test]
fn duplicates_at_a_polyline_vertex_merge_into_one_point() {
    let v = |x: f64, y: f64| PolylineVertex::new(Point2::new(x, y), 0.0);
    let p = Curve2::Polyline(
        Polyline2::new(vec![v(0.0, 0.0), v(1.0, 1.0), v(2.0, 0.0)], false).unwrap(),
    );
    let result = intersect(&p, &seg(1.0, 0.0, 1.0, 2.0), &TOL).unwrap();
    assert_eq!(result.len(), 1);
    let Intersection::Point { a, b, contact, .. } = result[0] else {
        panic!("expected a point");
    };
    assert_eq!((a, contact), (1.0, Contact::Crossing));
    assert!((b - 0.5).abs() < 1e-12);
}

#[test]
fn circle_seam_reports_the_start_of_the_range() {
    let circle = Curve2::Circle(Circle2::new(Point2::origin(), 1.0).unwrap());
    let result = intersect(&circle, &seg(1.0, -1.0, 1.0, 1.0), &TOL).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(
        result[0],
        Intersection::Point { a, contact: Contact::Tangent, .. } if a == 0.0
    ));
}

#[test]
fn a_full_arc_overlaps_its_circle_once() {
    let circle = Curve2::Circle(Circle2::new(Point2::origin(), 1.0).unwrap());
    let arc = Curve2::Arc(Arc2::new(Point2::origin(), 1.0, 0.0, -TAU).unwrap());
    let result = intersect(&circle, &arc, &TOL).unwrap();
    assert_eq!(
        result,
        vec![Intersection::Overlap {
            a: [0.0, TAU],
            b: [0.0, TAU],
            orientation: Orientation::Opposite
        }]
    );
}

fn simple_point() -> impl Strategy<Value = Point2> {
    (-10.0..10.0_f64, -10.0..10.0_f64).prop_map(|(x, y)| Point2::new(x, y))
}

/// Segments, circles and arcs of moderate size near the origin.
fn simple_curve() -> impl Strategy<Value = Curve2> {
    prop_oneof![
        (simple_point(), simple_point()).prop_filter_map("segment", |(a, b)| Segment2::new(a, b)
            .ok()
            .map(Curve2::Segment)),
        (simple_point(), 0.5..10.0_f64)
            .prop_map(|(c, r)| Curve2::Circle(Circle2::new(c, r).unwrap())),
        (
            simple_point(),
            0.5..10.0_f64,
            -PI..PI,
            crate::testing::sweep()
        )
            .prop_map(|(c, r, a, s)| Curve2::Arc(Arc2::new(c, r, a, s).unwrap())),
    ]
}

/// The signed distance of `p` from the carrier of `a`, and the fraction of `a`'s range at
/// which the nearest carrier point lies (outside [0, 1] when it is beyond the curve).
fn carrier(a: &Curve2, p: &Point2) -> (f64, f64) {
    match a {
        Curve2::Segment(s) => {
            let d = s.end() - s.start();
            let q = p - s.start();
            (
                (d.x * q.y - d.y * q.x) / d.norm(),
                q.dot(&d) / d.norm_squared(),
            )
        }
        Curve2::Circle(c) => ((p - c.center()).norm() - c.radius(), 0.5),
        Curve2::Arc(c) => {
            let q = p - c.center();
            let traveled =
                (c.sweep().signum() * (q.y.atan2(q.x) - c.start_angle())).rem_euclid(TAU);
            let fraction = if traveled <= c.sweep().abs() {
                traveled / c.sweep().abs()
            } else if traveled - c.sweep().abs() < TAU - traveled {
                1.0 + (traveled - c.sweep().abs()) / c.sweep().abs()
            } else {
                -(TAU - traveled) / c.sweep().abs()
            };
            (q.norm() - c.radius(), fraction)
        }
        _ => panic!("not a segment, circle or arc"),
    }
}

fn direction(c: &Curve2, t: f64) -> Vector2 {
    let (lo, hi) = c.parameter_range();
    let h = 1e-6 * (hi - lo);
    c.point_at((t + h).min(hi)) - c.point_at((t - h).max(lo))
}

proptest! {
    #[test]
    fn intersection_is_symmetric(a in curve(), b in curve()) {
        match (intersect(&a, &b, &TOL), intersect(&b, &a, &TOL)) {
            (Ok(ab), Ok(ba)) => {
                let mut swapped: Vec<_> = ba.iter().map(Intersection::swapped).collect();
                sort(&mut swapped);
                prop_assert_eq!(ab, swapped);
            }
            (Err(x), Err(y)) => prop_assert_eq!(x.code(), y.code()),
            (x, y) => prop_assert!(false, "{x:?} versus {y:?}"),
        }
    }

    #[test]
    fn reported_points_lie_on_both_curves(a in curve(), b in curve()) {
        let Ok(result) = intersect(&a, &b, &TOL) else { return Ok(()) };
        for hit in result {
            match hit {
                Intersection::Point { point, a: ta, b: tb, .. } => {
                    prop_assert!((point - a.point_at(ta)).norm() <= TOL.linear, "{hit:?}");
                    prop_assert!((point - b.point_at(tb)).norm() <= TOL.linear, "{hit:?}");
                }
                Intersection::Overlap { a: ia, b: ib, orientation } => {
                    let ib = if orientation == Orientation::Same { ib } else { [ib[1], ib[0]] };
                    for k in 0..2 {
                        if ia[k].is_finite() && ib[k].is_finite() {
                            prop_assert!((a.point_at(ia[k]) - b.point_at(ib[k])).norm() <= 2.0 * TOL.linear);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn crossing_counts_match_dense_sampling(a in simple_curve(), b in simple_curve()) {
        let result = intersect(&a, &b, &TOL).unwrap();
        let (lo, hi) = b.parameter_range();
        let (alo, ahi) = a.parameter_range();
        let margin = 1e-3;
        // Skip pairs that sampling cannot resolve: tangencies, shallow crossings and
        // intersections near an end of either curve.
        for hit in &result {
            let Intersection::Point { a: ta, b: tb, contact, .. } = hit else {
                return Ok(());
            };
            let shallow = {
                let (u, v) = (direction(&a, *ta), direction(&b, *tb));
                ((u.x * v.y - u.y * v.x) / (u.norm() * v.norm())).abs() < 0.1
            };
            let near_end = |t: f64, lo: f64, hi: f64, closed: bool| {
                !closed && ((t - lo).abs() < margin * (hi - lo) || (hi - t).abs() < margin * (hi - lo))
            };
            if *contact == Contact::Tangent || shallow
                || near_end(*ta, alo, ahi, a.is_closed()) || near_end(*tb, lo, hi, b.is_closed()) {
                return Ok(());
            }
        }
        let n = 20_000;
        let mut count = 0;
        let mut previous = carrier(&a, &b.point_at(lo));
        for i in 1..=n {
            let next = carrier(&a, &b.point_at(lo + (hi - lo) * f64::from(i) / f64::from(n)));
            if previous.0.signum() != next.0.signum() {
                let middle = lo + (hi - lo) * (f64::from(i) - 0.5) / f64::from(n);
                let fraction = carrier(&a, &b.point_at(middle)).1;
                if fraction.abs() < margin || (fraction - 1.0).abs() < margin {
                    return Ok(());
                }
                if (0.0..=1.0).contains(&fraction) {
                    count += 1;
                }
            }
            previous = next;
        }
        prop_assert_eq!(result.len(), count, "{:?}", result);
    }
}

#[test]
fn quarter_turns_of_arcs_report_exact_end_parameters() {
    let quarter = Curve2::Arc(Arc2::new(Point2::origin(), 1.0, 0.0, FRAC_PI_2).unwrap());
    let result = intersect(&quarter, &seg(0.0, -2.0, 0.0, 2.0), &TOL).unwrap();
    assert!(matches!(result[..], [Intersection::Point { a, .. }] if a == FRAC_PI_2));
}
