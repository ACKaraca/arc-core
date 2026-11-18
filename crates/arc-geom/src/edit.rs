//! Splitting, trimming, extending and joining curves (CORE-GEOM-006), and composite curves.
//!
//! - [`split_at_parameters`] cuts a curve into the pieces between the given parameters; each
//!   piece keeps the kind of its curve where the kind allows (a piece of a circle is an arc, of
//!   a ray or line a segment or ray, of a polyline a polyline, of a NURBS curve a NURBS curve)
//!   and runs in the curve's direction, except that the first piece of a split line is the ray
//!   from the first cut backward. Cuts closer than `tolerance.linear` of curve length to each
//!   other or to an end are merged into one or dropped, so no piece is shorter than that. On a
//!   closed curve the piece across the seam is one curve, except for a NURBS curve, where it is
//!   the two pieces on either side of the seam.
//! - [`split_at_points`] projects each point onto the curve and splits at the parameters.
//! - [`trim`] removes the stretch of a curve around a picked parameter that the nearest
//!   intersections with boundary curves on either side delimit (or the curve's end where there
//!   is none on one side of an open curve), using [`crate::intersect`].
//! - [`extend`] lengthens the start or end of an open curve to the nearest intersection beyond
//!   it with a boundary curve: a segment along its line, an arc along its circle, an elliptical
//!   arc along its ellipse and a polyline by its end segment, at most to a full turn.
//! - [`join`] chains curves whose ends meet within `tolerance.linear` into composites, reversing
//!   curves as needed, and reports for every end left free the nearest other free end.

use std::f64::consts::TAU;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::bbox::Aabb2;
use crate::closest::CurveProject;
use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::CurveEval;
use crate::intersect::{Intersection, intersect};
use crate::polyline::{Polyline2, PolylineSegment, PolylineVertex};
use crate::primitives::{Arc2, EllipticalArc2, Ray2, Segment2};

/// A chain of bounded curves, each starting within the linear tolerance of where the previous
/// one ends; closed when the last ends within that tolerance of where the first starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Composite2 {
    curves: Vec<Curve2>,
    closed: bool,
}

impl Composite2 {
    /// Creates a composite from curves joined end to start in order.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for an invalid tolerance; `geom.composite.empty` for no curves;
    /// `geom.curve.unbounded` for a ray or line; `geom.composite.not_connected` when the end of
    /// a curve is farther than `tolerance.linear` from the start of the next, with the details
    /// `curve` (the index of the first of the two) and `gap`.
    pub fn new(curves: Vec<Curve2>, tolerance: &Tolerance) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        if curves.is_empty() {
            return Err(Error::new(codes::COMPOSITE_EMPTY));
        }
        if let Some(index) = curves.iter().position(|c| c.bounding_box().is_none()) {
            return Err(Error::new(codes::CURVE_UNBOUNDED).with_detail("curve", detail(index)));
        }
        for (index, pair) in curves.windows(2).enumerate() {
            if let [a, b] = pair {
                let gap = distance(a.end_point(), b.start_point());
                if gap > tolerance.linear || gap.is_nan() {
                    return Err(Error::new(codes::COMPOSITE_NOT_CONNECTED)
                        .with_detail("curve", detail(index))
                        .with_detail("gap", gap));
                }
            }
        }
        let closed = distance(
            curves.last().and_then(Curve2::end_point),
            curves.first().and_then(Curve2::start_point),
        ) <= tolerance.linear;
        Ok(Self { curves, closed })
    }

    /// The curves in order.
    #[must_use]
    pub fn curves(&self) -> &[Curve2] {
        &self.curves
    }

    /// Whether the chain ends where it starts, within the tolerance it was built with.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The start of the first curve.
    #[must_use]
    pub fn start_point(&self) -> Point2 {
        self.curves
            .first()
            .and_then(Curve2::start_point)
            .unwrap_or_else(Point2::origin)
    }

    /// The end of the last curve.
    #[must_use]
    pub fn end_point(&self) -> Point2 {
        self.curves
            .last()
            .and_then(Curve2::end_point)
            .unwrap_or_else(Point2::origin)
    }

    /// The union of the curves' bounding boxes.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        self.curves
            .iter()
            .filter_map(Curve2::bounding_box)
            .fold(Aabb2::EMPTY, |a, b| a.union(&b))
    }

    /// The same chain traversed backward.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            curves: self
                .curves
                .iter()
                .rev()
                .filter_map(Curve2::reversed)
                .collect(),
            closed: self.closed,
        }
    }

    /// The total length.
    ///
    /// # Errors
    ///
    /// As [`CurveEval::length`].
    pub fn length(&self, tolerance: &Tolerance) -> Result<f64> {
        self.curves.iter().map(|c| c.length(tolerance)).sum()
    }

    /// The chain as one polyline when every curve is a segment, an arc or an open polyline;
    /// `None` otherwise or when the vertices do not make a valid polyline. A closed chain gives
    /// a closed polyline. Vertices are the start points of the curves; arcs become bulges.
    #[must_use]
    pub fn to_polyline(&self) -> Option<Polyline2> {
        let mut vertices: Vec<PolylineVertex> = Vec::new();
        for curve in &self.curves {
            match curve {
                Curve2::Segment(s) => vertices.push(PolylineVertex::new(s.start(), 0.0)),
                Curve2::Arc(a) if a.sweep().abs() < TAU => {
                    let bulge = a
                        .chord()
                        .map_or_else(|| (a.sweep() / 4.0).tan(), |c| c.bulge());
                    vertices.push(PolylineVertex::new(a.point_at(0.0), bulge));
                }
                Curve2::Polyline(p) if !p.is_closed() => {
                    let count = p.vertices().len();
                    vertices.extend(p.vertices().iter().take(count - 1).copied());
                }
                _ => return None,
            }
        }
        if !self.closed {
            vertices.push(PolylineVertex::new(self.end_point(), 0.0));
        }
        Polyline2::new(vertices, self.closed).ok()
    }
}

