use std::f64::consts::{FRAC_PI_2, PI, TAU};

use arc_base::math::{Point2, Vector2};
use proptest::prelude::*;

use super::*;
use crate::curve::tests::curve;
use crate::polyline::{Polyline2, PolylineVertex};
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2};

const TOL: Tolerance = Tolerance::DEFAULT;

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap())
}

#[test]
fn invalid_tolerances_are_rejected() {
    let s = seg(0.0, 0.0, 1.0, 1.0);
    for (linear, angular) in [
        (0.0, 1e-9),
        (-0.0, 1e-9),
        (-1.0, 1e-9),
        (f64::NAN, 1e-9),
        (f64::INFINITY, 1e-9),
        (1e-6, -1.0),
        (1e-6, f64::NAN),
        (1e-6, f64::INFINITY),
        (1e-6, FRAC_PI_2 * (1.0 + f64::EPSILON)),
        (1e-6, 10.0),
    ] {
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

#[test]
fn a_quarter_turn_angular_tolerance_is_accepted() {
    let tolerance = Tolerance {
        angular: FRAC_PI_2,
        ..TOL
    };
    let result = intersect(
        &seg(0.0, 0.0, 1.0, 1.0),
        &seg(0.0, 1.0, 1.0, 0.0),
        &tolerance,
    );
    assert!(result.is_ok());
}

fn poly(vertices: &[(f64, f64, f64)], closed: bool) -> Curve2 {
    let vertices = vertices
        .iter()
        .map(|&(x, y, bulge)| PolylineVertex::new(Point2::new(x, y), bulge))
        .collect();
    Curve2::Polyline(Polyline2::new(vertices, closed).unwrap())
}

fn ellipse(x: f64, y: f64, ax: f64, ay: f64, ratio: f64) -> Ellipse2 {
    Ellipse2::new(Point2::new(x, y), Vector2::new(ax, ay), ratio).unwrap()
}

/// Every reported point lies on both curves within the linear tolerance.
fn assert_on_both(a: &Curve2, b: &Curve2, result: &[Intersection]) {
    for hit in result {
        if let Intersection::Point {
            point,
            a: ta,
            b: tb,
            ..
        } = hit
        {
            assert!((point - a.point_at(*ta)).norm() <= TOL.linear, "{hit:?}");
            assert!((point - b.point_at(*tb)).norm() <= TOL.linear, "{hit:?}");
        }
    }
}

fn points(result: &[Intersection]) -> Vec<(Point2, f64, f64, Contact)> {
    result
        .iter()
        .filter_map(|r| match *r {
            Intersection::Point {
                point,
                a,
                b,
                contact,
            } => Some((point, a, b, contact)),
            Intersection::Overlap { .. } => None,
        })
        .collect()
}

// Regression: a 4e-7 m segment made the parametric tolerance of the whole polyline 2.5, so the
// two visits of the polyline to its self-crossing were merged at the averaged parameter 1.5,
// half a meter from both curves.
#[test]
fn a_tiny_polyline_segment_does_not_merge_distinct_visits() {
    let a = poly(
        &[
            (-1.0, -1.0, 0.0),
            (1.0, 1.0, 0.0),
            (1.0, -1.0, 0.0),
            (-1.0, 1.0, 0.0),
            (-1.0, 1.0 + 4e-7, 0.0),
        ],
        false,
    );
    let b = seg(0.0, -2.0, 0.0, 2.0);
    let result = intersect(&a, &b, &TOL).unwrap();
    assert_on_both(&a, &b, &result);
    let found = points(&result);
    assert_eq!(found.len(), 2, "{result:?}");
    assert_eq!((found[0].1, found[1].1), (0.5, 2.5));
}

// Regression: an ellipse with a minor axis of 2e-7 had a parametric tolerance of 5 radians, so
// its two crossings with a segment were merged at the averaged angle π, 1.5 m from both curves.
#[test]
fn crossings_of_a_thin_ellipse_are_not_averaged() {
    for ratio in [1e-7, 1e-8, 1e-9] {
        let thin = Curve2::Ellipse(ellipse(0.0, 0.0, 2.0, 0.0, ratio));
        for other in [seg(1.0, -1.0, 1.0, 1.0), circle(0.0, 0.0, 1.0)] {
            let result = intersect(&thin, &other, &TOL).unwrap();
            assert_on_both(&thin, &other, &result);
            assert!(!result.is_empty());
            assert!(
                points(&result)
                    .iter()
                    .all(|p| (p.0.x.abs() - 1.0).abs() < 1e-6),
                "{result:?}"
            );
        }
    }
}

fn circle(x: f64, y: f64, r: f64) -> Curve2 {
    Curve2::Circle(Circle2::new(Point2::new(x, y), r).unwrap())
}

// Regression: a crossing 0.6 m beyond the end of an overlap was dropped as inside it, because
// a 5e-7 m segment on each curve made their parametric tolerances 2 parameter units.
#[test]
fn a_crossing_next_to_an_overlap_is_kept() {
    let a = poly(
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 5e-7, 0.0)],
        false,
    );
    let b = poly(
        &[
            (-1.0, 0.0, 0.0),
            (500.0, 0.0, 0.0),
            (500.4, 1.0, 0.0),
            (500.8, -1.0, 0.0),
            (500.8, -1.0 + 5e-7, 0.0),
        ],
        false,
    );
    let result = intersect(&a, &b, &TOL).unwrap();
    assert_on_both(&a, &b, &result);
    assert_eq!(result.len(), 2, "{result:?}");
    let found = points(&result);
    assert_eq!(found.len(), 1);
    assert!((found[0].0 - Point2::new(500.6, 0.0)).norm() < 1e-9);
}

