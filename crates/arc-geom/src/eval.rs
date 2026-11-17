//! Curve evaluation (CORE-GEOM-003): derivatives, tangent, normal, curvature, length and the
//! parameter at a given length, for every 2D curve kind.
//!
//! [`CurveEval`] is implemented by every concrete curve type and by [`Curve2`]. Derivatives are
//! taken with respect to the documented parameter of each kind (see [`Curve2`]) and the
//! parameter is clamped to the range, as for `point_at`.
//!
//! - The unit normal is the unit tangent turned a quarter turn counterclockwise, so it points to
//!   the left of the direction of travel.
//! - Curvature is signed: positive where the curve turns counterclockwise, negative where it
//!   turns clockwise, zero on straight parts.
//! - A polyline's derivatives jump at its vertices. At an interior vertex `k` (integer `t`) they
//!   are those of the segment that starts at the vertex, as `point_at` selects it; at `t = n`
//!   those of the last segment.
//! - The length of a ray or line is `f64::INFINITY`.
//! - Lengths of circles, arcs, segments and polylines are closed forms. The length of an
//!   elliptical arc has no closed form; it is integrated with an adaptive 8-point
//!   Gauss–Legendre rule. Each interval is accepted when the rule on the interval and the sum of
//!   the rule on its two halves differ by at most the interval's share of a quarter of
//!   `tolerance.linear` (or of 64 units in the last place of the length scale when that is
//!   larger); the finer sum is kept. At most [`MAX_QUADRATURE_INTERVALS`] intervals are split;
//!   beyond that the computation fails with `geom.precision.insufficient`.

use std::f64::consts::{FRAC_PI_2, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::ensure_finite;
use crate::error::codes;
use crate::polyline::{Polyline2, PolylineSegment};
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2, perp};

/// The positive nodes of the 8-point Gauss–Legendre rule on [−1, 1]; the rule is symmetric.
const GAUSS_NODES: [f64; 4] = [
    0.183_434_642_495_649_78,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_2,
];
/// The weights belonging to [`GAUSS_NODES`].
const GAUSS_WEIGHTS: [f64; 4] = [
    0.362_683_783_378_361_66,
    0.313_706_645_877_886_9,
    0.222_381_034_453_374_43,
    0.101_228_536_290_377_06,
];
/// The most intervals the length quadrature of an elliptical arc splits before it fails.
pub const MAX_QUADRATURE_INTERVALS: usize = 4096;
/// The floor of the quadrature error target in units in the last place of the length scale, so
/// that the target stays reachable in `f64` for large ellipses.
const QUADRATURE_ULPS: f64 = 64.0;
/// The most iterations of a safeguarded Newton or bisection root search.
pub(crate) const MAX_ROOT_ITERATIONS: usize = 200;

/// Fails with `geom.tolerance.invalid` unless `value` is finite and positive.
pub(crate) fn ensure_tolerance(name: &'static str, value: f64) -> Result<()> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(Error::new(codes::TOLERANCE_INVALID).with_detail(name, value))
    }
}