/// Which end of a curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CurveEnd {
    /// The start of the parameter range.
    Start,
    /// The end of the parameter range.
    End,
}

/// A free end of a joined chain and the nearest free end of another chain or of the same one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinGap {
    /// The index of the input curve at the first free end.
    pub curve: usize,
    /// Which end of that curve.
    pub end: CurveEnd,
    /// The index of the input curve at the nearest other free end.
    pub other: usize,
    /// Which end of that curve.
    pub other_end: CurveEnd,
    /// The distance between the two ends.
    pub distance: f64,
}

/// The result of [`join`].
#[derive(Debug, Clone, PartialEq)]
pub struct Joined {
    /// The chains, each with the indices of its input curves in chain order.
    pub chains: Vec<(Composite2, Vec<usize>)>,
    /// For every free end, the nearest other free end, sorted by distance; each pair once.
    pub gaps: Vec<JoinGap>,
}

fn detail(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

fn distance(a: Option<Point2>, b: Option<Point2>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => crate::length(&(b - a)),
        _ => f64::INFINITY,
    }
}

fn out_of_range(t: f64) -> Error {
    Error::new(codes::PARAMETER_OUT_OF_RANGE).with_detail("parameter", t)
}

/// The bulge of the part of a bulged segment spanning `fraction` of its included angle:
/// `tan(fraction · atan(bulge))`, computed near a half turn through the complementary angle,
/// where the tangent is ill-conditioned; the whole segment keeps its bulge exactly.
fn partial_bulge(bulge: f64, fraction: f64) -> f64 {
    if bulge == 0.0 || fraction >= 1.0 {
        return bulge;
    }
    let quarter = std::f64::consts::FRAC_PI_4;
    let magnitude = bulge.abs();
    let angle = fraction * magnitude.atan();
    let part = if angle > quarter {
        // fraction·atan(b) = π/2 − ((1 − fraction)·π/2 + fraction·atan(1/b)).
        let complement = (1.0 - fraction) * 2.0 * quarter + fraction * magnitude.recip().atan();
        complement.tan().recip()
    } else {
        angle.tan()
    };
    part.copysign(bulge)
}

/// The vertices of the part of a polyline between parameters `t0 < t1` in its range; the last
/// vertex has bulge and widths 0.
fn polyline_part(p: &Polyline2, t0: f64, t1: f64) -> Vec<PolylineVertex> {
    let count = p.segment_count();
    let vertices = p.vertices();
    let mut out = Vec::new();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "t0 is clamped to [0, n] by the caller and floored"
    )]
    let first = (t0.floor().max(0.0) as usize).min(count.saturating_sub(1));
    for k in first..count {
        let (lo, hi) = (k as f64, (k + 1) as f64);
        if hi <= t0 || lo >= t1 {
            continue;
        }
        let (Some(segment), Some(start)) = (p.segment(k), vertices.get(k)) else {
            continue;
        };
        let (f0, f1) = ((t0 - lo).max(0.0), (t1 - lo).min(1.0));
        let point = if f0 == 0.0 {
            start.point
        } else {
            segment.point_at_fraction(f0)
        };
        let bulge = partial_bulge(start.bulge, f1 - f0);
        let width = |f: f64| start.start_width + (start.end_width - start.start_width) * f;
        out.push(PolylineVertex {
            point,
            bulge,
            start_width: width(f0),
            end_width: width(f1),
        });
    }
    let end = if t1 >= count as f64 {
        vertices
            .get(count % vertices.len().max(1))
            .map_or_else(|| p.point_at(t1), |v| v.point)
    } else {
        p.point_at(t1)
    };
    out.push(PolylineVertex::new(end, 0.0));
    out
}

/// The part of a bounded curve, ray or line between parameters `t0 < t1` (clamped), as a
/// single curve. Not for the wrap piece of a closed curve.
pub(crate) fn subcurve(curve: &Curve2, t0: f64, t1: f64) -> Result<Curve2> {
    let (lo, hi) = curve.parameter_range();
    let (t0, t1) = (t0.max(lo), t1.min(hi));
    if t0 >= t1 || t0.is_nan() || t1.is_nan() {
        return Err(out_of_range(t0));
    }
    if t0 == lo && t1 == hi {
        return Ok(curve.clone());
    }
    let point = |t: f64| curve.point_at(t);
    Ok(match curve {
        Curve2::Segment(_) | Curve2::Ray(_) | Curve2::Line(_) => {
            if t1.is_finite() {
                Curve2::Segment(Segment2::new(point(t0), point(t1))?)
            } else if let Curve2::Ray(r) = curve {
                Curve2::Ray(Ray2::new(point(t0), r.direction())?)
            } else if let Curve2::Line(l) = curve {
                Curve2::Ray(Ray2::new(point(t0), l.direction())?)
            } else {
                return Err(out_of_range(t1));
            }
        }
        Curve2::Circle(c) => Curve2::Arc(Arc2::new(c.center(), c.radius(), t0, t1 - t0)?),
        Curve2::Arc(a) => Curve2::Arc(arc_part(a, t0, t1)?),
        Curve2::Ellipse(e) => Curve2::EllipticalArc(EllipticalArc2::new(*e, t0, t1 - t0)?),
        Curve2::EllipticalArc(e) => Curve2::EllipticalArc(EllipticalArc2::new(
            *e.ellipse(),
            e.angle_at(t0),
            (t1 - t0) * e.sweep().signum(),
        )?),
        Curve2::Polyline(p) => Curve2::Polyline(Polyline2::new(polyline_part(p, t0, t1), false)?),
        Curve2::Nurbs(n) => {
            let right = if t0 > lo {
                n.split_at(t0)?.1
            } else {
                n.clone()
            };
            Curve2::Nurbs(if t1 < hi {
                right.split_at(t1)?.0
            } else {
                right
            })
        }
    })
}

