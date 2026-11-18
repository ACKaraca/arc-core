//! Rational B-spline (NURBS) curves in 2D (CORE-GEOM-008).
//!
//! A [`Nurbs2`] of degree `p` has `n` control points `Pᵢ` with weights `wᵢ > 0` and a clamped
//! knot vector `u₀ ≤ … ≤ u_{n+p}` whose first and last values occur exactly `p + 1` times and
//! every interior value at most `p` times, so the curve starts at the first control point, ends at
//! the last and is continuous. The parameter range is `[u_p, u_n]`, the first and last knots.
//!
//! - Evaluation uses the de Boor algorithm on homogeneous points `(wᵢ·Pᵢ, wᵢ)`, whose steps are
//!   convex combinations. The weights of the `p + 1` points a span uses are divided by their
//!   largest one first, so no product overflows. Derivatives are those of the B-spline of the
//!   points relative to the evaluated point, `wᵢ·(Pᵢ − C)`, through the derivative control
//!   points, which avoids the cancellation of the quotient rule far from the origin.
//! - Knot insertion (Boehm's algorithm) forms each new control point as a convex combination of
//!   two old ones and keeps the points away from the knot exactly; splitting inserts the split
//!   parameter until its multiplicity is `p`. Bézier segments are computed per span as blossoms
//!   (polar forms) at the span's end knots, also by convex combinations.
//! - Knot removal solves the insertion equations from both ends and measures the inconsistency
//!   left in the middle; the change of the curve is at most that inconsistency, converted to a
//!   distance with the smallest weight and the extent of the affected points, and removal is
//!   accepted only while the sum of these bounds stays within `tolerance.linear`.
//! - Degree elevation elevates the Bézier segments with the closed binomial formula and then
//!   removes the knots that the original continuity allows, within `tolerance.linear`; knots
//!   that rounding keeps from being removed stay, so the curve is the same either way.
//! - Interpolation uses chord-length parameters and knots averaged from them; the banded system
//!   is solved without pivoting, which is stable for this totally positive matrix. The result
//!   passes through every point within `tolerance.linear` or the fit fails.
//! - Bounding boxes, foot points and flattening work on the Bézier segments: the extrema in x
//!   and y and the foot points are the roots of polynomials in Bernstein form, isolated by
//!   subdivision with the variation-diminishing property and refined by bisection.

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::bbox::Aabb2;
use crate::error::codes;
use crate::eval::{GAUSS_NODES, GAUSS_WEIGHTS, MAX_QUADRATURE_INTERVALS, MAX_ROOT_ITERATIONS};
use crate::{deserialize_through_new, ensure_finite};

/// A homogeneous point `(w·x, w·y, w)`.
pub(crate) type H = [f64; 3];

/// The share of the linear tolerance the length quadrature aims at, as for elliptical arcs.
const QUADRATURE_SHARE: f64 = 1.0 / 64.0;
/// The floor of the quadrature error target in units in the last place of the curve extent.
const QUADRATURE_ULPS: f64 = 64.0;
/// The deepest subdivision of a Bernstein polynomial when isolating roots; below a width of
/// `2^-48` of a segment, a remaining candidate is reported as one root.
const ROOT_DEPTH: usize = 48;
/// The most subdivision nodes the root isolation of one polynomial visits, per unit of degree.
const ROOT_NODES_PER_DEGREE: usize = 4096;
/// The rounding allowance of a polynomial coefficient, in units of the machine epsilon times
/// the magnitude of its terms.
const NOISE_ULPS: f64 = 64.0;
/// The deepest subdivision of a Bézier segment while flattening.
const FLATTEN_DEPTH: usize = 60;

fn at(values: &[f64], i: usize) -> f64 {
    values.get(i).copied().unwrap_or(f64::NAN)
}

fn hat(values: &[H], i: usize) -> H {
    values.get(i).copied().unwrap_or([f64::NAN; 3])
}

fn mix(a: H, b: H, t: f64) -> H {
    let s = 1.0 - t;
    [
        a[0] * s + b[0] * t,
        a[1] * s + b[1] * t,
        a[2] * s + b[2] * t,
    ]
}

fn dehomogenize(h: H) -> Point2 {
    Point2::new(h[0] / h[2], h[1] / h[2])
}

fn count_detail(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// The blossom of the degree-`q` piece on span `k` at the arguments `arg(1), …, arg(q)`; with
/// every argument equal to `t` it is the point at `t` (de Boor). `d` holds the `q + 1` local
/// points of indices `k − q ..= k`.
fn blossom(knots: &[f64], k: usize, q: usize, mut d: Vec<H>, arg: impl Fn(usize) -> f64) -> H {
    for r in 1..=q {
        for j in (r..=q).rev() {
            let m = k + j - q;
            let (lo, hi) = (at(knots, m), at(knots, m + q + 1 - r));
            let alpha = if hi > lo {
                (arg(r) - lo) / (hi - lo)
            } else {
                0.0
            };
            let previous = hat(&d, j - 1);
            if let Some(current) = d.get_mut(j) {
                *current = mix(previous, *current, alpha);
            }
        }
    }
    hat(&d, q)
}

/// The local derivative points of the degree-`q` local points `d` on span `k`: `q` points of
/// degree `q − 1`.
fn derivative_points(knots: &[f64], k: usize, q: usize, d: &[H]) -> Vec<H> {
    let qf = q as f64;
    d.windows(2)
        .enumerate()
        .map(|(j, pair)| {
            let m = k + 1 + j - q;
            let width = at(knots, m + q) - at(knots, m);
            let (a, b) = (hat(pair, 0), hat(pair, 1));
            let f = qf / width;
            [(b[0] - a[0]) * f, (b[1] - a[1]) * f, (b[2] - a[2]) * f]
        })
        .collect()
}

/// The nonzero B-spline basis functions of degree `p` at `u` on span `k` (Cox–de Boor in its
/// triangular form): `p + 1` values for the functions `k − p ..= k`.
fn basis(knots: &[f64], k: usize, p: usize, u: f64) -> Vec<f64> {
    let mut n = vec![0.0; p + 1];
    let mut left = vec![0.0; p + 1];
    let mut right = vec![0.0; p + 1];
    if let Some(first) = n.first_mut() {
        *first = 1.0;
    }
    for j in 1..=p {
        if let Some(l) = left.get_mut(j) {
            *l = u - at(knots, k + 1 - j);
        }
        if let Some(r) = right.get_mut(j) {
            *r = at(knots, k + j) - u;
        }
        let mut saved = 0.0;
        for r in 0..j {
            let denominator = at(&right, r + 1) + at(&left, j - r);
            let temp = if denominator == 0.0 {
                0.0
            } else {
                at(&n, r) / denominator
            };
            if let Some(value) = n.get_mut(r) {
                *value = saved + at(&right, r + 1) * temp;
            }
            saved = at(&left, j - r) * temp;
        }
        if let Some(value) = n.get_mut(j) {
            *value = saved;
        }
    }
    n
}

/// A Bézier segment of a NURBS curve: its parameter interval and its control points and weights.
#[derive(Debug, Clone)]
pub(crate) struct Bezier {
    /// The parameter at the start.
    pub lo: f64,
    /// The parameter at the end.
    pub hi: f64,
    /// The control points.
    pub points: Vec<Point2>,
    /// The weights, on the scale of the curve's weights.
    pub weights: Vec<f64>,
}

impl Bezier {
    /// Homogeneous points relative to `origin`, divided by `scale`, with the weights divided by
    /// their largest value.
    fn homogeneous(&self, origin: Point2, scale: f64) -> Vec<H> {
        let largest = self.weights.iter().fold(0.0_f64, |m, w| m.max(*w));
        self.points
            .iter()
            .zip(&self.weights)
            .map(|(p, w)| {
                let w = w / largest;
                [
                    w * ((p.x - origin.x) / scale),
                    w * ((p.y - origin.y) / scale),
                    w,
                ]
            })
            .collect()
    }

    /// The largest coordinate magnitude of the control points relative to `origin`.
    fn extent(&self, origin: Point2) -> f64 {
        self.points.iter().fold(0.0_f64, |m, p| {
            m.max((p.x - origin.x).abs()).max((p.y - origin.y).abs())
        })
    }
}

/// A NURBS curve in 2D: degree, clamped knot vector, control points and positive weights. See
/// the [module documentation](self).
///
/// Parameter `t` in `[u_p, u_n]`, the first and last knots; the curve starts at the first
/// control point and ends at the last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Nurbs2Raw")]
pub struct Nurbs2 {
    degree: usize,
    knots: Vec<f64>,
    control_points: Vec<Point2>,
    weights: Vec<f64>,
}

deserialize_through_new!(Nurbs2, Nurbs2Raw {
    degree: usize,
    knots: Vec<f64>,
    control_points: Vec<Point2>,
    weights: Vec<f64>,
});

/// A foot point of a query on a NURBS curve: one parameter, or an interval of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NurbsFoot {
    /// One parameter.
    Point(f64),
    /// Every parameter of the interval.
    Interval(f64, f64),
}

impl Nurbs2 {
    /// The largest degree accepted. Higher degrees are of no use for drawing geometry and make
    /// the Bernstein polynomials of foot point queries (degree `3p − 1`) ill-conditioned.
    pub const MAX_DEGREE: usize = 24;

    /// Creates a curve, with the control point count bounded by [`Limits::DEFAULT`].
    ///
    /// # Errors
    ///
    /// As [`Self::with_limits`].
    pub fn new(
        degree: usize,
        knots: Vec<f64>,
        control_points: Vec<Point2>,
        weights: Vec<f64>,
    ) -> Result<Self> {
        Self::with_limits(degree, knots, control_points, weights, &Limits::DEFAULT)
    }