// Regression: two collinear stretches 0.5 m apart were joined into one overlap across the gap,
// for the same reason.
#[test]
fn overlaps_separated_by_a_gap_are_not_joined() {
    let a = poly(
        &[(0.0, 0.0, 0.0), (1000.0, 0.0, 0.0), (1000.0, 2e-7, 0.0)],
        false,
    );
    let c = poly(
        &[
            (100.0, 0.0, 0.0),
            (200.0, 0.0, 0.0),
            (200.25, 0.5, 0.0),
            (200.5, 0.0, 0.0),
            (300.0, 0.0, 0.0),
            (300.0, 2e-7, 0.0),
        ],
        false,
    );
    let result = intersect(&a, &c, &TOL).unwrap();
    let overlaps: Vec<_> = result
        .iter()
        .filter(|r| matches!(r, Intersection::Overlap { .. }))
        .collect();
    assert_eq!(overlaps.len(), 2, "{result:?}");
}

#[test]
fn overlaps_of_a_retracing_polyline_join_across_other_overlaps() {
    let p = poly(&[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 0.0, 0.0)], false);
    let result = intersect(&p, &p, &TOL).unwrap();
    assert_eq!(
        result,
        vec![
            Intersection::Overlap {
                a: [0.0, 2.0],
                b: [0.0, 2.0],
                orientation: Orientation::Same
            },
            Intersection::Overlap {
                a: [0.0, 2.0],
                b: [0.0, 2.0],
                orientation: Orientation::Opposite
            },
        ]
    );
}

// Regression: crossings within the linear tolerance of the seams of two nearly coincident
// ellipses were snapped to the seams before the crossing angle was taken, where both tangents
// are exactly parallel, and the operation failed with an error bound of 72.
#[test]
fn crossings_near_the_seam_use_the_unsnapped_angle() {
    let e = Curve2::Ellipse(ellipse(0.0, 0.0, 2.0, 0.0, 0.5));
    let shifted = Curve2::Ellipse(ellipse(0.0, 1.1e-6, 2.0, 0.0, 0.5));
    let result = intersect(&e, &shifted, &TOL).unwrap();
    assert_on_both(&e, &shifted, &result);
    let found = points(&result);
    assert_eq!(found.len(), 2, "{result:?}");
    assert!(found.iter().all(|p| p.3 == Contact::Crossing));
    // The same pair with the seams away from the crossings.
    let arc = |e: &Ellipse2| Curve2::EllipticalArc(EllipticalArc2::new(*e, 1.0, TAU).unwrap());
    let moved = intersect(
        &arc(&ellipse(0.0, 0.0, 2.0, 0.0, 0.5)),
        &arc(&ellipse(0.0, 1.1e-6, 2.0, 0.0, 0.5)),
        &TOL,
    )
    .unwrap();
    assert_eq!(points(&moved).len(), 2);
}