fn arc_part(a: &Arc2, t0: f64, t1: f64) -> Result<Arc2> {
    let sign = a.sweep().signum();
    if a.chord().is_some() {
        let bulge = (sign * (t1 - t0) / 4.0).tan();
        if let Some(arc) = crate::encoding::chord_arc(a.point_at(t0), a.point_at(t1), bulge)? {
            return Ok(arc);
        }
    }
    Arc2::new(a.center(), a.radius(), a.angle_at(t0), sign * (t1 - t0))
}

/// The pieces of a closed curve from `t0` to the end of the range and on from the start to
/// `t1`, as one curve where the kind allows.
fn wrap_piece(curve: &Curve2, t0: f64, t1: f64) -> Result<Vec<Curve2>> {
    let (lo, hi) = curve.parameter_range();
    let span = (hi - t0) + (t1 - lo);
    Ok(match curve {
        Curve2::Circle(c) => vec![Curve2::Arc(Arc2::new(c.center(), c.radius(), t0, span)?)],
        Curve2::Ellipse(e) => vec![Curve2::EllipticalArc(EllipticalArc2::new(*e, t0, span)?)],
        Curve2::Arc(a) => vec![Curve2::Arc(Arc2::new(
            a.center(),
            a.radius(),
            a.angle_at(t0),
            a.sweep().signum() * span,
        )?)],
        Curve2::EllipticalArc(e) => vec![Curve2::EllipticalArc(EllipticalArc2::new(
            *e.ellipse(),
            e.angle_at(t0),
            e.sweep().signum() * span,
        )?)],
        Curve2::Polyline(p) => {
            let mut vertices = if t0 < hi {
                polyline_part(p, t0, hi)
            } else {
                Vec::new()
            };
            vertices.pop();
            if t1 > lo {
                vertices.extend(polyline_part(p, lo, t1));
            } else {
                vertices.push(PolylineVertex::new(p.point_at(lo), 0.0));
            }
            vec![Curve2::Polyline(Polyline2::new(vertices, false)?)]
        }
        _ => {
            let mut out = Vec::new();
            if t0 < hi {
                out.push(subcurve(curve, t0, hi)?);
            }
            if t1 > lo {
                out.push(subcurve(curve, lo, t1)?);
            }
            out
        }
    })
}

/// The cut parameters sorted, inside the range, with cuts closer than `tolerance.linear` of
/// length to the previous kept cut or to an end of an open curve dropped.
fn clean_cuts(curve: &Curve2, parameters: &[f64], tolerance: &Tolerance) -> Result<Vec<f64>> {
    let (lo, hi) = curve.parameter_range();
    if let Some(bad) = parameters
        .iter()
        .find(|t| !(**t >= lo && **t <= hi && t.is_finite()))
    {
        return Err(out_of_range(*bad));
    }
    let mut sorted = parameters.to_vec();
    sorted.sort_by(f64::total_cmp);
    let closed = curve.is_closed();
    let mut kept: Vec<f64> = Vec::new();
    for t in sorted {
        let previous = kept.last().copied().unwrap_or(lo);
        let near_previous = if kept.is_empty() && (closed || !lo.is_finite()) {
            false
        } else {
            curve.length_between(previous, t, tolerance)? <= tolerance.linear
        };
        if near_previous {
            continue;
        }
        if !closed && hi.is_finite() && curve.length_between(t, hi, tolerance)? <= tolerance.linear
        {
            continue;
        }
        kept.push(t);
    }
    // On a closed curve the last cut may lie within the tolerance of the first across the seam.
    if closed
        && kept.len() > 1
        && let (Some(first), Some(last)) = (kept.first().copied(), kept.last().copied())
    {
        let across = curve.length_between(last, hi, tolerance)?
            + curve.length_between(lo, first, tolerance)?;
        if across <= tolerance.linear {
            kept.pop();
        }
    }
    Ok(kept)
}

/// The pieces of `curve` between `parameters`, in the curve's direction; see the [module
/// documentation](self).
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance; `geom.parameter.out_of_range` for a
/// parameter outside the range or NaN; `geom.precision.insufficient` or a constructor's code
/// when a piece is not representable.
pub fn split_at_parameters(
    curve: &Curve2,
    parameters: &[f64],
    tolerance: &Tolerance,
) -> Result<Vec<Curve2>> {
    crate::ensure_tolerance(tolerance)?;
    crate::eval::ensure_tolerance("linear", tolerance.linear)?;
    let cuts = clean_cuts(curve, parameters, tolerance)?;
    let (lo, hi) = curve.parameter_range();
    if cuts.is_empty() {
        return Ok(vec![curve.clone()]);
    }
    if curve.is_closed() {
        let mut out = Vec::new();
        for pair in cuts.windows(2) {
            if let [a, b] = pair {
                out.push(subcurve(curve, *a, *b)?);
            }
        }
        let (Some(first), Some(last)) = (cuts.first(), cuts.last()) else {
            return Ok(out);
        };
        out.extend(wrap_piece(curve, *last, *first)?);
        return Ok(out);
    }
    let mut out = Vec::new();
    let mut previous = lo;
    for (index, t) in cuts.iter().enumerate() {
        if index == 0 && !lo.is_finite() {
            if let Curve2::Line(line) = curve {
                out.push(Curve2::Ray(Ray2::new(
                    line.point_at(*t),
                    -line.direction(),
                )?));
            }
        } else {
            out.push(subcurve(curve, previous, *t)?);
        }
        previous = *t;
    }
    out.push(subcurve(curve, previous, hi)?);
    Ok(out)
}

/// The pieces of `curve` split at the projections of `points`.
///
/// # Errors
///
/// `geom.edit.point_off_curve` for a point farther than `tolerance.linear` from the curve,
/// with the detail `point` (its index); otherwise as [`CurveProject::closest_point`] and
/// [`split_at_parameters`].
pub fn split_at_points(
    curve: &Curve2,
    points: &[Point2],
    tolerance: &Tolerance,
) -> Result<Vec<Curve2>> {
    let mut parameters = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        let foot = curve.closest_point(*point, tolerance)?;
        if foot.distance > tolerance.linear || foot.distance.is_nan() {
            return Err(Error::new(codes::EDIT_POINT_OFF_CURVE)
                .with_detail("point", detail(index))
                .with_detail("distance", foot.distance));
        }
        parameters.push(foot.parameter);
    }
    split_at_parameters(curve, &parameters, tolerance)
}

