//! Curve offsets (CORE-GEOM-007).
//!
//! [`offset`] and [`offset_composite`] return the curves at a signed distance `d` from a curve:
//! positive to the left of the direction of travel (along [`CurveEval::normal`]), negative to
//! the right. The result is a list of composites, because removing loops can leave several
//! separate chains.
//!
//! 1. The curve is split into smooth elements: segments, arcs (polyline segments keep their
//!    chord form), elliptical arcs (a full circle or ellipse is one full arc) and the pieces of a
//!    NURBS curve between its corner knots.
//! 2. Each element is offset: a segment to the parallel segment, an arc to the concentric arc
//!    (a polyline arc to the chord arc with the same bulge between its offset end points), an
//!    elliptical arc or NURBS piece to a cubic NURBS curve interpolating the exact offset
//!    `P(t) + d·N(t)` at uniformly spaced parameters, with the number of samples doubled until
//!    the curve is within a quarter of `tolerance.linear` of the exact offset at the midpoints
//!    and quarter points of every sample interval. An arc whose offset reaches or passes its
//!    center has the arc on the other side of the center as its raw offset (removed with the
//!    loops, it still cuts its neighbors), and none when the offset is the center itself.
//! 3. Where consecutive offset elements do not meet within `tolerance.linear`, a join is
//!    inserted: at a convex corner (the offset side is outside the turn) a circular arc of
//!    radius `|d|` about the corner, anywhere else a straight connector, which lies closer to
//!    the curve than `|d|` and is removed in the next step. Open curves get no caps.
//! 4. Loops are removed: the raw chain is split at all its intersections with itself and with
//!    the rest of the boundary of the region within `|d|` of the curve (the offsets of the
//!    elements by `−d` and the circles of radius `|d|` about the vertices and ends), found
//!    (by [`crate::intersect`] between exact pieces; for NURBS pieces, by crossings of their
//!    flattened polylines refined with Newton iteration on the curves), and every sub-piece is
//!    sampled at its ends and seven interior points; a sample closer to the curve than
//!    `|d| − tolerance.linear` is invalid. Sub-pieces with only valid samples are kept, with only
//!    invalid ones dropped, and mixed ones are split where the validity changes (found by
//!    bisection) and decided by their midpoints. Parts at most twice `tolerance.linear` long are
//!    dropped. What remains of each round join is then replaced according to the join style:
//!    kept ([`JoinStyle::Round`]), by its chord ([`JoinStyle::Bevel`]), or by the tangent lines
//!    at its ends up to their intersection when that lies within `limit·|d|` of the corner and by
//!    its chord otherwise ([`JoinStyle::Miter`]), or by the tangent lines at its ends up to the
//!    line tangent to it at its middle and that line ([`JoinStyle::Square`]). A chord lies
//!    inside the circle of radius `|d|` about the corner, which no kept piece enters, so it
//!    crosses nothing; the lines of a miter or square join lie outside it and are not checked
//!    against other pieces.
//! 5. The kept sub-pieces are chained end to start within four times `tolerance.linear`, each
//!    chain starting from the first unused piece in order of the raw chain; a chain is closed
//!    when it ends within that distance of its start.
//!
//! Every point of a result with round joins lies at distance `|d|` from the curve within the
//! linear tolerance (plus the rounding of the constructions). Degenerate cases: `d = 0` returns
//! the curve unchanged; a circle or arc whose offset collapses, or any curve of which nothing
//! remains, fails with `geom.offset.collapsed`; a NURBS piece with a point of zero derivative,
//! where the normal is undefined, fails with `geom.offset.degenerate`; rays and lines fail with
//! `geom.curve.unbounded` (their offsets are [`crate::Line2::offset`] and [`crate::Ray2::offset`]).

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::bbox::Aabb2;
use crate::closest::CurveProject;
use crate::curve::Curve2;
use crate::edit::{Composite2, subcurve};
use crate::error::codes;
use crate::eval::{CurveEval, Shaped, cross};
use crate::index::SpatialIndex2;
use crate::intersect::{Intersection, intersect};
use crate::nurbs::Nurbs2;
use crate::primitives::{Arc2, Circle2, Line2, Ray2, Segment2};

/// The fewest samples of an elliptical arc or NURBS piece for its offset fit.
const FIRST_SAMPLES: usize = 16;
/// The most samples of one offset fit.
const MAX_SAMPLES: usize = 1 << 12;
/// Interior samples of a sub-piece, besides its ends, when deciding whether it belongs to the
/// offset.
const CLASSIFY_SAMPLES: usize = 7;
/// Pieces at most this many linear tolerances long are dropped: they are slivers between an
/// intersection and the point where bisection places the change of validity.
const SLIVER_TOLERANCES: f64 = 2.0;
/// Chained offset pieces meet within this many linear tolerances.
const CHAIN_TOLERANCES: f64 = 4.0;
/// The chord of the flattening used to find NURBS crossings, in linear tolerances.
const CROSSING_CHORD_TOLERANCES: f64 = 64.0;
/// The chord of that flattening relative to the size of the curve, at least.
const CROSSING_RELATIVE_CHORD: f64 = 1e-5;
/// The most vertices of that flattening.
const MAX_CROSSING_VERTICES: usize = 1 << 20;
/// The turning per segment of that flattening, in radians.
const CROSSING_ANGLE: f64 = 0.1;
/// Newton steps refining a NURBS crossing.
const NEWTON_STEPS: usize = 16;
/// Bisection steps locating a change of validity.
const BISECTION_STEPS: usize = 60;

