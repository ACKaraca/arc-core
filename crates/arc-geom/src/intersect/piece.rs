//! The smooth pieces curves decompose into, and the intersection of two pieces.
//!
//! Every curve is one piece (a segment, ray, line, circle, arc, ellipse or elliptical arc) or a
//! sequence of pieces (a polyline). A piece is either linear, parameterized by the signed
//! distance `s` from an origin, or conic, parameterized by the angle `τ` traveled from its start
//! (the eccentric angle for an ellipse). Each piece maps its own parameter to the parameter of
//! its curve by `offset + τ / divisor`.

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use super::trig::{Trig, bisect, roots};
use crate::bbox::Aabb2;
use crate::curve::Curve2;
use crate::error::codes;
use crate::polyline::PolylineSegment;
use crate::primitives::{Arc2, Ellipse2, Segment2, perp};

/// A conservative bound on the relative rounding error of the few floating-point steps of one
/// construction, as a multiple of machine epsilon.
pub(super) const ROUNDING: f64 = 8.0 * f64::EPSILON;
/// Newton steps that move the eccentric angle of a point near an ellipse to its foot point.
const REFINE_STEPS: u32 = 4;

fn cross(a: &Vector2, b: &Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// The sine of the angle between two nonzero vectors.
fn sine(a: &Vector2, b: &Vector2) -> f64 {
    (cross(a, b) / (a.norm() * b.norm())).abs()
}

fn midpoint(a: &Point2, b: &Point2) -> Point2 {
    a + (b - a) * 0.5
}

fn ordered(x: f64, y: f64) -> [f64; 2] {
    if x <= y { [x, y] } else { [y, x] }
}

/// `b − a` and the norm of its exact rounding error (by the two-sum construction per
/// coordinate); the error is zero when the coordinates are close, as for small geometry far from
/// the origin.
fn difference(b: &Point2, a: &Point2) -> (Vector2, f64) {
    let two_sum = |x: f64, y: f64| {
        let s = x + y;
        let virtual_y = s - x;
        let error = (x - (s - virtual_y)) + (y - virtual_y);
        (s, error)
    };
    let (x, ex) = two_sum(b.x, -a.x);
    let (y, ey) = two_sum(b.y, -a.y);
    (Vector2::new(x, y), ex.hypot(ey))
}

/// A linear piece: the points `origin + dir·s` for `s` in `[lo, hi]`, `dir` of unit length;
/// `lo` and `hi` may be infinite.
#[derive(Debug, Clone, Copy)]
pub(super) struct Linear {
    origin: Point2,
    dir: Vector2,
    lo: f64,
    hi: f64,
}

impl Linear {
    /// `s` clamped to the range, then moved to an end within `linear` of it.
    fn snap(&self, s: f64, linear: f64) -> f64 {
        let s = s.clamp(self.lo, self.hi);
        if (s - self.lo).abs() <= linear {
            self.lo
        } else if (s - self.hi).abs() <= linear {
            self.hi
        } else {
            s
        }
    }
}

/// A conic piece: the points `center + major·cos t + minor·sin t` for the eccentric angle
/// `t = start + sign·τ`, `τ` in `[0, sweep]`. The minor axis is the major axis turned a quarter
/// turn counterclockwise and scaled, so the eccentric angle runs counterclockwise.
#[derive(Debug, Clone, Copy)]
pub(super) struct Conic {
    center: Point2,
    major: Vector2,
    minor: Vector2,
    start: f64,
    sign: f64,
    sweep: f64,
}

impl Conic {
    fn new(center: Point2, major: Vector2, minor: Vector2, start: f64, sweep: f64) -> Self {
        Self {
            center,
            major,
            minor,
            start,
            sign: sweep.signum(),
            sweep: sweep.abs(),
        }
    }

    fn circle(center: Point2, radius: f64, start: f64, sweep: f64) -> Self {
        Self::new(
            center,
            Vector2::new(radius, 0.0),
            Vector2::new(0.0, radius),
            start,
            sweep,
        )
    }

    fn arc(arc: &Arc2) -> Self {
        Self::circle(arc.center(), arc.radius(), arc.start_angle(), arc.sweep())
    }

    fn ellipse(ellipse: &Ellipse2, start: f64, sweep: f64) -> Self {
        Self::new(
            ellipse.center(),
            ellipse.major_axis(),
            ellipse.minor_axis(),
            start,
            sweep,
        )
    }

    fn is_full(&self) -> bool {
        self.sweep >= TAU
    }

    fn is_circle(&self) -> bool {
        self.major.norm() == self.minor.norm()
    }

    fn ratio(&self) -> f64 {
        self.minor.norm() / self.major.norm()
    }

    fn at_angle(&self, t: f64) -> Point2 {
        let (sin, cos) = t.sin_cos();
        self.center + self.major * cos + self.minor * sin
    }

    fn derivative_at_angle(&self, t: f64) -> Vector2 {
        let (sin, cos) = t.sin_cos();
        self.minor * cos - self.major * sin
    }

    fn angle(&self, tau: f64) -> f64 {
        self.start + self.sign * tau
    }

    /// The parameter in `[0, 2π]` of the eccentric angle `t`, measured from the start.
    fn tau_of(&self, t: f64) -> f64 {
        (self.sign * (t - self.start)).rem_euclid(TAU)
    }

    /// The eccentric angle of the foot point of `p`, for `p` near the conic: the angle of `p` in
    /// the frame where the conic is the unit circle, then Newton steps on the foot-point
    /// condition `(P(t) − p)·P'(t) = 0` for an ellipse.
    fn angle_of(&self, p: &Point2) -> f64 {
        let q = p - self.center;
        let mut t = (q.dot(&self.minor) / self.minor.norm_squared())
            .atan2(q.dot(&self.major) / self.major.norm_squared());
        if self.is_circle() {
            return t;
        }
        for _ in 0..REFINE_STEPS {
            let (sin, cos) = t.sin_cos();
            let radial = self.major * cos + self.minor * sin;
            let offset = radial - q;
            let tangent = self.minor * cos - self.major * sin;
            let slope = tangent.norm_squared() - offset.dot(&radial);
            if slope <= 0.0 || !slope.is_finite() {
                break;
            }
            let step = (offset.dot(&tangent) / slope).clamp(-0.5, 0.5);
            t -= step;
            if step.abs() <= f64::EPSILON {
                break;
            }
        }
        t
    }
}

/// The geometry of a piece.
#[derive(Debug, Clone, Copy)]
pub(super) enum Shape {
    Linear(Linear),
    Conic(Conic),
}

impl Shape {
    fn point(&self, tau: f64) -> Point2 {
        match self {
            Self::Linear(l) => l.origin + l.dir * tau,
            Self::Conic(c) => c.at_angle(c.angle(tau)),
        }
    }

    /// The derivative with respect to the piece parameter.
    fn tangent(&self, tau: f64) -> Vector2 {
        match self {
            Self::Linear(l) => l.dir,
            Self::Conic(c) => c.derivative_at_angle(c.angle(tau)) * c.sign,
        }
    }

    /// The parameters of the finite end points; a full conic has none.
    fn ends(&self) -> Vec<f64> {
        match self {
            Self::Linear(l) => [l.lo, l.hi].into_iter().filter(|s| s.is_finite()).collect(),
            Self::Conic(c) if c.is_full() => Vec::new(),
            Self::Conic(c) => vec![0.0, c.sweep],
        }
    }

    /// The parameter of a candidate intersection point `p`, or `None` when it lies outside the
    /// piece. `hint` is the parameter computed with `p`: the distance along a linear piece, the
    /// eccentric angle on a conic. A point within `linear` of an end point is reported at that
    /// end.
    fn locate(&self, hint: Option<f64>, p: &Point2, linear: f64) -> Option<f64> {
        match self {
            Self::Linear(l) => {
                let s = hint.unwrap_or_else(|| (p - l.origin).dot(&l.dir));
                if l.lo.is_finite() && (s - l.lo).abs() <= linear {
                    Some(l.lo)
                } else if l.hi.is_finite() && (s - l.hi).abs() <= linear {
                    Some(l.hi)
                } else {
                    (s >= l.lo && s <= l.hi).then_some(s)
                }
            }
            Self::Conic(c) => {
                if (p - self.point(0.0)).norm() <= linear {
                    return Some(0.0);
                }
                if !c.is_full() && (p - self.point(c.sweep)).norm() <= linear {
                    return Some(c.sweep);
                }
                let tau = c.tau_of(hint.unwrap_or_else(|| c.angle_of(p)));
                (tau <= c.sweep).then_some(tau)
            }
        }
    }

    /// The parameter of the point of the piece closest to `p`, and the distance.
    fn closest(&self, p: &Point2) -> Result<(f64, f64)> {
        let candidates = match self {
            Self::Linear(l) => vec![(p - l.origin).dot(&l.dir).clamp(l.lo, l.hi)],
            Self::Conic(c) => {
                // d/dt of |P(t) − p|²/2 is a trigonometric polynomial of degree 2.
                let (q, _) = difference(&c.center, p);
                let derivative = Trig([
                    0.0,
                    q.dot(&c.minor),
                    -q.dot(&c.major),
                    c.major.dot(&c.minor),
                    0.5 * (c.minor.norm_squared() - c.major.norm_squared()),
                ]);
                let mut taus: Vec<f64> = roots(&derivative)?
                    .into_iter()
                    .map(|t| c.tau_of(t))
                    .filter(|tau| *tau <= c.sweep)
                    .collect();
                taus.push(0.0);
                if !c.is_full() {
                    taus.push(c.sweep);
                }
                taus
            }
        };
        Ok(candidates
            .into_iter()
            .map(|tau| (tau, (self.point(tau) - p).norm()))
            .fold((0.0, f64::INFINITY), |best, next| {
                if next.1 < best.1 { next } else { best }
            }))
    }

    fn bounding_box(&self) -> Option<Aabb2> {
        match self {
            Self::Linear(l) => (l.lo.is_finite() && l.hi.is_finite())
                .then(|| Aabb2::from_corners(self.point(l.lo), self.point(l.hi))),
            Self::Conic(c) => {
                let half = Vector2::new(c.major.x.hypot(c.minor.x), c.major.y.hypot(c.minor.y));
                Some(Aabb2::from_corners(c.center - half, c.center + half))
            }
        }
    }

    /// The largest coordinate magnitude the computations with this piece involve.
    fn magnitude(&self) -> f64 {
        match self {
            Self::Linear(l) => l.origin.coords.amax(),
            Self::Conic(c) => c.center.coords.amax() + c.major.norm(),
        }
    }
}

/// A piece of a curve and the map from the piece parameter to the curve parameter.
#[derive(Debug, Clone, Copy)]
pub(super) struct Piece {
    shape: Shape,
    offset: f64,
    divisor: f64,
}

impl Piece {
    fn segment(segment: &Segment2, offset: f64) -> Self {
        let chord = segment.end() - segment.start();
        let length = chord.norm();
        Self {
            shape: Shape::Linear(Linear {
                origin: segment.start(),
                dir: chord / length,
                lo: 0.0,
                hi: length,
            }),
            offset,
            divisor: length,
        }
    }

    fn whole(shape: Shape) -> Self {
        Self {
            shape,
            offset: 0.0,
            divisor: 1.0,
        }
    }

    /// The curve parameter of the piece parameter `tau`.
    pub(super) fn param(&self, tau: f64) -> f64 {
        self.offset + tau / self.divisor
    }

    /// A lower bound on the distance traveled per unit of curve parameter.
    pub(super) fn speed(&self) -> f64 {
        match self.shape {
            Shape::Linear(_) => self.divisor,
            Shape::Conic(c) => self.divisor * c.minor.norm(),
        }
    }

    /// Whether the bounding boxes, grown by `linear`, meet; unbounded pieces always may.
    pub(super) fn may_meet(&self, other: &Self, linear: f64) -> bool {
        match (self.shape.bounding_box(), other.shape.bounding_box()) {
            (Some(a), Some(b)) => a.expanded(linear).intersects(&b),
            _ => true,
        }
    }
}

/// The pieces of a curve, in parameter order.
pub(super) fn pieces(curve: &Curve2) -> Vec<Piece> {
    match curve {
        Curve2::Segment(s) => vec![Piece::segment(s, 0.0)],
        Curve2::Ray(r) => vec![Piece::whole(Shape::Linear(Linear {
            origin: r.origin(),
            dir: r.unit_direction(),
            lo: 0.0,
            hi: f64::INFINITY,
        }))],
        Curve2::Line(l) => vec![Piece::whole(Shape::Linear(Linear {
            origin: l.point(),
            dir: l.unit_direction(),
            lo: f64::NEG_INFINITY,
            hi: f64::INFINITY,
        }))],
        Curve2::Circle(c) => vec![Piece::whole(Shape::Conic(Conic::circle(
            c.center(),
            c.radius(),
            0.0,
            TAU,
        )))],
        Curve2::Arc(a) => vec![Piece::whole(Shape::Conic(Conic::arc(a)))],
        Curve2::Ellipse(e) => vec![Piece::whole(Shape::Conic(Conic::ellipse(e, 0.0, TAU)))],
        Curve2::EllipticalArc(e) => vec![Piece::whole(Shape::Conic(Conic::ellipse(
            e.ellipse(),
            e.start_parameter(),
            e.sweep(),
        )))],
        Curve2::Polyline(p) => p
            .segments()
            .enumerate()
            .map(|(k, segment)| match segment {
                PolylineSegment::Line(s) => Piece::segment(&s, k as f64),
                PolylineSegment::Arc(a) => Piece {
                    shape: Shape::Conic(Conic::arc(&a)),
                    offset: k as f64,
                    divisor: a.sweep().abs(),
                },
            })
            .collect(),
    }
}

/// An intersection of two pieces in their own parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Raw {
    /// An isolated point at `s` on the first piece and `t` on the second.
    Point { s: f64, t: f64, tangent: bool },
    /// A shared stretch, both intervals ascending; `same` when they run the same way.
    Overlap {
        s: [f64; 2],
        t: [f64; 2],
        same: bool,
    },
}