/// The parameters on `curve` where it meets any of `boundaries` (both ends of an overlap).
fn cut_parameters(
    curve: &Curve2,
    boundaries: &[Curve2],
    tolerance: &Tolerance,
) -> Result<Vec<f64>> {
    let mut out = Vec::new();
    for boundary in boundaries {
        for hit in intersect(curve, boundary, tolerance)? {
            match hit {
                Intersection::Point { a, .. } => out.push(a),
                Intersection::Overlap { a, .. } => out.extend(a.iter().filter(|t| t.is_finite())),
            }
        }
    }
    out.sort_by(f64::total_cmp);
    Ok(out)
}

/// The curve with the stretch around parameter `pick` removed, between the nearest
/// intersections with `boundaries` on either side; the remaining parts in the curve's order.
/// An open curve loses everything from an end when no intersection lies on that side; a
/// closed curve needs two distinct intersections. A remaining part of a line before the cut is
/// the ray from the cut backward.
///
/// # Errors
///
/// `geom.parameter.out_of_range` for a pick outside the range or NaN;
/// `geom.edit.no_intersection` when no boundary meets the curve (or only once, on a closed
/// curve); otherwise as [`intersect`] and [`split_at_parameters`].
pub fn trim(
    curve: &Curve2,
    boundaries: &[Curve2],
    pick: f64,
    tolerance: &Tolerance,
) -> Result<Vec<Curve2>> {
    let (lo, hi) = curve.parameter_range();
    if !(pick >= lo && pick <= hi) {
        return Err(out_of_range(pick));
    }
    let cuts = cut_parameters(curve, boundaries, tolerance)?;
    let cuts = clean_cuts(curve, &cuts, tolerance)?;
    let no_intersection = || Error::new(codes::EDIT_NO_INTERSECTION);
    let below = cuts.iter().rev().find(|t| **t < pick).copied();
    let above = cuts.iter().find(|t| **t > pick).copied();
    if curve.is_closed() {
        if cuts.len() < 2 {
            return Err(no_intersection());
        }
        let (Some(first), Some(last)) = (cuts.first().copied(), cuts.last().copied()) else {
            return Err(no_intersection());
        };
        let (from, to) = (above.unwrap_or(first), below.unwrap_or(last));
        return if from < to {
            Ok(vec![subcurve(curve, from, to)?])
        } else {
            wrap_piece(curve, from, to)
        };
    }
    if below.is_none() && above.is_none() {
        return Err(no_intersection());
    }
    let mut out = Vec::new();
    if let Some(b) = below {
        out.extend(
            split_at_parameters(curve, &[b], tolerance)?
                .into_iter()
                .take(1),
        );
    }
    if let Some(a) = above {
        out.extend(
            split_at_parameters(curve, &[a], tolerance)?
                .into_iter()
                .last(),
        );
    }
    Ok(out)
}

/// The nearest parameter on `probe`, beyond the linear tolerance of its start, where it meets
/// any boundary.
fn nearest_hit(probe: &Curve2, boundaries: &[Curve2], tolerance: &Tolerance) -> Result<f64> {
    let speed = crate::length(&probe.derivative(0.0));
    let skip = tolerance.parametric(speed);
    cut_parameters(probe, boundaries, tolerance)?
        .into_iter()
        .find(|t| *t > skip)
        .ok_or_else(|| Error::new(codes::EDIT_NO_INTERSECTION))
}

/// The probe that continues an arc of the given circle or ellipse beyond one end, through the
/// rest of the full turn, and the sign of travel.
fn conic_probe(curve: &Curve2, end: CurveEnd) -> Result<Curve2> {
    let rest = |sweep: f64| TAU - sweep.abs();
    Ok(match (curve, end) {
        (Curve2::Arc(a), CurveEnd::End) => Curve2::Arc(Arc2::new(
            a.center(),
            a.radius(),
            a.angle_at(a.sweep().abs()),
            a.sweep().signum() * rest(a.sweep()),
        )?),
        (Curve2::Arc(a), CurveEnd::Start) => Curve2::Arc(Arc2::new(
            a.center(),
            a.radius(),
            a.angle_at(0.0),
            -a.sweep().signum() * rest(a.sweep()),
        )?),
        (Curve2::EllipticalArc(e), CurveEnd::End) => Curve2::EllipticalArc(EllipticalArc2::new(
            *e.ellipse(),
            e.angle_at(e.sweep().abs()),
            e.sweep().signum() * rest(e.sweep()),
        )?),
        (Curve2::EllipticalArc(e), CurveEnd::Start) => Curve2::EllipticalArc(EllipticalArc2::new(
            *e.ellipse(),
            e.angle_at(0.0),
            -e.sweep().signum() * rest(e.sweep()),
        )?),
        _ => return Err(Error::new(codes::CURVE_KIND_UNSUPPORTED)),
    })
}

fn straight_probe(from: Point2, direction: Vector2) -> Result<Curve2> {
    Ok(Curve2::Ray(Ray2::new(from, direction)?))
}