/// The 2D cross product `a.x·b.y − a.y·b.x`.
pub(crate) fn cross(a: &Vector2, b: &Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// `v` scaled to unit length without overflow or underflow; the zero vector stays zero.
pub(crate) fn unit(v: Vector2) -> Vector2 {
    let scale = v.x.abs().max(v.y.abs());
    if scale == 0.0 || !scale.is_finite() {
        return v;
    }
    let w = v / scale;
    w / w.norm()
}

/// The ellipse frame of a circular or elliptical arc: `point(t) = center + a·cos θ + b·sin θ`
/// with eccentric angle `θ = start + sign(sweep)·t`, `t` in [0, |sweep|].
#[derive(Debug, Clone, Copy)]
pub(crate) struct Conic {
    pub(crate) center: Point2,
    pub(crate) a: Vector2,
    pub(crate) b: Vector2,
    pub(crate) start: f64,
    pub(crate) sweep: f64,
}

impl Conic {
    fn of_arc(arc: &Arc2) -> Self {
        let r = arc.radius();
        Self {
            center: arc.center(),
            a: Vector2::new(r, 0.0),
            b: Vector2::new(0.0, r),
            start: arc.start_angle(),
            sweep: arc.sweep(),
        }
    }

    fn of_elliptical(arc: &EllipticalArc2) -> Self {
        Self {
            center: arc.ellipse().center(),
            a: arc.ellipse().major_axis(),
            b: arc.ellipse().minor_axis(),
            start: arc.start_parameter(),
            sweep: arc.sweep(),
        }
    }

    pub(crate) fn span(&self) -> f64 {
        self.sweep.abs()
    }

    pub(crate) fn angle(&self, t: f64) -> f64 {
        self.start + t.clamp(0.0, self.span()) * self.sweep.signum()
    }

    /// The first derivative with respect to `t`, without clamping.
    pub(crate) fn d1(&self, t: f64) -> Vector2 {
        let (sin, cos) = (self.start + t * self.sweep.signum()).sin_cos();
        (self.b * cos - self.a * sin) * self.sweep.signum()
    }

    fn derivatives(&self, t: f64) -> (Vector2, Vector2) {
        let (sin, cos) = self.angle(t).sin_cos();
        let sign = self.sweep.signum();
        (
            (self.b * cos - self.a * sin) * sign,
            -(self.a * cos + self.b * sin),
        )
    }

    /// The length between `t0 <= t1` by adaptive Gauss–Legendre quadrature.
    fn length(&self, t0: f64, t1: f64, tolerance: &Tolerance) -> Result<f64> {
        let width = t1 - t0;
        if width <= 0.0 {
            return Ok(0.0);
        }
        let speed = |t: f64| self.d1(t).norm();
        let rule = |lo: f64, hi: f64| {
            let (mid, half) = (0.5 * (lo + hi), 0.5 * (hi - lo));
            half * GAUSS_NODES
                .iter()
                .zip(GAUSS_WEIGHTS)
                .map(|(x, w)| w * (speed(mid - half * x) + speed(mid + half * x)))
                .sum::<f64>()
        };
        let scale = self.a.norm().max(self.b.norm());
        let target = (0.25 * tolerance.linear).max(QUADRATURE_ULPS * f64::EPSILON * scale * width);
        // Split at the quadrant angles, where the speed has its extrema, so each piece is monotone.
        let quarter = FRAC_PI_2;
        let (from, to) = (
            self.start + t0 * self.sweep.signum(),
            self.start + t1 * self.sweep.signum(),
        );
        let (lo, hi) = (from.min(to), from.max(to));
        let mut cuts = vec![t0, t1];
        let mut k = (lo / quarter).ceil();
        while k * quarter < hi && cuts.len() < 8 {
            let t = (k * quarter - self.start) * self.sweep.signum();
            if t > t0 && t < t1 {
                cuts.push(t);
            }
            k += 1.0;
        }
        cuts.sort_by(f64::total_cmp);
        let mut stack: Vec<(f64, f64, f64)> = cuts
            .windows(2)
            .filter_map(|w| match w {
                [a, b] if b > a => Some((*a, *b, rule(*a, *b))),
                _ => None,
            })
            .collect();
        let mut total = 0.0;
        let mut splits = 0;
        while let Some((lo, hi, whole)) = stack.pop() {
            let mid = 0.5 * (lo + hi);
            let (left, right) = (rule(lo, mid), rule(mid, hi));
            if (left + right - whole).abs() <= target * (hi - lo) / width || mid <= lo || mid >= hi
            {
                total += left + right;
                continue;
            }
            splits += 1;
            if splits > MAX_QUADRATURE_INTERVALS {
                return Err(
                    Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "length")
                );
            }
            stack.push((lo, mid, left));
            stack.push((mid, hi, right));
        }
        Ok(total)
    }

    /// The parameter at length `s` in [0, total] by safeguarded Newton iteration.
    fn parameter_at_length(&self, s: f64, total: f64, tolerance: &Tolerance) -> Result<f64> {
        let span = self.span();
        if s <= 0.0 {
            return Ok(0.0);
        }
        if s >= total {
            return Ok(span);
        }
        let (mut lo, mut hi) = (0.0, span);
        let mut t = span * s / total;
        for _ in 0..MAX_ROOT_ITERATIONS {
            let residual = self.length(0.0, t, tolerance)? - s;
            if residual.abs() <= 0.5 * tolerance.linear {
                return Ok(t);
            }
            if residual > 0.0 {
                hi = t;
            } else {
                lo = t;
            }
            let newton = t - residual / self.d1(t).norm();
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                return Ok(t);
            }
            t = if newton > lo && newton < hi {
                newton
            } else {
                mid
            };
        }
        Err(Error::new(codes::PRECISION_INSUFFICIENT)
            .with_detail("operation", "parameter_at_length"))
    }
}

