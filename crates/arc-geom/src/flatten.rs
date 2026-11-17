//! Curve flattening (CORE-GEOM-011): conversion of a bounded curve to a polyline that stays
//! within a chord tolerance and an angle tolerance.
//!
//! [`CurveFlatten`] is implemented by every concrete curve type and by [`crate::Curve2`].
//!
//! - The chord tolerance bounds the distance between the curve and its polyline, both ways.
//! - The angle tolerance bounds the turning of the curve's tangent across the part of the curve
//!   each polyline segment replaces.
//! - The first and last vertices are the curve's exact end points; a closed curve ends at its
//!   first vertex. A polyline keeps every vertex exactly; straight segments are not subdivided.
//! - A circular arc of radius r and sweep φ is divided into `n = ⌈|φ| / step⌉` equal parts with
//!   `step = min(angle, 4·asin(√(chord / 2r)))` (no chord limit when chord ≥ 2r, since an arc's
//!   sagitta 2r·sin²(φ/4) never exceeds 2r). Equal parts minimize the largest part, so no
//!   polyline with fewer segments meets both tolerances: the count is minimal.
//! - An ellipse is divided greedily in its eccentric angle: from each vertex the next is the
//!   farthest one, found by bisection between a step that always satisfies both tolerances and
//!   one that never does, whose part meets both. A part from eccentric angle θ to θ + Δ deviates
//!   from its chord by exactly `a·b·2·sin²(Δ/4) / |C′(θ + Δ/2)|` (the affine image of a circular
//!   sagitta), and its turning is the angle between its end tangents. Parts also turn by at most
//!   a quarter turn, which keeps the deviation formula exact; the count is not minimal, but each
//!   part except the last is maximal to within the bisection resolution.
//! - Interior vertices are rounded to `f64` coordinates. When a circular or elliptical part
//!   needs interior vertices and the chord tolerance is below [`CHORD_RESOLUTION_ULPS`] units
//!   in the last place of the largest coordinate magnitude of its bounding box, the rounding
//!   would be a noticeable share of the tolerance, and flattening fails with
//!   `geom.precision.insufficient`.
//! - The ellipse computations are carried out on the axes divided by the major semi-axis
//!   length, so no product overflows or underflows.
//! - Rays and lines fail with `geom.curve.unbounded`. A result with more vertices than the limit
//!   fails with `geom.flatten.too_many_vertices` before the vertices are generated where the
//!   count is known in advance, and as soon as the limit is passed otherwise.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use arc_base::math::Point2;

use crate::bbox::Aabb2;
use arc_base::{Error, Limits, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::{
    Conic, Piece, Shape, Shaped, cross, ensure_tolerance, global_parameter, is_full_turn, norm,
    segment_piece,
};
use crate::polyline::Polyline2;
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2};

/// The bisection steps that locate the farthest admissible next vertex on an ellipse.
const STEP_ITERATIONS: usize = 40;
/// The most halvings of the safe step on an ellipse before giving up on rounding.
const MAX_HALVINGS: usize = 64;
/// The smallest chord tolerance, in units of the machine epsilon times the largest coordinate
/// magnitude of a curved part, at which its interior vertices can be placed; below it the
/// rounding of the vertices would be more than a sixteenth of the tolerance.
pub const CHORD_RESOLUTION_ULPS: f64 = 16.0;

/// Conversion of a bounded 2D curve to a polyline (CORE-GEOM-011), implemented by every curve
/// kind and [`crate::Curve2`].
pub trait CurveFlatten: Shaped {
    /// The vertices of a polyline within `chord` (a distance) and `angle` (radians of turning
    /// per segment) of the curve, at most `max_vertices` of them.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` when `chord` or `angle` is not finite and positive;
    /// `geom.curve.unbounded` for a ray or line; `geom.flatten.too_many_vertices` when more than
    /// `max_vertices` vertices are needed; `geom.precision.insufficient` when the tolerances are
    /// below the resolution of `f64` at the curve.
    fn flatten(&self, chord: f64, angle: f64, max_vertices: usize) -> Result<Vec<Point2>> {
        Ok(flatten_shape(&self.shape(), chord, angle, max_vertices)?
            .into_iter()
            .map(|(_, point)| point)
            .collect())
    }