impl Raw {
    fn swapped(self) -> Self {
        match self {
            Self::Point { s, t, tangent } => Self::Point {
                s: t,
                t: s,
                tangent,
            },
            Self::Overlap { s, t, same } => Self::Overlap { s: t, t: s, same },
        }
    }
}

/// A candidate point before it is checked against the piece ranges and the error bound.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    hint_p: Option<f64>,
    hint_q: Option<f64>,
    point: Point2,
    tangent: bool,
    /// The error bound of the quantities the point is constructed from, in length units.
    delta: f64,
}

impl Candidate {
    fn swapped(self) -> Self {
        Self {
            hint_p: self.hint_q,
            hint_q: self.hint_p,
            ..self
        }
    }
}

enum Outcome {
    /// The pieces lie on one carrier: the final result.
    Coincident(Vec<Raw>),
    /// Isolated candidate points.
    Candidates(Vec<Candidate>),
}

impl Outcome {
    fn swapped(self) -> Self {
        match self {
            Self::Coincident(raws) => {
                Self::Coincident(raws.into_iter().map(Raw::swapped).collect())
            }
            Self::Candidates(c) => {
                Self::Candidates(c.into_iter().map(Candidate::swapped).collect())
            }
        }
    }
}

/// Every intersection of two pieces.
///
/// # Errors
///
/// `geom.precision.insufficient` when a coordinate is so large that `f64` cannot resolve the
/// linear tolerance there, or when the error bound of a constructed point exceeds it.
pub(super) fn intersect_pieces(p: &Piece, q: &Piece, tolerance: &Tolerance) -> Result<Vec<Raw>> {
    let linear = tolerance.linear;
    let magnitude = p.shape.magnitude().max(q.shape.magnitude());
    if ROUNDING * magnitude > linear || !magnitude.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("magnitude", magnitude));
    }
    let outcome = match (&p.shape, &q.shape) {
        (Shape::Linear(a), Shape::Linear(b)) => linear_linear(a, b, tolerance),
        (Shape::Linear(a), Shape::Conic(b)) => linear_conic(a, b, linear),
        (Shape::Conic(a), Shape::Linear(b)) => linear_conic(b, a, linear).swapped(),
        (Shape::Conic(a), Shape::Conic(b)) if a.is_circle() && b.is_circle() => {
            circle_circle(a, b, linear)
        }
        (Shape::Conic(a), Shape::Conic(b)) => conic_conic(a, b, linear)?,
    };
    let candidates = match outcome {
        Outcome::Coincident(raws) => return Ok(raws),
        Outcome::Candidates(candidates) => candidates,
    };
    let mut out = Vec::new();
    for candidate in &candidates {
        accept(&p.shape, &q.shape, candidate, linear, &mut out)?;
    }
    end_contacts(&p.shape, &q.shape, tolerance, &mut out)?;
    Ok(out)
}

