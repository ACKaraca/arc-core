//! Closest point and projection (CORE-GEOM-004) for every 2D curve kind.
//!
//! [`CurveProject`] is implemented by every concrete curve type and by [`crate::Curve2`].
//!
//! - A foot point is a parameter inside the range where the vector from the curve to the query
//!   point is perpendicular to the tangent within `tolerance.linear` (the component of that
//!   vector along the unit tangent is at most `tolerance.linear`). Ends of the range are foot
//!   points only when they are perpendicular. At a polyline vertex a foot point may be
//!   perpendicular to either adjacent segment.
//! - When every point of a circle, arc or ellipse is a foot point (the query is at the center of
//!   a circle or arc, or of an ellipse that is a circle within the tolerance) the result is one
//!   [`FootPoint::Interval`] covering the whole piece instead of a sample of points.
//! - Ellipse foot points are the zeros of `f(θ) = (a² − b²)·sin θ·cos θ − a·u·sin θ + b·v·cos θ`
//!   (query `(u, v)` in the frame of the axes). Dividing by `sin θ·cos θ` gives a function whose
//!   derivative vanishes only where `a·u·sin³ θ + b·v·cos³ θ = 0`, which has two solutions half
//!   a turn apart. Splitting the turn at the quadrant angles and those two angles leaves at most
//!   six intervals on each of which `f` has at most one zero, found by a sign change and refined
//!   by safeguarded Newton iteration; a zero at a split angle (a tangent, double zero included)
//!   is found by testing the split angle itself.
//! - The closest point is chosen among the foot points and the ends of the range (and the
//!   vertices of a polyline). Candidates whose distances differ by at most `tolerance.linear`
//!   tie, and the one with the smallest parameter wins; the result is within
//!   `tolerance.linear` of the true minimum distance.

use std::f64::consts::{FRAC_PI_2, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::{
    Conic, MAX_ROOT_ITERATIONS, Piece, Shape, Shaped, ensure_tolerance, global_parameter,
    segment_piece, unit,
};
use crate::polyline::Polyline2;
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2, perp};

/// A point on a curve with its parameter and its distance to a query point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    /// The parameter on the curve.
    pub parameter: f64,
    /// The point at the parameter.
    pub point: Point2,
    /// The distance from the query point.
    pub distance: f64,
}

/// A perpendicular foot of a query point on a curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FootPoint {
    /// One foot point.
    Point(CurvePoint),
    /// Every parameter in [start, end] is a foot point: the query is the center of a circle, an
    /// arc or a polyline arc segment. `distance` is the distance at `start`.
    Interval {
        /// The first parameter of the interval.
        start: f64,
        /// The last parameter of the interval.
        end: f64,
        /// The distance from the query point to the point at `start`.
        distance: f64,
    },
}

impl FootPoint {
    /// The first parameter: the parameter of a point or the start of an interval.
    #[must_use]
    pub fn parameter(&self) -> f64 {
        match self {
            Self::Point(p) => p.parameter,
            Self::Interval { start, .. } => *start,
        }
    }
}

/// Closest point and perpendicular projection onto a 2D curve (CORE-GEOM-004), implemented by
/// every curve kind and [`crate::Curve2`].
pub trait CurveProject: Shaped {
    /// The closest point of the curve to `query` and its parameter. Ties within
    /// `tolerance.linear` go to the smallest parameter, so the center of a circle yields the
    /// point at parameter 0.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite query; `geom.tolerance.invalid` when
    /// `tolerance.linear` is not finite and positive; `geom.precision.insufficient` when a foot
    /// point cannot be located to the tolerance in `f64`.
    fn closest_point(&self, query: Point2, tolerance: &Tolerance) -> Result<CurvePoint> {
        closest(&self.shape(), query, tolerance)
    }

    /// Every perpendicular foot point of `query` inside the parameter range, sorted by
    /// parameter, with foot points closer than `tolerance.linear` to each other merged.
    ///
    /// # Errors
    ///
    /// As [`Self::closest_point`].
    fn foot_points(&self, query: Point2, tolerance: &Tolerance) -> Result<Vec<FootPoint>> {
        feet(&self.shape(), query, tolerance)
    }
}

impl CurveProject for Segment2 {}
impl CurveProject for Ray2 {}
impl CurveProject for Line2 {}
impl CurveProject for Circle2 {}
impl CurveProject for Arc2 {}
impl CurveProject for Ellipse2 {}
impl CurveProject for EllipticalArc2 {}
impl CurveProject for Polyline2 {}
impl CurveProject for Curve2 {}

