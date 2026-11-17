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
//! - An arc taken from a bulged polyline segment is projected in its chord frame, without its
//!   center: with `(x, y)` the query relative to the chord midpoint in units of the chord length
//!   (`x` along the chord, `y` to its left) and `h = (1/b − b)/4` the offset of the center, the
//!   foot points lie at angle fraction `1/2 + atan2(s·x, s·σ·(h − y)) / (2|α|)` for `s = ±1`,
//!   `σ` the sign of the bulge. For a half angle so small that it is near the subnormal range
//!   this is evaluated as `1/2 + x / (1 − b² − 4b·y)`.
//! - Ellipse foot points are the zeros of `f(θ) = (a² − b²)·sin θ·cos θ − a·u·sin θ + b·v·cos θ`
//!   (query `(u, v)` in the frame of the axes, all divided by `a`). Dividing by `sin θ·cos θ`
//!   gives a function whose derivative vanishes only where `a·u·sin³ θ + b·v·cos³ θ = 0`, which
//!   has two solutions half a turn apart. Splitting the turn at the quadrant angles and those
//!   two angles leaves at most six intervals on each of which `f` has at most one zero, found by
//!   a sign change and refined to convergence by safeguarded Newton iteration; a zero at a split
//!   angle (a tangent, double zero included) is found by testing the split angle itself.
//! - A refined zero is accepted when `f` is within its rounding error or the bracket cannot
//!   shrink further in `f64`. Its tangential residual is then at most `tolerance.linear` where
//!   `f64` resolves that, and otherwise of the order of
//!   `ε·(a + |q − center|)·a / |C′(θ)|` (`ε` the machine epsilon): far from the curve or for a
//!   very large curve the perpendicularity is as good as `f64` allows.
//! - The closest point is chosen among the foot points and the ends of the range (and the
//!   vertices of a polyline). Candidates whose distances differ by at most `tolerance.linear`
//!   tie, and the one with the smallest parameter wins; the result is within
//!   `tolerance.linear` of the true minimum distance where `f64` resolves that.
//! - Every length is computed from scaled values, so every curve a constructor accepts can be
//!   projected; a query so far away that its scaled coordinates overflow fails with
//!   `geom.precision.insufficient`.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::{
    Conic, MAX_ROOT_ITERATIONS, Piece, Shape, Shaped, ensure_tolerance, global_parameter, norm,
    segment_piece, unit,
};
use crate::polyline::{Chord, Polyline2, SMALL_HALF_ANGLE};
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2, perp};

/// The rounding error of the ellipse foot function, in units of the machine epsilon times the
/// magnitude of its terms: a value within it is a zero to the precision of `f64`.
const ROOT_ULPS: f64 = 16.0;

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
    /// `tolerance.linear` is not finite and positive; `geom.precision.insufficient` when the
    /// query is so far from the curve, relative to its size, that its coordinates in the
    /// curve's frame overflow.
    fn closest_point(&self, query: Point2, tolerance: &Tolerance) -> Result<CurvePoint> {
        closest(&self.shape(), query, tolerance)
    }

    /// Every perpendicular foot point of `query` inside the parameter range, sorted by
    /// parameter, with foot points closer than `tolerance.linear` (or than the rounding of
    /// their coordinates) to each other merged.
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

fn insufficient() -> Error {
    Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "foot_points")
}

fn curve_point(shape: &Shape<'_>, parameter: f64, query: Point2) -> CurvePoint {
    let point = shape.point(parameter);
    CurvePoint {
        parameter,
        point,
        distance: norm(query - point),
    }
}

fn piece_point(piece: &Piece, parameter: f64, query: Point2) -> CurvePoint {
    curve_point(&Shape::Piece(*piece), parameter, query)
}

/// The interval foot point covering a whole piece.
fn whole(piece: &Piece, query: Point2) -> FootPoint {
    let (lo, hi) = piece.range();
    FootPoint::Interval {
        start: lo,
        end: hi,
        distance: norm(query - piece.point(lo)),
    }
}