/// Validates a length measured from the start against a total length and clamps it.
fn clamp_length(s: f64, total: f64, tolerance: &Tolerance) -> Result<f64> {
    ensure_finite(&[s])?;
    if s < -tolerance.linear || s > total + tolerance.linear {
        return Err(Error::new(codes::LENGTH_OUT_OF_RANGE)
            .with_detail("length", s)
            .with_detail("total", total));
    }
    Ok(s.clamp(0.0, total))
}

/// The machinery behind the public curve traits; not nameable outside the crate, so the traits
/// cannot be implemented elsewhere.
pub(crate) mod sealed {
    use super::{Arc2, Conic, EllipticalArc2, Line2, Polyline2, Ray2, Segment2};

    /// One smooth curve with its own parameterization.
    #[derive(Debug, Clone, Copy)]
    pub enum Piece {
        /// A segment.
        Segment(Segment2),
        /// A ray.
        Ray(Ray2),
        /// A line.
        Line(Line2),
        /// A circular arc; a circle is its full counterclockwise arc.
        Arc(Arc2),
        /// An elliptical arc; an ellipse is its full counterclockwise arc.
        Elliptical(EllipticalArc2),
    }

    /// A single smooth piece or a polyline of pieces.
    #[derive(Debug, Clone, Copy)]
    pub enum Shape<'a> {
        /// One smooth curve.
        Piece(Piece),
        /// A polyline.
        Polyline(&'a Polyline2),
    }

    /// Gives the shape of a curve.
    pub trait Shaped {
        /// The shape of the curve.
        fn shape(&self) -> Shape<'_>;
    }

    impl Piece {
        /// The ellipse frame of an arc or elliptical arc.
        pub(crate) fn conic(&self) -> Option<Conic> {
            match self {
                Self::Arc(arc) => Some(Conic::of_arc(arc)),
                Self::Elliptical(arc) => Some(Conic::of_elliptical(arc)),
                _ => None,
            }
        }
    }
}

pub(crate) use sealed::{Piece, Shape, Shaped};

impl Piece {
    /// The parameter range.
    pub(crate) fn range(&self) -> (f64, f64) {
        match self {
            Self::Segment(_) => (0.0, 1.0),
            Self::Ray(_) => (0.0, f64::INFINITY),
            Self::Line(_) => (f64::NEG_INFINITY, f64::INFINITY),
            Self::Arc(arc) => (0.0, arc.sweep().abs()),
            Self::Elliptical(arc) => (0.0, arc.sweep().abs()),
        }
    }

    /// The point at `t`, clamped to the range.
    pub(crate) fn point(&self, t: f64) -> Point2 {
        match self {
            Self::Segment(c) => c.point_at(t),
            Self::Ray(c) => c.point_at(t),
            Self::Line(c) => c.point_at(t),
            Self::Arc(c) => c.point_at(t),
            Self::Elliptical(c) => c.point_at(t),
        }
    }

    /// The origin and the derivative of a straight piece: `point(t) = origin + direction·t`.
    pub(crate) fn linear(&self) -> Option<(Point2, Vector2)> {
        match self {
            Self::Segment(c) => Some((c.start(), c.end() - c.start())),
            Self::Ray(c) => Some((c.origin(), c.unit_direction())),
            Self::Line(c) => Some((c.point(), c.unit_direction())),
            Self::Arc(_) | Self::Elliptical(_) => None,
        }
    }

    fn derivatives(&self, t: f64) -> (Vector2, Vector2) {
        match (self.linear(), self.conic()) {
            (Some((_, direction)), _) => (direction, Vector2::zeros()),
            (None, Some(conic)) => conic.derivatives(t),
            (None, None) => (Vector2::zeros(), Vector2::zeros()),
        }
    }