// Regression: a bulge whose arc has a radius of about 5e9 made every intersection with the
// polyline fail with `geom.precision.insufficient`, even with a curve 50 m away.
#[test]
fn nearly_straight_bulges_are_intersected_accurately() {
    for offset in [0.0, 1e5] {
        for bulge in [1e-1, 1e-3, 1e-6, 1e-8, 1e-9, 1e-10, 1e-12, -1e-12] {
            let a = poly(
                &[(offset, offset, bulge), (offset + 1.0, offset, 0.0)],
                false,
            );
            let b = seg(offset + 0.5, offset - 1.0, offset + 0.5, offset + 1.0);
            let result = intersect(&a, &b, &TOL).unwrap();
            assert_on_both(&a, &b, &result);
            assert_eq!(points(&result).len(), 1, "{bulge}: {result:?}");
            let far = seg(offset + 50.0, offset + 50.0, offset + 51.0, offset + 51.0);
            assert!(intersect(&a, &far, &TOL).unwrap().is_empty());
        }
    }
    // Two nearly straight bulges crossing each other.
    for bulge in [1e-3, 1e-6, 1e-9, 1e-12] {
        let a = poly(&[(0.0, 0.0, bulge), (1.0, 0.0, 0.0)], false);
        let b = poly(&[(0.0, 0.1, -bulge), (1.0, -0.1, 0.0)], false);
        let result = intersect(&a, &b, &TOL).unwrap();
        assert_on_both(&a, &b, &result);
        assert_eq!(result.len(), 1, "{bulge}: {result:?}");
    }
}

// A 1 km chord with a bulge of 1e-8 has a sagitta of 5 µm and a radius of 2.5e10: it is split
// into chords instead of using the center-radius form.
#[test]
fn a_long_flat_bulge_is_split_into_chords() {
    let a = poly(&[(0.0, 0.0, 1e-8), (1000.0, 0.0, 0.0)], false);
    for x in [1.0, 250.0, 500.0, 999.0] {
        let b = seg(x, -1.0, x, 1.0);
        let result = intersect(&a, &b, &TOL).unwrap();
        assert_on_both(&a, &b, &result);
        assert_eq!(points(&result).len(), 1, "{x}: {result:?}");
    }
    let result = intersect(&a, &a, &TOL).unwrap();
    assert_eq!(
        result,
        vec![Intersection::Overlap {
            a: [0.0, 1.0],
            b: [0.0, 1.0],
            orientation: Orientation::Same
        }]
    );
}

fn comb(n: usize, vertical: bool, shift: f64) -> Curve2 {
    let step = 2.0 / n as f64;
    let vertices: Vec<(f64, f64, f64)> = (0..=n)
        .map(|i| {
            let along = (i / 2) as f64 * step + shift;
            let across = if i % 4 == 1 || i % 4 == 2 { 1.0 } else { 0.0 };
            if vertical {
                (along, across, 0.0)
            } else {
                (across, along, 0.0)
            }
        })
        .collect();
    poly(&vertices, false)
}

#[test]
fn many_crossings_are_found_through_the_index_and_limited() {
    // Two meanders of 120 segments: 60 rungs each, crossing in a 30 × 30 grid.
    let (a, b) = (comb(120, false, 3e-3), comb(120, true, 7e-3));
    let result = intersect(&a, &b, &TOL).unwrap();
    assert_on_both(&a, &b, &result);
    let rungs = |c: &Curve2| match c {
        Curve2::Polyline(p) => p
            .segments()
            .filter(|s| match s {
                crate::polyline::PolylineSegment::Line(l) => {
                    let d = l.end() - l.start();
                    d.x.abs() > 0.5 || d.y.abs() > 0.5
                }
                crate::polyline::PolylineSegment::Arc(_) => false,
            })
            .count(),
        _ => 0,
    };
    assert_eq!(result.len(), rungs(&a) * rungs(&b));
    let limits = Limits {
        array_length: 100,
        ..Limits::DEFAULT
    };
    let error = intersect_with_limits(&a, &b, &TOL, &limits).unwrap_err();
    assert_eq!(error.code(), "geom.intersect.too_many_results");
    let enough = Limits {
        array_length: result.len() as u64,
        ..Limits::DEFAULT
    };
    assert_eq!(
        intersect_with_limits(&a, &b, &TOL, &enough).unwrap(),
        result
    );
}