    /// Creates a curve with at most `limits.array_length` control points.
    ///
    /// # Errors
    ///
    /// `geom.nurbs.too_many_control_points` for more control points than the limit;
    /// `geom.nurbs.degree_invalid` for a degree of 0, above [`Self::MAX_DEGREE`], or not below
    /// the number of control points; `geom.nurbs.weights_invalid` when the weights do not match
    /// the control points in number or one is not positive; `geom.nurbs.knots_invalid` for a
    /// knot vector that does not have `n + p + 1` values, decreases, is not clamped (first and
    /// last values exactly `p + 1` times), repeats an interior value more than `p` times, or
    /// has an empty range; `geom.input.not_finite` for a non-finite number;
    /// `geom.nurbs.degenerate` when all control points are equal; `geom.precision.insufficient`
    /// when the weights span more than the normal range of `f64` or a bound on the first and
    /// second derivatives (control point extent times `2p · w_max / w_min` divided by the
    /// shortest nonempty knot span, and its square) overflows.
    pub fn with_limits(
        degree: usize,
        knots: Vec<f64>,
        control_points: Vec<Point2>,
        weights: Vec<f64>,
        limits: &Limits,
    ) -> Result<Self> {
        let n = control_points.len();
        let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
        if n > limit || knots.len() > limit.saturating_add(Self::MAX_DEGREE + 1) {
            return Err(Error::new(codes::NURBS_TOO_MANY_CONTROL_POINTS)
                .with_detail("count", count_detail(n))
                .with_detail("limit", count_detail(limit)));
        }
        if degree == 0 || degree > Self::MAX_DEGREE || degree >= n {
            return Err(Error::new(codes::NURBS_DEGREE_INVALID)
                .with_detail("degree", count_detail(degree))
                .with_detail("count", count_detail(n)));
        }
        if weights.len() != n {
            return Err(Error::new(codes::NURBS_WEIGHTS_INVALID)
                .with_detail("weights", count_detail(weights.len())));
        }
        if knots.len() != n + degree + 1 {
            return Err(Error::new(codes::NURBS_KNOTS_INVALID)
                .with_detail("reason", "count")
                .with_detail("knots", count_detail(knots.len())));
        }
        ensure_finite(&knots)?;
        ensure_finite(&weights)?;
        for p in &control_points {
            ensure_finite(&[p.x, p.y])?;
        }
        // Written so that NaN fails.
        if !weights.iter().all(|w| *w > 0.0) {
            return Err(Error::new(codes::NURBS_WEIGHTS_INVALID).with_detail("reason", "sign"));
        }
        validate_knots(&knots, degree)?;
        let first = control_points
            .first()
            .copied()
            .unwrap_or_else(Point2::origin);
        if control_points.iter().all(|p| *p == first) {
            return Err(Error::new(codes::NURBS_DEGENERATE));
        }
        let curve = Self {
            degree,
            knots,
            control_points,
            weights,
        };
        curve.ensure_representable()?;
        Ok(curve)
    }