/// Foot points on one smooth piece, in its own parameter.
fn piece_feet(piece: &Piece, query: Point2, tolerance: &Tolerance) -> Result<Vec<FootPoint>> {
    let (lo, hi) = piece.range();
    if let Some((origin, direction)) = piece.linear() {
        let speed = norm(direction);
        let along = (query - origin).dot(&unit(direction));
        if !along.is_finite() {
            return Err(insufficient());
        }
        let t = along / speed;
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
    if let Piece::Arc(arc) = piece
        && let Some(chord) = arc.chord()
    {
        return chord_feet(piece, arc, chord, query, tolerance);
    }
    let Some(conic) = piece.conic() else {
        return Ok(Vec::new());
    };
    let Some(angles) = conic_feet(&conic, query, tolerance)? else {
        return Ok(vec![whole(piece, query)]);
    };
    let span = conic.span();
    let slack = tolerance.linear / norm(conic.a).max(norm(conic.b));
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

/// Foot points on an arc taken from a bulged polyline segment, solved in its chord frame (see
/// the [module documentation](self)).
fn chord_feet(
    piece: &Piece,
    arc: &Arc2,
    chord: &Chord,
    query: Point2,
    tolerance: &Tolerance,
) -> Result<Vec<FootPoint>> {
    let (start, bulge) = (chord.start(), chord.bulge());
    let c = chord.end() - start;
    let length = norm(c);
    let direction = unit(c);
    let relative = query - (start + c * 0.5);
    let (x, y) = (
        relative.dot(&direction) / length,
        relative.dot(&perp(&direction)) / length,
    );
    let offset = 0.25 * (1.0 / bulge - bulge);
    let w = bulge.signum() * (offset - y);
    if !(x.is_finite() && w.is_finite()) {
        return Err(insufficient());
    }
    // Every point is a foot point when the query is the center.
    if length * x.hypot(w) <= tolerance.linear {
        return Ok(vec![whole(piece, query)]);
    }
    let half = chord.half_angle().abs();
    let fractions = if half < SMALL_HALF_ANGLE {
        // atan2 of a tiny angle is its tangent; this form avoids subnormal angles.
        vec![0.5 + x / ((1.0 - bulge * bulge) - 4.0 * bulge * y)]
    } else {
        [1.0, -1.0]
            .iter()
            .map(|s| 0.5 + (s * x).atan2(s * w) / (2.0 * half))
            .collect()
    };
    let slack = tolerance.linear / (length * (half / chord.sin_half_angle().abs()));
    let span = arc.sweep().abs();
    let mut out: Vec<FootPoint> = fractions
        .into_iter()
        .filter(|f| *f >= -slack && *f <= 1.0 + slack)
        .map(|f| FootPoint::Point(piece_point(piece, f.clamp(0.0, 1.0) * span, query)))
        .collect();
    out.sort_by(|x, y| x.parameter().total_cmp(&y.parameter()));
    Ok(out)
}

/// The eccentric angles in [0, 2π) of the foot points of `query` on the full ellipse of
/// `conic`, or `None` when every point is a foot point. Everything is divided by the major
/// semi-axis length first, so no product overflows or underflows.
fn conic_feet(conic: &Conic, query: Point2, tolerance: &Tolerance) -> Result<Option<Vec<f64>>> {
    let scale = norm(conic.a).max(norm(conic.b));
    let (a, b) = (norm(conic.a) / scale, norm(conic.b) / scale);
    let major = unit(conic.a);
    let d = query - conic.center;
    let (u, v) = (d.dot(&major) / scale, d.dot(&perp(&major)) / scale);
    if !(u.is_finite() && v.is_finite()) {
        return Err(insufficient());
    }
    let tol = tolerance.linear / scale;
    let c = (a - b) * (a + b);
    // The tangential component of query − point is at most |d| + c / (2b) everywhere.
    if u.hypot(v) + c / (2.0 * b) <= tol {
        return Ok(None);
    }
    let f_at = |sin: f64, cos: f64| c * sin * cos - a * u * sin + b * v * cos;
    // The value of f at t with the rounding error of its evaluation there.
    let f_with_rounding = |t: f64| {
        let (sin, cos) = t.sin_cos();
        let terms = (c * sin * cos).abs() + (a * u * sin).abs() + (b * v * cos).abs();
        (f_at(sin, cos), ROOT_ULPS * f64::EPSILON * terms)
    };
    let fp_at = |sin: f64, cos: f64| c * (cos - sin) * (cos + sin) - a * u * cos - b * v * sin;
    let fp = |t: f64| {
        let (sin, cos) = t.sin_cos();
        fp_at(sin, cos)
    };
    let floor = ROOT_ULPS * f64::EPSILON * (c + a * u.abs() + b * v.abs());
    // Each cut with its sine and cosine; those of the quadrant angles are exact, since the f64
    // nearest to π is not a zero of the sine and a sign at the cut must be the true one.
    let mut cuts = vec![
        (0.0, 0.0, 1.0),
        (FRAC_PI_2, 1.0, 0.0),
        (PI, 0.0, -1.0),
        (3.0 * FRAC_PI_2, -1.0, 0.0),
        (TAU, 0.0, 1.0),
    ];
    let (p, q) = ((b * v).cbrt(), (a * u).cbrt());
    if p != 0.0 || q != 0.0 {
        let turn = (-p).atan2(q);
        for angle in [turn.rem_euclid(TAU), (turn + PI).rem_euclid(TAU)] {
            let (sin, cos) = angle.sin_cos();
            cuts.push((angle, sin, cos));
        }
    }
    // Stable, so a quadrant angle stays ahead of an equal computed one and is kept.
    cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
    cuts.dedup_by(|x, y| x.0 == y.0);
    let mut angles = Vec::new();
    for pair in cuts.windows(2) {
        let &[(lo, sin_lo, cos_lo), (hi, sin_hi, cos_hi)] = pair else {
            continue;
        };
        let (f_lo, f_hi) = (f_at(sin_lo, cos_lo), f_at(sin_hi, cos_hi));
        let speed = norm(Vector2::new(a * sin_lo, b * cos_lo));
        if f_lo.abs() <= (tol * speed).max(floor) {
            angles.push(lo);
        }
        // The sign of f just inside the interval: at a cut where f vanishes to rounding (a
        // zero of sin θ or cos θ, or a foot at the cut), the sign of its slope there.
        let inside_lo = if f_lo.abs() > floor {
            f_lo
        } else {
            fp_at(sin_lo, cos_lo)
        };
        let inside_hi = if f_hi.abs() > floor {
            f_hi
        } else {
            -fp_at(sin_hi, cos_hi)
        };
        if (inside_lo < 0.0 && inside_hi > 0.0) || (inside_lo > 0.0 && inside_hi < 0.0) {
            angles.push(refine(lo, hi, inside_lo, &f_with_rounding, &fp));
        }
    }
    Ok(Some(angles))
}

/// The zero of `f` in (lo, hi), where `f` has the sign of `f_lo` just above `lo` and the
/// opposite sign just below `hi`, by Newton
/// iteration kept inside a bracket that at least halves every second step. It stops when `f`
/// is within the rounding error of its evaluation, or when the bracket cannot shrink further in
/// `f64`.
fn refine(
    mut lo: f64,
    mut hi: f64,
    f_lo: f64,
    f: &impl Fn(f64) -> (f64, f64),
    fp: &impl Fn(f64) -> f64,
) -> f64 {
    let mut t = 0.5 * (lo + hi);
    let mut width = hi - lo;
    for _ in 0..MAX_ROOT_ITERATIONS {
        let (value, rounding) = f(t);
        if value.abs() <= rounding {
            return t;
        }
        if (value < 0.0) == (f_lo < 0.0) {
            lo = t;
        } else {
            hi = t;
        }
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            return t;
        }
        let newton = t - value / fp(t);
        let halved = hi - lo <= 0.5 * width;
        width = hi - lo;
        t = if halved && newton > lo && newton < hi {
            newton
        } else {
            mid
        };
    }
    t
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

/// Sorts foot points, joins touching intervals and drops points within the tolerance (or the
/// coordinate rounding) of an interval or of the previous point (and, on a closed curve, of the
/// first point).
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
    // Points closer than the rounding of their coordinates are the same point.
    let near = |a: Point2, b: Point2| {
        let extent = [a.x, a.y, b.x, b.y]
            .iter()
            .fold(0.0_f64, |m, x| m.max(x.abs()));
        norm(a - b) <= tol.linear.max(ROOT_ULPS * f64::EPSILON * extent)
    };
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
    use crate::curve::tests::{curve, extreme_curve, parameter, wide_curve};
    use crate::eval::CurveEval;
    use crate::eval::tests::{bulged, extent};
    use crate::polyline::PolylineVertex;
    use crate::testing::{far_point, point};

    /// The bound on the rounding of a projection of `q` onto `c`: the tolerance, or a small
    /// multiple of the rounding of the coordinates involved.
    fn rounding(c: &Curve2, q: Point2) -> f64 {
        TOL.linear
            .max(1e3 * f64::EPSILON * (extent(c) + q.x.abs().max(q.y.abs())))
    }

    /// The documented bound on the tangential residual of a foot point: the rounding, times the
    /// ratio of the major semi-axis to the speed for an ellipse.
    fn residual_bound(c: &Curve2, q: Point2) -> f64 {
        let ratio = match c {
            Curve2::Ellipse(e) => e.ratio(),
            Curve2::EllipticalArc(e) => e.ellipse().ratio(),
            _ => 1.0,
        };
        rounding(c, q) / ratio
    }

    fn params(feet: &[FootPoint]) -> Vec<f64> {
        feet.iter().map(FootPoint::parameter).collect()
    }

    #[test]
    fn nearly_straight_bulged_segments_project_in_the_chord_frame() {
        for (length, bulge) in [
            (1.0, 1e-10),
            (1000.0, 1e-10),
            (100.0, 1e-8),
            (1.0, 1e-150),
            (1.0, 1e-200),
            (1.0, -1e-200),
            (3.0, 1e-300),
        ] {
            let p = bulged(length, bulge);
            let q = Point2::new(0.5 * length, 1.0);
            // The middle of the arc lies the sagitta, half the chord times the bulge, below it.
            let want = 1.0 + 0.5 * length * bulge;
            let found = p.closest_point(q, &TOL).unwrap();
            assert!(
                (found.parameter - 0.5).abs() < 1e-12,
                "{length} {bulge}: {found:?}"
            );
            assert!(
                (found.distance - want).abs() < 1e-12,
                "{length} {bulge}: {found:?}"
            );
            let feet = p.foot_points(q, &TOL).unwrap();
            assert_eq!(feet.len(), 1, "{feet:?}");
            assert!(p.tangent(feet[0].parameter()).dot(&(q - found.point)).abs() <= TOL.linear);
        }
    }

    #[test]
    fn nearly_full_bulged_segments_project_in_the_chord_frame() {
        // Counterclockwise with bulge 1e10: a circle of radius 2.5e9 through the chord, apex
        // 5e9 below it; the query is 1 inside the apex.
        let p = bulged(1.0, 1e10);
        let found = p.closest_point(Point2::new(0.5, -5e9 + 1.0), &TOL).unwrap();
        assert!((found.parameter - 0.5).abs() < 1e-9 && (found.distance - 1.0).abs() < 1e-5);
        // Clockwise: the circle lies above the chord, so below the gap the ends are closest.
        let p = bulged(1.0, -1e10);
        let found = p.closest_point(Point2::new(0.5, -1.0), &TOL).unwrap();
        assert_eq!(found.parameter, 0.0);
        assert!((found.distance - 1.25_f64.sqrt()).abs() < 1e-15);
    }

    #[test]
    fn extreme_sizes_project_to_scale() {
        for s in [1e-150, 1e-100, 1e100, 1e150] {
            let o = Point2::origin();
            let segment = Segment2::new(o, Point2::new(s, s)).unwrap();
            let found = segment.closest_point(Point2::new(0.0, s), &TOL).unwrap();
            if s < TOL.linear {
                // Every candidate ties within the tolerance; the start wins.
                assert_eq!(found.parameter, 0.0);
                assert!((found.distance / s - 1.0).abs() < 1e-15, "{s}");
            } else {
                assert!((found.parameter - 0.5).abs() < 1e-15);
                assert!((found.distance / s - 0.5_f64.sqrt()).abs() < 1e-15, "{s}");
            }
            let circle = Circle2::new(o, s).unwrap();
            let found = circle
                .closest_point(Point2::new(2.0 * s, 0.0), &TOL)
                .unwrap();
            assert!((found.distance / s - 1.0).abs() < 1e-15, "{s}");
            assert_eq!(found.parameter, 0.0);
            // Below the tolerance the two feet are one point.
            let feet = circle.foot_points(Point2::new(2.0 * s, 0.0), &TOL).unwrap();
            assert_eq!(feet.len(), if s > TOL.linear { 2 } else { 1 });
            let ellipse = Ellipse2::new(o, Vector2::new(s, 0.0), 0.5).unwrap();
            let found = ellipse
                .closest_point(Point2::new(0.0, 2.0 * s), &TOL)
                .unwrap();
            if s > TOL.linear {
                assert!((found.parameter - FRAC_PI_2).abs() < 1e-15);
                assert!((found.distance / s - 1.5).abs() < 1e-15, "{s}");
            } else {
                assert!(found.distance <= 1.5 * s + TOL.linear);
            }
        }
    }

    #[test]
    fn far_queries_converge() {
        let e = Ellipse2::new(Point2::origin(), Vector2::new(2.0, 0.0), 0.5).unwrap();
        let c = Curve2::Ellipse(e);
        for q in [Point2::new(1e12, 3e11), Point2::new(-3e11, 1e12)] {
            let feet = c.foot_points(q, &TOL).unwrap();
            assert_eq!(feet.len(), 2, "{feet:?}");
            for foot in &feet {
                let t = foot.parameter();
                let residual = c.tangent(t).dot(&(q - c.point_at(t))).abs();
                assert!(residual <= residual_bound(&c, q), "{residual}");
            }
            let found = c.closest_point(q, &TOL).unwrap();
            assert!(found.distance < (q - Point2::origin()).norm());
        }
    }

    #[test]
    fn nearly_circular_ellipses_have_no_duplicate_feet() {
        for ratio in [1.0 - 1e-12, 1.0 - 1e-9] {
            let e = Ellipse2::new(Point2::origin(), Vector2::new(1e7, 0.0), ratio).unwrap();
            let center = params(&e.foot_points(Point2::origin(), &TOL).unwrap());
            assert_eq!(center.len(), 4, "{center:?}");
            let off = params(&e.foot_points(Point2::new(1.0, 0.0), &TOL).unwrap());
            assert_eq!(off.len(), 2, "{off:?}");
        }
    }

    #[test]
    fn flat_ellipses_project_their_own_points() {
        // Found by the extreme-curve property: a quadrant angle rounded to f64 is not a zero
        // of the sine, and a foot exactly at a quadrant angle hid the one next to it.
        let flat = Ellipse2::new(
            Point2::new(1.0, 1.0),
            Vector2::new(1.0, 1.0),
            1.194_593_958_155_402_9e-11,
        )
        .unwrap();
        let arc =
            Curve2::EllipticalArc(EllipticalArc2::new(flat, 4.337_990_582_423_08, -0.05).unwrap());
        let huge = Curve2::Ellipse(
            Ellipse2::new(
                Point2::new(1.0, 4.601_197_943_847_03e88),
                Vector2::new(2.746_456_378_470_755_5e137, 4.654_360_744_934_588e-75),
                8.706_830_190_415_798e-149,
            )
            .unwrap(),
        );
        // Two feet a tiny angle apart: refinement stops at the rounding of f where it is
        // evaluated, not at the rounding of its largest possible terms.
        let clustered = Curve2::Ellipse(
            Ellipse2::new(
                Point2::new(1.0, 1.0),
                Vector2::new(1.261_949_017_393_344_8e74, 1.0),
                2.017_664_774_723_684_6e-34,
            )
            .unwrap(),
        );
        for (c, f) in [
            (arc, 0.654_486_897_140_628_1),
            (huge, 0.501_919_374_651_039_3),
            (clustered, 0.499_449_654_329_216_7),
        ] {
            let q = c.point_at(parameter(&c, f));
            let found = c.closest_point(q, &TOL).unwrap();
            assert!(found.distance <= rounding(&c, q), "{found:?}");
        }
    }

    #[test]
    fn far_queries_on_tiny_curves_fail_instead_of_overflowing() {
        let e = Ellipse2::new(Point2::origin(), Vector2::new(1e-300, 0.0), 0.5).unwrap();
        assert_eq!(
            e.closest_point(Point2::new(1e300, 0.0), &TOL)
                .unwrap_err()
                .code(),
            "geom.precision.insufficient"
        );
    }

    const TOL: Tolerance = Tolerance::DEFAULT;

    /// Distance of the curve to `q` minimized over a dense sample and refined by golden-section
    /// search around the best sample.
    fn reference_distance(c: &Curve2, q: Point2) -> f64 {
        let (lo, hi) = c.parameter_range();
        let (lo, hi) = (lo.max(-500.0), hi.min(500.0));
        // A polyline's segments are built once; `point_at` would rebuild one per call.
        let segments: Vec<_> = match c {
            Curve2::Polyline(p) => p.segments().collect(),
            _ => Vec::new(),
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "t lies in [0, n]"
        )]
        let point = |t: f64| match segments.len() {
            0 => c.point_at(t),
            count => {
                let k = (t.max(0.0).floor() as usize).min(count - 1);
                segments[k].point_at_fraction(t - k as f64)
            }
        };
        let dist = |t: f64| (point(t) - q).norm();
        let n = 20_000;
        let h = (hi - lo) / f64::from(n);
        let best = (0..=n)
            .map(|i| lo + h * f64::from(i))
            .map(|t| (dist(t), t))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1;
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
        fn wide_closest_point_matches_dense_sampling(
            c in wide_curve(), q in prop_oneof![point(), far_point()],
        ) {
            let found = c.closest_point(q, &TOL).unwrap();
            prop_assert!(c.point_at(found.parameter) == found.point);
            let reference = reference_distance(&c, q);
            // The found point lies on the curve, so it cannot beat the true minimum; it must be
            // at least as close as the sampled one.
            prop_assert!(found.distance <= reference + rounding(&c, q), "{} vs {reference}", found.distance);
        }

        #[test]
        fn wide_foot_points_are_perpendicular(
            c in wide_curve(), q in prop_oneof![point(), far_point()],
        ) {
            let bound = residual_bound(&c, q);
            for foot in c.foot_points(q, &TOL).unwrap() {
                if let FootPoint::Point(p) = foot {
                    let r = q - p.point;
                    let ok = |tangent: Vector2| tangent.dot(&r).abs() <= bound;
                    let t = p.parameter;
                    prop_assert!(ok(c.tangent(t)) || ok(c.tangent(t - 1e-12)), "{foot:?}");
                }
            }
        }

        #[test]
        fn extreme_curves_project_their_own_points(c in extreme_curve(), f in 0.0..=1.0_f64) {
            let q = c.point_at(parameter(&c, f));
            let found = c.closest_point(q, &TOL).unwrap();
            prop_assert!(c.point_at(found.parameter) == found.point);
            prop_assert!(found.distance <= rounding(&c, q), "{found:?}");
        }

        #[test]
        fn closest_point_matches_dense_sampling(c in curve(), q in point()) {
            let found = c.closest_point(q, &TOL).unwrap();
            prop_assert!((c.point_at(found.parameter) - found.point).norm() == 0.0);
            prop_assert!((norm(q - found.point) - found.distance).abs() == 0.0);
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