/// A log-uniform magnitude in [10^lo, 10^hi] with a random sign.
fn signed_magnitude(lo: f64, hi: f64) -> impl Strategy<Value = f64> {
    (lo..hi, any::<bool>()).prop_map(|(e, negative)| {
        let m = 10f64.powf(e);
        if negative { -m } else { m }
    })
}

fn small_point() -> impl Strategy<Value = Point2> {
    (-5.0..5.0_f64, -5.0..5.0_f64).prop_map(|(x, y)| Point2::new(x, y))
}

fn heading() -> impl Strategy<Value = Vector2> {
    (0.0..TAU, 0.5..3.0_f64).prop_map(|(a, r)| Vector2::new(a.cos() * r, a.sin() * r))
}

/// Curves of all eight kinds near the origin. `thinnest` is the smallest ellipse ratio and
/// `shortest` the smallest polyline segment length, both log-uniform; bulges are 0 or
/// log-uniform in magnitude between 1e-12 and 1e3.
fn any_curve(thinnest: f64, shortest: f64) -> impl Strategy<Value = Curve2> {
    let ratio = (thinnest.log10()..=0.0).prop_map(|e| 10f64.powf(e));
    let bulge = prop_oneof![Just(0.0), signed_magnitude(-12.0, 3.0)];
    let step = (0.0..TAU, shortest.log10()..1.0_f64)
        .prop_map(|(a, e)| Vector2::new(a.cos(), a.sin()) * 10f64.powf(e));
    let polyline = (
        small_point(),
        prop::collection::vec((step, bulge), 1..7),
        any::<bool>(),
    )
        .prop_filter_map("valid polyline", |(start, steps, closed)| {
            let mut at = start;
            let mut vertices = Vec::new();
            for (d, b) in steps {
                vertices.push(PolylineVertex::new(at, b));
                at += d;
            }
            vertices.push(PolylineVertex::new(at, 0.0));
            Polyline2::new(vertices, closed).ok()
        });
    let sweep = (0.2..=TAU, any::<bool>()).prop_map(|(m, ccw)| if ccw { m } else { -m });
    prop_oneof![
        (small_point(), heading(), shortest.log10()..1.0_f64).prop_filter_map(
            "segment",
            |(p, d, e)| Segment2::new(p, p + d * 10f64.powf(e))
                .ok()
                .map(Curve2::Segment)
        ),
        (small_point(), heading()).prop_map(|(p, d)| Curve2::Ray(Ray2::new(p, d).unwrap())),
        (small_point(), heading()).prop_map(|(p, d)| Curve2::Line(Line2::new(p, d).unwrap())),
        (small_point(), 0.3..5.0_f64).prop_map(|(c, r)| circle(c.x, c.y, r)),
        (small_point(), 0.3..5.0_f64, -10.0..10.0_f64, sweep.clone())
            .prop_map(|(c, r, a, s)| Curve2::Arc(Arc2::new(c, r, a, s).unwrap())),
        (small_point(), heading(), ratio.clone())
            .prop_map(|(c, u, k)| Curve2::Ellipse(Ellipse2::new(c, u, k).unwrap())),
        (small_point(), heading(), ratio, -10.0..10.0_f64, sweep).prop_map(|(c, u, k, a, s)| {
            Curve2::EllipticalArc(
                EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap(),
            )
        }),
        polyline.prop_map(Curve2::Polyline),
    ]
}

/// Parameters and points densely sampled along a curve; rays and lines are cut at 40 from
/// their reference point.
fn samples(c: &Curve2) -> Vec<(f64, Point2)> {
    let (lo, hi) = c.parameter_range();
    let (lo, hi) = (lo.max(-40.0), hi.min(40.0));
    let per_unit: f64 = match c {
        Curve2::Polyline(_) => 300.0,
        Curve2::Ray(_) | Curve2::Line(_) => 2.0,
        _ => 2000.0 / (hi - lo),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive sample count"
    )]
    let n = ((hi - lo) * per_unit).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let t = lo + (hi - lo) * i as f64 / n as f64;
            (t, c.point_at(t))
        })
        .collect()
}