    fn ensure_representable(&self) -> Result<()> {
        let (wmin, wmax) = self
            .weights
            .iter()
            .fold((f64::INFINITY, 0.0_f64), |(lo, hi), w| {
                (lo.min(*w), hi.max(*w))
            });
        let ratio = wmax / wmin;
        let shortest = self
            .knots
            .windows(2)
            .filter_map(|w| match w {
                [a, b] if b > a => Some(b - a),
                _ => None,
            })
            .fold(f64::INFINITY, f64::min);
        let boxed = Aabb2::from_points(&self.control_points);
        // Halved first, so that the extent of points of opposite large coordinates is finite.
        let extent = ((boxed.max().x * 0.5 - boxed.min().x * 0.5)
            .max(boxed.max().y * 0.5 - boxed.min().y * 0.5))
            * 2.0;
        let growth = 2.0 * self.degree as f64 * ratio / shortest;
        let bound = extent * growth * growth.max(1.0);
        if wmin / wmax >= f64::MIN_POSITIVE && bound.is_finite() {
            Ok(())
        } else {
            Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "nurbs"))
        }
    }

    /// A curve of `degree` through the clamped uniform knot vector on [0, 1] (interior knots at
    /// `j / (n − p)`).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn from_control_points(
        degree: usize,
        control_points: Vec<Point2>,
        weights: Vec<f64>,
    ) -> Result<Self> {
        let n = control_points.len();
        if degree == 0 || degree >= n || degree > Self::MAX_DEGREE {
            return Err(Error::new(codes::NURBS_DEGREE_INVALID)
                .with_detail("degree", count_detail(degree))
                .with_detail("count", count_detail(n)));
        }
        let spans = n - degree;
        let knots = std::iter::repeat_n(0.0, degree + 1)
            .chain((1..spans).map(|j| j as f64 / spans as f64))
            .chain(std::iter::repeat_n(1.0, degree + 1))
            .collect();
        Self::new(degree, knots, control_points, weights)
    }

    /// The non-rational curve of `degree` that passes through `points` in order, at parameters
    /// proportional to the cumulative chord length on [0, 1], with knots averaged from the
    /// parameters. Each point is within `tolerance.linear` of the curve at its parameter.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a linear tolerance that is not finite and positive;
    /// `geom.input.not_finite` for a non-finite point; `geom.fit.points_invalid` for fewer
    /// points than `degree + 1` or two consecutive equal points; `geom.nurbs.degree_invalid`
    /// for a degree of 0 or above [`Self::MAX_DEGREE`]; `geom.nurbs.too_many_control_points`
    /// for more points than [`Limits::DEFAULT`] allows; `geom.precision.insufficient` when the
    /// chord lengths are so uneven that the parameters do not increase in `f64`, or the solved
    /// curve misses a point by more than `tolerance.linear`.
    pub fn interpolate(points: &[Point2], degree: usize, tolerance: &Tolerance) -> Result<Self> {
        crate::eval::ensure_tolerance("linear", tolerance.linear)?;
        for p in points {
            ensure_finite(&[p.x, p.y])?;
        }
        let chords: Vec<f64> = points
            .windows(2)
            .map(|w| match w {
                [a, b] => crate::length(&(b - a)),
                _ => 0.0,
            })
            .collect();
        if chords.contains(&0.0) {
            return Err(Error::new(codes::FIT_POINTS_INVALID).with_detail("reason", "coincident"));
        }
        // Accumulated relative to the largest chord, so the sum cannot overflow.
        let largest = chords.iter().fold(0.0_f64, |m, c| m.max(*c));
        let mut cumulative = vec![0.0];
        let mut sum = 0.0;
        for c in &chords {
            sum += c / largest;
            cumulative.push(sum);
        }
        let parameters: Vec<f64> = cumulative.iter().map(|s| s / sum).collect();
        Self::interpolate_at(points, &parameters, degree, tolerance)
    }

    /// The non-rational curve of `degree` through `points` at the strictly increasing
    /// `parameters`, with knots averaged from them.
    pub(crate) fn interpolate_at(
        points: &[Point2],
        parameters: &[f64],
        degree: usize,
        tolerance: &Tolerance,
    ) -> Result<Self> {
        let count = points.len();
        if degree == 0 || degree > Self::MAX_DEGREE {
            return Err(
                Error::new(codes::NURBS_DEGREE_INVALID).with_detail("degree", count_detail(degree))
            );
        }
        if count <= degree || parameters.len() != count {
            return Err(Error::new(codes::FIT_POINTS_INVALID)
                .with_detail("reason", "count")
                .with_detail("count", count_detail(count)));
        }
        let limit = usize::try_from(Limits::DEFAULT.array_length).unwrap_or(usize::MAX);
        if count > limit {
            return Err(Error::new(codes::NURBS_TOO_MANY_CONTROL_POINTS)
                .with_detail("count", count_detail(count)));
        }
        if !parameters.windows(2).all(|w| matches!(w, [a, b] if b > a)) {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "fit"));
        }
        let p = degree;
        let (first, last) = (at(parameters, 0), at(parameters, count - 1));
        let interior = (1..count - p).map(|j| {
            parameters
                .get(j..j + p)
                .map_or(f64::NAN, |w| w.iter().sum::<f64>() / p as f64)
        });
        let knots: Vec<f64> = std::iter::repeat_n(first, p + 1)
            .chain(interior)
            .chain(std::iter::repeat_n(last, p + 1))
            .collect();
        // The banded system, row r holding columns r − p ..= r + p.
        let width = 2 * p + 1;
        let mut band = vec![0.0; count * width];
        let origin = points.first().copied().unwrap_or_else(Point2::origin);
        let rhs: Vec<[f64; 2]> = points
            .iter()
            .map(|q| [q.x - origin.x, q.y - origin.y])
            .collect();
        for (row, u) in parameters.iter().enumerate() {
            let span = span_of(&knots, p, count, *u);
            for (j, value) in basis(&knots, span, p, *u).into_iter().enumerate() {
                let column = span + j - p;
                if value == 0.0 {
                    continue;
                }
                let offset = (column + p).checked_sub(row).filter(|o| *o < width);
                match offset.and_then(|o| band.get_mut(row * width + o)) {
                    Some(cell) => *cell = value,
                    None => {
                        return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                            .with_detail("operation", "fit"));
                    }
                }
            }
        }
        let solution = solve_banded(band, rhs, count, p)?;
        let control_points: Vec<Point2> = solution
            .iter()
            .map(|[x, y]| Point2::new(origin.x + x, origin.y + y))
            .collect();
        let curve = Self::new(p, knots, control_points, vec![1.0; count])?;
        for (q, u) in points.iter().zip(parameters) {
            let miss = crate::length(&(curve.point_at(*u) - q));
            if miss > tolerance.linear || miss.is_nan() {
                return Err(
                    Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "fit")
                );
            }
        }
        Ok(curve)
    }

    /// The degree.
    #[must_use]
    pub fn degree(&self) -> usize {
        self.degree
    }

    /// The knot vector, `n + p + 1` values.
    #[must_use]
    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    /// The control points.
    #[must_use]
    pub fn control_points(&self) -> &[Point2] {
        &self.control_points
    }

    /// The weights, one per control point.
    #[must_use]
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    /// Whether the weights differ, so the curve is not a polynomial B-spline.
    #[must_use]
    pub fn is_rational(&self) -> bool {
        let first = self.weights.first().copied().unwrap_or(1.0);
        self.weights.iter().any(|w| *w != first)
    }

    /// The parameter range, the first and last knots.
    #[must_use]
    pub fn parameter_range(&self) -> (f64, f64) {
        (
            self.knots.first().copied().unwrap_or(0.0),
            self.knots.last().copied().unwrap_or(0.0),
        )
    }

    /// Whether the first and last control points are equal, so the curve ends where it starts.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.control_points.first() == self.control_points.last()
    }

    /// The span index `k` with `u_k ≤ t < u_{k+1}`, in `p ..= n − 1`.
    fn span(&self, t: f64) -> usize {
        span_of(&self.knots, self.degree, self.control_points.len(), t)
    }

    /// The `p + 1` homogeneous points of span `k` relative to `origin`, with the weights divided
    /// by the largest of them, and that largest weight.
    fn window(&self, k: usize, origin: Point2) -> (Vec<H>, f64) {
        let range = k - self.degree..=k;
        let largest = self
            .weights
            .get(range.clone())
            .map_or(1.0, |w| w.iter().fold(0.0_f64, |m, x| m.max(*x)));
        let points = self
            .control_points
            .get(range.clone())
            .unwrap_or_default()
            .iter()
            .zip(self.weights.get(range).unwrap_or_default())
            .map(|(p, w)| {
                let w = w / largest;
                [w * (p.x - origin.x), w * (p.y - origin.y), w]
            })
            .collect();
        (points, largest)
    }

    /// The point at `t`, clamped to the parameter range; the ends are the first and last
    /// control points exactly. A NaN parameter gives a NaN point.
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        let (lo, hi) = self.parameter_range();
        if t.is_nan() {
            return Point2::new(f64::NAN, f64::NAN);
        }
        if t <= lo {
            return self
                .control_points
                .first()
                .copied()
                .unwrap_or_else(Point2::origin);
        }
        if t >= hi {
            return self
                .control_points
                .last()
                .copied()
                .unwrap_or_else(Point2::origin);
        }
        let k = self.span(t);
        let (local, _) = self.window(k, Point2::origin());
        dehomogenize(blossom(&self.knots, k, self.degree, local, |_| t))
    }

    /// The point and the first and second derivatives at `t`, clamped to the range.
    pub(crate) fn derivatives(&self, t: f64) -> (Point2, Vector2, Vector2) {
        if t.is_nan() {
            let nan = Vector2::new(f64::NAN, f64::NAN);
            return (Point2::new(f64::NAN, f64::NAN), nan, nan);
        }
        let (lo, hi) = self.parameter_range();
        let t = t.clamp(lo, hi);
        let point = self.point_at(t);
        let p = self.degree;
        let k = self.span(t);
        let (local, _) = self.window(k, point);
        let w = blossom(&self.knots, k, p, local.clone(), |_| t)[2];
        let first = derivative_points(&self.knots, k, p, &local);
        let d1 = blossom(&self.knots, k, p - 1, first.clone(), |_| t);
        let d2 = if p >= 2 {
            let second = derivative_points(&self.knots, k, p - 1, &first);
            blossom(&self.knots, k, p - 2, second, |_| t)
        } else {
            [0.0; 3]
        };
        let velocity = Vector2::new(d1[0], d1[1]) / w;
        let acceleration = (Vector2::new(d2[0], d2[1]) - velocity * (2.0 * d1[2])) / w;
        (point, velocity, acceleration)
    }

    /// The knots, control points and weights after inserting `u` `times` times by Boehm's
    /// algorithm: each new point is a convex combination of two old ones (in homogeneous form,
    /// with the weights of the affected points divided by their largest), and points away from
    /// `u` are kept exactly. `u` lies inside the range with multiplicity plus `times` at most `p`.
    fn inserted(&self, u: f64, times: usize) -> (Vec<f64>, Vec<Point2>, Vec<f64>) {
        let p = self.degree;
        let mut knots = self.knots.clone();
        let mut points = self.control_points.clone();
        let mut weights = self.weights.clone();
        for _ in 0..times {
            let k = span_of(&knots, p, points.len(), u);
            let s = knots.iter().filter(|x| **x == u).count();
            let (lo, hi) = (k + 1 - p, k - s);
            let largest = weights
                .get(lo - 1..=hi)
                .map_or(1.0, |w| w.iter().fold(0.0_f64, |m, x| m.max(*x)));
            let h = |i: usize| -> H {
                let (pt, w) = (
                    points.get(i).copied().unwrap_or_else(Point2::origin),
                    weights.get(i).copied().unwrap_or(1.0) / largest,
                );
                [w * pt.x, w * pt.y, w]
            };
            let fresh: Vec<H> = (lo..=hi)
                .map(|i| {
                    let alpha = (u - at(&knots, i)) / (at(&knots, i + p) - at(&knots, i));
                    mix(h(i - 1), h(i), alpha)
                })
                .collect();
            // Points lo ..= hi − 1 are replaced and one point is added.
            points.splice(lo..hi, fresh.iter().map(|v| dehomogenize(*v)));
            weights.splice(lo..hi, fresh.iter().map(|v| v[2] * largest));
            knots.insert(k + 1, u);
        }
        (knots, points, weights)
    }

    /// The distinct knot values in increasing order with their multiplicities.
    fn runs(&self) -> Vec<(f64, usize)> {
        self.knots
            .chunk_by(|a, b| a == b)
            .filter_map(|run| run.first().map(|v| (*v, run.len())))
            .collect()
    }

    /// The distinct knot values, ends included.
    pub(crate) fn breakpoints(&self) -> Vec<f64> {
        self.runs().into_iter().map(|(v, _)| v).collect()
    }

    /// The interior knot values of multiplicity `p`, where the curve may have a corner.
    pub(crate) fn corner_knots(&self) -> Vec<f64> {
        let runs = self.runs();
        let last = runs.len().saturating_sub(1);
        runs.into_iter()
            .enumerate()
            .filter(|(i, (_, m))| *i != 0 && *i != last && *m == self.degree)
            .map(|(_, (v, _))| v)
            .collect()
    }

    /// The curve as Bézier segments in parameter order.
    pub(crate) fn bezier_segments(&self) -> Vec<Bezier> {
        let p = self.degree;
        let runs = self.runs();
        let first = self.control_points.first().copied();
        let end = self.control_points.last().copied();
        let segments = runs.len().saturating_sub(1);
        (0..segments)
            .map(|s| {
                let lo = runs.get(s).map_or(f64::NAN, |r| r.0);
                let hi = runs.get(s + 1).map_or(f64::NAN, |r| r.0);
                let k = self.span(lo);
                let (local, largest) = self.window(k, Point2::origin());
                // Bézier point j is the blossom at (lo^(p − j), hi^j): a convex computation.
                let (mut points, weights): (Vec<Point2>, Vec<f64>) = (0..=p)
                    .map(|j| {
                        let h = blossom(&self.knots, k, p, local.clone(), |r| {
                            if r <= p - j { lo } else { hi }
                        });
                        (dehomogenize(h), h[2] * largest)
                    })
                    .unzip();
                // The curve's ends are its first and last control points exactly.
                if s == 0
                    && let (Some(slot), Some(first)) = (points.first_mut(), first)
                {
                    *slot = first;
                }
                if s + 1 == segments
                    && let (Some(slot), Some(end)) = (points.last_mut(), end)
                {
                    *slot = end;
                }
                Bezier {
                    lo,
                    hi,
                    points,
                    weights,
                }
            })
            .collect()
    }

    fn rebuilt(
        &self,
        degree: usize,
        knots: Vec<f64>,
        points: Vec<Point2>,
        weights: Vec<f64>,
    ) -> Result<Self> {
        Self::new(degree, knots, points, weights)
    }

    /// The same curve with the knot `u` inserted `times` times.
    ///
    /// # Errors
    ///
    /// `geom.parameter.out_of_range` unless `u` lies strictly inside the parameter range;
    /// `geom.nurbs.knots_invalid` when the multiplicity of `u` would exceed the degree;
    /// otherwise as [`Self::new`] for the larger curve.
    pub fn insert_knot(&self, u: f64, times: usize) -> Result<Self> {
        let (lo, hi) = self.parameter_range();
        if !(u > lo && u < hi) {
            return Err(Error::new(codes::PARAMETER_OUT_OF_RANGE).with_detail("parameter", u));
        }
        let existing = self.knots.iter().filter(|k| **k == u).count();
        if existing.saturating_add(times) > self.degree {
            return Err(Error::new(codes::NURBS_KNOTS_INVALID)
                .with_detail("reason", "multiplicity")
                .with_detail("multiplicity", count_detail(existing.saturating_add(times))));
        }
        if times == 0 {
            return Ok(self.clone());
        }
        let (knots, points, weights) = self.inserted(u, times);
        self.rebuilt(self.degree, knots, points, weights)
    }

    /// The two curves on either side of parameter `t`, which keep their parameters: the first
    /// on `[u_p, t]`, the second on `[t, u_n]`. They meet at the point at `t` exactly.
    ///
    /// # Errors
    ///
    /// `geom.parameter.out_of_range` unless `t` lies strictly inside the parameter range;
    /// `geom.nurbs.degenerate` when a piece has all control points equal.
    pub fn split_at(&self, t: f64) -> Result<(Self, Self)> {
        let (lo, hi) = self.parameter_range();
        if !(t > lo && t < hi) {
            return Err(Error::new(codes::PARAMETER_OUT_OF_RANGE).with_detail("parameter", t));
        }
        let p = self.degree;
        let existing = self.knots.iter().filter(|k| **k == t).count();
        let (knots, points, weights) = self.inserted(t, p.saturating_sub(existing));
        let a = knots.partition_point(|k| *k < t);
        let invalid = || Error::new(codes::PARAMETER_OUT_OF_RANGE).with_detail("parameter", t);
        let left_knots: Vec<f64> = knots
            .get(..a + p)
            .ok_or_else(invalid)?
            .iter()
            .copied()
            .chain([t])
            .collect();
        let right_knots: Vec<f64> = std::iter::once(t)
            .chain(knots.get(a..).ok_or_else(invalid)?.iter().copied())
            .collect();
        let left = self.rebuilt(
            p,
            left_knots,
            points.get(..a).ok_or_else(invalid)?.to_vec(),
            weights.get(..a).ok_or_else(invalid)?.to_vec(),
        )?;
        let right = self.rebuilt(
            p,
            right_knots,
            points.get(a - 1..).ok_or_else(invalid)?.to_vec(),
            weights.get(a - 1..).ok_or_else(invalid)?.to_vec(),
        )?;
        Ok((left, right))
    }

    /// The same point set traversed backward. The knots are negated and reversed, so the range
    /// becomes `[−u_n, −u_p]` and `reversed.point_at(−t) = point_at(t)` exactly; reversing twice
    /// gives the original curve.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            degree: self.degree,
            // Adding zero turns −0.0 into +0.0.
            knots: self.knots.iter().rev().map(|k| -k + 0.0).collect(),
            control_points: self.control_points.iter().rev().copied().collect(),
            weights: self.weights.iter().rev().copied().collect(),
        }
    }

    /// One removal of the knot `u`, with the bound on the change of the curve, or `None` when
    /// `u` is not a knot or the new weights would not be positive.
    fn remove_once(&self, u: f64) -> Option<(Self, f64)> {
        let p = self.degree;
        let s = self.knots.iter().filter(|k| **k == u).count();
        if s == 0 {
            return None;
        }
        let r = self.knots.partition_point(|k| *k <= u).checked_sub(1)?;
        let first = r.checked_sub(p)?;
        let last = r.checked_sub(s)?;
        let before = first.checked_sub(1)?;
        let origin = *self.control_points.get(before)?;
        let local = before..=last + 1;
        let largest = self
            .weights
            .get(local.clone())?
            .iter()
            .fold(0.0_f64, |m, w| m.max(*w));
        let homogeneous = |i: usize| -> Option<H> {
            let point = self.control_points.get(i)?;
            let w = self.weights.get(i)? / largest;
            Some([w * (point.x - origin.x), w * (point.y - origin.y), w])
        };
        let alpha =
            |i: usize| (u - at(&self.knots, i)) / (at(&self.knots, i + p + 1) - at(&self.knots, i));
        let unknowns = last - first;
        let reach = (before..=last + 1)
            .filter_map(|i| self.control_points.get(i))
            .fold(0.0_f64, |m, point| m.max(crate::length(&(point - origin))));
        let known = (homogeneous(before)?, homogeneous(last + 1)?);
        let old_min = (before..=last + 1)
            .filter_map(|i| homogeneous(i).map(|h| h[2]))
            .fold(f64::INFINITY, f64::min);
        // The equations solved from both ends, meeting at `split`; every meeting point is tried
        // and the one with the smallest bound is kept, since dividing by a small α or 1 − α
        // amplifies rounding.
        let solve = |split: usize| -> Option<(Vec<H>, f64)> {
            // q[j] holds Q_{first − 1 + j}, j = 0 ..= unknowns + 1.
            let mut q: Vec<H> = vec![[0.0; 3]; unknowns + 2];
            *q.first_mut()? = known.0;
            *q.last_mut()? = known.1;
            for j in 1..=split {
                let i = first + j - 1;
                let (a, h, previous) = (alpha(i), homogeneous(i)?, hat(&q, j - 1));
                *q.get_mut(j)? = std::array::from_fn(|c| (h[c] - (1.0 - a) * previous[c]) / a);
            }
            for j in (split + 1..=unknowns).rev() {
                let i = first + j;
                let (a, h, next) = (alpha(i), homogeneous(i)?, hat(&q, j + 1));
                *q.get_mut(j)? = std::array::from_fn(|c| (h[c] - a * next[c]) / (1.0 - a));
            }
            let check = first + split;
            let (a, h) = (alpha(check), homogeneous(check)?);
            let (upper, lower) = (hat(&q, split + 1), hat(&q, split));
            let delta: H = std::array::from_fn(|c| h[c] - (a * upper[c] + (1.0 - a) * lower[c]));
            let wmin = q.iter().map(|h| h[2]).fold(old_min, f64::min);
            if wmin <= 0.0 || wmin.is_nan() {
                return None;
            }
            let bound = (delta[0].hypot(delta[1]) + reach * delta[2].abs()) / wmin;
            bound.is_finite().then_some((q, bound))
        };
        let (q, bound) = (0..=unknowns)
            .filter_map(solve)
            .min_by(|x, y| x.1.total_cmp(&y.1))?;
        let inner = q.get(1..=unknowns)?;
        let points: Vec<Point2> = self
            .control_points
            .get(..first)?
            .iter()
            .copied()
            .chain(inner.iter().map(|h| origin + dehomogenize(*h).coords))
            .chain(self.control_points.get(last + 1..)?.iter().copied())
            .collect();
        let weights: Vec<f64> = self
            .weights
            .get(..first)?
            .iter()
            .copied()
            .chain(inner.iter().map(|h| h[2] * largest))
            .chain(self.weights.get(last + 1..)?.iter().copied())
            .collect();
        let mut knots = self.knots.clone();
        knots.remove(r);
        let curve = Self::new(p, knots, points, weights).ok()?;
        Some((curve, bound))
    }

    /// The curve with the knot `u` removed `times` times, when the curve moves by at most
    /// `tolerance.linear`; the bound used is stated in the [module documentation](self).
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a linear tolerance that is not finite and positive;
    /// `geom.parameter.out_of_range` unless `u` lies strictly inside the parameter range;
    /// `geom.nurbs.knot_not_removable` when `u` is not a knot `times` times or the removals
    /// would move the curve by more than the tolerance, with the detail `removable`, the number
    /// of removals that stay within it.
    pub fn remove_knot(&self, u: f64, times: usize, tolerance: &Tolerance) -> Result<Self> {
        crate::eval::ensure_tolerance("linear", tolerance.linear)?;
        let (lo, hi) = self.parameter_range();
        if !(u > lo && u < hi) {
            return Err(Error::new(codes::PARAMETER_OUT_OF_RANGE).with_detail("parameter", u));
        }
        let mut current = self.clone();
        let mut spent = 0.0;
        for done in 0..times {
            match current.remove_once(u) {
                Some((next, bound)) if spent + bound <= tolerance.linear => {
                    current = next;
                    spent += bound;
                }
                _ => {
                    return Err(Error::new(codes::NURBS_KNOT_NOT_REMOVABLE)
                        .with_detail("parameter", u)
                        .with_detail("removable", count_detail(done)));
                }
            }
        }
        Ok(current)
    }

    /// The same curve with its degree raised by `times`. Interior knots get the multiplicity
    /// that keeps the original continuity, except where removing a knot would move the curve,
    /// by rounding, more than `tolerance.linear`; such knots are kept.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a linear tolerance that is not finite and positive;
    /// `geom.nurbs.degree_invalid` when the new degree exceeds [`Self::MAX_DEGREE`]; otherwise
    /// as [`Self::new`] for the larger curve.
    pub fn elevate_degree(&self, times: usize, tolerance: &Tolerance) -> Result<Self> {
        crate::eval::ensure_tolerance("linear", tolerance.linear)?;
        if times == 0 {
            return Ok(self.clone());
        }
        let p = self.degree;
        let q = p.saturating_add(times);
        if q > Self::MAX_DEGREE {
            return Err(
                Error::new(codes::NURBS_DEGREE_INVALID).with_detail("degree", count_detail(q))
            );
        }
        let segments = self.bezier_segments();
        let mut points = Vec::new();
        let mut weights = Vec::new();
        for (s, segment) in segments.iter().enumerate() {
            let largest = segment.weights.iter().fold(0.0_f64, |m, w| m.max(*w));
            let h: Vec<H> = segment
                .points
                .iter()
                .zip(&segment.weights)
                .map(|(pt, w)| {
                    let w = w / largest;
                    [w * pt.x, w * pt.y, w]
                })
                .collect();
            for i in 0..=q {
                if s > 0 && i == 0 {
                    continue;
                }
                let mut sum = [0.0; 3];
                for j in i.saturating_sub(times)..=i.min(p) {
                    let c = binomial(p, j) * binomial(times, i - j) / binomial(q, i);
                    let hj = hat(&h, j);
                    for (acc, v) in sum.iter_mut().zip(hj) {
                        *acc += c * v;
                    }
                }
                points.push(dehomogenize(sum));
                weights.push(sum[2] * largest);
            }
        }
        // The curve's ends stay exact.
        if let (Some(slot), Some(first)) = (points.first_mut(), self.control_points.first()) {
            *slot = *first;
        }
        if let (Some(slot), Some(last)) = (points.last_mut(), self.control_points.last()) {
            *slot = *last;
        }
        let runs = self.runs();
        let last = runs.len().saturating_sub(1);
        let knots: Vec<f64> = runs
            .iter()
            .enumerate()
            .flat_map(|(i, (v, _))| {
                std::iter::repeat_n(*v, if i == 0 || i == last { q + 1 } else { q })
            })
            .collect();
        let mut current = Self::new(q, knots, points, weights)?;
        let mut spent = 0.0;
        for (i, (u, multiplicity)) in runs.iter().enumerate() {
            if i == 0 || i == last {
                continue;
            }
            for _ in 0..p - multiplicity {
                match current.remove_once(*u) {
                    Some((next, bound)) if spent + bound <= tolerance.linear => {
                        current = next;
                        spent += bound;
                    }
                    _ => break,
                }
            }
        }
        Ok(current)
    }

    /// The image of the curve under a map of its control points (an affine map keeps a NURBS
    /// curve a NURBS curve with the same knots and weights).
    pub(crate) fn mapped(&self, map: impl Fn(&Point2) -> Point2) -> Result<Self> {
        Self::new(
            self.degree,
            self.knots.clone(),
            self.control_points.iter().map(map).collect(),
            self.weights.clone(),
        )
    }

    /// The exact bounding box: the ends of the Bézier segments and their extrema in x and y,
    /// found as roots of the numerators of the derivatives. It contains the curve up to the
    /// rounding of the evaluated points.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        let mut points: Vec<Point2> = Vec::new();
        for segment in self.bezier_segments() {
            points.extend(segment.points.first());
            points.extend(segment.points.last());
            let Some(origin) = segment.points.first().copied() else {
                continue;
            };
            let scale = segment.extent(origin);
            if scale == 0.0 {
                continue;
            }
            let h = segment.homogeneous(origin, scale);
            let w: Vec<f64> = h.iter().map(|v| v[2]).collect();
            let dw = bernstein_derivative(&w);
            for axis in 0..2 {
                let a: Vec<f64> = h.iter().map(|v| v[axis]).collect();
                let numerator = bernstein_sub(
                    &bernstein_mul(&bernstein_derivative(&a), &w),
                    &bernstein_mul(&a, &dw),
                );
                let abs = |v: &[f64]| -> Vec<f64> { v.iter().map(|x| x.abs()).collect() };
                let magnitude = bernstein_add(
                    &bernstein_mul(&abs(&bernstein_derivative(&a)), &w),
                    &bernstein_mul(&abs(&a), &abs(&dw)),
                );
                if let Ok(roots) = bernstein_roots(&numerator, &noise_of(&magnitude)) {
                    for (u0, u1) in roots {
                        for u in [u0, u1] {
                            points.push(self.point_at(segment.lo + u * (segment.hi - segment.lo)));
                        }
                    }
                }
            }
        }
        Aabb2::from_points(&points)
    }

    /// The foot points of `query`: parameters where the vector to the query is perpendicular to
    /// the tangent, as roots of `W·(A·A′) − W′·|A|²` per Bézier segment (`A` the homogeneous
    /// points relative to the query, `W` the weight), sorted by parameter.
    pub(crate) fn feet(&self, query: Point2) -> Result<Vec<NurbsFoot>> {
        let mut out = Vec::new();
        for segment in self.bezier_segments() {
            let scale = segment.extent(query);
            if scale == 0.0 || !scale.is_finite() {
                continue;
            }
            let h = segment.homogeneous(query, scale);
            let column = |c: usize| -> Vec<f64> { h.iter().map(|v| v[c]).collect() };
            let (ax, ay, w) = (column(0), column(1), column(2));
            let (dx, dy, dw) = (
                bernstein_derivative(&ax),
                bernstein_derivative(&ay),
                bernstein_derivative(&w),
            );
            let dot = bernstein_add(&bernstein_mul(&ax, &dx), &bernstein_mul(&ay, &dy));
            let square = bernstein_add(&bernstein_mul(&ax, &ax), &bernstein_mul(&ay, &ay));
            let g = bernstein_sub(&bernstein_mul(&w, &dot), &bernstein_mul(&dw, &square));
            // The magnitude of the terms, for the rounding allowance.
            let abs = |v: &[f64]| -> Vec<f64> { v.iter().map(|x| x.abs()).collect() };
            let magnitude = bernstein_add(
                &bernstein_mul(
                    &abs(&w),
                    &bernstein_add(
                        &bernstein_mul(&abs(&ax), &abs(&dx)),
                        &bernstein_mul(&abs(&ay), &abs(&dy)),
                    ),
                ),
                &bernstein_mul(&abs(&dw), &square),
            );
            let noise = noise_of(&magnitude);
            let width = segment.hi - segment.lo;
            let to_t = |u: f64| {
                if u >= 1.0 {
                    segment.hi
                } else {
                    segment.lo + u * width
                }
            };
            for (u0, u1) in bernstein_roots(&g, &noise)? {
                out.push(if u1 - u0 > 1e-9 {
                    NurbsFoot::Interval(to_t(u0), to_t(u1))
                } else {
                    NurbsFoot::Point(to_t(0.5 * (u0 + u1)))
                });
            }
            for (u, value, allowance) in [
                (0.0, g.first(), noise.first()),
                (1.0, g.last(), noise.last()),
            ] {
                if let (Some(v), Some(n)) = (value, allowance)
                    && v.abs() <= *n
                {
                    out.push(NurbsFoot::Point(to_t(u)));
                }
            }
        }
        out.sort_by(|a, b| foot_start(a).total_cmp(&foot_start(b)));
        Ok(out)
    }

    /// The length between parameters `t0 <= t1` (clamped), by adaptive Gauss–Legendre
    /// quadrature on each knot span.
    pub(crate) fn length_between(&self, t0: f64, t1: f64, tolerance: &Tolerance) -> Result<f64> {
        let (lo, hi) = self.parameter_range();
        let (a, b) = (t0.clamp(lo, hi), t1.clamp(lo, hi));
        let (a, b) = (a.min(b), a.max(b));
        if b <= a {
            return Ok(0.0);
        }
        let speed = |t: f64| crate::length(&self.derivatives(t).1);
        let rule = |lo: f64, hi: f64| {
            let (mid, half) = (0.5 * (lo + hi), 0.5 * (hi - lo));
            half * GAUSS_NODES
                .iter()
                .zip(GAUSS_WEIGHTS)
                .map(|(x, w)| w * (speed(mid - half * x) + speed(mid + half * x)))
                .sum::<f64>()
        };
        let boxed = Aabb2::from_points(&self.control_points);
        let scale = (boxed.max() - boxed.min()).abs().max();
        let width = b - a;
        let target =
            (QUADRATURE_SHARE * tolerance.linear).max(QUADRATURE_ULPS * f64::EPSILON * scale);
        let mut cuts: Vec<f64> = self
            .breakpoints()
            .into_iter()
            .filter(|k| *k > a && *k < b)
            .collect();
        cuts.push(a);
        cuts.push(b);
        cuts.sort_by(f64::total_cmp);
        let mut stack: Vec<(f64, f64, f64)> = cuts
            .windows(2)
            .filter_map(|w| match w {
                [x, y] if y > x => Some((*x, *y, rule(*x, *y))),
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

    /// The parameter at length `s` in `[0, total]` from the start, by safeguarded Newton
    /// iteration on the length.
    pub(crate) fn parameter_at_length(
        &self,
        s: f64,
        total: f64,
        tolerance: &Tolerance,
    ) -> Result<f64> {
        let (lo, hi) = self.parameter_range();
        if s <= 0.0 {
            return Ok(lo);
        }
        if s >= total {
            return Ok(hi);
        }
        let (mut a, mut b) = (lo, hi);
        let mut t = lo + (hi - lo) * s / total;
        for _ in 0..MAX_ROOT_ITERATIONS {
            let residual = self.length_between(lo, t, tolerance)? - s;
            if residual.abs() <= 0.5 * tolerance.linear {
                return Ok(t);
            }
            if residual > 0.0 {
                b = t;
            } else {
                a = t;
            }
            let newton = t - residual / crate::length(&self.derivatives(t).1);
            let mid = 0.5 * (a + b);
            if mid <= a || mid >= b {
                return Ok(t);
            }
            t = if newton > a && newton < b {
                newton
            } else {
                mid
            };
        }
        Err(Error::new(codes::PRECISION_INSUFFICIENT)
            .with_detail("operation", "parameter_at_length"))
    }

    /// The flattened vertices with their parameters, each Bézier segment subdivided until its
    /// control polygon lies within half of `chord` of its chord (the margin covers the way back,
    /// from the chord to the curve) and its legs turn by at most `angle`
    /// (the curve lies in the hull and its tangents in the cone of the legs).
    pub(crate) fn flatten(
        &self,
        chord: f64,
        angle: f64,
        limit: usize,
    ) -> Result<Vec<(f64, Point2)>> {
        let too_many = || {
            Error::new(codes::FLATTEN_TOO_MANY_VERTICES).with_detail("limit", count_detail(limit))
        };
        let (lo, hi) = self.parameter_range();
        let boxed = Aabb2::from_points(&self.control_points);
        let magnitude = [boxed.min().x, boxed.min().y, boxed.max().x, boxed.max().y]
            .iter()
            .fold(0.0_f64, |m, x| m.max(x.abs()));
        let resolvable = chord >= crate::flatten::CHORD_RESOLUTION_ULPS * f64::EPSILON * magnitude;
        let mut out = vec![(lo, self.point_at(lo))];
        for segment in self.bezier_segments() {
            let largest = segment.weights.iter().fold(0.0_f64, |m, w| m.max(*w));
            let h: Vec<H> = segment
                .points
                .iter()
                .zip(&segment.weights)
                .map(|(pt, w)| {
                    let w = w / largest;
                    [w * pt.x, w * pt.y, w]
                })
                .collect();
            let mut stack = vec![(h, segment.lo, segment.hi, 0usize)];
            while let Some((h, a, b, depth)) = stack.pop() {
                let points: Vec<Point2> = h.iter().map(|v| dehomogenize(*v)).collect();
                if flat_enough(&points, 0.5 * chord, angle) {
                    if out.len() >= limit {
                        return Err(too_many());
                    }
                    let end = if b >= hi {
                        self.point_at(hi)
                    } else {
                        points.last().copied().unwrap_or_else(Point2::origin)
                    };
                    out.push((b, end));
                    continue;
                }
                if !resolvable || depth >= FLATTEN_DEPTH {
                    return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                        .with_detail("operation", "flatten"));
                }
                let (left, right) = split_homogeneous(&h, 0.5);
                let mid = 0.5 * (a + b);
                stack.push((right, mid, b, depth + 1));
                stack.push((left, a, mid, depth + 1));
            }
        }
        if out.len() > limit {
            return Err(too_many());
        }
        Ok(out)
    }
}

/// Solves the banded system (row `r` holding columns `r − p ..= r + p`) without pivoting.
fn solve_banded(
    mut band: Vec<f64>,
    mut rhs: Vec<[f64; 2]>,
    count: usize,
    p: usize,
) -> Result<Vec<[f64; 2]>> {
    let width = 2 * p + 1;
    let cell = |band: &[f64], r: usize, c: usize| -> f64 {
        (c + p)
            .checked_sub(r)
            .filter(|o| *o < width)
            .map_or(0.0, |o| at(band, r * width + o))
    };
    for c in 0..count {
        let pivot = cell(&band, c, c);
        if pivot.abs() <= f64::EPSILON || pivot.is_nan() {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "fit"));
        }
        let pivot_row = rhs.get(c).copied().unwrap_or([0.0; 2]);
        for r in c + 1..(c + p + 1).min(count) {
            let factor = cell(&band, r, c) / pivot;
            if factor == 0.0 {
                continue;
            }
            for cc in c..(c + p + 1).min(count) {
                let value = cell(&band, r, cc) - factor * cell(&band, c, cc);
                if let Some(slot) = (cc + p)
                    .checked_sub(r)
                    .filter(|o| *o < width)
                    .and_then(|o| band.get_mut(r * width + o))
                {
                    *slot = value;
                }
            }
            if let Some(b) = rhs.get_mut(r) {
                b[0] -= factor * pivot_row[0];
                b[1] -= factor * pivot_row[1];
            }
        }
    }
    let mut solution = vec![[0.0; 2]; count];
    for r in (0..count).rev() {
        let mut value = rhs.get(r).copied().unwrap_or([0.0; 2]);
        for c in r + 1..(r + p + 1).min(count) {
            let a = cell(&band, r, c);
            let x = solution.get(c).copied().unwrap_or([0.0; 2]);
            value[0] -= a * x[0];
            value[1] -= a * x[1];
        }
        let pivot = cell(&band, r, r);
        if let Some(slot) = solution.get_mut(r) {
            *slot = [value[0] / pivot, value[1] / pivot];
        }
    }
    Ok(solution)
}