    fn length_between(&self, t0: f64, t1: f64, tolerance: &Tolerance) -> Result<f64> {
        let (lo, hi) = self.range();
        let (a, b) = (t0.clamp(lo, hi), t1.clamp(lo, hi));
        let (a, b) = (a.min(b), a.max(b));
        match self {
            Self::Elliptical(arc) => Conic::of_elliptical(arc).length(a, b, tolerance),
            Self::Arc(arc) => Ok((b - a) * arc.radius()),
            _ => Ok((b - a) * self.derivatives(a).0.norm()),
        }
    }

    fn length(&self, tolerance: &Tolerance) -> Result<f64> {
        let (lo, hi) = self.range();
        if lo.is_finite() && hi.is_finite() {
            self.length_between(lo, hi, tolerance)
        } else {
            Ok(f64::INFINITY)
        }
    }

    fn parameter_at_length(&self, s: f64, tolerance: &Tolerance) -> Result<f64> {
        if let Self::Line(_) = self {
            ensure_finite(&[s])?;
            return Ok(s);
        }
        let total = self.length(tolerance)?;
        let s = clamp_length(s, total, tolerance)?;
        match self {
            Self::Elliptical(arc) => {
                Conic::of_elliptical(arc).parameter_at_length(s, total, tolerance)
            }
            Self::Arc(arc) => Ok((s / arc.radius()).min(arc.sweep().abs())),
            _ => Ok((s / self.derivatives(0.0).0.norm()).min(self.range().1)),
        }
    }
}

/// A polyline segment as a piece.
pub(crate) fn segment_piece(segment: PolylineSegment) -> Piece {
    match segment {
        PolylineSegment::Line(s) => Piece::Segment(s),
        PolylineSegment::Arc(a) => Piece::Arc(a),
    }
}

/// The segment index, the segment as a piece and its local parameter at polyline parameter `t`,
/// clamped to [0, n]; `None` only for an invalid polyline.
pub(crate) fn locate(polyline: &Polyline2, t: f64) -> Option<(usize, Piece, f64)> {
    let count = polyline.segment_count();
    let t = t.clamp(0.0, count as f64);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "t is clamped to [0, n] first"
    )]
    let index = (t.floor() as usize).min(count.checked_sub(1)?);
    let piece = segment_piece(polyline.segment(index)?);
    let (lo, hi) = piece.range();
    Some((index, piece, lo + (t - index as f64) * (hi - lo)))
}

/// The polyline parameter of local parameter `local` on segment `index`.
pub(crate) fn global_parameter(index: usize, piece: &Piece, local: f64) -> f64 {
    let (lo, hi) = piece.range();
    index as f64 + (local - lo) / (hi - lo)
}

impl Shape<'_> {
    /// The parameter range.
    pub(crate) fn range(&self) -> (f64, f64) {
        match self {
            Self::Piece(piece) => piece.range(),
            Self::Polyline(p) => (0.0, p.segment_count() as f64),
        }
    }

    /// The point at `t`, clamped to the range.
    pub(crate) fn point(&self, t: f64) -> Point2 {
        match self {
            Self::Piece(piece) => piece.point(t),
            Self::Polyline(p) => p.point_at(t),
        }
    }

    fn derivatives(&self, t: f64) -> (Vector2, Vector2) {
        match self {
            Self::Piece(piece) => piece.derivatives(t),
            Self::Polyline(p) => {
                locate(p, t).map_or((Vector2::zeros(), Vector2::zeros()), |(_, piece, local)| {
                    let (lo, hi) = piece.range();
                    let scale = hi - lo;
                    let (d1, d2) = piece.derivatives(local);
                    (d1 * scale, d2 * (scale * scale))
                })
            }
        }
    }

    fn length_between(&self, t0: f64, t1: f64, tolerance: &Tolerance) -> Result<f64> {
        ensure_finite(&[t0, t1])?;
        ensure_tolerance("linear", tolerance.linear)?;
        let p = match self {
            Self::Piece(piece) => return piece.length_between(t0, t1, tolerance),
            Self::Polyline(p) => p,
        };
        let (a, b) = (t0.min(t1), t0.max(t1));
        let (Some((first, _, from)), Some((last, _, to))) = (locate(p, a), locate(p, b)) else {
            return Ok(0.0);
        };
        let mut total = 0.0;
        for index in first..=last {
            let Some(segment) = p.segment(index) else {
                continue;
            };
            let piece = segment_piece(segment);
            let (lo, hi) = piece.range();
            let start = if index == first { from } else { lo };
            let end = if index == last { to } else { hi };
            total += piece.length_between(start, end, tolerance)?;
        }
        Ok(total)
    }

    fn length(&self, tolerance: &Tolerance) -> Result<f64> {
        ensure_tolerance("linear", tolerance.linear)?;
        match self {
            Self::Piece(piece) => piece.length(tolerance),
            Self::Polyline(p) => p
                .segments()
                .map(|s| segment_piece(s).length(tolerance))
                .sum(),
        }
    }

    fn parameter_at_length(&self, s: f64, tolerance: &Tolerance) -> Result<f64> {
        ensure_tolerance("linear", tolerance.linear)?;
        let p = match self {
            Self::Piece(piece) => return piece.parameter_at_length(s, tolerance),
            Self::Polyline(p) => p,
        };
        let s = clamp_length(s, self.length(tolerance)?, tolerance)?;
        let count = p.segment_count();
        let mut walked = 0.0;
        for (index, segment) in p.segments().enumerate() {
            let piece = segment_piece(segment);
            let length = piece.length(tolerance)?;
            if s <= walked + length || index + 1 == count {
                let local =
                    piece.parameter_at_length((s - walked).clamp(0.0, length), tolerance)?;
                return Ok(global_parameter(index, &piece, local));
            }
            walked += length;
        }
        Ok(count as f64)
    }
}