/// Parameter pairs where the sampled polygons of two curves cross.
fn sampled_crossings(a: &[(f64, Point2)], b: &[(f64, Point2)]) -> Vec<(f64, f64)> {
    let boxed = |w: &[(f64, Point2)]| {
        let (p, q) = (w[0].1, w[1].1);
        (p.x.min(q.x), p.x.max(q.x), p.y.min(q.y), p.y.max(q.y))
    };
    let mut b_edges: Vec<(usize, (f64, f64, f64, f64))> =
        b.windows(2).map(boxed).enumerate().collect();
    b_edges.sort_by(|x, y| x.1.0.total_cmp(&y.1.0));
    let widest = b_edges.iter().map(|e| e.1.1 - e.1.0).fold(0.0, f64::max);
    let mut found = Vec::new();
    for (i, w) in a.windows(2).enumerate() {
        let (x0, x1, y0, y1) = boxed(w);
        let first = b_edges.partition_point(|e| e.1.0 < x0 - widest);
        for &(j, (bx0, bx1, by0, by1)) in &b_edges[first..] {
            if bx0 > x1 {
                break;
            }
            if bx1 < x0 || by1 < y0 || by0 > y1 {
                continue;
            }
            let (p0, p1, q0, q1) = (w[0].1, w[1].1, b[j].1, b[j + 1].1);
            let (r, s, d) = (p1 - p0, q1 - q0, q0 - p0);
            let den = r.x * s.y - r.y * s.x;
            if den == 0.0 {
                continue;
            }
            let u = (d.x * s.y - d.y * s.x) / den;
            let v = (d.x * r.y - d.y * r.x) / den;
            if (0.0..1.0).contains(&u) && (0.0..1.0).contains(&v) {
                found.push((
                    a[i].0 + u * (a[i + 1].0 - a[i].0),
                    b[j].0 + v * (b[j + 1].0 - b[j].0),
                ));
            }
        }
    }
    found
}

/// Newton steps from a sampled crossing to a true one: the crossing point and the sine of the
/// crossing angle, or `None` when the iteration does not settle inside both ranges.
fn refine(a: &Curve2, b: &Curve2, mut ta: f64, mut tb: f64) -> Option<(Point2, f64)> {
    let derivative = |c: &Curve2, t: f64| {
        let (lo, hi) = c.parameter_range();
        let h = 1e-7 * (1.0 + t.abs());
        let (t0, t1) = ((t - h).max(lo), (t + h).min(hi));
        (c.point_at(t1) - c.point_at(t0)) / (t1 - t0)
    };
    for _ in 0..30 {
        let f = a.point_at(ta) - b.point_at(tb);
        let (da, db) = (derivative(a, ta), derivative(b, tb));
        let det = da.x * -db.y + db.x * da.y;
        if det == 0.0 {
            return None;
        }
        // Solve [da, −db]·(δa, δb) = −f.
        let dta = -(f.x * -db.y + db.x * f.y) / det;
        let dtb = -(da.x * f.y - da.y * f.x) / det;
        ta += dta;
        tb += dtb;
        let (alo, ahi) = a.parameter_range();
        let (blo, bhi) = b.parameter_range();
        if !(alo..=ahi).contains(&ta) || !(blo..=bhi).contains(&tb) {
            return None;
        }
    }
    let (pa, pb) = (a.point_at(ta), b.point_at(tb));
    let (da, db) = (derivative(a, ta), derivative(b, tb));
    let sine = ((da.x * db.y - da.y * db.x) / (da.norm() * db.norm())).abs();
    ((pa - pb).norm() < 1e-9).then_some((pa, sine))
}