fn foot_start(foot: &NurbsFoot) -> f64 {
    match foot {
        NurbsFoot::Point(t) | NurbsFoot::Interval(t, _) => *t,
    }
}

/// The span `k` in `p ..= n − 1` with `u_k ≤ t < u_{k+1}` on a clamped knot vector.
fn span_of(knots: &[f64], p: usize, n: usize, t: f64) -> usize {
    knots
        .partition_point(|k| *k <= t)
        .saturating_sub(1)
        .clamp(p, n.saturating_sub(1).max(p))
}

fn validate_knots(knots: &[f64], degree: usize) -> Result<()> {
    let invalid = |reason: &'static str| {
        Err(Error::new(codes::NURBS_KNOTS_INVALID).with_detail("reason", reason))
    };
    if !knots.windows(2).all(|w| matches!(w, [a, b] if a <= b)) {
        return invalid("decreasing");
    }
    let runs: Vec<usize> = knots.chunk_by(|a, b| a == b).map(<[f64]>::len).collect();
    if runs.len() < 2 {
        return invalid("empty range");
    }
    if runs.first() != Some(&(degree + 1)) || runs.last() != Some(&(degree + 1)) {
        return invalid("not clamped");
    }
    if runs
        .get(1..runs.len() - 1)
        .is_some_and(|inner| inner.iter().any(|m| *m > degree))
    {
        return invalid("multiplicity");
    }
    Ok(())
}