/// How the offsets of two elements are joined at a convex corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinStyle {
    /// A circular arc of radius `|d|` about the corner.
    Round,
    /// The tangent lines extended to their intersection, when it lies within `limit` times
    /// `|d|` of the corner; a straight segment otherwise. `limit` is at least 1.
    Miter {
        /// The largest distance of the miter point from the corner, in units of `|d|`.
        limit: f64,
    },
    /// A straight segment between the offset ends.
    Bevel,
    /// The tangent lines extended from the offset ends to the line tangent to the round join
    /// at its middle, at distance `|d|` from the corner, and that line between them.
    Square,
}

impl Line2 {
    /// The parallel line at signed distance `distance`, positive to the left of the direction.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite distance or an image that overflows.
    pub fn offset(&self, distance: f64) -> Result<Self> {
        crate::ensure_finite(&[distance])?;
        let normal = crate::primitives::perp(&self.unit_direction());
        let point = self.point() + normal * distance;
        crate::ensure_finite(&[point.x, point.y])?;
        Self::new(point, self.direction())
    }
}

impl Ray2 {
    /// The parallel ray at signed distance `distance`, positive to the left of the direction.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite distance or an image that overflows.
    pub fn offset(&self, distance: f64) -> Result<Self> {
        crate::ensure_finite(&[distance])?;
        let normal = crate::primitives::perp(&self.unit_direction());
        let origin = self.origin() + normal * distance;
        crate::ensure_finite(&[origin.x, origin.y])?;
        Self::new(origin, self.direction())
    }
}