fn ends(c: &Curve2) -> Vec<Point2> {
    let (lo, hi) = c.parameter_range();
    if c.is_closed() {
        return Vec::new();
    }
    [lo, hi]
        .into_iter()
        .filter(|t| t.is_finite())
        .map(|t| c.point_at(t))
        .collect()
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
    fn wide_curves_give_symmetric_results_on_both_curves(
        a in any_curve(1e-6, 1e-7),
        b in any_curve(1e-6, 1e-7),
    ) {
        let (ab, ba) = (intersect(&a, &b, &TOL), intersect(&b, &a, &TOL));
        match (&ab, &ba) {
            (Ok(ab), Ok(ba)) => {
                let mut swapped: Vec<_> = ba.iter().map(Intersection::swapped).collect();
                sort(&mut swapped);
                prop_assert_eq!(ab, &swapped);
            }
            (Err(x), Err(y)) => {
                prop_assert_eq!(x.code(), y.code());
                prop_assert_eq!(x.code(), "geom.precision.insufficient");
            }
            (x, y) => prop_assert!(false, "{x:?} versus {y:?}"),
        }
        let Ok(result) = ab else { return Ok(()) };
        for hit in result {
            match hit {
                Intersection::Point { point, a: ta, b: tb, .. } => {
                    prop_assert!((point - a.point_at(ta)).norm() <= TOL.linear, "{hit:?}");
                    prop_assert!((point - b.point_at(tb)).norm() <= TOL.linear, "{hit:?}");
                }
                Intersection::Overlap { a: ia, b: ib, orientation } => {
                    prop_assert!(ia[0] <= ia[1] && ib[0] <= ib[1], "{hit:?}");
                    let ib = if orientation == Orientation::Same { ib } else { [ib[1], ib[0]] };
                    for k in 0..2 {
                        if ia[k].is_finite() && ib[k].is_finite() {
                            prop_assert!(
                                (a.point_at(ia[k]) - b.point_at(ib[k])).norm() <= 2.0 * TOL.linear,
                                "{hit:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    // Every crossing that dense sampling finds, refined by Newton steps on the curves and away
    // from curve ends and not shallow, is reported. Ellipses are kept at a ratio of 1e-3 or more
    // and polyline segments at 1e-3 or longer so that the sampled polygons resolve them;
    // thinner curves are covered by the property above.
    #[test]
    fn every_sampled_crossing_is_reported(
        a in any_curve(1e-3, 1e-3),
        b in any_curve(1e-3, 1e-3),
    ) {
        let result = match intersect(&a, &b, &TOL) {
            Ok(result) => result,
            Err(error) => {
                prop_assert_eq!(error.code(), "geom.precision.insufficient");
                return Ok(());
            }
        };
        if result
            .iter()
            .any(|r| !matches!(r, Intersection::Point { contact: Contact::Crossing, .. }))
        {
            return Ok(());
        }
        let near_end = |x: &Point2| {
            ends(&a).iter().chain(ends(&b).iter()).any(|e| (e - x).norm() < 1e-3)
        };
        for (ta, tb) in sampled_crossings(&samples(&a), &samples(&b)) {
            let Some((x, sine)) = refine(&a, &b, ta, tb) else { continue };
            if near_end(&x) || sine < 0.05 {
                continue;
            }
            prop_assert!(
                result.iter().any(|r| matches!(
                    r,
                    Intersection::Point { point, .. } if (point - x).norm() <= 2.0 * TOL.linear / sine
                )),
                "sampled crossing at {x:?} missing from {result:?}"
            );
        }
    }
}

// Regression: the half chord of two crossing circles was computed from the center of the first
// one; for a circle of radius 1.7e6 crossed by one of radius 4.4 at 66 degrees, the reported
// point was 2e-5 from both curves although the error bound claimed 7e-9.
#[test]
fn a_small_circle_crossing_a_huge_one_is_accurate() {
    let huge = circle(4.260540506942471, 1663901.8377620345, 1663905.669950165);
    let small = circle(-3.5973507549789647, -2.086912538155849, 4.43988966301487);
    let result = intersect(&huge, &small, &TOL).unwrap();
    assert_eq!(points(&result).len(), 2, "{result:?}");
    assert_on_both(&huge, &small, &result);
    let a = poly(
        &[
            (-0.8582676827397954, -3.832180794604453, 0.0),
            (
                -0.6804212867150927,
                -3.832180794604453,
                1.4847481689921881e-6,
            ),
            (9.201502300600033, -3.832180794604453, 0.0),
        ],
        false,
    );
    let b = poly(
        &[
            (0.0, -4.689159214664163, 17.703071278525993),
            (-0.6736225985732175, -5.42823471465822, 0.0),
        ],
        false,
    );
    let result = intersect(&a, &b, &TOL).unwrap();
    assert_eq!(points(&result).len(), 1, "{result:?}");
    assert_on_both(&a, &b, &result);
}