/// Whether a control polygon is within `chord` of its chord and its legs turn by at most
/// `angle`.
fn flat_enough(points: &[Point2], chord: f64, angle: f64) -> bool {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return true;
    };
    let base = last - first;
    let base_length = crate::length(&base);
    let distance = |p: &Point2| {
        let v = p - first;
        if base_length == 0.0 {
            return crate::length(&v);
        }
        let along = v.dot(&base) / base_length;
        if along <= 0.0 {
            crate::length(&v)
        } else if along >= base_length {
            crate::length(&(p - last))
        } else {
            (crate::eval::cross(&base, &v) / base_length).abs()
        }
    };
    if !points.iter().all(|p| distance(p) <= chord) {
        return false;
    }
    let legs: Vec<Vector2> = points
        .windows(2)
        .filter_map(|w| match w {
            [a, b] if a != b => Some(b - a),
            _ => None,
        })
        .collect();
    let Some(reference) = legs.first() else {
        return true;
    };
    let (mut low, mut high) = (0.0_f64, 0.0_f64);
    for leg in &legs {
        let turn = crate::eval::cross(reference, leg).atan2(reference.dot(leg));
        low = low.min(turn);
        high = high.max(turn);
    }
    high - low <= angle
}

/// The two halves of a rational Bézier segment at `u` (de Casteljau on homogeneous points).
fn split_homogeneous(h: &[H], u: f64) -> (Vec<H>, Vec<H>) {
    let mut level = h.to_vec();
    let mut left = Vec::with_capacity(h.len());
    let mut right = Vec::with_capacity(h.len());
    while let Some(first) = level.first().copied() {
        left.push(first);
        right.push(level.last().copied().unwrap_or(first));
        level = level
            .windows(2)
            .map(|w| mix(hat(w, 0), hat(w, 1), u))
            .collect();
    }
    right.reverse();
    (left, right)
}