    /// [`Self::flatten`] with `tolerance.tessellation_chord`, `tolerance.tessellation_angle` and
    /// at most `limits.array_length` vertices.
    ///
    /// # Errors
    ///
    /// As [`Self::flatten`].
    fn flatten_with(&self, tolerance: &Tolerance, limits: &Limits) -> Result<Vec<Point2>> {
        self.flatten(
            tolerance.tessellation_chord,
            tolerance.tessellation_angle,
            usize::try_from(limits.array_length).unwrap_or(usize::MAX),
        )
    }
}

impl CurveFlatten for Segment2 {}
impl CurveFlatten for Ray2 {}
impl CurveFlatten for Line2 {}
impl CurveFlatten for Circle2 {}
impl CurveFlatten for Arc2 {}
impl CurveFlatten for Ellipse2 {}
impl CurveFlatten for EllipticalArc2 {}
impl CurveFlatten for Polyline2 {}
impl CurveFlatten for Curve2 {}

/// The vertices with their parameters, bounded by a vertex limit.
struct Sink {
    vertices: Vec<(f64, Point2)>,
    limit: usize,
}

impl Sink {
    fn too_many(&self) -> Error {
        Error::new(codes::FLATTEN_TOO_MANY_VERTICES)
            .with_detail("limit", i64::try_from(self.limit).unwrap_or(i64::MAX))
    }

    /// Fails unless `more` further vertices fit.
    fn ensure_room(&self, more: f64) -> Result<()> {
        if self.vertices.len() as f64 + more > self.limit as f64 {
            Err(self.too_many())
        } else {
            Ok(())
        }
    }

    fn push(&mut self, parameter: f64, point: Point2) -> Result<()> {
        if self.vertices.len() >= self.limit {
            return Err(self.too_many());
        }
        self.vertices.push((parameter, point));
        Ok(())
    }
}

/// The largest sweep of a circular arc of radius `radius` whose sagitta is at most `chord`.
fn chord_step(radius: f64, chord: f64) -> f64 {
    if chord >= 2.0 * radius {
        TAU
    } else {
        4.0 * (chord / (2.0 * radius)).sqrt().asin()
    }
}

/// The minimal number of equal parts of a circular arc that meets both tolerances.
pub(crate) fn arc_parts(radius: f64, span: f64, chord: f64, angle: f64) -> f64 {
    let step = angle.min(chord_step(radius, chord));
    let parts = (span / step).ceil().max(1.0);
    // Guard against ceil rounding up past an exact multiple.
    if parts > 1.0 && span / (parts - 1.0) <= step {
        parts - 1.0
    } else {
        parts
    }
}

/// Fails with `geom.precision.insufficient` when `chord` is below the coordinate resolution of
/// `boxed`.
fn ensure_resolution(boxed: &Aabb2, chord: f64) -> Result<()> {
    let extent = [boxed.min(), boxed.max()]
        .iter()
        .fold(0.0_f64, |m, p| m.max(p.x.abs()).max(p.y.abs()));
    if chord < CHORD_RESOLUTION_ULPS * f64::EPSILON * extent {
        Err(insufficient())
    } else {
        Ok(())
    }
}

/// The interior vertices of a piece; `index` maps local parameters into a polyline's.
fn interior(
    piece: &Piece,
    chord: f64,
    angle: f64,
    index: Option<usize>,
    sink: &mut Sink,
) -> Result<()> {
    let to_global = |t: f64| index.map_or(t, |k| global_parameter(k, piece, t));
    match piece {
        Piece::Arc(arc) => {
            let span = arc.sweep().abs();
            let parts = arc_parts(arc.radius(), span, chord, angle);
            if parts > 1.0 {
                ensure_resolution(&arc.bounding_box(), chord)?;
            }
            // parts − 1 interior vertices and the end vertex.
            sink.ensure_room(parts)?;
            let mut i = 1.0;
            while i < parts {
                let t = span * i / parts;
                sink.push(to_global(t), arc.point_at(t))?;
                i += 1.0;
            }
            Ok(())
        }
        Piece::Elliptical(arc) => match piece.conic() {
            Some(conic) => ellipse_interior(
                piece,
                &conic,
                &arc.bounding_box(),
                chord,
                angle,
                &to_global,
                sink,
            ),
            None => Ok(()),
        },
        Piece::Segment(_) | Piece::Ray(_) | Piece::Line(_) => Ok(()),
    }
}