fn curve_point(shape: &Shape<'_>, parameter: f64, query: Point2) -> CurvePoint {
    let point = shape.point(parameter);
    CurvePoint {
        parameter,
        point,
        distance: (query - point).norm(),
    }
}

fn piece_point(piece: &Piece, parameter: f64, query: Point2) -> CurvePoint {
    curve_point(&Shape::Piece(*piece), parameter, query)
}

/// Foot points on one smooth piece, in its own parameter.
fn piece_feet(piece: &Piece, query: Point2, tolerance: &Tolerance) -> Result<Vec<FootPoint>> {
    let (lo, hi) = piece.range();
    if let Some((origin, direction)) = piece.linear() {
        let speed = direction.norm();
        let t = (query - origin).dot(&unit(direction)) / speed;
        let slack = tolerance.linear / speed;
        if t < lo - slack || t > hi + slack {
            return Ok(Vec::new());
        }
        return Ok(vec![FootPoint::Point(piece_point(
            piece,
            t.clamp(lo, hi),
            query,
        ))]);
    }
    let Some(conic) = piece.conic() else {
        return Ok(Vec::new());
    };
    let Some(angles) = conic_feet(&conic, query, tolerance)? else {
        return Ok(vec![FootPoint::Interval {
            start: lo,
            end: hi,
            distance: (query - piece.point(lo)).norm(),
        }]);
    };
    let span = conic.span();
    let slack = tolerance.linear / conic.a.norm().max(conic.b.norm());
    let mut out: Vec<FootPoint> = angles
        .into_iter()
        .filter_map(|angle| {
            let t = ((angle - conic.start) * conic.sweep.signum()).rem_euclid(TAU);
            let t = if t <= span {
                t
            } else if t - span <= slack {
                span
            } else if TAU - t <= slack {
                0.0
            } else {
                return None;
            };
            Some(FootPoint::Point(piece_point(piece, t, query)))
        })
        .collect();
    out.sort_by(|x, y| x.parameter().total_cmp(&y.parameter()));
    Ok(out)
}

/// The eccentric angles in [0, 2π) of the foot points of `query` on the full ellipse of
/// `conic`, or `None` when every point is a foot point.
fn conic_feet(conic: &Conic, query: Point2, tolerance: &Tolerance) -> Result<Option<Vec<f64>>> {
    let (a, b) = (conic.a.norm(), conic.b.norm());
    let major = unit(conic.a);
    let d = query - conic.center;
    let (u, v) = (d.dot(&major), d.dot(&perp(&major)));
    let c = (a - b) * (a + b);
    // The tangential component of query − point is at most |d| + c / (2b) everywhere.
    if d.norm() + c / (2.0 * b) <= tolerance.linear {
        return Ok(None);
    }
    let f = |t: f64| {
        let (sin, cos) = t.sin_cos();
        c * sin * cos - a * u * sin + b * v * cos
    };
    let fp = |t: f64| {
        let (sin, cos) = t.sin_cos();
        c * (cos - sin) * (cos + sin) - a * u * cos - b * v * sin
    };
    let speed = |t: f64| {
        let (sin, cos) = t.sin_cos();
        Vector2::new(a * sin, b * cos).norm()
    };
    let mut cuts = vec![0.0, FRAC_PI_2, std::f64::consts::PI, 3.0 * FRAC_PI_2, TAU];
    let (p, q) = ((b * v).cbrt(), (a * u).cbrt());
    if p != 0.0 || q != 0.0 {
        let turn = (-p).atan2(q);
        cuts.push(turn.rem_euclid(TAU));
        cuts.push((turn + std::f64::consts::PI).rem_euclid(TAU));
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut angles = Vec::new();
    for pair in cuts.windows(2) {
        let &[lo, hi] = pair else { continue };
        let (f_lo, f_hi) = (f(lo), f(hi));
        if f_lo.abs() <= tolerance.linear * speed(lo) {
            angles.push(lo);
        }
        if f_lo * f_hi < 0.0 {
            angles.push(refine(lo, hi, f_lo, &f, &fp, &speed, tolerance)?);
        }
    }
    Ok(Some(angles))
}

/// The zero of `f` in (lo, hi), where `f(lo)` and `f(hi)` have opposite signs, by Newton
/// iteration kept inside a shrinking bracket.
fn refine(
    mut lo: f64,
    mut hi: f64,
    f_lo: f64,
    f: &impl Fn(f64) -> f64,
    fp: &impl Fn(f64) -> f64,
    speed: &impl Fn(f64) -> f64,
    tolerance: &Tolerance,
) -> Result<f64> {
    let mut t = 0.5 * (lo + hi);
    for _ in 0..MAX_ROOT_ITERATIONS {
        let value = f(t);
        if value.abs() <= 0.5 * tolerance.linear * speed(t) {
            return Ok(t);
        }
        if (value < 0.0) == (f_lo < 0.0) {
            lo = t;
        } else {
            hi = t;
        }
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            // The bracket cannot shrink further in f64.
            if value.abs() <= tolerance.linear * speed(t) {
                return Ok(t);
            }
            break;
        }
        let newton = t - value / fp(t);
        t = if newton > lo && newton < hi {
            newton
        } else {
            mid
        };
    }
    Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "foot_points"))
}