/// `C(n, k)` as `f64`.
pub(crate) fn binomial(n: usize, k: usize) -> f64 {
    if k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    (0..k).fold(1.0, |acc, i| acc * (n - i) as f64 / (i + 1) as f64)
}

/// The product of two polynomials in Bernstein form.
pub(crate) fn bernstein_mul(a: &[f64], b: &[f64]) -> Vec<f64> {
    let (m, n) = (a.len().saturating_sub(1), b.len().saturating_sub(1));
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    (0..=m + n)
        .map(|k| {
            let scale = binomial(m + n, k);
            (k.saturating_sub(n)..=k.min(m))
                .map(|i| binomial(m, i) * binomial(n, k - i) * at(a, i) * at(b, k - i))
                .sum::<f64>()
                / scale
        })
        .collect()
}

/// The derivative of a polynomial in Bernstein form, one degree lower.
pub(crate) fn bernstein_derivative(c: &[f64]) -> Vec<f64> {
    let n = c.len().saturating_sub(1) as f64;
    let d: Vec<f64> = c.windows(2).map(|w| n * (at(w, 1) - at(w, 0))).collect();
    if d.is_empty() { vec![0.0] } else { d }
}

/// The polynomial raised to degree `degree` (Bernstein degree elevation).
fn bernstein_elevate(c: &[f64], degree: usize) -> Vec<f64> {
    let mut c = c.to_vec();
    while c.len() < degree + 1 {
        let n = c.len();
        c = (0..=n)
            .map(|i| {
                let f = i as f64 / n as f64;
                let below = if i == 0 { 0.0 } else { at(&c, i - 1) };
                let here = if i == n { 0.0 } else { at(&c, i) };
                f * below + (1.0 - f) * here
            })
            .collect();
    }
    c
}

fn bernstein_add(a: &[f64], b: &[f64]) -> Vec<f64> {
    let degree = a.len().max(b.len()).saturating_sub(1);
    let (a, b) = (bernstein_elevate(a, degree), bernstein_elevate(b, degree));
    a.iter().zip(&b).map(|(x, y)| x + y).collect()
}

fn bernstein_sub(a: &[f64], b: &[f64]) -> Vec<f64> {
    let negated: Vec<f64> = b.iter().map(|x| -x).collect();
    bernstein_add(a, &negated)
}

/// The rounding allowance of each coefficient of a polynomial whose terms have the magnitudes
/// `magnitude` (the same products taken of absolute values).
fn noise_of(magnitude: &[f64]) -> Vec<f64> {
    let factor = NOISE_ULPS * f64::EPSILON * magnitude.len() as f64;
    magnitude.iter().map(|m| factor * m).collect()
}

/// The value of a polynomial in Bernstein form at `u` (de Casteljau).
pub(crate) fn bernstein_eval(c: &[f64], u: f64) -> f64 {
    let mut level = c.to_vec();
    while level.len() > 1 {
        level = level
            .windows(2)
            .map(|w| at(w, 0) * (1.0 - u) + at(w, 1) * u)
            .collect();
    }
    level.first().copied().unwrap_or(0.0)
}

fn bernstein_split(c: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let mut level = c.to_vec();
    let mut left = Vec::with_capacity(c.len());
    let mut right = Vec::with_capacity(c.len());
    while let Some(first) = level.first().copied() {
        left.push(first);
        right.push(level.last().copied().unwrap_or(first));
        level = level
            .windows(2)
            .map(|w| 0.5 * (at(w, 0) + at(w, 1)))
            .collect();
    }
    right.reverse();
    (left, right)
}

/// The roots in [0, 1] of a polynomial in Bernstein form whose coefficients carry the rounding
/// allowances `noise` (one per coefficient, subdivided with them), as intervals `(u0, u1)`: a point root has `u0 == u1` or a width below
/// `2^-48`; a wider interval is a stretch where the polynomial is zero within the allowance.
///
/// Subdivision stops where all coefficients have one sign beyond the allowance (no root), and
/// where they change sign exactly once with nonzero ends (one root, refined by bisection).
pub(crate) fn bernstein_roots(c: &[f64], noise: &[f64]) -> Result<Vec<(f64, f64)>> {
    let sign = |x: f64, noise: f64| {
        if x > noise {
            1
        } else if x < -noise {
            -1
        } else {
            0
        }
    };
    let budget = ROOT_NODES_PER_DEGREE * c.len().max(1);
    let mut nodes = 0;
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut stack = vec![(c.to_vec(), noise.to_vec(), 0.0_f64, 1.0_f64, 0usize)];
    while let Some((c, noise, u0, u1, depth)) = stack.pop() {
        nodes += 1;
        if nodes > budget {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "roots"));
        }
        let signs: Vec<i32> = c
            .iter()
            .zip(noise.iter().chain(std::iter::repeat(&0.0)))
            .map(|(x, n)| sign(*x, *n))
            .collect();
        if signs.iter().all(|s| *s == 0) {
            out.push((u0, u1));
            continue;
        }
        if signs.iter().all(|s| *s > 0) || signs.iter().all(|s| *s < 0) {
            continue;
        }
        let nonzero: Vec<i32> = signs.iter().copied().filter(|s| *s != 0).collect();
        let changes = nonzero
            .windows(2)
            .filter(|w| at_i(w, 0) != at_i(w, 1))
            .count();
        let (first, last) = (
            signs.first().copied().unwrap_or(0),
            signs.last().copied().unwrap_or(0),
        );
        if changes == 1 && first != 0 && last != 0 && signs.iter().all(|s| *s != 0) {
            let (mut a, mut b) = (0.0_f64, 1.0_f64);
            for _ in 0..MAX_ROOT_ITERATIONS {
                let m = 0.5 * (a + b);
                if m <= a || m >= b {
                    break;
                }
                if (bernstein_eval(&c, m) > 0.0) == (first > 0) {
                    a = m;
                } else {
                    b = m;
                }
            }
            let u = u0 + 0.5 * (a + b) * (u1 - u0);
            out.push((u, u));
            continue;
        }
        if depth >= ROOT_DEPTH {
            let u = 0.5 * (u0 + u1);
            out.push((u, u));
            continue;
        }
        let (left, right) = bernstein_split(&c);
        let (noise_left, noise_right) = bernstein_split(&noise);
        let mid = 0.5 * (u0 + u1);
        stack.push((right, noise_right, mid, u1, depth + 1));
        stack.push((left, noise_left, u0, mid, depth + 1));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Adjacent zero stretches join.
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for (a, b) in out {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    Ok(merged)
}