/// Evaluation of a 2D curve (CORE-GEOM-003), implemented by every curve kind and [`Curve2`].
///
/// The parameter `t` follows the parameterization of the kind and is clamped to its range. A
/// NaN parameter yields NaN results.
pub trait CurveEval: Shaped {
    /// The first derivative of the point with respect to `t`. It is never zero for a valid curve.
    #[must_use]
    fn derivative(&self, t: f64) -> Vector2 {
        self.shape().derivatives(t).0
    }

    /// The second derivative of the point with respect to `t`.
    #[must_use]
    fn second_derivative(&self, t: f64) -> Vector2 {
        self.shape().derivatives(t).1
    }

    /// The unit tangent in the direction of increasing `t`.
    #[must_use]
    fn tangent(&self, t: f64) -> Vector2 {
        unit(self.derivative(t))
    }

    /// The unit normal: the unit tangent turned a quarter turn counterclockwise, pointing to the
    /// left of the direction of travel.
    #[must_use]
    fn normal(&self, t: f64) -> Vector2 {
        perp(&self.tangent(t))
    }

    /// The signed curvature: positive where the curve turns counterclockwise (toward the
    /// normal), negative where it turns clockwise, 0 on straight parts. A circle of radius r
    /// has curvature 1/r.
    #[must_use]
    fn curvature(&self, t: f64) -> f64 {
        let (d1, d2) = self.shape().derivatives(t);
        let speed = d1.norm();
        if speed == 0.0 {
            return 0.0;
        }
        cross(&unit(d1), &(d2 / speed)) / speed
    }

    /// The total length; `f64::INFINITY` for a ray or line.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` when `tolerance.linear` is not finite and positive;
    /// `geom.precision.insufficient` when the length of an elliptical arc cannot be integrated
    /// to the stated accuracy.
    fn length(&self, tolerance: &Tolerance) -> Result<f64> {
        self.shape().length(tolerance)
    }

    /// The length of the curve between parameters `t0` and `t1`, in either order, after both are
    /// clamped to the range.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite parameter, otherwise as [`Self::length`].
    fn length_between(&self, t0: f64, t1: f64, tolerance: &Tolerance) -> Result<f64> {
        self.shape().length_between(t0, t1, tolerance)
    }

    /// The parameter at arc length `s` from the start of the range; the point at the result is
    /// within `tolerance.linear` of the true point at that length. A length within
    /// `tolerance.linear` outside [0, length] is clamped. For a line the length is measured from
    /// parameter 0, negative lengths backwards, and the result is `s`.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite length; `geom.length.out_of_range` for a length
    /// below 0 or above the total length by more than `tolerance.linear`; otherwise as
    /// [`Self::length`].
    fn parameter_at_length(&self, s: f64, tolerance: &Tolerance) -> Result<f64> {
        self.shape().parameter_at_length(s, tolerance)
    }
}