/// Locates a candidate on both pieces and checks its error bound: the input error divided by
/// the sine of the crossing angle for a crossing (the displacement of the crossing when one
/// curve moves by that much), the input error itself for a tangency, plus the rounding of the
/// point's own coordinates.
fn accept(p: &Shape, q: &Shape, c: &Candidate, linear: f64, out: &mut Vec<Raw>) -> Result<()> {
    let (Some(s), Some(t)) = (
        p.locate(c.hint_p, &c.point, linear),
        q.locate(c.hint_q, &c.point, linear),
    ) else {
        return Ok(());
    };
    let mut bound = c.delta;
    if !c.tangent {
        bound /= sine(&p.tangent(s), &q.tangent(t)).max(f64::EPSILON);
    }
    bound += ROUNDING * c.point.coords.amax();
    if bound > linear || bound.is_nan() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("error_bound", bound));
    }
    out.push(Raw::Point {
        s,
        t,
        tangent: c.tangent,
    });
    Ok(())
}

/// Adds the end points of either piece that lie within the linear tolerance of the other piece
/// and are not yet near a found point. Such a point is tangent when the directions there are
/// parallel within the angular tolerance.
fn end_contacts(p: &Shape, q: &Shape, tolerance: &Tolerance, out: &mut Vec<Raw>) -> Result<()> {
    let linear = tolerance.linear;
    for tau in p.ends() {
        let end = p.point(tau);
        let known = out
            .iter()
            .any(|r| matches!(r, Raw::Point { s, .. } if (p.point(*s) - end).norm() <= linear));
        let (t, distance) = q.closest(&end)?;
        if !known && distance <= linear {
            out.push(Raw::Point {
                s: tau,
                t,
                tangent: sine(&p.tangent(tau), &q.tangent(t)) <= tolerance.angular,
            });
        }
    }
    for tau in q.ends() {
        let end = q.point(tau);
        let known = out
            .iter()
            .any(|r| matches!(r, Raw::Point { t, .. } if (q.point(*t) - end).norm() <= linear));
        let (s, distance) = p.closest(&end)?;
        if !known && distance <= linear {
            out.push(Raw::Point {
                s,
                t: tau,
                tangent: sine(&p.tangent(s), &q.tangent(tau)) <= tolerance.angular,
            });
        }
    }
    Ok(())
}