fn ensure_query(query: Point2, tolerance: &Tolerance) -> Result<()> {
    crate::ensure_finite(&[query.x, query.y])?;
    ensure_tolerance("linear", tolerance.linear)
}

fn feet(shape: &Shape<'_>, query: Point2, tolerance: &Tolerance) -> Result<Vec<FootPoint>> {
    ensure_query(query, tolerance)?;
    let raw = match shape {
        Shape::Piece(piece) => piece_feet(piece, query, tolerance)?,
        Shape::Polyline(polyline) => polyline_feet(polyline, query, tolerance)?,
    };
    let closed = match shape {
        Shape::Piece(piece) => piece.conic().is_some_and(|c| c.span() == TAU),
        Shape::Polyline(polyline) => polyline.is_closed(),
    };
    Ok(merge(shape, raw, closed, tolerance))
}

fn polyline_feet(
    polyline: &Polyline2,
    query: Point2,
    tolerance: &Tolerance,
) -> Result<Vec<FootPoint>> {
    let mut out = Vec::new();
    for (index, segment) in polyline.segments().enumerate() {
        let piece = segment_piece(segment);
        for foot in piece_feet(&piece, query, tolerance)? {
            out.push(match foot {
                FootPoint::Point(p) => FootPoint::Point(CurvePoint {
                    parameter: global_parameter(index, &piece, p.parameter),
                    ..p
                }),
                FootPoint::Interval {
                    start,
                    end,
                    distance,
                } => FootPoint::Interval {
                    start: global_parameter(index, &piece, start),
                    end: global_parameter(index, &piece, end),
                    distance,
                },
            });
        }
    }
    Ok(out)
}

/// Sorts foot points, joins touching intervals and drops points within the tolerance of an
/// interval or of the previous point (and, on a closed curve, of the first point).
fn merge(
    shape: &Shape<'_>,
    mut raw: Vec<FootPoint>,
    closed: bool,
    tol: &Tolerance,
) -> Vec<FootPoint> {
    raw.sort_by(|x, y| x.parameter().total_cmp(&y.parameter()));
    let intervals: Vec<(f64, f64)> = raw
        .iter()
        .filter_map(|f| match f {
            FootPoint::Interval { start, end, .. } => Some((*start, *end)),
            FootPoint::Point(_) => None,
        })
        .collect();
    let near = |a: Point2, b: Point2| (a - b).norm() <= tol.linear;
    let in_interval = |p: &CurvePoint| {
        intervals.iter().any(|&(s, e)| {
            (s..=e).contains(&p.parameter)
                || near(p.point, shape.point(s))
                || near(p.point, shape.point(e))
        })
    };
    let mut out: Vec<FootPoint> = Vec::with_capacity(raw.len());
    for foot in raw {
        match (out.last_mut(), foot) {
            (
                Some(FootPoint::Interval { end, .. }),
                FootPoint::Interval {
                    start, end: next, ..
                },
            ) if *end >= start => {
                *end = next;
            }
            (_, FootPoint::Point(p)) if in_interval(&p) => {}
            (Some(FootPoint::Point(prev)), FootPoint::Point(p)) if near(prev.point, p.point) => {}
            (_, foot) => out.push(foot),
        }
    }
    if closed
        && out.len() > 1
        && let (Some(FootPoint::Point(first)), Some(FootPoint::Point(last))) =
            (out.first(), out.last())
        && near(first.point, last.point)
    {
        out.pop();
    }
    out
}