macro_rules! shaped {
    ($($ty:ty => |$c:ident| $piece:expr),* $(,)?) => {
        $(
            impl Shaped for $ty {
                fn shape(&self) -> Shape<'_> {
                    let $c = self;
                    Shape::Piece($piece)
                }
            }
            impl CurveEval for $ty {}
        )*
    };
}

shaped!(
    Segment2 => |c| Piece::Segment(*c),
    Ray2 => |c| Piece::Ray(*c),
    Line2 => |c| Piece::Line(*c),
    Circle2 => |c| Piece::Arc(c.to_arc()),
    Arc2 => |c| Piece::Arc(*c),
    Ellipse2 => |c| Piece::Elliptical(c.to_arc()),
    EllipticalArc2 => |c| Piece::Elliptical(*c),
);

impl Shaped for Polyline2 {
    fn shape(&self) -> Shape<'_> {
        Shape::Polyline(self)
    }
}
impl CurveEval for Polyline2 {}

impl Shaped for Curve2 {
    fn shape(&self) -> Shape<'_> {
        match self {
            Self::Segment(c) => c.shape(),
            Self::Ray(c) => c.shape(),
            Self::Line(c) => c.shape(),
            Self::Circle(c) => c.shape(),
            Self::Arc(c) => c.shape(),
            Self::Ellipse(c) => c.shape(),
            Self::EllipticalArc(c) => c.shape(),
            Self::Polyline(c) => c.shape(),
        }
    }
}
impl CurveEval for Curve2 {}