/// Two linear pieces. They are parallel when the sine of their angle is at most the angular
/// tolerance, and collinear when also each origin lies within the linear tolerance of the other
/// carrier. Collinear pieces overlap where their ranges overlap by more than the linear
/// tolerance and touch at one tangent point where they meet within it.
fn linear_linear(a: &Linear, b: &Linear, tolerance: &Tolerance) -> Outcome {
    let linear = tolerance.linear;
    let (d, input_error) = difference(&b.origin, &a.origin);
    let sin = cross(&a.dir, &b.dir);
    if sin.abs() > tolerance.angular {
        let s = cross(&d, &b.dir) / sin;
        let t = cross(&d, &a.dir) / sin;
        return Outcome::Candidates(vec![Candidate {
            hint_p: Some(s),
            hint_q: Some(t),
            point: a.origin + a.dir * s,
            tangent: false,
            delta: input_error + ROUNDING * (d.norm() + s.abs() + t.abs()),
        }]);
    }
    let distance = cross(&a.dir, &d).abs().max(cross(&b.dir, &d).abs());
    if distance > linear {
        return Outcome::Coincident(Vec::new());
    }
    let sigma = if a.dir.dot(&b.dir) < 0.0 { -1.0 } else { 1.0 };
    let s0 = d.dot(&a.dir);
    let to_a = |t: f64| s0 + sigma * t;
    let to_b = |s: f64| sigma * (s - s0);
    let [m0, m1] = ordered(to_a(b.lo), to_a(b.hi));
    let (lo, hi) = (a.lo.max(m0), a.hi.min(m1));
    Outcome::Coincident(if hi - lo > linear {
        vec![Raw::Overlap {
            s: [a.snap(lo, linear), a.snap(hi, linear)],
            t: ordered(b.snap(to_b(lo), linear), b.snap(to_b(hi), linear)),
            same: sigma > 0.0,
        }]
    } else if hi - lo >= -linear {
        let mid = 0.5 * (lo + hi);
        vec![Raw::Point {
            s: a.snap(mid, linear),
            t: b.snap(to_b(mid), linear),
            tangent: true,
        }]
    } else {
        Vec::new()
    })
}