/// The curve with its `end` lengthened to the nearest intersection with `boundaries` beyond it;
/// see the [module documentation](self).
///
/// # Errors
///
/// `geom.edit.curve_closed` for a closed curve; `geom.curve.unbounded` for a ray or line;
/// `geom.curve.kind_unsupported` for a NURBS curve, whose extension is not defined yet;
/// `geom.edit.no_intersection` when no boundary meets the extension; otherwise as
/// [`intersect`] or a constructor of the result.
pub fn extend(
    curve: &Curve2,
    end: CurveEnd,
    boundaries: &[Curve2],
    tolerance: &Tolerance,
) -> Result<Curve2> {
    if curve.bounding_box().is_none() {
        return Err(Error::new(codes::CURVE_UNBOUNDED));
    }
    if curve.is_closed() {
        return Err(Error::new(codes::EDIT_CURVE_CLOSED));
    }
    match curve {
        Curve2::Segment(s) => {
            let (from, direction) = match end {
                CurveEnd::End => (s.end(), s.end() - s.start()),
                CurveEnd::Start => (s.start(), s.start() - s.end()),
            };
            let probe = straight_probe(from, direction)?;
            let hit = probe.point_at(nearest_hit(&probe, boundaries, tolerance)?);
            Ok(Curve2::Segment(match end {
                CurveEnd::End => Segment2::new(s.start(), hit)?,
                CurveEnd::Start => Segment2::new(hit, s.end())?,
            }))
        }
        Curve2::Arc(a) => {
            let probe = conic_probe(curve, end)?;
            let s = nearest_hit(&probe, boundaries, tolerance)?;
            let sign = a.sweep().signum();
            let sweep = sign * (a.sweep().abs() + s);
            Ok(Curve2::Arc(match end {
                CurveEnd::End => Arc2::new(a.center(), a.radius(), a.start_angle(), sweep)?,
                CurveEnd::Start => {
                    Arc2::new(a.center(), a.radius(), a.start_angle() - sign * s, sweep)?
                }
            }))
        }
        Curve2::EllipticalArc(e) => {
            let probe = conic_probe(curve, end)?;
            let s = nearest_hit(&probe, boundaries, tolerance)?;
            let sign = e.sweep().signum();
            let sweep = sign * (e.sweep().abs() + s);
            let start = match end {
                CurveEnd::End => e.start_parameter(),
                CurveEnd::Start => e.start_parameter() - sign * s,
            };
            Ok(Curve2::EllipticalArc(EllipticalArc2::new(
                *e.ellipse(),
                start,
                sweep,
            )?))
        }
        Curve2::Polyline(p) => extend_polyline(p, end, boundaries, tolerance).map(Curve2::Polyline),
        Curve2::Nurbs(_) => {
            Err(Error::new(codes::CURVE_KIND_UNSUPPORTED).with_detail("operation", "extend"))
        }
        Curve2::Circle(_) | Curve2::Ellipse(_) => Err(Error::new(codes::EDIT_CURVE_CLOSED)),
        Curve2::Ray(_) | Curve2::Line(_) => Err(Error::new(codes::CURVE_UNBOUNDED)),
    }
}

fn extend_polyline(
    p: &Polyline2,
    end: CurveEnd,
    boundaries: &[Curve2],
    tolerance: &Tolerance,
) -> Result<Polyline2> {
    let count = p.segment_count();
    let index = match end {
        CurveEnd::Start => 0,
        CurveEnd::End => count.saturating_sub(1),
    };
    let segment = p.segment(index).ok_or_else(|| out_of_range(index as f64))?;
    let mut vertices = p.vertices().to_vec();
    let (point, bulge) = match segment {
        PolylineSegment::Line(s) => {
            let probe = match end {
                CurveEnd::End => straight_probe(s.end(), s.end() - s.start())?,
                CurveEnd::Start => straight_probe(s.start(), s.start() - s.end())?,
            };
            (
                probe.point_at(nearest_hit(&probe, boundaries, tolerance)?),
                0.0,
            )
        }
        PolylineSegment::Arc(a) => {
            let arc = Curve2::Arc(Arc2::new(
                a.center(),
                a.radius(),
                a.start_angle(),
                a.sweep(),
            )?);
            let probe = conic_probe(&arc, end)?;
            let s = nearest_hit(&probe, boundaries, tolerance)?;
            let sweep = a.sweep().signum() * (a.sweep().abs() + s);
            (probe.point_at(s), (sweep / 4.0).tan())
        }
    };
    match end {
        CurveEnd::End => {
            if let Some(last) = vertices.last_mut() {
                last.point = point;
            }
            if let Some(start) = vertices.get_mut(index) {
                start.bulge = bulge;
            }
        }
        CurveEnd::Start => {
            if let Some(first) = vertices.first_mut() {
                first.point = point;
                first.bulge = bulge;
            }
        }
    }
    Polyline2::new(vertices, false)
}

/// Chains the bounded curves whose ends meet within `tolerance.linear` into composites, and
/// reports the gaps between the remaining free ends; see the [module documentation](self).
///
/// Chains grow greedily from the lowest-indexed unused curve, first forward from its end, then
/// backward from its start; at each end the lowest-indexed unused curve with an end within the
/// tolerance continues the chain, reversed when needed. Where more than two ends meet, the
/// curves left over start chains of their own.
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance; `geom.curve.unbounded` for a ray or line.
pub fn join(curves: &[Curve2], tolerance: &Tolerance) -> Result<Joined> {
    crate::ensure_tolerance(tolerance)?;
    if let Some(index) = curves.iter().position(|c| c.bounding_box().is_none()) {
        return Err(Error::new(codes::CURVE_UNBOUNDED).with_detail("curve", detail(index)));
    }
    let chains = chain_indices(curves, tolerance);
    let mut result = Vec::new();
    // Free ends: (curve, end, point).
    let mut free: Vec<(usize, CurveEnd, Point2)> = Vec::new();
    for chain in &chains {
        let pieces: Vec<Curve2> = chain
            .iter()
            .filter_map(|(i, reversed)| {
                let c = curves.get(*i)?;
                if *reversed {
                    c.reversed()
                } else {
                    Some(c.clone())
                }
            })
            .collect();
        let composite = Composite2::new(pieces, tolerance)?;
        if !composite.is_closed() {
            if let Some((i, reversed)) = chain.first() {
                let end = if *reversed {
                    CurveEnd::End
                } else {
                    CurveEnd::Start
                };
                free.push((*i, end, composite.start_point()));
            }
            if let Some((i, reversed)) = chain.last() {
                let end = if *reversed {
                    CurveEnd::Start
                } else {
                    CurveEnd::End
                };
                free.push((*i, end, composite.end_point()));
            }
        }
        result.push((composite, chain.iter().map(|(i, _)| *i).collect()));
    }
    Ok(Joined {
        chains: result,
        gaps: gaps(&free),
    })
}