fn at_i(values: &[i32], i: usize) -> i32 {
    values.get(i).copied().unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testing::{log_uniform, point};

    const TOL: Tolerance = Tolerance::DEFAULT;

    /// The B-spline basis function `N_{i,p}(u)` by the recursive definition, with 0/0 = 0 and
    /// the last nonempty span closed at the end of the range. Shares no code with the curve.
    fn reference_basis(knots: &[f64], i: usize, p: usize, u: f64) -> f64 {
        if p == 0 {
            let (a, b) = (knots[i], knots[i + 1]);
            let end = *knots.last().unwrap();
            let last_span = b == end && a < b;
            return if (a <= u && u < b) || (last_span && u == end) {
                1.0
            } else {
                0.0
            };
        }
        let mut value = 0.0;
        let d1 = knots[i + p] - knots[i];
        if d1 > 0.0 {
            value += (u - knots[i]) / d1 * reference_basis(knots, i, p - 1, u);
        }
        let d2 = knots[i + p + 1] - knots[i + 1];
        if d2 > 0.0 {
            value += (knots[i + p + 1] - u) / d2 * reference_basis(knots, i + 1, p - 1, u);
        }
        value
    }

    /// The point by the rational sum of reference basis functions.
    pub(crate) fn reference_point(c: &Nurbs2, u: f64) -> Point2 {
        let (mut x, mut y, mut w) = (0.0, 0.0, 0.0);
        for (i, (p, weight)) in c.control_points().iter().zip(c.weights()).enumerate() {
            let n = reference_basis(c.knots(), i, c.degree(), u) * weight;
            x += n * p.x;
            y += n * p.y;
            w += n;
        }
        Point2::new(x / w, y / w)
    }

    /// The size of a curve, for relative error bounds.
    pub(crate) fn size(c: &Nurbs2) -> f64 {
        let b = Aabb2::from_points(c.control_points());
        (b.max() - b.min()).abs().max().max(1e-300)
    }

    /// Random valid curves: degree 1 to 5, up to 12 control points, knots with repeated interior
    /// values, weights over 1e-2 to 1e2, coordinates in ±100.
    pub(crate) fn nurbs() -> impl Strategy<Value = Nurbs2> {
        (1usize..=5, 0usize..=7)
            .prop_flat_map(|(p, extra)| {
                let n = p + 1 + extra;
                (
                    Just(p),
                    prop::collection::vec(point(), n),
                    prop::collection::vec(log_uniform(-2.0, 2.0), n),
                    prop::collection::vec(0.0..1.0_f64, n - p - 1),
                    1usize..=3,
                    -10.0..10.0_f64,
                    log_uniform(-2.0, 2.0),
                )
            })
            .prop_filter_map(
                "valid",
                |(p, points, weights, mut interior, repeat, start, width)| {
                    // Repeat some interior values, at most p times.
                    let r = repeat.min(p);
                    for k in (0..interior.len()).step_by(r + 1) {
                        for j in 1..=r {
                            if k + j < interior.len() {
                                interior[k + j] = interior[k];
                            }
                        }
                    }
                    interior.sort_by(f64::total_cmp);
                    let knots: Vec<f64> = std::iter::repeat_n(0.0, p + 1)
                        .chain(interior.iter().copied())
                        .chain(std::iter::repeat_n(1.0, p + 1))
                        .map(|u| start + u * width)
                        .collect();
                    Nurbs2::new(p, knots, points, weights).ok()
                },
            )
    }

    /// Curves with extreme coordinates: a unit-size shape scaled and moved by powers of ten up
    /// to 1e±100.
    fn extreme_nurbs() -> impl Strategy<Value = Nurbs2> {
        (nurbs(), log_uniform(-100.0, 100.0), -100.0..100.0_f64).prop_filter_map(
            "representable",
            |(c, scale, shift)| {
                let offset = 10f64.powf(shift.abs()) * shift.signum();
                c.mapped(|p| Point2::new(p.x * scale + offset, p.y * scale - offset))
                    .ok()
            },
        )
    }

    fn sample(c: &Nurbs2, f: f64) -> f64 {
        let (a, b) = c.parameter_range();
        a + (b - a) * f
    }

    fn close(a: Point2, b: Point2, bound: f64) -> bool {
        crate::length(&(a - b)) <= bound
    }

    fn line() -> Nurbs2 {
        Nurbs2::new(
            1,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![Point2::new(0.0, 0.0), Point2::new(2.0, 0.0)],
            vec![1.0, 1.0],
        )
        .unwrap()
    }

    /// A quarter circle of radius 1 as a rational quadratic.
    pub(crate) fn quarter_circle() -> Nurbs2 {
        Nurbs2::new(
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![
                Point2::new(1.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(0.0, 1.0),
            ],
            vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0],
        )
        .unwrap()
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        r.unwrap_err().code()
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one list of hostile definitions")]
    fn hostile_definitions_are_rejected_with_codes() {
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        let k = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let w = vec![1.0; 3];
        assert!(Nurbs2::new(2, k.clone(), pts.clone(), w.clone()).is_ok());
        assert_eq!(
            code(Nurbs2::new(0, k.clone(), pts.clone(), w.clone())),
            "geom.nurbs.degree_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(3, k.clone(), pts.clone(), w.clone())),
            "geom.nurbs.degree_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(99, k.clone(), pts.clone(), w.clone())),
            "geom.nurbs.degree_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(2, k.clone(), pts.clone(), vec![1.0; 2])),
            "geom.nurbs.weights_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(2, k.clone(), pts.clone(), vec![1.0, 0.0, 1.0])),
            "geom.nurbs.weights_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(2, k.clone(), pts.clone(), vec![1.0, -1.0, 1.0])),
            "geom.nurbs.weights_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                k.clone(),
                pts.clone(),
                vec![1.0, f64::NAN, 1.0]
            )),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![0.0, 0.0, 1.0, 1.0, 1.0],
                pts.clone(),
                w.clone()
            )),
            "geom.nurbs.knots_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
                pts.clone(),
                w.clone()
            )),
            "geom.nurbs.knots_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
                pts.clone(),
                w.clone()
            )),
            "geom.nurbs.knots_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
                pts.clone(),
                w.clone()
            )),
            "geom.nurbs.knots_invalid"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![0.0, 0.0, 0.0, f64::INFINITY, 1.0, 1.0],
                pts.clone(),
                w.clone()
            )),
            "geom.input.not_finite"
        );
        let mut nan = pts.clone();
        nan[1].x = f64::NAN;
        assert_eq!(
            code(Nurbs2::new(2, k.clone(), nan, w.clone())),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(Nurbs2::new(
                2,
                k.clone(),
                vec![Point2::new(1.0, 1.0); 3],
                w.clone()
            )),
            "geom.nurbs.degenerate"
        );
        let five = vec![0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0];
        let six_pts: Vec<Point2> = (0..6).map(|i| Point2::new(i as f64, 0.0)).collect();
        assert_eq!(
            code(Nurbs2::new(2, five, six_pts.clone(), vec![1.0; 6])),
            "geom.nurbs.knots_invalid"
        );
        let tight = Limits {
            array_length: 2,
            ..Limits::STRICT
        };
        assert_eq!(
            code(Nurbs2::with_limits(
                2,
                k.clone(),
                pts.clone(),
                w.clone(),
                &tight
            )),
            "geom.nurbs.too_many_control_points"
        );
    }

    #[test]
    fn hostile_counts_and_magnitudes_are_rejected() {
        let pts = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
        ];
        let k = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let w = vec![1.0; 3];
        let six_pts: Vec<Point2> = (0..6).map(|i| Point2::new(f64::from(i), 0.0)).collect();
        // Huge counts are rejected before anything is computed.
        let tiny = Limits {
            array_length: 5,
            ..Limits::STRICT
        };
        assert_eq!(
            code(Nurbs2::with_limits(
                2,
                vec![0.0; 1000],
                six_pts,
                vec![1.0; 6],
                &tiny
            )),
            "geom.nurbs.too_many_control_points"
        );
        // Unrepresentable derivatives and weight spans.
        assert_eq!(
            code(Nurbs2::new(
                2,
                vec![0.0, 0.0, 0.0, 1e-300, 1e-300, 1e-300],
                vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(1e300, 0.0),
                    Point2::new(0.0, 1e300)
                ],
                w.clone()
            )),
            "geom.precision.insufficient"
        );
        assert_eq!(
            code(Nurbs2::new(2, k, pts, vec![1.0, 1e-310, 1.0])),
            "geom.precision.insufficient"
        );
    }

    #[test]
    fn known_curves_evaluate_exactly() {
        let c = quarter_circle();
        for i in 0..=16 {
            let p = c.point_at(f64::from(i) / 16.0);
            assert!((p.coords.norm() - 1.0).abs() < 1e-15);
        }
        let (_, d1, d2) = c.derivatives(0.0);
        assert!((d1.normalize() - Vector2::new(0.0, 1.0)).norm() < 1e-15);
        // Curvature of a unit circle.
        let kappa = crate::eval::cross(&d1, &d2) / d1.norm().powi(3);
        assert!((kappa - 1.0).abs() < 1e-12);
        let l = line();
        assert_eq!(l.point_at(0.25), Point2::new(0.5, 0.0));
        assert_eq!(l.derivatives(0.3).1, Vector2::new(2.0, 0.0));
        assert!(l.point_at(f64::NAN).x.is_nan());
        assert_eq!(l.point_at(-5.0), Point2::new(0.0, 0.0));
        assert_eq!(l.point_at(5.0), Point2::new(2.0, 0.0));
        let b = c.bounding_box();
        assert!((b.min() - Point2::new(0.0, 0.0)).norm() < 1e-15);
        assert!((b.max() - Point2::new(1.0, 1.0)).norm() < 1e-15);
        let length = c.length_between(0.0, 1.0, &TOL).unwrap();
        assert!((length - std::f64::consts::FRAC_PI_2).abs() < 1e-7);
    }

    #[test]
    fn parameter_and_multiplicity_errors_have_codes() {
        let c = quarter_circle();
        assert_eq!(code(c.insert_knot(0.0, 1)), "geom.parameter.out_of_range");
        assert_eq!(
            code(c.insert_knot(f64::NAN, 1)),
            "geom.parameter.out_of_range"
        );
        assert_eq!(code(c.insert_knot(0.5, 3)), "geom.nurbs.knots_invalid");
        assert_eq!(code(c.split_at(1.0)), "geom.parameter.out_of_range");
        assert_eq!(
            code(c.remove_knot(0.5, 1, &TOL)),
            "geom.nurbs.knot_not_removable"
        );
        assert_eq!(
            code(c.elevate_degree(30, &TOL)),
            "geom.nurbs.degree_invalid"
        );
        let bad = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(code(c.remove_knot(0.5, 1, &bad)), "geom.tolerance.invalid");
        // Inserted and removed again exactly.
        let inserted = c.insert_knot(0.5, 2).unwrap();
        let removed = inserted.remove_knot(0.5, 2, &TOL).unwrap();
        assert_eq!(removed.control_points().len(), 3);
        for i in 0..=8 {
            let t = f64::from(i) / 8.0;
            assert!(close(removed.point_at(t), c.point_at(t), 1e-14));
        }
        // A real corner cannot be removed.
        let corner = Nurbs2::new(
            1,
            vec![0.0, 0.0, 0.5, 1.0, 1.0],
            vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 0.0),
            ],
            vec![1.0; 3],
        )
        .unwrap();
        let error = corner.remove_knot(0.5, 1, &TOL).unwrap_err();
        assert_eq!(error.code(), "geom.nurbs.knot_not_removable");
    }

    #[test]
    fn interpolation_rejects_bad_points() {
        let p = |x: f64| Point2::new(x, x * x);
        assert_eq!(
            code(Nurbs2::interpolate(&[p(0.0), p(1.0)], 3, &TOL)),
            "geom.fit.points_invalid"
        );
        assert_eq!(
            code(Nurbs2::interpolate(
                &[p(0.0), p(1.0), p(1.0), p(2.0)],
                2,
                &TOL
            )),
            "geom.fit.points_invalid"
        );
        assert_eq!(
            code(Nurbs2::interpolate(&[p(0.0), p(f64::NAN), p(2.0)], 1, &TOL)),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(Nurbs2::interpolate(&[p(0.0), p(1.0), p(2.0)], 0, &TOL)),
            "geom.nurbs.degree_invalid"
        );
        let bad = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(
            code(Nurbs2::interpolate(&[p(0.0), p(1.0), p(2.0)], 1, &bad)),
            "geom.tolerance.invalid"
        );
        let line = Nurbs2::interpolate(&[p(0.0), p(1.0)], 1, &TOL).unwrap();
        assert_eq!(line.control_points(), &[p(0.0), p(1.0)]);
    }

    #[test]
    fn bernstein_roots_find_simple_double_and_zero_roots() {
        // (u − 0.25)(u − 0.75) in Bernstein form: 0.1875, −0.3125, 0.1875.
        let roots = bernstein_roots(&[0.1875, -0.3125, 0.1875], &[1e-15; 3]).unwrap();
        assert_eq!(roots.len(), 2);
        assert!((roots[0].0 - 0.25).abs() < 1e-12 && (roots[1].0 - 0.75).abs() < 1e-12);
        // (u − 0.5)²: 0.25, −0.25, 0.25.
        let double = bernstein_roots(&[0.25, -0.25, 0.25], &[1e-15; 3]).unwrap();
        assert!(!double.is_empty() && double.iter().all(|r| (r.0 - 0.5).abs() < 1e-6));
        assert_eq!(
            bernstein_roots(&[0.0, 0.0, 0.0], &[1e-15; 3]).unwrap(),
            vec![(0.0, 1.0)]
        );
        assert!(
            bernstein_roots(&[1.0, 2.0, 1.0], &[1e-15; 3])
                .unwrap()
                .is_empty()
        );
        assert_eq!(binomial(5, 2), 10.0);
        assert_eq!(binomial(2, 5), 0.0);
    }

    proptest! {
        #[test]
        fn evaluation_matches_the_reference(c in nurbs(), f in 0.0..=1.0_f64) {
            let t = sample(&c, f);
            let bound = 1e-12 * (size(&c) + c.point_at(t).coords.abs().max());
            prop_assert!(close(c.point_at(t), reference_point(&c, t), bound),
                "{:?} vs {:?}", c.point_at(t), reference_point(&c, t));
        }

        #[test]
        fn extreme_curves_evaluate_and_bound(c in extreme_nurbs(), f in 0.0..=1.0_f64) {
            let t = sample(&c, f);
            let p = c.point_at(t);
            let bound = 1e-12 * (size(&c) + p.coords.abs().max());
            prop_assert!(close(p, reference_point(&c, t), bound));
            let b = c.bounding_box().expanded(bound);
            prop_assert!(b.contains_point(&p));
            let (_, d1, d2) = c.derivatives(t);
            prop_assert!(d1.x.is_finite() && d1.y.is_finite() && d2.x.is_finite() && d2.y.is_finite());
        }

        #[test]
        fn derivatives_match_finite_differences(c in nurbs(), f in 0.05..0.95_f64) {
            let t = sample(&c, f);
            // Away from knots, where the curve is smooth.
            let (a, b) = c.parameter_range();
            let h = 1e-6 * (b - a);
            prop_assume!(c.knots().iter().all(|k| (k - t).abs() > 1e3 * h));
            let (_, d1, d2) = c.derivatives(t);
            let fd1 = (c.point_at(t + h) - c.point_at(t - h)) / (2.0 * h);
            let (_, e1, _) = c.derivatives(t + h);
            let (_, e0, _) = c.derivatives(t - h);
            let fd2 = (e1 - e0) / (2.0 * h);
            let scale1 = d1.norm().max(size(&c) / (b - a));
            prop_assert!((d1 - fd1).norm() <= 1e-4 * scale1, "{d1:?} {fd1:?}");
            prop_assert!((d2 - fd2).norm() <= 1e-4 * d2.norm().max(scale1 / (b - a)), "{d2:?} {fd2:?}");
        }

        #[test]
        fn knot_insertion_and_removal_preserve_the_curve(c in nurbs(), f in 0.01..0.99_f64, times in 1usize..=3, g in 0.0..=1.0_f64) {
            let u = sample(&c, f);
            let existing = c.knots().iter().filter(|k| **k == u).count();
            prop_assume!(existing + times <= c.degree());
            let inserted = c.insert_knot(u, times).unwrap();
            prop_assert_eq!(inserted.control_points().len(), c.control_points().len() + times);
            let t = sample(&c, g);
            let bound = 1e-12 * size(&c) * 1e2;
            prop_assert!(close(inserted.point_at(t), c.point_at(t), bound));
            let removed = inserted.remove_knot(u, times, &TOL).unwrap();
            prop_assert_eq!(removed.control_points().len(), c.control_points().len());
            prop_assert!(close(removed.point_at(t), c.point_at(t), TOL.linear));
        }

        #[test]
        fn split_pieces_reproduce_the_curve(c in nurbs(), f in 0.01..0.99_f64, g in 0.0..=1.0_f64) {
            let t = sample(&c, f);
            let (left, right) = match c.split_at(t) {
                Ok(pieces) => pieces,
                // A piece on which the curve stands still at one point.
                Err(e) => {
                    prop_assert_eq!(e.code(), "geom.nurbs.degenerate");
                    return Ok(());
                }
            };
            prop_assert_eq!(left.parameter_range(), (c.parameter_range().0, t));
            prop_assert_eq!(right.parameter_range(), (t, c.parameter_range().1));
            prop_assert_eq!(left.point_at(t), right.point_at(t));
            let bound = 1e-11 * size(&c);
            prop_assert!(close(left.point_at(t), c.point_at(t), bound));
            let s = sample(&c, g);
            let piece = if s <= t { &left } else { &right };
            prop_assert!(close(piece.point_at(s), c.point_at(s), bound));
        }

        #[test]
        fn reversal_retraces_and_is_an_involution(c in nurbs(), f in 0.0..=1.0_f64) {
            let r = c.reversed();
            prop_assert_eq!(&r.reversed(), &c);
            let t = sample(&c, f);
            prop_assert!(close(r.point_at(-t), c.point_at(t), 1e-12 * size(&c)));
            prop_assert_eq!(r.point_at(r.parameter_range().0), c.point_at(c.parameter_range().1));
        }

        #[test]
        fn degree_elevation_preserves_the_curve(c in nurbs(), times in 1usize..=2, f in 0.0..=1.0_f64) {
            let e = c.elevate_degree(times, &TOL).unwrap();
            prop_assert_eq!(e.degree(), c.degree() + times);
            prop_assert_eq!(e.parameter_range(), c.parameter_range());
            let t = sample(&c, f);
            prop_assert!(close(e.point_at(t), c.point_at(t), TOL.linear.max(1e-12 * size(&c))));
        }

        #[test]
        fn interpolation_meets_its_tolerance(
            points in prop::collection::vec(point(), 2..12),
            degree in 1usize..=4,
        ) {
            prop_assume!(points.windows(2).all(|w| (w[1] - w[0]).norm() > 1e-3));
            prop_assume!(degree < points.len());
            match Nurbs2::interpolate(&points, degree, &TOL) {
                Ok(curve) => {
                    // Each point lies on the curve within the tolerance.
                    for q in &points {
                        let foot = crate::closest::CurveProject::closest_point(
                            &crate::Curve2::Nurbs(curve.clone()), *q, &TOL).unwrap();
                        prop_assert!(foot.distance <= TOL.linear);
                    }
                    prop_assert_eq!(curve.point_at(0.0), points[0]);
                }
                Err(e) => prop_assert_eq!(e.code(), "geom.precision.insufficient"),
            }
        }

    }

    proptest! {
        // Projection and quadrature of NURBS curves are slow in unoptimized builds.
        #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

        #[test]
        fn generic_paths_handle_nurbs(c in nurbs(), q in point(), t in crate::testing::affine(), f in 0.0..=1.0_f64) {
            use crate::closest::CurveProject;
            use crate::encoding::GeomCodec;
            use crate::eval::CurveEval;
            use crate::flatten::CurveFlatten;
            let curve = crate::Curve2::Nurbs(c.clone());
            // Encoding and serde round trips.
            let bytes = curve.encode();
            prop_assert_eq!(&crate::Curve2::decode(&bytes, &Limits::STRICT).unwrap(), &curve);
            let json = serde_json::to_string(&curve).unwrap();
            prop_assert_eq!(&serde_json::from_str::<crate::Curve2>(&json).unwrap(), &curve);
            // Affine images map point by point.
            let image = curve.transformed(&t, &TOL).unwrap();
            let u = sample(&c, f);
            let mapped = t.transform_point(&c.point_at(u));
            prop_assert!(close(image.point_at(u), mapped, 1e-9 * (size(&c) + mapped.coords.abs().max())));
            // The closest point is no farther than any sample.
            let foot = curve.closest_point(q, &TOL).unwrap();
            let sampled = (0..=200)
                .map(|i| crate::length(&(c.point_at(sample(&c, f64::from(i) / 200.0)) - q)))
                .fold(f64::INFINITY, f64::min);
            prop_assert!(foot.distance <= sampled + TOL.linear, "{} {}", foot.distance, sampled);
            // Flattened vertices lie on the curve and chord midpoints within the chord tolerance.
            if let Ok(vertices) = curve.flatten(0.1, 0.3, 100_000) {
                let step = (vertices.len() / 40).max(1);
                for pair in vertices.windows(2).step_by(step) {
                    let mid = pair[0] + (pair[1] - pair[0]) * 0.5;
                    let gap = curve.closest_point(mid, &TOL).unwrap().distance;
                    prop_assert!(gap <= 0.1 + 1e-9, "{gap} at {mid:?} of {pair:?}");
                }
            }
            // Lengths add up.
            let (a, b) = c.parameter_range();
            if let (Ok(whole), Ok(left), Ok(right)) = (
                curve.length(&TOL),
                curve.length_between(a, u, &TOL),
                curve.length_between(u, b, &TOL),
            ) {
                prop_assert!((whole - left - right).abs() <= TOL.linear);
                // Speeds with sharp peaks can exhaust the quadrature, with a code.
                match curve.parameter_at_length(left, &TOL) {
                    Ok(back) => prop_assert!(close(c.point_at(back), c.point_at(u), 1e-5)),
                    Err(e) => prop_assert_eq!(e.code(), "geom.precision.insufficient"),
                }
            }
            // Not supported yet, with a code.
            prop_assert_eq!(crate::intersect(&curve, &curve, &TOL).unwrap_err().code(), "geom.curve.kind_unsupported");
            prop_assert_eq!(crate::Loop2::new(vec![curve], &TOL).unwrap_err().code(), "geom.curve.kind_unsupported");
        }

        #[test]
        fn bounding_boxes_are_tight_and_contain_samples(c in nurbs()) {
            let b = c.bounding_box();
            let margin = 1e-12 * size(&c);
            let mut sampled = Aabb2::EMPTY;
            // Every knot span sampled densely, so narrow spans are not skipped.
            for pair in c.breakpoints().windows(2) {
                for i in 0..=200 {
                    let p = c.point_at(pair[0] + (pair[1] - pair[0]) * f64::from(i) / 200.0);
                    prop_assert!(b.expanded(margin).contains_point(&p));
                    sampled = sampled.union(&Aabb2::from_points(&[p]));
                }
            }
            // Tight: the sampled box is within the sampling resolution of the exact one, where
            // the weights are even enough for uniform samples to reach the extremes.
            let (wmin, wmax) = c.weights().iter().fold((f64::INFINITY, 0.0_f64), |(a, b), w| (a.min(*w), b.max(*w)));
            if wmax <= 4.0 * wmin {
                prop_assert!(sampled.expanded(0.01 * size(&c)).contains(&b));
            }
        }
    }
}