/// A linear piece and a conic. With `h` the signed distance of the line from the center and
/// `w` the half-width of the conic across the line's normal (the radius for a circle), the line
/// is tangent when `||h| − w| ≤ linear`, crosses twice when `|h| < w − linear` and misses
/// otherwise.
fn linear_conic(l: &Linear, c: &Conic, linear: f64) -> Outcome {
    let (q0, input_error) = difference(&l.origin, &c.center);
    let normal = perp(&l.dir);
    let h = normal.dot(&q0);
    let (an, bn) = (c.major.dot(&normal), c.minor.dot(&normal));
    let w = an.hypot(bn);
    let delta = input_error + ROUNDING * (q0.norm() + c.major.norm());
    if h.abs() > w + linear {
        return Outcome::Candidates(Vec::new());
    }
    let tangent_at = |s: f64, t: f64, point: Point2| Candidate {
        hint_p: Some(s),
        hint_q: Some(t),
        point,
        tangent: true,
        delta,
    };
    if (h.abs() - w).abs() <= linear {
        // The support point of the conic in the direction of the line.
        let side = if h < 0.0 { -1.0 } else { 1.0 };
        let t = (side * bn).atan2(side * an);
        let radial = c.major * t.cos() + c.minor * t.sin();
        let s = (radial - q0).dot(&l.dir);
        return Outcome::Candidates(vec![tangent_at(s, t, c.center + radial)]);
    }
    // In the frame where the conic is the unit circle the line is u(s), v(s), affine in s.
    let alpha = c.major / c.major.norm_squared();
    let beta = c.minor / c.minor.norm_squared();
    let (u0, u1, v0, v1) = (
        q0.dot(&alpha),
        l.dir.dot(&alpha),
        q0.dot(&beta),
        l.dir.dot(&beta),
    );
    let speed = u1 * u1 + v1 * v1;
    let s0 = -(u0 * u1 + v0 * v1) / speed;
    let m = (u0 + s0 * u1).hypot(v0 + s0 * v1);
    if m >= 1.0 {
        // Rounding contradicts the half-width test: the line grazes the conic.
        let t = (v0 + s0 * v1).atan2(u0 + s0 * u1);
        return Outcome::Candidates(vec![tangent_at(s0, t, l.origin + l.dir * s0)]);
    }
    let half = ((1.0 - m) * (1.0 + m) / speed).sqrt();
    Outcome::Candidates(
        [s0 - half, s0 + half]
            .into_iter()
            .map(|s| Candidate {
                hint_p: Some(s),
                hint_q: Some((v0 + s * v1).atan2(u0 + s * u1)),
                point: l.origin + l.dir * s,
                tangent: false,
                delta,
            })
            .collect(),
    )
}