fn ellipse_interior(
    piece: &Piece,
    conic: &Conic,
    boxed: &Aabb2,
    chord: f64,
    angle: f64,
    to_global: &impl Fn(f64) -> f64,
    sink: &mut Sink,
) -> Result<()> {
    // Lengths in units of the major semi-axis.
    let scale = norm(conic.a).max(norm(conic.b));
    let (a, b) = (norm(conic.a) / scale, norm(conic.b) / scale);
    let scaled_chord = chord / scale;
    let d1 = |t: f64| conic.d1(t) / scale;
    let max_turn = angle.min(FRAC_PI_2);
    let accept = |t: f64, step: f64| {
        let (start, end) = (d1(t), d1(t + step));
        let turning = cross(&start, &end).abs().atan2(start.dot(&end));
        let sine = (0.25 * step).sin();
        let deviation = a * b * 2.0 * sine * sine / norm(d1(t + 0.5 * step));
        turning <= max_turn && deviation <= scaled_chord
    };
    // Turning per unit of eccentric angle lies in [b/a, a/b]; deviation in [b, a]·2·sin²(Δ/4).
    let safe = chord_step(a, scaled_chord).min(max_turn * b / a);
    let upper = chord_step(b, scaled_chord).min(max_turn * a / b).min(PI);
    let span = conic.span();
    if span > upper || !accept(0.0, span) {
        ensure_resolution(boxed, chord)?;
    }
    sink.ensure_room(span / upper)?;
    let mut t = 0.0;
    while t < span {
        let remaining = span - t;
        // A part longer than `upper` may turn by more than a half turn, which its end
        // tangents cannot show.
        if remaining <= upper && accept(t, remaining) {
            break;
        }
        let mut lo = safe.min(remaining);
        let mut halvings = 0;
        while !accept(t, lo) {
            lo *= 0.5;
            halvings += 1;
            if halvings > MAX_HALVINGS {
                return Err(insufficient());
            }
        }
        let mut hi = upper.min(remaining);
        for _ in 0..STEP_ITERATIONS {
            if hi <= lo {
                break;
            }
            let mid = 0.5 * (lo + hi);
            if accept(t, mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let next = t + lo;
        if next <= t {
            return Err(insufficient());
        }
        t = next;
        sink.push(to_global(t), piece.point(t))?;
    }
    Ok(())
}

fn insufficient() -> Error {
    Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "flatten")
}

/// The polyline vertices of a bounded curve with their parameters.
pub(crate) fn flatten_shape(
    shape: &Shape<'_>,
    chord: f64,
    angle: f64,
    max_vertices: usize,
) -> Result<Vec<(f64, Point2)>> {
    ensure_tolerance("chord", chord)?;
    ensure_tolerance("angle", angle)?;
    let mut sink = Sink {
        vertices: Vec::new(),
        limit: max_vertices,
    };
    match shape {
        Shape::Piece(piece) => {
            let (lo, hi) = piece.range();
            if !lo.is_finite() || !hi.is_finite() {
                return Err(Error::new(codes::CURVE_UNBOUNDED));
            }
            let start = piece.point(lo);
            sink.push(lo, start)?;
            interior(piece, chord, angle, None, &mut sink)?;
            let closed = piece.conic().is_some_and(|c| is_full_turn(c.span()));
            sink.push(hi, if closed { start } else { piece.point(hi) })?;
        }
        Shape::Polyline(polyline) => {
            let vertices = polyline.vertices();
            if let Some(first) = vertices.first() {
                sink.push(0.0, first.point)?;
            }
            for (index, segment) in polyline.segments().enumerate() {
                interior(
                    &segment_piece(segment),
                    chord,
                    angle,
                    Some(index),
                    &mut sink,
                )?;
                if let Some(end) = vertices.get((index + 1) % vertices.len()) {
                    sink.push((index + 1) as f64, end.point)?;
                }
            }
        }
    }
    Ok(sink.vertices)
}

#[cfg(test)]
mod tests {
    use arc_base::math::Vector2;
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, wide_curve};
    use crate::eval::CurveEval;
    use crate::eval::tests::{bulged, extent};
    use crate::testing::{angle as any_angle, point, sweep, wide_bulge};

    fn distance_to_segment(p: Point2, a: Point2, b: Point2) -> f64 {
        let ab = b - a;
        let length2 = ab.norm_squared();
        if length2 == 0.0 {
            return (p - a).norm();
        }
        let f = ((p - a).dot(&ab) / length2).clamp(0.0, 1.0);
        (p - (a + ab * f)).norm()
    }

    /// Checks the flattened `vertices` of `c` against both tolerances; `slack` absorbs the
    /// rounding of the vertex coordinates.
    fn check_flattened(
        c: &Curve2,
        vertices: &[(f64, Point2)],
        chord: f64,
        angle: f64,
        slack: f64,
    ) -> std::result::Result<(), TestCaseError> {
        // Exact for every kind; a polyline's are its vertices, checked below, and
        // `Curve2::start_point` evaluates its first arc instead.
        let (first, last) = (vertices[0].1, vertices[vertices.len() - 1].1);
        let (start, end) = (c.start_point().unwrap(), c.end_point().unwrap());
        if matches!(c, Curve2::Polyline(_)) {
            prop_assert!(
                (first - start).norm() < 1e-9 + slack && (last - end).norm() < 1e-9 + slack
            );
        } else {
            prop_assert_eq!((first, last), (start, end));
        }
        if let Curve2::Polyline(p) = c {
            for (k, v) in p.vertices().iter().enumerate() {
                prop_assert!(vertices.iter().any(|&(t, q)| t == k as f64 && q == v.point));
            }
        }
        for pair in vertices.windows(2) {
            let [(t0, p0), (t1, p1)] = pair else {
                panic!("pairs")
            };
            prop_assert!(t1 > t0);
            prop_assert!((c.point_at(*t0) - p0).norm() < 1e-9 + slack);
            // Curve to chord, sampled densely inside the span.
            let n = 64;
            for i in 0..=n {
                let s = t0 + (t1 - t0) * f64::from(i) / f64::from(n);
                let d = distance_to_segment(c.point_at(s), *p0, *p1);
                prop_assert!(d <= chord * (1.0 + 1e-9) + slack, "deviation {d} > {chord}");
            }
            // Turning of the exact tangent across the span (it is monotone inside a span).
            let w = 1e-9 * (t1 - t0);
            let (a, b) = (c.tangent(t0 + w), c.tangent(t1 - w));
            let turning = cross(&a, &b).abs().atan2(a.dot(&b));
            prop_assert!(turning <= angle + 1e-6, "turning {turning} > {angle}");
        }
        Ok(())
    }

    #[test]
    fn vertex_limits_are_exact() {
        let arc = Arc2::new(Point2::origin(), 1.0, 0.0, PI).unwrap();
        assert_eq!(arc.flatten(10.0, FRAC_PI_2, 3).unwrap().len(), 3);
        assert_eq!(
            arc.flatten(10.0, FRAC_PI_2, 2).unwrap_err().code(),
            "geom.flatten.too_many_vertices"
        );
        let circle = Circle2::new(Point2::origin(), 1.0).unwrap();
        assert_eq!(circle.flatten(10.0, 10.0, 2).unwrap().len(), 2);
        assert_eq!(
            circle.flatten(10.0, 10.0, 1).unwrap_err().code(),
            "geom.flatten.too_many_vertices"
        );
        assert_eq!(bulged(2.0, 1.0).flatten(10.0, 10.0, 2).unwrap().len(), 2);
    }

    #[test]
    fn tolerances_below_the_coordinate_resolution_fail() {
        let far = Ellipse2::new(Point2::new(1e9, 1e9), Vector2::new(1.0, 0.0), 0.5).unwrap();
        assert_eq!(
            far.flatten(1e-12, 0.1, 100_000_000).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        let big = Circle2::new(Point2::origin(), 1e300).unwrap();
        assert_eq!(
            big.flatten(1e-300, 0.1, 1000).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        // A part that needs no interior vertex is exact at any tolerance.
        let small = Arc2::new(Point2::new(1e9, 0.0), 1.0, 0.0, 1e-9).unwrap();
        assert_eq!(small.flatten(1e-12, 0.1, 10).unwrap().len(), 2);
        assert_eq!(bulged(1.0, 1e-200).flatten(1e-3, 0.1, 10).unwrap().len(), 2);
        // A bulge of 1e15 is a circle of radius 2.5e14 whose coordinates are rounded to 1/16.
        assert_eq!(
            bulged(1.0, 1e15)
                .flatten(1.0, 0.5, 1_000_000)
                .unwrap_err()
                .code(),
            "geom.precision.insufficient"
        );
    }

    #[test]
    fn long_flat_elliptical_parts_are_split_at_most_half_a_turn() {
        // Found by the wide-curve property: the whole arc, more than half a turn of a flat
        // ellipse, turns by nearly a full turn, which its end tangents do not show.
        let e = Ellipse2::new(
            Point2::origin(),
            Vector2::new(0.0, -59.025_499_605_839_094),
            2.483_933_476_811_485_7e-8,
        )
        .unwrap();
        let arc = Curve2::EllipticalArc(
            EllipticalArc2::new(e, 2.434_408_171_265_759, -5.788_467_065_884_705_5).unwrap(),
        );
        let vertices = flatten_shape(&arc.shape(), 1.0, 0.05, 10_000).unwrap();
        assert!(vertices.len() > 2);
        check_flattened(&arc, &vertices, 1.0, 0.05, 0.0).unwrap();
    }

    #[test]
    fn unbounded_and_invalid_arguments_are_rejected() {
        let ray = Ray2::new(Point2::origin(), Vector2::x()).unwrap();
        assert_eq!(
            ray.flatten(0.1, 0.1, 100).unwrap_err().code(),
            "geom.curve.unbounded"
        );
        let line = Curve2::Line(Line2::new(Point2::origin(), Vector2::x()).unwrap());
        assert_eq!(
            line.flatten(0.1, 0.1, 100).unwrap_err().code(),
            "geom.curve.unbounded"
        );
        let circle = Circle2::new(Point2::origin(), 1.0).unwrap();
        for (chord, angle) in [
            (0.0, 0.1),
            (0.1, -1.0),
            (f64::NAN, 0.1),
            (0.1, f64::INFINITY),
        ] {
            assert_eq!(
                circle.flatten(chord, angle, 100).unwrap_err().code(),
                "geom.tolerance.invalid"
            );
        }
        assert_eq!(
            circle.flatten(1e-12, 0.1, 1000).unwrap_err().code(),
            "geom.flatten.too_many_vertices"
        );
        // Below the coordinate resolution of the curve.
        assert_eq!(
            circle.flatten(1e-300, 0.1, 1000).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        let ellipse = Ellipse2::new(Point2::origin(), Vector2::new(1.0, 0.0), 0.1).unwrap();
        assert_eq!(
            ellipse.flatten(1e-12, 0.1, 1000).unwrap_err().code(),
            "geom.flatten.too_many_vertices"
        );
        assert_eq!(
            ellipse.flatten(1e-300, 0.1, 1000).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        assert_eq!(
            Segment2::new(Point2::origin(), Point2::new(1.0, 0.0))
                .unwrap()
                .flatten(1.0, 1.0, 1)
                .unwrap_err()
                .code(),
            "geom.flatten.too_many_vertices"
        );
    }

    #[test]
    fn known_counts() {
        let circle = Circle2::new(Point2::new(1.0, 1.0), 1.0).unwrap();
        // 15° per segment: 24 segments, 25 vertices, the last equal to the first.
        let points = circle.flatten(10.0, PI / 12.0, 100).unwrap();
        assert_eq!(points.len(), 25);
        assert_eq!(points.first(), points.last());
        // Tolerances larger than the circle: one segment from the start back to the start.
        assert_eq!(
            circle.flatten(10.0, 10.0, 100).unwrap(),
            vec![Point2::new(2.0, 1.0); 2]
        );
        let defaults = circle
            .flatten_with(&Tolerance::DEFAULT, &Limits::DEFAULT)
            .unwrap();
        // A 1 mm chord on a unit circle allows 4·asin(√0.0005) ≈ 5.1° per segment: 71 segments.
        assert_eq!(defaults.len(), 72);
        // An ellipse with ratio 1 is a circle; the greedy division does as well as the formula.
        let round = Ellipse2::new(Point2::origin(), Vector2::new(1.0, 0.0), 1.0).unwrap();
        assert!(round.flatten(1e-3, 1.0, 1000).unwrap().len() <= 72);
    }

    proptest! {
        #[test]
        fn flattened_curves_stay_within_both_tolerances(
            c in curve(), chord_exp in -3.0..0.5_f64, angle in 0.05..2.0_f64,
        ) {
            let chord = 10.0_f64.powf(chord_exp);
            let result = flatten_shape(&c.shape(), chord, angle, 1_000_000);
            if c.bounding_box().is_none() {
                prop_assert_eq!(result.unwrap_err().code(), "geom.curve.unbounded");
                return Ok(());
            }
            check_flattened(&c, &result.unwrap(), chord, angle, 1e-12)?;
        }

        #[test]
        fn wide_curves_stay_within_both_tolerances(
            c in wide_curve(), chord_exp in -3.0..0.5_f64, angle in 0.05..2.0_f64,
        ) {
            let chord = 10.0_f64.powf(chord_exp);
            // The limit keeps the dense check below affordable.
            match flatten_shape(&c.shape(), chord, angle, 2_000) {
                Ok(vertices) => {
                    // Rounding of the vertices and of polyline parameters along the length.
                    let length = c.length(&Tolerance::DEFAULT).unwrap();
                    let slack = 16.0 * f64::EPSILON * (extent(&c) + length);
                    check_flattened(&c, &vertices, chord, angle, slack)?;
                }
                Err(e) => prop_assert!(
                    matches!(e.code(), "geom.flatten.too_many_vertices" | "geom.precision.insufficient"),
                    "{}", e.code()
                ),
            }
        }

        #[test]
        fn the_vertex_limit_is_exact(
            c in curve(), chord_exp in -3.0..0.5_f64, angle in 0.05..2.0_f64,
        ) {
            let chord = 10.0_f64.powf(chord_exp);
            prop_assume!(c.bounding_box().is_some());
            let n = c.flatten(chord, angle, 1_000_000).unwrap().len();
            prop_assert_eq!(c.flatten(chord, angle, n).unwrap().len(), n);
            prop_assert_eq!(
                c.flatten(chord, angle, n - 1).unwrap_err().code(),
                "geom.flatten.too_many_vertices"
            );
        }

        #[test]
        fn circle_and_bulged_segment_vertex_counts_are_minimal(
            b in wide_bulge(), length in 0.1..100.0_f64, r in 0.01..100.0_f64,
            chord_exp in -4.0..1.0_f64, angle in 0.01..3.0_f64,
        ) {
            let chord = 10.0_f64.powf(chord_exp);
            let pieces = [
                Curve2::Circle(Circle2::new(Point2::origin(), r).unwrap()),
                Curve2::Polyline(bulged(length, b)),
            ];
            for c in pieces {
                let (radius, span) = match &c {
                    Curve2::Circle(circle) => (circle.radius(), TAU),
                    Curve2::Polyline(p) => match p.segment(0) {
                        Some(crate::PolylineSegment::Arc(arc)) => (arc.radius(), arc.sweep().abs()),
                        _ => continue,
                    },
                    _ => continue,
                };
                let Ok(points) = c.flatten(chord, angle, 1_000_000) else { continue };
                let parts = (points.len() - 1) as f64;
                let sagitta = |phi: f64| 2.0 * radius * (0.25 * phi).sin().powi(2);
                let phi = span / parts;
                prop_assert!(phi <= angle * (1.0 + 1e-12) && sagitta(phi) <= chord * (1.0 + 1e-12));
                if parts > 1.0 {
                    let fewer = span / (parts - 1.0);
                    prop_assert!(fewer > angle || sagitta(fewer) > chord, "{} parts are enough", parts - 1.0);
                }
            }
        }

        #[test]
        fn arc_vertex_count_is_minimal(
            c in point(), r in 0.01..100.0_f64, a in any_angle(), s in sweep(),
            chord_exp in -4.0..1.0_f64, angle in 0.01..3.0_f64,
        ) {
            let chord = 10.0_f64.powf(chord_exp);
            let arc = Arc2::new(c, r, a, s).unwrap();
            let parts = (arc.flatten(chord, angle, 10_000_000).unwrap().len() - 1) as f64;
            let sagitta = |phi: f64| 2.0 * r * (0.25 * phi).sin().powi(2);
            let phi = s.abs() / parts;
            prop_assert!(phi <= angle * (1.0 + 1e-12) && sagitta(phi) <= chord * (1.0 + 1e-12));
            if parts > 1.0 {
                let fewer = s.abs() / (parts - 1.0);
                prop_assert!(fewer > angle || sagitta(fewer) > chord, "{} parts are enough", parts - 1.0);
            }
        }
    }
}