/// Whether a sweep magnitude is a full turn, so the curve ends where it starts.
pub(crate) fn is_full_turn(span: f64) -> bool {
    span == TAU
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, parameter};
    use crate::polyline::PolylineVertex;

    const TOL: Tolerance = Tolerance::DEFAULT;

    /// Reference length of a smooth piece by composite Simpson's rule on `n` intervals.
    fn simpson(c: &Curve2, a: f64, b: f64, n: usize) -> f64 {
        let h = (b - a) / n as f64;
        let f = |i: usize| c.derivative(a + h * i as f64).norm();
        let inner: f64 = (1..n)
            .map(|i| if i % 2 == 1 { 4.0 * f(i) } else { 2.0 * f(i) })
            .sum();
        h / 3.0 * (f(0) + inner + f(n))
    }

    /// Reference length between range start and `t`: Simpson for smooth kinds, exact per segment
    /// (constant speed) for polylines, distance for rays and lines.
    fn reference_length(c: &Curve2, t: f64) -> f64 {
        match c {
            Curve2::Polyline(p) => {
                let mut total = 0.0;
                for k in 0..p.segment_count() {
                    let k = k as f64;
                    let part = (t - k).clamp(0.0, 1.0);
                    total += part * c.derivative(k + 0.5).norm();
                }
                total
            }
            Curve2::Ray(_) | Curve2::Line(_) => t.abs(),
            _ => simpson(c, 0.0, t, 20_000),
        }
    }

    fn near_vertex(c: &Curve2, t: f64) -> bool {
        matches!(c, Curve2::Polyline(_)) && (t - t.round()).abs() < 2e-3
    }

    #[test]
    fn gauss_rule_is_exact_for_degree_15() {
        let integral: f64 = GAUSS_NODES
            .iter()
            .zip(GAUSS_WEIGHTS)
            .map(|(x, w)| 2.0 * w * x.powi(14))
            .sum();
        assert!((integral - 2.0 / 15.0).abs() < 1e-15);
        assert!((GAUSS_WEIGHTS.iter().sum::<f64>() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn known_values_of_simple_kinds() {
        let o = Point2::origin();
        let arc = Arc2::new(o, 2.0, 0.0, -FRAC_PI_2).unwrap();
        assert!((arc.curvature(0.3) + 0.5).abs() < 1e-15);
        assert!((arc.tangent(0.0) - Vector2::new(0.0, -1.0)).norm() < 1e-15);
        assert!((arc.normal(0.0) - Vector2::new(1.0, 0.0)).norm() < 1e-15);
        assert!((arc.length(&TOL).unwrap() - std::f64::consts::PI).abs() < 1e-15);
        let circle = Circle2::new(o, 3.0).unwrap();
        assert!((circle.curvature(1.0) - 1.0 / 3.0).abs() < 1e-15);
        let segment = Segment2::new(o, Point2::new(3.0, 4.0)).unwrap();
        assert_eq!(segment.curvature(0.5), 0.0);
        assert_eq!(segment.length(&TOL).unwrap(), 5.0);
        assert_eq!(segment.parameter_at_length(2.5, &TOL).unwrap(), 0.5);
        let ray = Ray2::new(o, Vector2::new(0.0, 2.0)).unwrap();
        assert_eq!(ray.length(&TOL).unwrap(), f64::INFINITY);
        assert_eq!(ray.parameter_at_length(7.0, &TOL).unwrap(), 7.0);
        let line = Line2::new(o, Vector2::x()).unwrap();
        assert_eq!(line.length(&TOL).unwrap(), f64::INFINITY);
        assert_eq!(line.parameter_at_length(-4.0, &TOL).unwrap(), -4.0);
        assert_eq!(line.length_between(-1.0, 2.0, &TOL).unwrap(), 3.0);
    }

    #[test]
    fn degenerate_arguments_are_rejected() {
        let segment = Segment2::new(Point2::origin(), Point2::new(1.0, 0.0)).unwrap();
        let code = |r: Result<f64>| r.unwrap_err().code();
        assert_eq!(
            code(segment.parameter_at_length(1.1, &TOL)),
            "geom.length.out_of_range"
        );
        assert_eq!(
            code(segment.parameter_at_length(-0.1, &TOL)),
            "geom.length.out_of_range"
        );
        assert_eq!(segment.parameter_at_length(1.0 + 1e-7, &TOL).unwrap(), 1.0);
        assert_eq!(
            code(segment.parameter_at_length(f64::NAN, &TOL)),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(segment.length_between(0.0, f64::INFINITY, &TOL)),
            "geom.input.not_finite"
        );
        let bad = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(code(segment.length(&bad)), "geom.tolerance.invalid");
        let ray = Ray2::new(Point2::origin(), Vector2::x()).unwrap();
        assert_eq!(
            code(ray.parameter_at_length(-1.0, &TOL)),
            "geom.length.out_of_range"
        );
    }

    #[test]
    fn polyline_derivatives_at_a_vertex_belong_to_the_next_segment() {
        let p = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::new(0.0, 0.0), 0.0),
                PolylineVertex::new(Point2::new(2.0, 0.0), 1.0),
                PolylineVertex::new(Point2::new(4.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        assert!((p.derivative(0.5) - Vector2::new(2.0, 0.0)).norm() < 1e-15);
        // Segment 1 is a counterclockwise half circle of radius 1, heading down at its start.
        assert!((p.tangent(1.0) - Vector2::new(0.0, -1.0)).norm() < 1e-12);
        assert!((p.curvature(1.0) - 1.0).abs() < 1e-12);
        assert!((p.derivative(2.0).norm() - std::f64::consts::PI).abs() < 1e-12);
        let length = p.length(&TOL).unwrap();
        assert!((length - (2.0 + std::f64::consts::PI)).abs() < 1e-12);
        assert!((p.parameter_at_length(2.0 + FRAC_PI_2, &TOL).unwrap() - 1.5).abs() < 1e-12);
    }

    #[test]
    fn full_ellipse_length_matches_the_series() {
        // Gauss–Kummer series: π(a + b) Σ C(1/2, n)² hⁿ with h = ((a − b)/(a + b))².
        for (a, ratio) in [(1.0_f64, 1.0_f64), (3.0, 0.5), (100.0, 0.05), (1.0, 1e-3)] {
            let b = a * ratio;
            let h = ((a - b) / (a + b)).powi(2);
            let (mut term, mut sum) = (1.0_f64, 1.0_f64);
            for n in 1..200_000 {
                let n = f64::from(n);
                let c = (0.5 - (n - 1.0)) / n;
                term *= c * c * h;
                sum += term;
                if term < 1e-18 {
                    break;
                }
            }
            let series = std::f64::consts::PI * (a + b) * sum;
            let e = Ellipse2::new(Point2::new(5.0, -2.0), Vector2::new(a, 0.0), ratio).unwrap();
            let length = e.length(&TOL).unwrap();
            assert!(
                (length - series).abs() <= TOL.linear,
                "{a} {ratio}: {length} vs {series}"
            );
        }
    }

    proptest! {
        #[test]
        fn derivatives_match_finite_differences(c in curve(), f in 0.01..0.99_f64) {
            let t = parameter(&c, f);
            prop_assume!(!near_vertex(&c, t));
            let h = 1e-4;
            let (d1, d2) = (c.derivative(t), c.second_derivative(t));
            let fd1 = (c.point_at(t + h) - c.point_at(t - h)) / (2.0 * h);
            let fd2 = (c.derivative(t + h) - c.derivative(t - h)) / (2.0 * h);
            let scale = 1.0 + d1.norm() + d2.norm();
            prop_assert!((fd1 - d1).norm() <= 1e-4 * scale, "{fd1} vs {d1}");
            prop_assert!((fd2 - d2).norm() <= 1e-4 * scale, "{fd2} vs {d2}");
        }

        #[test]
        fn tangent_normal_and_curvature_are_consistent(c in curve(), f in 0.01..0.99_f64) {
            let t = parameter(&c, f);
            prop_assume!(!near_vertex(&c, t));
            let (tangent, normal, d1) = (c.tangent(t), c.normal(t), c.derivative(t));
            prop_assert!((tangent.norm() - 1.0).abs() < 1e-12);
            prop_assert!((tangent - d1 / d1.norm()).norm() < 1e-12);
            prop_assert!((cross(&tangent, &normal) - 1.0).abs() < 1e-12);
            // Curvature is the rate of turning of the tangent per unit length.
            let h = 1e-5;
            let (before, after) = (c.tangent(t - h), c.tangent(t + h));
            let turning = cross(&before, &after).atan2(before.dot(&after));
            let rate = turning / (2.0 * h * d1.norm());
            let kappa = c.curvature(t);
            prop_assert!((rate - kappa).abs() <= 1e-4 * (1.0 + kappa.abs()), "{rate} vs {kappa}");
            match &c {
                Curve2::Circle(circle) => prop_assert!((kappa - 1.0 / circle.radius()).abs() < 1e-12),
                Curve2::Arc(arc) => prop_assert!((kappa - arc.sweep().signum() / arc.radius()).abs() < 1e-12),
                Curve2::Segment(_) | Curve2::Ray(_) | Curve2::Line(_) => prop_assert_eq!(kappa, 0.0),
                _ => {}
            }
        }

        #[test]
        fn lengths_match_the_reference_and_add_up(c in curve(), f in 0.0..=1.0_f64) {
            let t = parameter(&c, f);
            let (lo, hi) = c.parameter_range();
            let start = if lo.is_finite() { lo } else { 0.0 };
            let part = c.length_between(start, t, &TOL).unwrap();
            let reference = reference_length(&c, t);
            prop_assert!((part - reference).abs() <= TOL.linear + 1e-9 * reference, "{part} vs {reference}");
            let total = c.length(&TOL).unwrap();
            if hi.is_finite() {
                let rest = c.length_between(t, hi, &TOL).unwrap();
                prop_assert!((part + rest - total).abs() <= TOL.linear);
                prop_assert!((c.length_between(hi, t, &TOL).unwrap() - rest).abs() <= 1e-12 * total);
            } else {
                prop_assert_eq!(total, f64::INFINITY);
            }
        }

        #[test]
        fn parameter_at_length_is_within_the_linear_tolerance(c in curve(), f in 0.0..=1.0_f64) {
            let total = c.length(&TOL).unwrap();
            let s = if total.is_finite() { f * total } else { 100.0 * f };
            let t = c.parameter_at_length(s, &TOL).unwrap();
            let (lo, hi) = c.parameter_range();
            prop_assert!(t >= lo.max(-1e300) - 1e-12 && t <= hi);
            // The arc length to the result differs from s by at most the tolerance, so the
            // point is within the tolerance of the true point at length s.
            let reached = reference_length(&c, t);
            prop_assert!((reached - s).abs() <= TOL.linear, "{reached} vs {s}");
        }
    }
}