/// Two circular pieces with center distance `d` and radii `r1`, `r2`, decided in this order:
/// the same circle when `d + |r1 − r2| ≤ linear`; externally tangent when
/// `|d − (r1 + r2)| ≤ linear`; internally tangent when `d > 0` and `|d − |r1 − r2|| ≤ linear`;
/// two crossings when `|r1 − r2| < d < r1 + r2`; otherwise nothing.
fn circle_circle(a: &Conic, b: &Conic, linear: f64) -> Outcome {
    let (r1, r2) = (a.major.norm(), b.major.norm());
    let (dv, input_error) = difference(&b.center, &a.center);
    let d = dv.norm();
    let delta = input_error + ROUNDING * (d + r1 + r2);
    if d + (r1 - r2).abs() <= linear {
        let phi = a.angle_of(&b.at_angle(0.0));
        return Outcome::Coincident(arc_overlap(a, b, phi, linear));
    }
    let candidate = |point: Point2, tangent: bool| Candidate {
        hint_p: None,
        hint_q: None,
        point,
        tangent,
        delta,
    };
    if d == 0.0 {
        return Outcome::Candidates(Vec::new());
    }
    let u = dv / d;
    Outcome::Candidates(if (d - (r1 + r2)).abs() <= linear {
        vec![candidate(
            midpoint(&(a.center + u * r1), &(b.center - u * r2)),
            true,
        )]
    } else if (d - (r1 - r2).abs()).abs() <= linear {
        // The touching point lies on the ray from the larger center through the smaller one.
        let u = if r1 >= r2 { u } else { -u };
        vec![candidate(
            midpoint(&(a.center + u * r1), &(b.center + u * r2)),
            true,
        )]
    } else if d < r1 + r2 && d > (r1 - r2).abs() {
        let x = 0.5 * (d + (r1 - r2) * (r1 + r2) / d);
        let h = ((r1 - x) * (r1 + x)).max(0.0).sqrt();
        let base = a.center + u * x;
        let n = perp(&u) * h;
        vec![candidate(base + n, false), candidate(base - n, false)]
    } else {
        Vec::new()
    })
}