fn closest(shape: &Shape<'_>, query: Point2, tolerance: &Tolerance) -> Result<CurvePoint> {
    let mut candidates: Vec<f64> = feet(shape, query, tolerance)?
        .iter()
        .map(FootPoint::parameter)
        .collect();
    let (lo, hi) = shape.range();
    match shape {
        Shape::Polyline(polyline) => {
            candidates.extend((0..=polyline.segment_count()).map(|k| k as f64));
        }
        Shape::Piece(_) => candidates.extend([lo, hi].into_iter().filter(|t| t.is_finite())),
    }
    candidates.sort_by(f64::total_cmp);
    let mut best: Option<CurvePoint> = None;
    for t in candidates {
        let candidate = curve_point(shape, t, query);
        if best.is_none_or(|b| candidate.distance < b.distance - tolerance.linear) {
            best = Some(candidate);
        }
    }
    best.ok_or_else(|| {
        Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "closest_point")
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::curve;
    use crate::eval::CurveEval;
    use crate::polyline::PolylineVertex;
    use crate::testing::point;

    const TOL: Tolerance = Tolerance::DEFAULT;

    /// Distance of the curve to `q` minimized over a dense sample and refined by golden-section
    /// search around the best sample.
    fn reference_distance(c: &Curve2, q: Point2) -> f64 {
        let (lo, hi) = c.parameter_range();
        let (lo, hi) = (lo.max(-500.0), hi.min(500.0));
        let n = 20_000;
        let h = (hi - lo) / f64::from(n);
        let dist = |t: f64| (c.point_at(t) - q).norm();
        let best = (0..=n)
            .map(|i| lo + h * f64::from(i))
            .min_by(|a, b| dist(*a).total_cmp(&dist(*b)))
            .unwrap();
        let (mut a, mut b) = ((best - h).max(lo), (best + h).min(hi));
        let g = 0.5 * (5.0_f64.sqrt() - 1.0);
        for _ in 0..200 {
            let (x, y) = (b - g * (b - a), a + g * (b - a));
            if dist(x) < dist(y) {
                b = y;
            } else {
                a = x;
            }
        }
        dist(best).min(dist(0.5 * (a + b)))
    }

    /// Whether the vector from the curve at `t` to `q` is perpendicular to the curve within the
    /// tolerance; at a polyline vertex either adjacent segment counts.
    fn perpendicular(c: &Curve2, t: f64, q: Point2) -> bool {
        let r = q - c.point_at(t);
        let ok = |tangent: Vector2| tangent.dot(&r).abs() <= TOL.linear;
        ok(c.tangent(t)) || (matches!(c, Curve2::Polyline(_)) && ok(c.tangent(t - 1e-12)))
    }

    #[test]
    fn center_of_a_circle_is_an_interval_and_closest_is_the_start() {
        let circle = Circle2::new(Point2::new(1.0, 2.0), 3.0).unwrap();
        let feet = circle.foot_points(Point2::new(1.0, 2.0), &TOL).unwrap();
        assert_eq!(
            feet,
            vec![FootPoint::Interval {
                start: 0.0,
                end: TAU,
                distance: 3.0
            }]
        );
        let closest = circle.closest_point(Point2::new(1.0, 2.0), &TOL).unwrap();
        assert_eq!(closest.parameter, 0.0);
        assert_eq!(closest.point, Point2::new(4.0, 2.0));
    }

    #[test]
    fn center_of_a_polyline_arc_is_an_interval_of_that_segment() {
        let p = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::new(0.0, 0.0), 0.0),
                PolylineVertex::new(Point2::new(2.0, 0.0), 1.0),
                PolylineVertex::new(Point2::new(4.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let feet = p.foot_points(Point2::new(3.0, 0.0), &TOL).unwrap();
        assert_eq!(
            feet,
            vec![FootPoint::Interval {
                start: 1.0,
                end: 2.0,
                distance: 1.0
            }]
        );
    }

    #[test]
    fn center_of_an_ellipse_has_the_four_vertices() {
        let e = Ellipse2::new(Point2::origin(), Vector2::new(2.0, 0.0), 0.5).unwrap();
        let params: Vec<f64> = e
            .foot_points(Point2::origin(), &TOL)
            .unwrap()
            .iter()
            .map(FootPoint::parameter)
            .collect();
        assert_eq!(params.len(), 4);
        for (got, want) in
            params
                .iter()
                .zip([0.0, FRAC_PI_2, std::f64::consts::PI, 3.0 * FRAC_PI_2])
        {
            assert!((got - want).abs() < 1e-12);
        }
        assert_eq!(
            e.closest_point(Point2::origin(), &TOL).unwrap().parameter,
            params[1]
        );
    }

    #[test]
    fn evolute_point_gives_a_double_foot() {
        // The center of curvature of the ellipse at its vertex (a, 0) is (a − b²/a, 0): the
        // feet at θ = 0 coincide there and are found as one.
        let (a, b) = (2.0, 1.0);
        let e = Ellipse2::new(Point2::origin(), Vector2::new(a, 0.0), b / a).unwrap();
        let feet = e
            .foot_points(Point2::new(a - b * b / a, 0.0), &TOL)
            .unwrap();
        assert_eq!(feet.len(), 2);
        assert!(feet[0].parameter().abs() < 1e-6);
    }

    #[test]
    fn degenerate_queries_are_rejected() {
        let s = Segment2::new(Point2::origin(), Point2::new(1.0, 0.0)).unwrap();
        let bad = Point2::new(f64::NAN, 0.0);
        assert_eq!(
            s.closest_point(bad, &TOL).unwrap_err().code(),
            "geom.input.not_finite"
        );
        let tol = Tolerance {
            linear: -1.0,
            ..TOL
        };
        assert_eq!(
            s.foot_points(Point2::origin(), &tol).unwrap_err().code(),
            "geom.tolerance.invalid"
        );
        assert!(
            s.foot_points(Point2::new(2.0, 1.0), &TOL)
                .unwrap()
                .is_empty()
        );
        let ray = Ray2::new(Point2::origin(), Vector2::x()).unwrap();
        assert_eq!(
            ray.closest_point(Point2::new(-3.0, 1.0), &TOL)
                .unwrap()
                .parameter,
            0.0
        );
    }

    proptest! {
        #[test]
        fn closest_point_matches_dense_sampling(c in curve(), q in point()) {
            let found = c.closest_point(q, &TOL).unwrap();
            prop_assert!((c.point_at(found.parameter) - found.point).norm() == 0.0);
            prop_assert!(((q - found.point).norm() - found.distance).abs() == 0.0);
            let reference = reference_distance(&c, q);
            prop_assert!(found.distance <= reference + TOL.linear, "{} vs {reference}", found.distance);
            prop_assert!(found.distance >= reference - TOL.linear - 1e-9, "{} vs {reference}", found.distance);
        }

        #[test]
        fn foot_points_are_perpendicular_and_complete(c in curve(), q in point()) {
            let feet = c.foot_points(q, &TOL).unwrap();
            let (lo, hi) = c.parameter_range();
            for foot in &feet {
                match foot {
                    FootPoint::Point(p) => {
                        prop_assert!(p.parameter >= lo && p.parameter <= hi);
                        prop_assert!(perpendicular(&c, p.parameter, q), "{foot:?}");
                    }
                    FootPoint::Interval { start, end, .. } => {
                        for i in 0..=10 {
                            let t = start + (end - start) * f64::from(i) / 10.0;
                            prop_assert!(perpendicular(&c, t, q), "{foot:?}");
                        }
                    }
                }
            }
            // Every sign change of the tangential component along a dense sample of a bounded
            // curve (away from polyline vertices) has a foot point nearby.
            prop_assume!(lo.is_finite() && hi.is_finite());
            let n = 4000;
            let h = (hi - lo) / f64::from(n);
            let g = |t: f64| c.derivative(t).dot(&(q - c.point_at(t)));
            for i in 0..n {
                let (t0, t1) = (lo + h * f64::from(i), lo + h * f64::from(i + 1));
                if matches!(c, Curve2::Polyline(_)) && t0.floor() != t1.floor() {
                    continue;
                }
                if g(t0) * g(t1) < 0.0 {
                    let covered = feet.iter().any(|f| match f {
                        FootPoint::Point(p) => p.parameter >= t0 - 2.0 * h && p.parameter <= t1 + 2.0 * h,
                        FootPoint::Interval { start, end, .. } => *start <= t1 && *end >= t0,
                    }) || (c.is_closed() && feet.iter().any(|f| f.parameter() <= 2.0 * h || f.parameter() >= hi - 2.0 * h));
                    prop_assert!(covered, "no foot near {t0}..{t1}: {feet:?}");
                }
            }
        }
    }
}