/// The chains of [`join`] as input indices with a reversal flag.
fn chain_indices(curves: &[Curve2], tolerance: &Tolerance) -> Vec<Vec<(usize, bool)>> {
    let ends: Vec<(Point2, Point2)> = curves
        .iter()
        .map(|c| {
            (
                c.start_point().unwrap_or_else(Point2::origin),
                c.end_point().unwrap_or_else(Point2::origin),
            )
        })
        .collect();
    let near = |a: Point2, b: Point2| crate::length(&(b - a)) <= tolerance.linear;
    let mut used = vec![false; curves.len()];
    // (curve, reversed)
    let mut chains: Vec<Vec<(usize, bool)>> = Vec::new();
    for seed in 0..curves.len() {
        if used.get(seed).copied().unwrap_or(true) {
            continue;
        }
        if let Some(u) = used.get_mut(seed) {
            *u = true;
        }
        let mut chain = std::collections::VecDeque::from([(seed, false)]);
        let oriented = |(i, reversed): (usize, bool)| {
            let (s, e) = ends.get(i).copied().unwrap_or_default();
            if reversed { (e, s) } else { (s, e) }
        };
        let closed = |chain: &std::collections::VecDeque<(usize, bool)>| {
            chain.len() > 1
                || curves.get(seed).is_some_and(Curve2::is_closed)
        } && match (chain.front(), chain.back()) {
            (Some(f), Some(b)) => near(oriented(*b).1, oriented(*f).0),
            _ => false,
        };
        // Forward, then backward.
        for forward in [true, false] {
            while !closed(&chain) {
                let tip = if forward {
                    chain.back().map(|c| oriented(*c).1)
                } else {
                    chain.front().map(|c| oriented(*c).0)
                };
                let Some(tip) = tip else { break };
                let next = (0..curves.len()).find_map(|i| {
                    if used.get(i).copied().unwrap_or(true) {
                        return None;
                    }
                    let (s, e) = ends.get(i).copied()?;
                    match (forward, near(tip, s), near(tip, e)) {
                        (true, true, _) => Some((i, false)),
                        (true, false, true) => Some((i, true)),
                        (false, _, true) => Some((i, false)),
                        (false, true, false) => Some((i, true)),
                        _ => None,
                    }
                });
                let Some(next) = next else { break };
                if let Some(u) = used.get_mut(next.0) {
                    *u = true;
                }
                if forward {
                    chain.push_back(next);
                } else {
                    chain.push_front(next);
                }
            }
        }
        chains.push(chain.into_iter().collect());
    }
    chains
}