/// Two conic pieces, at least one not a circle.
///
/// The rounder conic `E1` is written implicitly as `f = U² + V² − 1`, with `U`, `V` its unit
/// circle coordinates, and the other conic `E2(t)` is substituted, which gives a trigonometric
/// polynomial `f(t)` of degree 2. Its signed distance estimate is `f / |∇f|`. The critical
/// points of `f` split the period into monotone stretches. A critical point whose distance is
/// within the linear tolerance is a tangency, and the sign changes of the stretches next to it
/// belong to it; every other sign change is a crossing, found by bisection. When the distance
/// is within the linear tolerance at every critical point and at `t = 0`, the conics coincide.
fn conic_conic(a: &Conic, b: &Conic, linear: f64) -> Result<Outcome> {
    let flip = b.ratio() > a.ratio();
    let (e1, e2) = if flip { (b, a) } else { (a, b) };
    let (qv, input_error) = difference(&e2.center, &e1.center);
    let alpha = e1.major / e1.major.norm_squared();
    let beta = e1.minor / e1.minor.norm_squared();
    let (u0, ua, ub) = (qv.dot(&alpha), e2.major.dot(&alpha), e2.minor.dot(&alpha));
    let (v0, va, vb) = (qv.dot(&beta), e2.major.dot(&beta), e2.minor.dot(&beta));
    let f = Trig([
        u0 * u0 + v0 * v0 + 0.5 * (ua * ua + ub * ub + va * va + vb * vb) - 1.0,
        2.0 * (u0 * ua + v0 * va),
        2.0 * (u0 * ub + v0 * vb),
        0.5 * (ua * ua - ub * ub + va * va - vb * vb),
        ua * ub + va * vb,
    ]);
    let gradient = |t: f64| {
        let (sin, cos) = t.sin_cos();
        let (u, v) = (u0 + ua * cos + ub * sin, v0 + va * cos + vb * sin);
        2.0 * (alpha * u + beta * v).norm()
    };
    let distance = |t: f64| f.eval(t) / gradient(t);
    let aspect = e1.major.norm() / e1.minor.norm();
    let base_delta =
        (input_error + ROUNDING * (qv.norm() + e1.major.norm() + e2.major.norm())) * aspect;
    let f_rounding = ROUNDING * (1.0 + f.abs_sum());
    let derivative = f.derivative();
    let critical = if derivative.variation() <= f_rounding {
        Vec::new()
    } else {
        roots(&derivative)?
    };
    if critical
        .iter()
        .chain([0.0].iter())
        .all(|t| distance(*t).abs() <= linear)
    {
        let phi = (v0 + va).atan2(u0 + ua);
        let raws = arc_overlap(e1, e2, phi, linear);
        return Ok(Outcome::Coincident(if flip {
            raws.into_iter().map(Raw::swapped).collect()
        } else {
            raws
        }));
    }
    let marked: Vec<(f64, bool)> = critical
        .iter()
        .map(|t| (*t, distance(*t).abs() <= linear))
        .collect();
    let mut found: Vec<(f64, bool)> = marked.iter().copied().filter(|m| m.1).collect();
    if marked.len() >= 2 {
        let next = marked.iter().cycle().skip(1);
        for (&(x0, touch0), &(x1, touch1)) in marked.iter().zip(next) {
            let x1 = if x1 <= x0 { x1 + TAU } else { x1 };
            let (f0, f1) = (f.eval(x0), f.eval(x1));
            if !touch0 && !touch1 && f0.signum() != f1.signum() {
                found.push((bisect(&f, x0, x1, f0).rem_euclid(TAU), false));
            }
        }
    }
    Ok(Outcome::Candidates(
        found
            .into_iter()
            .map(|(t2, tangent)| {
                let point = e2.at_angle(t2);
                let t1 = e1.angle_of(&point);
                let (hint_p, hint_q) = if flip { (t2, t1) } else { (t1, t2) };
                Candidate {
                    hint_p: Some(hint_p),
                    hint_q: Some(hint_q),
                    point,
                    tangent,
                    delta: base_delta + f_rounding / gradient(t2),
                }
            })
            .collect(),
    ))
}