/// The offsets of `curve` at signed distance `distance`; see the [module documentation](self).
///
/// # Errors
///
/// `geom.tolerance.invalid` for a tolerance that is invalid or has a zero linear part;
/// `geom.input.not_finite` for a non-finite distance; `geom.offset.join_invalid` for a miter
/// limit that is not a finite number of at least 1; `geom.curve.unbounded` for a ray or line;
/// `geom.offset.collapsed` when nothing of the offset remains; `geom.offset.degenerate` for a
/// point without tangent; `geom.offset.too_many_pieces` when the offset needs more pieces than
/// `limits.array_length` or a fit more than 4,096 samples; `geom.precision.insufficient` when
/// a construction exceeds `f64`.
pub fn offset(
    curve: &Curve2,
    distance: f64,
    join: JoinStyle,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Composite2>> {
    validate(distance, join, tolerance)?;
    if curve.bounding_box().is_none() {
        return Err(Error::new(codes::CURVE_UNBOUNDED));
    }
    if distance == 0.0 {
        return Ok(vec![Composite2::new(vec![curve.clone()], tolerance)?]);
    }
    if let Curve2::Circle(c) = curve {
        // The left normal of a counterclockwise circle points to its center.
        let radius = c.radius() - distance;
        if radius <= 0.0 || radius.is_nan() {
            return Err(Error::new(codes::OFFSET_COLLAPSED));
        }
        let circle = Circle2::new(c.center(), radius)?;
        return Ok(vec![Composite2::new(
            vec![Curve2::Circle(circle)],
            tolerance,
        )?]);
    }
    let elements = elements(curve)?;
    run(
        &elements,
        curve.is_closed(),
        distance,
        join,
        tolerance,
        limits,
    )
}

/// The offsets of a composite at signed distance `distance`, with joins between its curves.
///
/// # Errors
///
/// As [`offset`].
pub fn offset_composite(
    composite: &Composite2,
    distance: f64,
    join: JoinStyle,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Composite2>> {
    validate(distance, join, tolerance)?;
    if distance == 0.0 {
        return Ok(vec![composite.clone()]);
    }
    let mut all = Vec::new();
    for curve in composite.curves() {
        all.extend(elements(curve)?);
    }
    run(
        &all,
        composite.is_closed(),
        distance,
        join,
        tolerance,
        limits,
    )
}

fn validate(distance: f64, join: JoinStyle, tolerance: &Tolerance) -> Result<()> {
    crate::ensure_tolerance(tolerance)?;
    crate::eval::ensure_tolerance("linear", tolerance.linear)?;
    crate::ensure_finite(&[distance])?;
    if let JoinStyle::Miter { limit } = join
        && !(limit.is_finite() && limit >= 1.0)
    {
        return Err(Error::new(codes::OFFSET_JOIN_INVALID).with_detail("limit", limit));
    }
    Ok(())
}

/// The smooth elements of a bounded curve, in order.
fn elements(curve: &Curve2) -> Result<Vec<Curve2>> {
    Ok(match curve {
        Curve2::Ray(_) | Curve2::Line(_) => return Err(Error::new(codes::CURVE_UNBOUNDED)),
        Curve2::Circle(c) => vec![Curve2::Arc(c.to_arc())],
        Curve2::Ellipse(e) => vec![Curve2::EllipticalArc(e.to_arc())],
        Curve2::Polyline(p) => p
            .segments()
            .map(|s| match s {
                crate::polyline::PolylineSegment::Line(s) => Curve2::Segment(s),
                crate::polyline::PolylineSegment::Arc(a) => Curve2::Arc(a),
            })
            .collect(),
        Curve2::Nurbs(n) => {
            let corners = n.corner_knots();
            crate::edit::split_at_parameters(curve, &corners, &Tolerance::DEFAULT).or_else(
                |_| {
                    // Corners too close to split cleanly: the curve as one element.
                    Ok::<_, Error>(vec![Curve2::Nurbs(n.clone())])
                },
            )?
        }
        other => vec![other.clone()],
    })
}

/// The offset of one element, or `None` when it collapses.
fn element_offset(
    element: &Curve2,
    d: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Option<Curve2>> {
    match element {
        Curve2::Segment(s) => {
            let n = crate::primitives::perp(&crate::unit(&(s.end() - s.start())));
            Ok(Some(Curve2::Segment(Segment2::new(
                s.start() + n * d,
                s.end() + n * d,
            )?)))
        }
        Curve2::Arc(a) => {
            let radius = a.radius() - a.sweep().signum() * d;
            if radius == 0.0 || radius.is_nan() {
                return Ok(None);
            }
            if radius < 0.0 {
                // Past the center the offset is the arc turned a half turn about it; it lies
                // closer than the distance and is removed, but its crossings cut the neighbors.
                return Ok(Some(Curve2::Arc(Arc2::new(
                    a.center(),
                    -radius,
                    a.start_angle() + PI,
                    a.sweep(),
                )?)));
            }
            if let Some(chord) = a.chord() {
                let start = chord.start() + element.normal(0.0) * d;
                let end = chord.end() + element.normal(a.sweep().abs()) * d;
                if let Some(arc) = crate::encoding::chord_arc(start, end, chord.bulge())? {
                    return Ok(Some(Curve2::Arc(arc)));
                }
            }
            Ok(Some(Curve2::Arc(Arc2::new(
                a.center(),
                radius,
                a.start_angle(),
                a.sweep(),
            )?)))
        }
        _ => fit_offset(element, d, tolerance, limits).map(Some),
    }
}

/// A cubic NURBS curve within a quarter of the linear tolerance of the exact offset of a smooth
/// element, checked at the midpoints and quarter points of the sample intervals.
fn fit_offset(element: &Curve2, d: f64, tolerance: &Tolerance, limits: &Limits) -> Result<Curve2> {
    let (lo, hi) = element.parameter_range();
    let exact = |t: f64| -> Result<Point2> {
        let derivative = element.derivative(t);
        if derivative.x == 0.0 && derivative.y == 0.0 {
            return Err(Error::new(codes::OFFSET_DEGENERATE).with_detail("parameter", t));
        }
        Ok(element.point_at(t) + element.normal(t) * d)
    };
    let cap = usize::try_from(limits.array_length)
        .unwrap_or(usize::MAX)
        .min(MAX_SAMPLES);
    let target = 0.25 * tolerance.linear;
    let mut samples = FIRST_SAMPLES;
    while samples <= cap {
        let parameters: Vec<f64> = (0..=samples)
            .map(|i| {
                if i == samples {
                    hi
                } else {
                    lo + (hi - lo) * i as f64 / samples as f64
                }
            })
            .collect();
        let mut points = parameters
            .iter()
            .map(|t| exact(*t))
            .collect::<Result<Vec<_>>>()?;
        if element.is_closed()
            && let (Some(first), Some(last)) = (points.first().copied(), points.last_mut())
        {
            *last = first;
        }
        let fit = Nurbs2::interpolate_at(&points, &parameters, 3, tolerance);
        if let Ok(fit) = fit {
            let mut within = true;
            'check: for pair in parameters.windows(2) {
                if let [a, b] = pair {
                    for f in [0.25, 0.5, 0.75] {
                        let t = a + (b - a) * f;
                        let error = crate::length(&(fit.point_at(t) - exact(t)?));
                        if error > target || error.is_nan() {
                            within = false;
                            break 'check;
                        }
                    }
                }
            }
            if within {
                return Ok(Curve2::Nurbs(fit));
            }
        }
        samples *= 2;
    }
    Err(Error::new(codes::OFFSET_TOO_MANY_PIECES).with_detail("operation", "fit"))
}

/// Whether the corner from tangent `t0` to `t1` is convex on the side of the offset `d`: the
/// offset side is outside the turn. A reversal counts as convex.
fn is_convex(t0: Vector2, t1: Vector2, d: f64) -> bool {
    let turn = cross(&t0, &t1).atan2(t0.dot(&t1));
    turn * d < 0.0 || turn.abs() == PI
}

/// The join from the end `e` of one offset element to the start `s` of the next, about the
/// corner `v` where the source tangents are `t0` and `t1`.
fn join_pieces(
    e: Point2,
    s: Point2,
    v: Point2,
    tangents: Option<(Vector2, Vector2)>,
    d: f64,
    join: JoinStyle,
) -> Result<Vec<Curve2>> {
    let straight = |a: Point2, b: Point2| -> Result<Vec<Curve2>> {
        Ok(if a == b {
            Vec::new()
        } else {
            vec![Curve2::Segment(Segment2::new(a, b)?)]
        })
    };
    let Some((t0, t1)) = tangents else {
        return straight(e, s);
    };
    if !is_convex(t0, t1, d) {
        return straight(e, s);
    }
    let sign = -d.signum();
    match join {
        JoinStyle::Bevel => straight(e, s),
        JoinStyle::Round => {
            let (a, b) = (e - v, s - v);
            let mut sweep = cross(&a, &b).atan2(a.dot(&b));
            if sweep * sign < 0.0 {
                sweep += sign * TAU;
            }
            if sweep == 0.0 {
                return straight(e, s);
            }
            Ok(vec![Curve2::Arc(Arc2::new(
                v,
                d.abs(),
                a.y.atan2(a.x),
                sweep,
            )?)])
        }
        JoinStyle::Miter { limit } => {
            let denominator = cross(&t0, &t1);
            if denominator == 0.0 {
                return straight(e, s);
            }
            // e + λ·t0 = s + μ·t1.
            let lambda = cross(&(s - e), &t1) / denominator;
            let miter = e + t0 * lambda;
            let reach = crate::length(&(miter - v));
            if reach > limit * d.abs() || reach.is_nan() || lambda < 0.0 {
                return straight(e, s);
            }
            let mut out = straight(e, miter)?;
            out.extend(straight(miter, s)?);
            Ok(out)
        }
        JoinStyle::Square => {
            // The tangent at the middle of the round join of sweep φ meets the tangents at its
            // ends |d|·tan(φ/4) from them.
            let (a, b) = (e - v, s - v);
            let sweep = cross(&a, &b).abs().atan2(a.dot(&b));
            let reach = d.abs() * (0.25 * sweep).tan();
            let (p, q) = (e + t0 * reach, s - t1 * reach);
            let mut out = straight(e, p)?;
            out.extend(straight(p, q)?);
            out.extend(straight(q, s)?);
            Ok(out)
        }
    }
}

/// The rest of the boundary of the region within `|d|` of the curve: the offsets of the
/// elements on the other side and the circles about their ends.
fn cutters(
    elements: &[Curve2],
    closed: bool,
    d: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Curve2>> {
    let mut out = Vec::new();
    for element in elements {
        out.extend(element_offset(element, -d, tolerance, limits)?);
        if let Some(start) = element.start_point() {
            out.push(Curve2::Circle(Circle2::new(start, d.abs())?));
        }
    }
    if !closed && let Some(end) = elements.last().and_then(Curve2::end_point) {
        out.push(Curve2::Circle(Circle2::new(end, d.abs())?));
    }
    Ok(out)
}

fn run(
    elements: &[Curve2],
    closed: bool,
    d: f64,
    join: JoinStyle,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Composite2>> {
    let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
    let offsets: Vec<Option<Curve2>> = elements
        .iter()
        .map(|e| element_offset(e, d, tolerance, limits))
        .collect::<Result<_>>()?;
    // The raw chain: offsets and joins in order.
    let mut raw: Vec<Curve2> = Vec::new();
    // For each raw piece of a round join, its corner.
    let mut corners: Vec<Option<Point2>> = Vec::new();
    let count = elements.len();
    let present: Vec<usize> = (0..count)
        .filter(|i| offsets.get(*i).is_some_and(Option::is_some))
        .collect();
    let joints = if closed {
        present.len()
    } else {
        present.len().saturating_sub(1)
    };
    for (k, &i) in present.iter().enumerate() {
        if let Some(Some(piece)) = offsets.get(i) {
            raw.push(piece.clone());
            corners.push(None);
        }
        if k >= joints {
            continue;
        }
        let j = present.get((k + 1) % present.len()).copied().unwrap_or(i);
        let (Some(Some(a)), Some(Some(b))) = (offsets.get(i), offsets.get(j)) else {
            continue;
        };
        let (Some(e), Some(s)) = (a.end_point(), b.start_point()) else {
            continue;
        };
        if crate::length(&(s - e)) <= tolerance.linear {
            continue;
        }
        // Tangents only across a corner between adjacent elements.
        let adjacent = j == (i + 1) % count.max(1);
        let (Some(source_i), Some(source_j)) = (elements.get(i), elements.get(j)) else {
            continue;
        };
        let tangents = adjacent.then(|| {
            (
                source_i.tangent(source_i.parameter_range().1),
                source_j.tangent(source_j.parameter_range().0),
            )
        });
        let corner = source_i.end_point().unwrap_or(e);
        // Loops are removed with round joins; other styles replace them afterwards.
        for piece in join_pieces(e, s, corner, tangents, d, JoinStyle::Round)? {
            let round = matches!(piece, Curve2::Arc(_));
            raw.push(piece);
            corners.push(round.then_some(corner));
        }
        if raw.len() > limit {
            return Err(Error::new(codes::OFFSET_TOO_MANY_PIECES));
        }
    }
    if raw.is_empty() {
        return Err(Error::new(codes::OFFSET_COLLAPSED));
    }
    // Cutters: the rest of the boundary of the region within |d| of the curve, the offsets on
    // the other side and the circles about the vertices. They only split the raw pieces, so
    // that every stretch closer than |d| to the curve begins and ends at a cut.
    let real = raw.len();
    let mut all = raw.clone();
    all.extend(cutters(elements, closed, d, tolerance, limits)?);
    if all.len() > limit {
        return Err(Error::new(codes::OFFSET_TOO_MANY_PIECES));
    }
    let mut cuts = self_intersections(&all, real, tolerance, limit)?;
    cuts.truncate(real);
    let source = Source::new(elements, d, tolerance)?;
    let mut kept: Vec<(Curve2, Option<Point2>)> = Vec::new();
    for ((piece, mut parameters), corner) in raw.iter().zip(cuts).zip(corners) {
        let (lo, hi) = piece.parameter_range();
        parameters.push(lo);
        parameters.push(hi);
        parameters.sort_by(f64::total_cmp);
        parameters.dedup();
        for pair in parameters.windows(2) {
            if let [a, b] = pair {
                classify(piece, *a, *b, &source, corner, tolerance, &mut kept)?;
            }
        }
        if kept.len() > limit {
            return Err(Error::new(codes::OFFSET_TOO_MANY_PIECES));
        }
    }
    let mut pieces = Vec::with_capacity(kept.len());
    for (piece, corner) in kept {
        match (corner, &piece, join) {
            (
                Some(v),
                Curve2::Arc(arc),
                JoinStyle::Miter { .. } | JoinStyle::Bevel | JoinStyle::Square,
            ) => {
                let (p, q) = (arc.point_at(0.0), arc.point_at(arc.sweep().abs()));
                let tangents = (piece.tangent(0.0), piece.tangent(arc.sweep().abs()));
                pieces.extend(join_pieces(p, q, v, Some(tangents), d, join)?);
            }
            _ => pieces.push(piece),
        }
    }
    chain(pieces, tolerance)
}

/// The curve to which distances are measured, with an index of its elements.
struct Source<'a> {
    elements: &'a [Curve2],
    index: SpatialIndex2<usize>,
    reach: f64,
    threshold: f64,
}

impl<'a> Source<'a> {
    fn new(elements: &'a [Curve2], d: f64, tolerance: &Tolerance) -> Result<Self> {
        let index = SpatialIndex2::from_entries(
            elements
                .iter()
                .enumerate()
                .filter_map(|(i, e)| e.bounding_box().map(|b| (i, b))),
        )?;
        Ok(Self {
            elements,
            index,
            reach: d.abs(),
            threshold: d.abs() - tolerance.linear,
        })
    }

    /// Whether `q` is at least the offset distance (less the tolerance) from the curve.
    fn valid(&self, q: Point2, tolerance: &Tolerance) -> Result<bool> {
        let probe = Aabb2::from_corners(q, q).expanded(self.reach);
        for i in self.index.crossing(&probe)? {
            if let Some(element) = self.elements.get(i)
                && element.closest_point(q, tolerance)?.distance < self.threshold
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Keeps the parts of `piece` on `[a, b]` whose samples are valid.
fn classify(
    piece: &Curve2,
    a: f64,
    b: f64,
    source: &Source<'_>,
    corner: Option<Point2>,
    tolerance: &Tolerance,
    kept: &mut Vec<(Curve2, Option<Point2>)>,
) -> Result<()> {
    if b <= a
        || b.is_nan()
        || a.is_nan()
        || piece.length_between(a, b, tolerance)? <= SLIVER_TOLERANCES * tolerance.linear
    {
        return Ok(());
    }
    // The ends too: the end of an open curve's offset can lie too close to another part.
    let fractions: Vec<f64> = (0..=CLASSIFY_SAMPLES + 1)
        .map(|i| i as f64 / (CLASSIFY_SAMPLES + 1) as f64)
        .collect();
    let at = |f: f64| a + (b - a) * f;
    let flags = fractions
        .iter()
        .map(|f| source.valid(piece.point_at(at(*f)), tolerance))
        .collect::<Result<Vec<bool>>>()?;
    if flags.iter().all(|f| *f) {
        kept.push((subcurve(piece, a, b)?, corner));
        return Ok(());
    }
    if flags.iter().all(|f| !*f) {
        return Ok(());
    }
    // Split where the validity changes between neighboring samples.
    let mut cuts = vec![a];
    for (pair, flag) in fractions.windows(2).zip(flags.windows(2)) {
        if let ([f0, f1], [v0, v1]) = (pair, flag)
            && v0 != v1
        {
            let (mut lo, mut hi) = (at(*f0), at(*f1));
            for _ in 0..BISECTION_STEPS {
                let mid = 0.5 * (lo + hi);
                if mid <= lo || mid >= hi {
                    break;
                }
                if source.valid(piece.point_at(mid), tolerance)? == *v0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            cuts.push(0.5 * (lo + hi));
        }
    }
    cuts.push(b);
    for pair in cuts.windows(2) {
        if let [x, y] = pair
            && *y > *x
            && piece.length_between(*x, *y, tolerance)? > SLIVER_TOLERANCES * tolerance.linear
            && source.valid(piece.point_at(0.5 * (x + y)), tolerance)?
        {
            kept.push((subcurve(piece, *x, *y)?, corner));
        }
    }
    Ok(())
}

/// The parameters at which each raw piece meets another one or itself; pieces from index
/// `cutters` on only cut the others.
fn self_intersections(
    raw: &[Curve2],
    cutters: usize,
    tolerance: &Tolerance,
    limit: usize,
) -> Result<Vec<Vec<f64>>> {
    let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); raw.len()];
    let boxes: Vec<(usize, Aabb2)> = raw
        .iter()
        .enumerate()
        .filter_map(|(i, c)| c.bounding_box().map(|b| (i, b.expanded(tolerance.linear))))
        .collect();
    let index = SpatialIndex2::from_entries(boxes.iter().copied())?;
    let mut found = 0usize;
    for (i, boxed) in &boxes {
        let Some(a) = raw.get(*i) else { continue };
        if *i < cutters && matches!(a, Curve2::Nurbs(_)) {
            for (s, t) in approximate_crossings(a, None, tolerance)? {
                push(&mut cuts, *i, s);
                push(&mut cuts, *i, t);
            }
        }
        for j in index.crossing(boxed)? {
            if j <= *i || (*i >= cutters && j >= cutters) {
                continue;
            }
            let Some(b) = raw.get(j) else { continue };
            let pairs: Vec<(f64, f64)> =
                if matches!(a, Curve2::Nurbs(_)) || matches!(b, Curve2::Nurbs(_)) {
                    approximate_crossings(a, Some(b), tolerance)?
                } else {
                    intersect(a, b, tolerance)?
                        .into_iter()
                        .flat_map(|hit| match hit {
                            Intersection::Point { a, b, .. } => vec![(a, b)],
                            Intersection::Overlap { a, b, .. } => vec![(a[0], b[0]), (a[1], b[1])],
                        })
                        .collect()
                };
            for (s, t) in pairs {
                push(&mut cuts, *i, s);
                push(&mut cuts, j, t);
                found += 1;
            }
            if found > limit {
                return Err(Error::new(codes::OFFSET_TOO_MANY_PIECES));
            }
        }
    }
    Ok(cuts)
}

fn push(cuts: &mut [Vec<f64>], i: usize, t: f64) {
    if let Some(list) = cuts.get_mut(i)
        && t.is_finite()
    {
        list.push(t);
    }
}

/// Crossings of two curves, or of a curve with itself when `b` is `None`, from their flattened
/// polylines, refined by Newton iteration on the curves.
fn approximate_crossings(
    a: &Curve2,
    b: Option<&Curve2>,
    tolerance: &Tolerance,
) -> Result<Vec<(f64, f64)>> {
    let flat = |c: &Curve2| -> Result<Vec<(f64, Point2)>> {
        let size = c
            .bounding_box()
            .map_or(1.0, |bb| (bb.max() - bb.min()).abs().max());
        let chord =
            (CROSSING_CHORD_TOLERANCES * tolerance.linear).max(CROSSING_RELATIVE_CHORD * size);
        crate::flatten::flatten_shape(&c.shape(), chord, CROSSING_ANGLE, MAX_CROSSING_VERTICES)
            .map_err(|e| {
                if e.code() == codes::FLATTEN_TOO_MANY_VERTICES.code {
                    Error::new(codes::OFFSET_TOO_MANY_PIECES).with_detail("operation", "crossings")
                } else {
                    e
                }
            })
    };
    let pa = flat(a)?;
    let pb = match b {
        Some(b) => flat(b)?,
        None => pa.clone(),
    };
    let other = b.unwrap_or(a);
    let segments_b =
        SpatialIndex2::from_entries(pb.windows(2).enumerate().filter_map(|(j, w)| match w {
            [(_, p0), (_, p1)] => Some((j, Aabb2::from_points(&[*p0, *p1]))),
            _ => None,
        }))?;
    let mut out = Vec::new();
    for (i, sa) in pa.windows(2).enumerate() {
        let [(ta0, a0), (ta1, a1)] = sa else { continue };
        for j in segments_b.crossing(&Aabb2::from_points(&[*a0, *a1]))? {
            if b.is_none() && j <= i + 1 {
                continue;
            }
            let Some([(tb0, b0), (tb1, b1)]) =
                pb.get(j..j + 2).and_then(|w| <&[_; 2]>::try_from(w).ok())
            else {
                continue;
            };
            let (da, db) = (a1 - a0, b1 - b0);
            let denominator = cross(&da, &db);
            if denominator == 0.0 {
                continue;
            }
            let w = b0 - a0;
            let (fa, fb) = (cross(&w, &db) / denominator, cross(&w, &da) / denominator);
            if !((0.0..=1.0).contains(&fa) && (0.0..=1.0).contains(&fb)) {
                continue;
            }
            let (s, t) = (ta0 + (ta1 - ta0) * fa, tb0 + (tb1 - tb0) * fb);
            let (s, t) = refine(a, other, s, t, tolerance);
            if b.is_none() && crate::length(&(a.point_at(s) - a.point_at(t))) > tolerance.linear {
                continue;
            }
            out.push((s, t));
        }
    }
    Ok(out)
}

/// Newton iteration on `a(s) = b(t)` from `(s, t)`, kept within the ranges; the start is kept
/// when the iteration does not reach the tolerance.
fn refine(a: &Curve2, b: &Curve2, s0: f64, t0: f64, tolerance: &Tolerance) -> (f64, f64) {
    let (ra, rb) = (a.parameter_range(), b.parameter_range());
    let (mut s, mut t) = (s0, t0);
    for _ in 0..NEWTON_STEPS {
        let f = a.point_at(s) - b.point_at(t);
        if crate::length(&f) <= 0.0625 * tolerance.linear {
            break;
        }
        let (da, db) = (a.derivative(s), b.derivative(t));
        let det = -cross(&da, &db);
        if det == 0.0 || !det.is_finite() {
            break;
        }
        let r = -f;
        let ds = (r.x * -db.y + db.x * r.y) / det;
        let dt = (da.x * r.y - da.y * r.x) / det;
        s = (s + ds).clamp(ra.0, ra.1);
        t = (t + dt).clamp(rb.0, rb.1);
    }
    if crate::length(&(a.point_at(s) - b.point_at(t))) <= tolerance.linear {
        (s, t)
    } else {
        (s0, t0)
    }
}

/// Chains kept pieces end to start; see the [module documentation](self).
fn chain(kept: Vec<Curve2>, tolerance: &Tolerance) -> Result<Vec<Composite2>> {
    if kept.is_empty() {
        return Err(Error::new(codes::OFFSET_COLLAPSED));
    }
    let gap = CHAIN_TOLERANCES * tolerance.linear;
    let loose = Tolerance {
        linear: gap,
        ..*tolerance
    };
    let near = |a: Option<Point2>, b: Option<Point2>| match (a, b) {
        (Some(a), Some(b)) => crate::length(&(b - a)) <= gap,
        _ => false,
    };
    let mut used = vec![false; kept.len()];
    let mut out = Vec::new();
    for seed in 0..kept.len() {
        if used.get(seed).copied().unwrap_or(true) {
            continue;
        }
        let mut members = vec![seed];
        if let Some(u) = used.get_mut(seed) {
            *u = true;
        }
        let start = kept.get(seed).and_then(Curve2::start_point);
        let mut current = seed;
        loop {
            let tip = kept.get(current).and_then(Curve2::end_point);
            if members.len() > 1 && near(tip, start) {
                break;
            }
            // The next piece in order first, then any.
            let next = (current + 1..kept.len()).chain(0..current).find(|j| {
                !used.get(*j).copied().unwrap_or(true)
                    && near(tip, kept.get(*j).and_then(Curve2::start_point))
            });
            let Some(next) = next else { break };
            if let Some(u) = used.get_mut(next) {
                *u = true;
            }
            members.push(next);
            current = next;
        }
        let curves = members
            .iter()
            .filter_map(|i| kept.get(*i).cloned())
            .collect();
        out.push(Composite2::new(curves, &loose)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::FRAC_PI_2;

    use proptest::prelude::*;

    use super::*;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::{Ellipse2, EllipticalArc2};
    use crate::testing::point;

    const TOL: Tolerance = Tolerance::DEFAULT;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn polyline(vertices: &[(f64, f64, f64)], closed: bool) -> Curve2 {
        Curve2::Polyline(
            Polyline2::new(
                vertices
                    .iter()
                    .map(|(x, y, b)| PolylineVertex::new(p(*x, *y), *b))
                    .collect(),
                closed,
            )
            .unwrap(),
        )
    }

    /// The distance from `q` to the source curve.
    fn distance_to(source: &Curve2, q: Point2) -> f64 {
        source.closest_point(q, &TOL).unwrap().distance
    }

    /// Checks every result point at `samples` points per curve: at distance |d| within `bound`.
    fn check_distances(
        source: &Curve2,
        result: &[Composite2],
        d: f64,
        samples: usize,
        bound: f64,
    ) -> std::result::Result<(), TestCaseError> {
        for composite in result {
            for c in composite.curves() {
                let (lo, hi) = c.parameter_range();
                for i in 0..=samples {
                    let t = lo + (hi - lo) * i as f64 / samples as f64;
                    let q = c.point_at(t);
                    let actual = distance_to(source, q);
                    prop_assert!(
                        (actual - d.abs()).abs() <= bound,
                        "{actual} vs {d} at {q:?} on {c:?}"
                    );
                }
            }
        }
        Ok(())
    }

    fn code<T: std::fmt::Debug>(r: Result<T>) -> &'static str {
        r.unwrap_err().code()
    }

    #[test]
    fn simple_kinds_and_degenerate_cases() {
        let lim = Limits::DEFAULT;
        let s = Curve2::Segment(Segment2::new(p(0.0, 0.0), p(2.0, 0.0)).unwrap());
        let out = offset(&s, 1.0, JoinStyle::Round, &TOL, &lim).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].curves()[0].start_point(), Some(p(0.0, 1.0)));
        let circle = Curve2::Circle(Circle2::new(p(0.0, 0.0), 2.0).unwrap());
        let out = offset(&circle, -1.0, JoinStyle::Round, &TOL, &lim).unwrap();
        assert!(matches!(out[0].curves()[0], Curve2::Circle(c) if c.radius() == 3.0));
        assert_eq!(
            code(offset(&circle, 2.0, JoinStyle::Round, &TOL, &lim)),
            "geom.offset.collapsed"
        );
        let arc = Curve2::Arc(Arc2::new(p(0.0, 0.0), 1.0, 0.0, FRAC_PI_2).unwrap());
        assert_eq!(
            code(offset(&arc, 1.5, JoinStyle::Round, &TOL, &lim)),
            "geom.offset.collapsed"
        );
        let out = offset(&arc, 0.5, JoinStyle::Round, &TOL, &lim).unwrap();
        assert!(matches!(out[0].curves()[0], Curve2::Arc(a) if (a.radius() - 0.5).abs() < 1e-15));
        assert_eq!(
            offset(&arc, 0.0, JoinStyle::Round, &TOL, &lim).unwrap()[0].curves(),
            std::slice::from_ref(&arc)
        );
        assert_eq!(
            code(offset(&arc, f64::NAN, JoinStyle::Round, &TOL, &lim)),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(offset(
                &arc,
                1.0,
                JoinStyle::Miter { limit: 0.5 },
                &TOL,
                &lim
            )),
            "geom.offset.join_invalid"
        );
        assert_eq!(
            code(offset(
                &arc,
                1.0,
                JoinStyle::Miter { limit: f64::NAN },
                &TOL,
                &lim
            )),
            "geom.offset.join_invalid"
        );
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::x()).unwrap());
        assert_eq!(
            code(offset(&line, 1.0, JoinStyle::Round, &TOL, &lim)),
            "geom.curve.unbounded"
        );
        let Curve2::Line(l) = line else {
            panic!("not a line")
        };
        assert_eq!(l.offset(2.0).unwrap().point(), p(0.0, 2.0));
        assert_eq!(
            Ray2::new(p(0.0, 0.0), Vector2::y())
                .unwrap()
                .offset(1.0)
                .unwrap()
                .origin(),
            p(-1.0, 0.0)
        );
        let bad = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(
            code(offset(&arc, 1.0, JoinStyle::Round, &bad, &lim)),
            "geom.tolerance.invalid"
        );
        // A degenerate NURBS piece: repeated control points give a zero derivative.
        let cusp = Curve2::Nurbs(
            Nurbs2::new(
                2,
                vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                vec![p(0.0, 0.0), p(0.0, 0.0), p(1.0, 1.0)],
                vec![1.0; 3],
            )
            .unwrap(),
        );
        assert_eq!(
            code(offset(&cusp, 0.1, JoinStyle::Round, &TOL, &lim)),
            "geom.offset.degenerate"
        );
    }

    #[test]
    fn square_joins_and_inner_collapse() {
        let lim = Limits::DEFAULT;
        // Counterclockwise unit square: left offsets go inside.
        let square = polyline(
            &[
                (0.0, 0.0, 0.0),
                (4.0, 0.0, 0.0),
                (4.0, 4.0, 0.0),
                (0.0, 4.0, 0.0),
            ],
            true,
        );
        let inner = offset(&square, 1.0, JoinStyle::Round, &TOL, &lim).unwrap();
        assert_eq!(inner.len(), 1);
        assert!(inner[0].is_closed());
        let poly = inner[0].to_polyline().unwrap();
        assert_eq!(poly.vertices().len(), 4);
        let outer = offset(&square, -1.0, JoinStyle::Round, &TOL, &lim).unwrap();
        assert_eq!(outer.len(), 1);
        assert_eq!(outer[0].curves().len(), 8);
        let mitered = offset(&square, -1.0, JoinStyle::Miter { limit: 2.0 }, &TOL, &lim).unwrap();
        let bb = mitered[0].bounding_box();
        assert!(
            (bb.min() - p(-1.0, -1.0)).norm() < 1e-12 && (bb.max() - p(5.0, 5.0)).norm() < 1e-12
        );
        let clipped = offset(&square, -1.0, JoinStyle::Miter { limit: 1.2 }, &TOL, &lim).unwrap();
        assert_eq!(clipped[0].curves().len(), 8);
        let beveled = offset(&square, -1.0, JoinStyle::Bevel, &TOL, &lim).unwrap();
        assert_eq!(beveled[0].curves().len(), 8);
        assert_eq!(
            code(offset(&square, 2.5, JoinStyle::Round, &TOL, &lim)),
            "geom.offset.collapsed"
        );
        // An open polyline with a hairpin: the inner side loses its loop.
        let hairpin = polyline(
            &[
                (0.0, 0.0, 0.0),
                (10.0, 0.0, 0.0),
                (10.0, 1.0, 0.0),
                (0.0, 1.0, 0.0),
            ],
            false,
        );
        let inside = offset(&hairpin, 0.3, JoinStyle::Round, &TOL, &lim).unwrap();
        check_distances(&hairpin, &inside, 0.3, 16, 1e-6).unwrap();
        // Wider than the hairpin on the inside: everything is closer than the distance.
        assert_eq!(
            code(offset(&hairpin, 2.0, JoinStyle::Round, &TOL, &lim)),
            "geom.offset.collapsed"
        );
        let outside = offset(&hairpin, -2.0, JoinStyle::Round, &TOL, &lim).unwrap();
        assert_eq!(outside.len(), 1);
        check_distances(&hairpin, &outside, -2.0, 16, 1e-6).unwrap();
    }

    #[test]
    fn ellipse_offsets_are_at_the_distance() {
        let lim = Limits::DEFAULT;
        let e = Ellipse2::new(p(1.0, 2.0), Vector2::new(3.0, 1.0), 0.4).unwrap();
        for d in [-0.7, 0.2, 0.9] {
            let c = Curve2::Ellipse(e);
            let out = offset(&c, d, JoinStyle::Round, &TOL, &lim).unwrap();
            check_distances(&c, &out, d, 64, 2e-6).unwrap();
            assert!(out.iter().all(Composite2::is_closed), "{d}");
        }
        let arc = Curve2::EllipticalArc(EllipticalArc2::new(e, 0.5, -2.0).unwrap());
        let out = offset(&arc, 0.3, JoinStyle::Round, &TOL, &lim).unwrap();
        check_distances(&arc, &out, 0.3, 64, 2e-6).unwrap();
    }

    fn open_polyline() -> impl Strategy<Value = Curve2> {
        (
            prop::collection::vec((point(), -1.0..1.0_f64), 2..6),
            any::<bool>(),
        )
            .prop_filter_map("valid polyline", |(v, closed)| {
                Polyline2::new(
                    v.into_iter()
                        .map(|(q, b)| PolylineVertex::new(q, b))
                        .collect(),
                    closed,
                )
                .ok()
                .map(Curve2::Polyline)
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        #[test]
        fn polyline_offsets_lie_at_the_distance(c in open_polyline(), d in prop_oneof![-20.0..20.0_f64, -1e-3..1e-3_f64]) {
            prop_assume!(d != 0.0);
            match offset(&c, d, JoinStyle::Round, &TOL, &Limits::DEFAULT) {
                Ok(out) => check_distances(&c, &out, d, 8, 1e-5)?,
                Err(e) => prop_assert!(
                    ["geom.offset.collapsed", "geom.precision.insufficient"].contains(&e.code()),
                    "{e:?}"
                ),
            }
        }

        #[test]
        fn miter_and_bevel_offsets_keep_their_distance_bounds(c in open_polyline(), d in -10.0..10.0_f64, limit in 1.0..4.0_f64) {
            prop_assume!(d.abs() > 1e-3);
            for join in [JoinStyle::Miter { limit }, JoinStyle::Bevel] {
                if let Ok(out) = offset(&c, d, join, &TOL, &Limits::DEFAULT) {
                    for composite in &out {
                        for piece in composite.curves() {
                            // Only a straight join (bevel or clipped miter) comes closer; the
                            // offsets of segments and arcs and round joins never do.
                            let join_segment = matches!(piece, Curve2::Segment(_));
                            for i in 0..=8 {
                                let (lo, hi) = piece.parameter_range();
                                let q = piece.point_at(lo + (hi - lo) * f64::from(i) / 8.0);
                                let actual = distance_to(&c, q);
                                prop_assert!(actual <= limit * d.abs() + 1e-5);
                                prop_assert!(actual >= d.abs() - 1e-5 || join_segment, "{actual} {d} {piece:?}");
                            }
                        }
                    }
                }
            }
        }

    }

    proptest! {
        // Each case fits and intersects several NURBS curves, slowly in unoptimized builds.
        #![proptest_config(ProptestConfig { cases: 6, ..ProptestConfig::default() })]

        #[test]
        fn nurbs_offsets_lie_at_the_distance(n in crate::nurbs::tests::nurbs(), d in -2.0..2.0_f64) {
            prop_assume!(d.abs() > 1e-2);
            let c = Curve2::Nurbs(n);
            match offset(&c, d, JoinStyle::Round, &TOL, &Limits::DEFAULT) {
                Ok(out) => check_distances(&c, &out, d, 16, 1e-5)?,
                Err(e) => prop_assert!(
                    ["geom.offset.collapsed", "geom.offset.degenerate", "geom.offset.too_many_pieces", "geom.precision.insufficient"].contains(&e.code()),
                    "{e:?}"
                ),
            }
        }
    }
}