/// For every free end the nearest other free end, each pair once, sorted by distance.
fn gaps(free: &[(usize, CurveEnd, Point2)]) -> Vec<JoinGap> {
    let mut gaps: Vec<JoinGap> = Vec::new();
    for (k, (curve, end, point)) in free.iter().enumerate() {
        let nearest = free
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != k)
            .map(|(j, (c, e, p))| (crate::length(&(p - point)), j, *c, *e))
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        if let Some((distance, _, other, other_end)) = nearest {
            let duplicate = gaps.iter().any(|g| {
                g.curve == other && g.end == other_end && g.other == *curve && g.other_end == *end
            });
            if !duplicate {
                gaps.push(JoinGap {
                    curve: *curve,
                    end: *end,
                    other,
                    other_end,
                    distance,
                });
            }
        }
    }
    gaps.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    gaps
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, PI};

    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, parameter, wide_curve};
    use crate::primitives::{Circle2, Ellipse2, Line2};

    const TOL: Tolerance = Tolerance::DEFAULT;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn seg(a: Point2, b: Point2) -> Curve2 {
        Curve2::Segment(Segment2::new(a, b).unwrap())
    }

    /// Every point of the curve at fraction `f` is within `bound` of some piece.
    fn covered(c: &Curve2, pieces: &[Curve2], f: f64, bound: f64) -> bool {
        let q = c.point_at(parameter(c, f));
        pieces.iter().any(|piece| {
            piece
                .closest_point(q, &TOL)
                .is_ok_and(|foot| foot.distance <= bound)
        })
    }

    #[test]
    fn split_degenerate_cases() {
        let s = seg(p(0.0, 0.0), p(1.0, 0.0));
        assert_eq!(split_at_parameters(&s, &[], &TOL).unwrap(), vec![s.clone()]);
        assert_eq!(
            split_at_parameters(&s, &[0.0, 1.0, 1e-9], &TOL).unwrap(),
            vec![s.clone()]
        );
        assert_eq!(
            split_at_parameters(&s, &[f64::NAN], &TOL)
                .unwrap_err()
                .code(),
            "geom.parameter.out_of_range"
        );
        assert_eq!(
            split_at_parameters(&s, &[1.5], &TOL).unwrap_err().code(),
            "geom.parameter.out_of_range"
        );
        let halves = split_at_parameters(&s, &[0.5, 0.5], &TOL).unwrap();
        assert_eq!(halves.len(), 2);
        assert_eq!(halves[0].end_point(), Some(p(0.5, 0.0)));
        let circle = Curve2::Circle(Circle2::new(p(0.0, 0.0), 1.0).unwrap());
        let once = split_at_parameters(&circle, &[1.0], &TOL).unwrap();
        assert_eq!(once.len(), 1);
        assert!(matches!(once[0], Curve2::Arc(a) if (a.sweep() - TAU).abs() < 1e-15));
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::x()).unwrap());
        let parts = split_at_parameters(&line, &[1.0, 2.0], &TOL).unwrap();
        assert!(matches!(parts[0], Curve2::Ray(_)) && matches!(parts[2], Curve2::Ray(_)));
        assert_eq!(
            split_at_points(&s, &[p(0.5, 1.0)], &TOL)
                .unwrap_err()
                .code(),
            "geom.edit.point_off_curve"
        );
        assert_eq!(split_at_points(&s, &[p(0.25, 0.0)], &TOL).unwrap().len(), 2);
    }

    #[test]
    fn trim_to_boundaries() {
        let s = seg(p(0.0, 0.0), p(10.0, 0.0));
        let walls = [
            seg(p(3.0, -1.0), p(3.0, 1.0)),
            seg(p(7.0, -1.0), p(7.0, 1.0)),
        ];
        let kept = trim(&s, &walls, 0.5, &TOL).unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].end_point(), Some(p(3.0, 0.0)));
        assert_eq!(kept[1].start_point(), Some(p(7.0, 0.0)));
        let kept = trim(&s, &walls, 0.1, &TOL).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].start_point(), Some(p(3.0, 0.0)));
        assert_eq!(
            trim(&s, &[], 0.5, &TOL).unwrap_err().code(),
            "geom.edit.no_intersection"
        );
        assert_eq!(
            trim(&s, &walls, 2.0, &TOL).unwrap_err().code(),
            "geom.parameter.out_of_range"
        );
        let circle = Curve2::Circle(Circle2::new(p(0.0, 0.0), 1.0).unwrap());
        let chord = [seg(p(-2.0, 0.0), p(2.0, 0.0))];
        let upper = trim(&circle, &chord, 3.0 * FRAC_PI_2, &TOL).unwrap();
        assert_eq!(upper.len(), 1);
        assert!(matches!(upper[0], Curve2::Arc(a) if (a.sweep() - PI).abs() < 1e-12));
        assert_eq!(
            trim(&circle, &[seg(p(1.0, -1.0), p(1.0, 1.0))], 1.0, &TOL)
                .unwrap_err()
                .code(),
            "geom.edit.no_intersection"
        );
    }

    #[test]
    fn extend_to_boundaries() {
        let circle = Curve2::Circle(Circle2::new(p(0.0, 0.0), 1.0).unwrap());
        let chord = [seg(p(-2.0, 0.0), p(2.0, 0.0))];
        let short = seg(p(0.0, 0.0), p(1.0, 0.0));
        let wall = [seg(p(5.0, -1.0), p(5.0, 1.0))];
        assert_eq!(
            extend(&short, CurveEnd::End, &wall, &TOL)
                .unwrap()
                .end_point(),
            Some(p(5.0, 0.0))
        );
        let back = [seg(p(-2.0, -1.0), p(-2.0, 1.0))];
        assert_eq!(
            extend(&short, CurveEnd::Start, &back, &TOL)
                .unwrap()
                .start_point(),
            Some(p(-2.0, 0.0))
        );
        assert_eq!(
            extend(&short, CurveEnd::Start, &wall, &TOL)
                .unwrap_err()
                .code(),
            "geom.edit.no_intersection"
        );
        let arc = Curve2::Arc(Arc2::new(p(0.0, 0.0), 1.0, 0.0, FRAC_PI_2).unwrap());
        let extended = extend(&arc, CurveEnd::End, &chord, &TOL).unwrap();
        assert!(matches!(extended, Curve2::Arc(a) if (a.sweep() - PI).abs() < 1e-9));
        let extended = extend(
            &arc,
            CurveEnd::Start,
            &[seg(p(-2.0, -0.5), p(2.0, -0.5))],
            &TOL,
        )
        .unwrap();
        assert!((extended.start_point().unwrap().y + 0.5).abs() < 1e-9);
        let ellipse = Ellipse2::new(p(0.0, 0.0), Vector2::new(2.0, 0.0), 0.5).unwrap();
        let earc = Curve2::EllipticalArc(EllipticalArc2::new(ellipse, 0.0, 1.0).unwrap());
        let extended = extend(&earc, CurveEnd::End, &chord, &TOL).unwrap();
        assert!((extended.end_point().unwrap() - p(-2.0, 0.0)).norm() < 1e-9);
        let poly = Curve2::Polyline(
            Polyline2::new(
                vec![
                    PolylineVertex::new(p(0.0, 0.0), 0.0),
                    PolylineVertex::new(p(1.0, 0.0), 0.0),
                ],
                false,
            )
            .unwrap(),
        );
        assert_eq!(
            extend(&poly, CurveEnd::End, &wall, &TOL)
                .unwrap()
                .end_point(),
            Some(p(5.0, 0.0))
        );
        let bulged = Curve2::Polyline(
            Polyline2::new(
                vec![
                    PolylineVertex::new(p(1.0, 0.0), (PI / 8.0).tan()),
                    PolylineVertex::new(p(0.0, 1.0), 0.0),
                ],
                false,
            )
            .unwrap(),
        );
        let extended = extend(&bulged, CurveEnd::End, &chord, &TOL).unwrap();
        assert!((extended.end_point().unwrap() - p(-1.0, 0.0)).norm() < 1e-9);
        assert_eq!(
            extend(&circle, CurveEnd::End, &wall, &TOL)
                .unwrap_err()
                .code(),
            "geom.edit.curve_closed"
        );
        let nurbs = Curve2::Nurbs(crate::nurbs::tests::quarter_circle());
        assert_eq!(
            extend(&nurbs, CurveEnd::End, &wall, &TOL)
                .unwrap_err()
                .code(),
            "geom.curve.kind_unsupported"
        );
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::x()).unwrap());
        assert_eq!(
            extend(&line, CurveEnd::End, &wall, &TOL)
                .unwrap_err()
                .code(),
            "geom.curve.unbounded"
        );
    }

    #[test]
    fn join_chains_and_reports_gaps() {
        let a = seg(p(0.0, 0.0), p(1.0, 0.0));
        let b = seg(p(2.0, 1.0), p(1.0, 0.0)); // reversed relative to a
        let c = seg(p(2.0, 1.0 + 1e-7), p(3.0, 1.0));
        let d = seg(p(10.0, 0.0), p(11.0, 0.0));
        let joined = join(&[a, b, c, d], &TOL).unwrap();
        assert_eq!(joined.chains.len(), 2);
        assert_eq!(joined.chains[0].1, vec![0, 1, 2]);
        assert!(!joined.chains[0].0.is_closed());
        let polyline = joined.chains[0].0.to_polyline().unwrap();
        assert_eq!(polyline.vertices().len(), 4);
        // Each free end's nearest: the two ends of the last chain, 1 apart, and the two ends of
        // the first chain, (0, 0) and (3, 1).
        assert_eq!(joined.gaps.len(), 2);
        assert_eq!(joined.gaps[0].distance, 1.0);
        assert!((joined.gaps[1].distance - 10.0_f64.sqrt()).abs() < 1e-12);
        let square = [
            seg(p(0.0, 0.0), p(1.0, 0.0)),
            seg(p(1.0, 1.0), p(0.0, 1.0)),
            seg(p(1.0, 0.0), p(1.0, 1.0)),
            seg(p(0.0, 1.0), p(0.0, 0.0)),
        ];
        let joined = join(&square, &TOL).unwrap();
        assert_eq!(joined.chains.len(), 1);
        assert!(joined.chains[0].0.is_closed());
        assert!(joined.gaps.is_empty());
        assert!(joined.chains[0].0.to_polyline().unwrap().is_closed());
        assert_eq!(
            Composite2::new(vec![], &TOL).unwrap_err().code(),
            "geom.composite.empty"
        );
        assert_eq!(
            Composite2::new(
                vec![seg(p(0.0, 0.0), p(1.0, 0.0)), seg(p(2.0, 0.0), p(3.0, 0.0))],
                &TOL
            )
            .unwrap_err()
            .code(),
            "geom.composite.not_connected"
        );
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::x()).unwrap());
        assert_eq!(
            join(&[line], &TOL).unwrap_err().code(),
            "geom.curve.unbounded"
        );
        assert!(join(&[], &TOL).unwrap().chains.is_empty());
    }

    proptest! {
        #[test]
        fn split_pieces_reproduce_the_curve(
            c in prop_oneof![curve(), wide_curve()],
            cuts in prop::collection::vec(0.0..=1.0_f64, 0..4),
            f in 0.0..=1.0_f64,
        ) {
            let parameters: Vec<f64> = cuts.iter().map(|g| parameter(&c, *g)).collect();
            let pieces = match split_at_parameters(&c, &parameters, &TOL) {
                Ok(pieces) => pieces,
                Err(e) => {
                    prop_assert_eq!(e.code(), "geom.precision.insufficient");
                    return Ok(());
                }
            };
            let size = c.bounding_box().map_or(100.0, |b| (b.max() - b.min()).abs().max());
            let bound = TOL.linear.max(1e-12 * size);
            prop_assert!(covered(&c, &pieces, f, bound));
            // Consecutive pieces meet, and each piece lies on the curve.
            for pair in pieces.windows(2) {
                if let (Some(e), Some(s)) = (pair[0].end_point(), pair[1].start_point()) {
                    prop_assert!((e - s).norm() <= bound, "{e:?} {s:?}");
                }
            }
            for piece in &pieces {
                prop_assert!(covered(piece, std::slice::from_ref(&c), f, bound));
            }
            // Rejoined, bounded pieces give back one chain.
            // Only where the rounding bound is the tolerance: a bound of a large share of a
            // piece makes chaining ambiguous.
            if c.bounding_box().is_some() && !pieces.is_empty() && bound == TOL.linear {
                let loose = Tolerance { linear: bound, ..TOL };
                let joined = join(&pieces, &loose).unwrap();
                prop_assert_eq!(joined.chains.len(), 1);
                // An open curve whose ends are within the bound counts as closed.
                let ends = distance(c.start_point(), c.end_point());
                if c.is_closed() || ends > bound {
                    prop_assert_eq!(joined.chains[0].0.is_closed(), c.is_closed());
                }
            }
        }

    }

    proptest! {
        // Projection onto NURBS pieces is slow in unoptimized builds.
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        #[test]
        fn nurbs_split_pieces_reproduce_the_curve(
            n in crate::nurbs::tests::nurbs(),
            cuts in prop::collection::vec(0.0..=1.0_f64, 0..4),
            f in 0.0..=1.0_f64,
        ) {
            let c = Curve2::Nurbs(n);
            let parameters: Vec<f64> = cuts.iter().map(|g| parameter(&c, *g)).collect();
            match split_at_parameters(&c, &parameters, &TOL) {
                Ok(pieces) => prop_assert!(covered(&c, &pieces, f, 1e-6)),
                // Lengths of curves with very uneven weights, and pieces that stand still.
                Err(e) => prop_assert!(
                    ["geom.precision.insufficient", "geom.nurbs.degenerate"].contains(&e.code()),
                    "{e:?}"
                ),
            }
        }

        #[test]
        fn join_reports_the_gaps_it_cannot_close(
            c in curve(),
            cut in 0.1..0.9_f64,
            gap in 1e-5..1.0_f64,
        ) {
            // Rays, lines and closed curves have no two free ends to separate.
            if c.bounding_box().is_none() || c.is_closed() {
                return Ok(());
            }
            let pieces = split_at_parameters(&c, &[parameter(&c, cut)], &TOL).unwrap();
            if pieces.len() != 2 {
                return Ok(());
            }
            let size = c.bounding_box().map_or(1.0, |b| (b.max() - b.min()).abs().max());
            // Move the second piece away by `gap` along x.
            let shift = arc_base::math::Transform2::from_matrix_unchecked(
                arc_base::math::Matrix3::new(1.0, 0.0, gap * size, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
            );
            let moved = pieces[1].transformed(&shift, &TOL).unwrap();
            let joined = join(&[pieces[0].clone(), moved], &TOL).unwrap();
            // The moved piece may still touch the first one elsewhere.
            if joined.chains.len() != 2 {
                return Ok(());
            }
            // The end of the first piece is reported with a free end at most the shift away.
            prop_assert!(joined.gaps.iter().any(|g| g.distance <= gap * size * (1.0 + 1e-9)));
        }
    }
}