/// Two conic pieces on the same conic, where eccentric angle `t` of `q` is `t + phi` of `p`.
/// Their angular ranges are intersected on the circle of angles: a common stretch whose chord
/// exceeds the linear tolerance is an overlap; a shorter one, or a gap whose chord is within the
/// tolerance, is a tangent touching point.
fn arc_overlap(p: &Conic, q: &Conic, phi: f64, linear: f64) -> Vec<Raw> {
    let ccw_start = |c: &Conic| {
        if c.sign > 0.0 {
            c.start
        } else {
            c.start - c.sweep
        }
    };
    let a1 = ccw_start(p);
    let (w1, w2) = (p.sweep, q.sweep);
    let chord = |x: f64, y: f64| (p.at_angle(a1 + x) - p.at_angle(a1 + y)).norm();
    let mut beta = (ccw_start(q) + phi - a1).rem_euclid(TAU);
    if chord(0.0, beta) <= linear {
        beta = 0.0;
    }
    let to_p = |x: f64| (if p.sign > 0.0 { x } else { w1 - x }).clamp(0.0, w1);
    let same = (p.sign > 0.0) == (q.sign > 0.0);
    let mut out = Vec::new();
    for offset in [beta, beta - TAU] {
        let to_q = |x: f64| {
            let u = x - offset;
            (if q.sign > 0.0 { u } else { w2 - u }).clamp(0.0, w2)
        };
        let touching = |x: f64| Raw::Point {
            s: to_p(x),
            t: to_q(x),
            tangent: true,
        };
        let (lo, hi) = (offset.max(0.0), (offset + w2).min(w1));
        if hi >= lo {
            if hi - lo < PI && chord(lo, hi) <= linear {
                out.push(touching(0.5 * (lo + hi)));
            } else {
                out.push(Raw::Overlap {
                    s: ordered(to_p(lo), to_p(hi)),
                    t: ordered(to_q(lo), to_q(hi)),
                    same,
                });
            }
        } else if lo - hi < PI && chord(hi, lo) <= linear {
            out.push(touching(0.5 * (lo + hi)));
        }
    }
    out
}
