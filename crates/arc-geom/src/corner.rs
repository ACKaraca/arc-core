//! Corner constructions between two planar curves: fillets and chamfers (CORE-GEOM-033).
//!
//! A corner joins two curves at a corner point, the intersection of their supports: the infinite
//! line of a segment, ray or line, or the full circle of an arc or circle. The joint is a fillet
//! arc of radius `r` tangent to both supports, or a chamfer segment between points at given
//! distances from the corner point. Each input is cut at its joint point and at the corner
//! point; the part that remains is the part beyond the joint point on the side away from the
//! corner. Pick hints (points on the curves) select among the solutions and, for a full circle,
//! choose the arc that remains.
//!
//! The functions return the joint and, per input curve, its replacement, so that a caller can
//! replace its entities atomically. Nothing here changes a document.

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::polyline::{Polyline2, PolylineSegment, PolylineVertex};
use crate::primitives::{Arc2, Line2, Ray2, Segment2};

/// Whether the inputs of a corner construction are trimmed or returned unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CornerMode {
    /// Each input is trimmed or extended to the joint.
    Trim,
    /// The inputs are not changed; only the joint is returned.
    Keep,
}

/// The size of a chamfer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChamferSize {
    /// Distances from the corner along the first and along the second curve.
    Distances {
        /// The distance along the first curve, not negative.
        first: f64,
        /// The distance along the second curve, not negative.
        second: f64,
    },
    /// A distance along the first curve and the angle between the chamfer and the first curve,
    /// measured at the chamfer end inside the corner.
    DistanceAngle {
        /// The distance along the first curve, not negative.
        distance: f64,
        /// The angle in radians, strictly between 0 and π.
        angle: f64,
    },
}

/// The replacement of one input curve of a corner construction.
#[derive(Debug, Clone, PartialEq)]
pub enum Replacement {
    /// The curve trimmed or extended to the joint.
    Trimmed(Curve2),
    /// The curve is not changed: no-trim mode, or a full circle that is not cut.
    Unchanged,
}

/// The result of a corner construction.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerResult {
    /// The fillet arc or chamfer segment, running from the cut on the first curve to the cut on
    /// the second; `None` for a sharp corner.
    pub joint: Option<Curve2>,
    /// The replacement of the first curve.
    pub first: Replacement,
    /// The replacement of the second curve.
    pub second: Replacement,
}

/// A segment of a polyline and the pick point that selects the part of the segment kept.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentPick {
    /// The index of the segment.
    pub index: usize,
    /// A point on the segment. The part of the segment beyond the joint, containing this point,
    /// is kept.
    pub hint: Point2,
}

/// Fillet of radius `radius` between two curves, tangent to both near the pick hints.
///
/// The hints are points on the first and the second curve. Among the solutions (the fillet
/// circles tangent to both supports, on each of the four sides), the one whose tangent points are
/// nearest to the hints is used, and it must keep the hints on the parts that remain. A radius of
/// zero gives the sharp corner: both curves are trimmed or extended to their intersection.
///
/// Two distinct parallel lines (segments, rays or lines) receive a semicircle instead; the radius is
/// then ignored and the semicircle has half the distance between the lines as its radius. See the
/// specification for the placement.
///
/// # Errors
///
/// `geom.tolerance.invalid` for a tolerance whose linear value is not positive and finite;
/// `geom.corner.radius_invalid` for a radius that is NaN, infinite or negative;
/// `geom.input.not_finite` for a non-finite hint; `geom.corner.unsupported_pair` for a curve
/// other than a segment, ray, line, arc or circle; `geom.corner.pick_off_curve` for a hint more
/// than `tolerance.linear` from its curve; `geom.corner.coincident` for curves on one line or
/// circle; `geom.corner.unsupported_pair` for a line as the first curve of a parallel pair;
/// `geom.corner.no_intersection` when the supports do not meet; `geom.corner.radius_too_large` when no fillet circle of the radius
/// exists; `geom.corner.pick_no_solution` when no solution keeps the hints on the remaining parts;
/// `geom.corner.curve_too_short` when a remaining part would be empty.
pub fn fillet(
    first: &Curve2,
    first_hint: Point2,
    second: &Curve2,
    second_hint: Point2,
    radius: f64,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    ensure_linear(tolerance)?;
    if !(radius.is_finite() && radius >= 0.0) {
        return Err(Error::new(codes::CORNER_RADIUS_INVALID));
    }
    let (a, b) = (Side::of(first)?, Side::of(second)?);
    a.check_hint(first_hint, tolerance)?;
    b.check_hint(second_hint, tolerance)?;
    if parallel_gap(&a, &b, tolerance).is_some() {
        return parallel_fillet(first, &b, first_hint, second_hint, mode, tolerance);
    }
    check_pair(&a, &b, tolerance)?;
    let corners = support_points(a.support, b.support);
    let Some(corner) = nearest(&corners, midpoint(first_hint, second_hint)) else {
        return Err(Error::new(codes::CORNER_NO_INTERSECTION));
    };
    let candidates = if radius <= 0.0 {
        vec![Candidate {
            cut_first: corner,
            cut_second: corner,
            corner,
            joint: None,
        }]
    } else {
        fillet_candidates(&a, &b, radius, &corners, tolerance)
    };
    select(
        &a,
        &b,
        (first_hint, second_hint),
        candidates,
        mode,
        tolerance,
    )
}

/// Chamfer between two curves, by distances along each curve or by a distance and an angle.
///
/// Both curves must be segments, rays or lines. The chamfer runs between the points at the
/// given distances from their intersection, on the side of the pick hints. Zero distances on
/// both curves give the sharp corner.
///
/// # Errors
///
/// As [`fillet`], with `geom.corner.distance_invalid` for a distance that is NaN, infinite or
/// negative, or an angle outside (0, π); `geom.corner.unsupported_pair` for a circular curve
/// (chamfers of arcs are not defined here).
pub fn chamfer(
    first: &Curve2,
    first_hint: Point2,
    second: &Curve2,
    second_hint: Point2,
    size: ChamferSize,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    ensure_linear(tolerance)?;
    validate_size(size)?;
    let (a, b) = (Side::of(first)?, Side::of(second)?);
    if !a.is_linear() || !b.is_linear() {
        return Err(Error::new(codes::CORNER_UNSUPPORTED_PAIR));
    }
    a.check_hint(first_hint, tolerance)?;
    b.check_hint(second_hint, tolerance)?;
    check_pair(&a, &b, tolerance)?;
    let Some(corner) = support_points(a.support, b.support).first().copied() else {
        return Err(Error::new(codes::CORNER_NO_INTERSECTION));
    };
    let candidate = chamfer_candidate(first_hint, second_hint, corner, size, tolerance)?;
    select(
        &a,
        &b,
        (first_hint, second_hint),
        vec![candidate],
        mode,
        tolerance,
    )
}

/// Fillet between two segments of one polyline, given by their indices and pick points.
///
/// Adjacent and non-adjacent segments are both accepted. Each segment is returned as its own
/// replacement; the caller rebuilds the polyline from them.
///
/// # Errors
///
/// `geom.corner.segment_invalid` for an index outside the polyline or two equal indices; otherwise
/// as [`fillet`].
pub fn fillet_polyline_segments(
    polyline: &Polyline2,
    first: SegmentPick,
    second: SegmentPick,
    radius: f64,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    if first.index == second.index {
        return Err(Error::new(codes::CORNER_SEGMENT_INVALID));
    }
    let a = segment_curve(polyline, first.index)?;
    let b = segment_curve(polyline, second.index)?;
    fillet(&a, first.hint, &b, second.hint, radius, mode, tolerance)
}

/// Chamfer between two segments of one polyline, given by their indices and pick points.
///
/// # Errors
///
/// As [`fillet_polyline_segments`], with the chamfer errors of [`chamfer`].
pub fn chamfer_polyline_segments(
    polyline: &Polyline2,
    first: SegmentPick,
    second: SegmentPick,
    size: ChamferSize,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    if first.index == second.index {
        return Err(Error::new(codes::CORNER_SEGMENT_INVALID));
    }
    let a = segment_curve(polyline, first.index)?;
    let b = segment_curve(polyline, second.index)?;
    chamfer(&a, first.hint, &b, second.hint, size, mode, tolerance)
}

/// Why a vertex is left unchanged by a whole-polyline construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// One of the two segments at the vertex is bulged (an arc), which the construction does not handle.
    BulgedSegment,
    /// The segments are too short for the radius or distance, or a neighbouring cut would leave a
    /// segment with no part.
    TooShort,
}

/// A vertex that a whole-polyline construction left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkippedVertex {
    /// The vertex index in the input polyline.
    pub index: usize,
    /// Why the vertex was left unchanged.
    pub reason: SkipReason,
}

/// A polyline rebuilt by a whole-polyline construction, with the vertices left unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct CornerPolyline {
    /// The rebuilt polyline.
    pub polyline: Polyline2,
    /// The vertices that were not processed, in vertex order.
    pub skipped: Vec<SkippedVertex>,
}

/// A fillet at every vertex of a polyline whose two adjacent segments are straight.
///
/// Vertices with a bulged neighbour are left as they are and listed as `BulgedSegment`; vertices
/// whose segments are too short for the radius, or whose cuts would leave a segment with no part,
/// are listed as `TooShort`. The radius is applied to each processed vertex in order, and the
/// fillet arcs are stored as bulges. A radius of zero returns the polyline with no vertices
/// skipped. Bulged segments keep their bulges; constant and variable widths are refused.
///
/// # Errors
///
/// `geom.corner.radius_invalid` and `geom.tolerance.invalid` for bad arguments;
/// `geom.corner.unsupported_pair` for a polyline with widths; `geom.corner.parallel` or
/// `geom.corner.coincident` for a vertex whose two straight segments are collinear (the whole
/// call is refused, not that vertex only).
pub fn fillet_polyline_vertices(
    polyline: &Polyline2,
    radius: f64,
    tolerance: &Tolerance,
) -> Result<CornerPolyline> {
    ensure_linear(tolerance)?;
    if !(radius.is_finite() && radius >= 0.0) {
        return Err(Error::new(codes::CORNER_RADIUS_INVALID));
    }
    if radius <= 0.0 {
        return Ok(CornerPolyline {
            polyline: polyline.clone(),
            skipped: Vec::new(),
        });
    }
    rebuild_corners(polyline, tolerance, |before, after| {
        fillet_cut(before, after, radius, tolerance)
    })
}

/// A chamfer with equal distances at every vertex of a polyline whose two adjacent segments are
/// straight. Skipping and refusals are as for [`fillet_polyline_vertices`]. A distance of zero
/// returns the polyline with no vertices skipped.
///
/// # Errors
///
/// As [`fillet_polyline_vertices`], with `geom.corner.distance_invalid` for a bad distance.
pub fn chamfer_polyline_vertices(
    polyline: &Polyline2,
    distance: f64,
    tolerance: &Tolerance,
) -> Result<CornerPolyline> {
    ensure_linear(tolerance)?;
    validate_size(ChamferSize::Distances {
        first: distance,
        second: distance,
    })?;
    if distance <= 0.0 {
        return Ok(CornerPolyline {
            polyline: polyline.clone(),
            skipped: Vec::new(),
        });
    }
    rebuild_corners(polyline, tolerance, |before, after| {
        chamfer_cut(before, after, distance, tolerance)
    })
}

/// The cut of a fillet at the corner of two straight segments.
fn fillet_cut(
    before: &Segment2,
    after: &Segment2,
    radius: f64,
    tolerance: &Tolerance,
) -> Result<Cut> {
    let a = Side::of(&Curve2::Segment(*before))?;
    let b = Side::of(&Curve2::Segment(*after))?;
    check_pair(&a, &b, tolerance)?;
    let corners = support_points(a.support, b.support);
    let hints = (
        midpoint(before.start(), before.end()),
        midpoint(after.start(), after.end()),
    );
    let candidates = fillet_candidates(&a, &b, radius, &corners, tolerance);
    let chosen = best_candidate(candidates, hints)?;
    Ok(Cut {
        on_previous: chosen.cut_first,
        on_next: chosen.cut_second,
        bulge: bulge_of(chosen.joint.as_ref()),
    })
}

/// The cut of a chamfer with equal distances at the corner of two straight segments.
fn chamfer_cut(
    before: &Segment2,
    after: &Segment2,
    distance: f64,
    tolerance: &Tolerance,
) -> Result<Cut> {
    let a = Side::of(&Curve2::Segment(*before))?;
    let b = Side::of(&Curve2::Segment(*after))?;
    check_pair(&a, &b, tolerance)?;
    let corner = before.end();
    Ok(Cut {
        on_previous: corner - unit_of(before) * distance,
        on_next: corner + unit_of(after) * distance,
        bulge: 0.0,
    })
}

/// Applies `make` at every corner vertex of a polyline, in vertex order, and rebuilds it. A corner
/// is applied only when both segments it touches keep a part afterwards. Corners whose segments
/// are bulged, or that cannot be cut, are skipped; collinear corners refuse the whole call.
fn rebuild_corners(
    polyline: &Polyline2,
    tolerance: &Tolerance,
    make: impl Fn(&Segment2, &Segment2) -> Result<Cut>,
) -> Result<CornerPolyline> {
    if polyline
        .vertices()
        .iter()
        .any(|v| v.start_width != 0.0 || v.end_width != 0.0)
    {
        return Err(Error::new(codes::CORNER_UNSUPPORTED_PAIR));
    }
    let count = polyline.segment_count();
    let closed = polyline.is_closed();
    let segments: Vec<Option<Segment2>> = (0..count)
        .map(|index| match polyline.segment(index) {
            Some(PolylineSegment::Line(line)) => Some(line),
            _ => None,
        })
        .collect();
    let mut applied: Vec<Option<Cut>> = vec![None; polyline.vertices().len()];
    let mut skipped = Vec::new();
    for vertex in corner_vertices(polyline) {
        let (previous, next) = neighbours(vertex, count, closed);
        let (Some(Some(before)), Some(Some(after))) = (segments.get(previous), segments.get(next))
        else {
            skipped.push(SkippedVertex {
                index: vertex,
                reason: SkipReason::BulgedSegment,
            });
            continue;
        };
        let cut = match make(before, after) {
            Ok(cut) => cut,
            Err(error)
                if error.code() == codes::CORNER_PARALLEL.code
                    || error.code() == codes::CORNER_COINCIDENT.code =>
            {
                return Err(error);
            }
            Err(_) => {
                skipped.push(SkippedVertex {
                    index: vertex,
                    reason: SkipReason::TooShort,
                });
                continue;
            }
        };
        if let Some(slot) = applied.get_mut(vertex) {
            *slot = Some(cut);
        }
        let fits = segment_fits(previous, &segments, &applied, closed, tolerance.linear)
            && segment_fits(next, &segments, &applied, closed, tolerance.linear);
        if !fits {
            if let Some(slot) = applied.get_mut(vertex) {
                *slot = None;
            }
            skipped.push(SkippedVertex {
                index: vertex,
                reason: SkipReason::TooShort,
            });
        }
    }
    let mut out = Vec::with_capacity(polyline.vertices().len() + 2 * skipped.len());
    for (index, vertex) in polyline.vertices().iter().enumerate() {
        match applied.get(index).and_then(Option::as_ref) {
            Some(cut) => {
                out.push(PolylineVertex::new(cut.on_previous, cut.bulge));
                out.push(PolylineVertex::new(cut.on_next, 0.0));
            }
            None => out.push(PolylineVertex::new(vertex.point, vertex.bulge)),
        }
    }
    Ok(CornerPolyline {
        polyline: Polyline2::new(out, closed)?,
        skipped,
    })
}

/// Whether segment `index` keeps a part between the cuts currently applied at its ends.
fn segment_fits(
    index: usize,
    segments: &[Option<Segment2>],
    applied: &[Option<Cut>],
    closed: bool,
    eps: f64,
) -> bool {
    let Some(Some(segment)) = segments.get(index) else {
        return false;
    };
    let end_vertex = if closed {
        (index + 1) % segments.len()
    } else {
        index + 1
    };
    let length = crate::length(&(segment.end() - segment.start()));
    let unit = unit_of(segment);
    let along = |p: Point2| (p - segment.start()).dot(&unit);
    let from = applied
        .get(index)
        .and_then(Option::as_ref)
        .map_or(0.0, |cut| along(cut.on_next));
    let to = applied
        .get(end_vertex)
        .and_then(Option::as_ref)
        .map_or(length, |cut| along(cut.on_previous));
    from >= -eps && to <= length + eps && to - from > eps
}

/// The corner points of a joint between two sides/// The corner points of a joint between two sides, one cut on each and an optional joint.
#[derive(Debug, Clone)]
struct Candidate {
    /// The cut on the first curve, where the joint starts.
    cut_first: Point2,
    /// The cut on the second curve, where the joint ends.
    cut_second: Point2,
    /// The corner point, the intersection of the two supports, removed between each cut and it.
    corner: Point2,
    /// The fillet arc or chamfer segment from `cut_first` to `cut_second`.
    joint: Option<Curve2>,
}

/// A cut of a polyline at one of its vertices.
#[derive(Debug, Clone)]
struct Cut {
    /// The cut on the segment before the vertex.
    on_previous: Point2,
    /// The cut on the segment after the vertex.
    on_next: Point2,
    /// The bulge of the joint from `on_previous` to `on_next`.
    bulge: f64,
}

/// The carrier of a curve: the support it lies on and the part of the support it occupies.
#[derive(Debug, Clone, Copy)]
struct Side {
    support: Support,
    domain: Domain,
}

/// The infinite line or the full circle that carries a curve.
#[derive(Debug, Clone, Copy)]
enum Support {
    /// The line through `origin` with the unit direction `unit`.
    Line { origin: Point2, unit: Vector2 },
    /// The circle with the center and radius.
    Circle { center: Point2, radius: f64 },
}

/// The part of a support that a curve occupies, in the coordinate of [`Side::coord`].
#[derive(Debug, Clone, Copy)]
enum Domain {
    /// A stretch of a line: distances from the origin in `[lo, hi]`, possibly infinite.
    Linear { lo: f64, hi: f64 },
    /// An arc: the angle `start + sign * t` for `t` in `[0, len]`.
    Arc { start: f64, sign: f64, len: f64 },
    /// A full circle.
    Full,
}

/// Why one solution does not keep the pick hints on the remaining parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// No solution keeps the hint on the part that remains.
    PickNoSolution,
    /// The remaining part would be empty.
    TooShort,
}

impl Side {
    /// The side of a curve; a curve kind other than the five supported fails with
    /// `geom.corner.unsupported_pair`.
    fn of(curve: &Curve2) -> Result<Self> {
        let side = match curve {
            Curve2::Segment(segment) => {
                let length = crate::length(&(segment.end() - segment.start()));
                Self {
                    support: Support::Line {
                        origin: segment.start(),
                        unit: unit_of(segment),
                    },
                    domain: Domain::Linear {
                        lo: 0.0,
                        hi: length,
                    },
                }
            }
            Curve2::Ray(ray) => Self {
                support: Support::Line {
                    origin: ray.origin(),
                    unit: ray.unit_direction(),
                },
                domain: Domain::Linear {
                    lo: 0.0,
                    hi: f64::INFINITY,
                },
            },
            Curve2::Line(line) => Self {
                support: Support::Line {
                    origin: line.point(),
                    unit: line.unit_direction(),
                },
                domain: Domain::Linear {
                    lo: f64::NEG_INFINITY,
                    hi: f64::INFINITY,
                },
            },
            Curve2::Arc(arc) => Self {
                support: Support::Circle {
                    center: arc.center(),
                    radius: arc.radius(),
                },
                domain: Domain::Arc {
                    start: arc.start_angle(),
                    sign: if arc.is_counterclockwise() { 1.0 } else { -1.0 },
                    len: arc.sweep().abs(),
                },
            },
            Curve2::Circle(circle) => Self {
                support: Support::Circle {
                    center: circle.center(),
                    radius: circle.radius(),
                },
                domain: Domain::Full,
            },
            _ => return Err(Error::new(codes::CORNER_UNSUPPORTED_PAIR)),
        };
        Ok(side)
    }

    /// Whether the curve is a segment, ray or line.
    fn is_linear(&self) -> bool {
        matches!(self.domain, Domain::Linear { .. })
    }

    /// Fails with `geom.corner.pick_off_curve` unless `hint` lies on the curve.
    fn check_hint(&self, hint: Point2, tolerance: &Tolerance) -> Result<()> {
        match self.on_curve(hint, tolerance) {
            Some(_) => Ok(()),
            None => Err(Error::new(codes::CORNER_PICK_OFF_CURVE)),
        }
    }

    /// The scale that turns a coordinate difference into a length: 1 for lines, the radius for
    /// circles.
    fn scale(&self) -> f64 {
        match self.support {
            Support::Line { .. } => 1.0,
            Support::Circle { radius, .. } => radius,
        }
    }

    /// The coordinate of a point of the support. Along a line it is the distance from the origin;
    /// on a circle it is the angle, measured from the start of an arc in the sweep direction and
    /// reduced into the window of one turn around the middle of the arc.
    fn coord(&self, point: Point2) -> f64 {
        match self.support {
            Support::Line { origin, unit } => (point - origin).dot(&unit),
            Support::Circle { center, .. } => {
                let theta = angle_of(point - center);
                match self.domain {
                    Domain::Arc { start, sign, len } => arc_window(sign * (theta - start), len),
                    _ => theta.rem_euclid(TAU),
                }
            }
        }
    }

    /// The coordinate of `point` when it lies on the curve within `tolerance.linear`.
    fn on_curve(&self, point: Point2, tolerance: &Tolerance) -> Option<f64> {
        let tol = tolerance.linear;
        match (self.support, self.domain) {
            (Support::Line { origin, unit }, Domain::Linear { lo, hi }) => {
                let offset = point - origin;
                let across = offset.x * unit.y - offset.y * unit.x;
                let t = offset.dot(&unit);
                (across.abs() <= tol && t >= lo - tol && t <= hi + tol).then_some(t)
            }
            (Support::Circle { center, radius }, domain) => {
                if (crate::length(&(point - center)) - radius).abs() > tol {
                    return None;
                }
                let t = self.coord(point);
                match domain {
                    Domain::Arc { len, .. } => {
                        let slack = tol / radius;
                        (t >= -slack && t <= len + slack).then_some(t)
                    }
                    _ => Some(t),
                }
            }
            _ => None,
        }
    }

    /// The part of the curve that remains after the cut at `joint` with the corner at `corner`,
    /// as an interval of coordinates; `None` for a full circle that is not cut.
    ///
    /// The part is the one beyond `joint`, away from `corner`. When the joint is the corner (a
    /// sharp corner), the part containing the hint is kept. A full circle keeps the arc that
    /// contains the hint.
    fn remaining(
        &self,
        joint: Point2,
        corner: Point2,
        hint: Point2,
        tolerance: &Tolerance,
    ) -> std::result::Result<Option<(f64, f64)>, Refusal> {
        let hint_t = self
            .on_curve(hint, tolerance)
            .ok_or(Refusal::PickNoSolution)?;
        let eps = tolerance.linear / self.scale();
        let joint_t = self.coord(joint);
        let corner_t = self.coord(corner);
        match self.domain {
            Domain::Full => full_remaining(joint_t, corner_t, hint_t, eps),
            Domain::Linear { lo, hi } => {
                linear_remaining(lo, hi, joint_t, corner_t, hint_t, eps).map(Some)
            }
            Domain::Arc { len, .. } => {
                linear_remaining(0.0, len, joint_t, corner_t, hint_t, eps).map(Some)
            }
        }
    }

    /// The replacement of the curve for the interval of coordinates `kept`.
    fn replacement(&self, kept: Option<(f64, f64)>) -> Result<Replacement> {
        let Some((lo, hi)) = kept else {
            return Ok(Replacement::Unchanged);
        };
        let curve = match (self.support, self.domain) {
            (Support::Line { origin, unit }, _) => {
                let at = |t: f64| origin + unit * t;
                if lo.is_finite() && hi.is_finite() {
                    Curve2::Segment(Segment2::new(at(lo), at(hi))?)
                } else if hi.is_finite() {
                    Curve2::Ray(Ray2::new(at(hi), -unit)?)
                } else if lo.is_finite() {
                    Curve2::Ray(Ray2::new(at(lo), unit)?)
                } else {
                    Curve2::Line(Line2::new(origin, unit)?)
                }
            }
            (Support::Circle { center, radius }, Domain::Arc { start, sign, .. }) => Curve2::Arc(
                Arc2::new(center, radius, start + sign * lo, sign * (hi - lo))?,
            ),
            (Support::Circle { center, radius }, _) => {
                Curve2::Arc(Arc2::new(center, radius, lo, hi - lo)?)
            }
        };
        Ok(Replacement::Trimmed(curve))
    }
}

/// The remaining interval of a segment, ray, line or arc (in its own coordinates, with the
/// domain `[lo, hi]`) after the cut at `joint_t` and the corner at `corner_t`.
fn linear_remaining(
    lo: f64,
    hi: f64,
    joint_t: f64,
    corner_t: f64,
    hint_t: f64,
    eps: f64,
) -> std::result::Result<(f64, f64), Refusal> {
    if (joint_t - corner_t).abs() <= eps {
        // A sharp corner: the piece on the hint's side of the cut remains.
        if (hint_t - joint_t).abs() <= eps {
            return Err(Refusal::PickNoSolution);
        }
        let kept = if hint_t < joint_t {
            (lo.min(joint_t), joint_t)
        } else {
            (joint_t, hi.max(joint_t))
        };
        return check_length(kept, eps);
    }
    let direction = if joint_t > corner_t { 1.0 } else { -1.0 };
    let base_lo = lo.min(joint_t).min(corner_t);
    let base_hi = hi.max(joint_t).max(corner_t);
    let kept = if direction > 0.0 {
        (joint_t, base_hi)
    } else {
        (base_lo, joint_t)
    };
    let kept = check_length(kept, eps)?;
    if (hint_t - joint_t) * direction <= eps {
        return Err(Refusal::PickNoSolution);
    }
    Ok(kept)
}

/// Fails with `TooShort` unless the interval is longer than `eps`.
fn check_length(kept: (f64, f64), eps: f64) -> std::result::Result<(f64, f64), Refusal> {
    if kept.1 - kept.0 > eps {
        Ok(kept)
    } else {
        Err(Refusal::TooShort)
    }
}

/// The arc of a full circle that remains after the cuts at `joint_t` and `corner_t`: the one
/// that contains the hint. Both arcs end at the joint.
fn full_remaining(
    joint_t: f64,
    corner_t: f64,
    hint_t: f64,
    eps: f64,
) -> std::result::Result<Option<(f64, f64)>, Refusal> {
    if (joint_t - corner_t).abs() <= eps {
        return Ok(None);
    }
    let (lo, hi) = (joint_t.min(corner_t), joint_t.max(corner_t));
    if (hint_t - joint_t).abs() <= eps || (hint_t - corner_t).abs() <= eps {
        return Err(Refusal::PickNoSolution);
    }
    if hint_t > lo && hint_t < hi {
        Ok(Some((lo, hi)))
    } else {
        Ok(Some((hi, lo + TAU)))
    }
}

/// Chooses the solution whose remaining parts keep both hints, and builds the result.
fn select(
    first: &Side,
    second: &Side,
    hints: (Point2, Point2),
    candidates: Vec<Candidate>,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    if candidates.is_empty() {
        return Err(Error::new(codes::CORNER_RADIUS_TOO_LARGE));
    }
    type Kept = Option<(f64, f64)>;
    let mut best: Option<(f64, Kept, Kept, Candidate)> = None;
    let mut refusals = Vec::new();
    for candidate in candidates {
        let kept_first = first.remaining(candidate.cut_first, candidate.corner, hints.0, tolerance);
        let kept_second =
            second.remaining(candidate.cut_second, candidate.corner, hints.1, tolerance);
        match (kept_first, kept_second) {
            (Ok(kept_a), Ok(kept_b)) => {
                let score = distance(hints.0, candidate.cut_first)
                    + distance(hints.1, candidate.cut_second);
                let better = match &best {
                    None => true,
                    Some((best_score, ..)) => score < *best_score,
                };
                if better {
                    best = Some((score, kept_a, kept_b, candidate));
                }
            }
            (Err(refusal), _) | (_, Err(refusal)) => refusals.push(refusal),
        }
    }
    let Some((_, kept_a, kept_b, candidate)) = best else {
        let code = if refusals.contains(&Refusal::PickNoSolution) {
            codes::CORNER_PICK_NO_SOLUTION
        } else {
            codes::CORNER_CURVE_TOO_SHORT
        };
        return Err(Error::new(code));
    };
    let (first_replacement, second_replacement) = match mode {
        CornerMode::Trim => (first.replacement(kept_a)?, second.replacement(kept_b)?),
        CornerMode::Keep => (Replacement::Unchanged, Replacement::Unchanged),
    };
    Ok(CornerResult {
        joint: candidate.joint,
        first: first_replacement,
        second: second_replacement,
    })
}

/// The distance between two lines that are parallel but not on one line; `None` otherwise.
fn parallel_gap(a: &Side, b: &Side, tolerance: &Tolerance) -> Option<f64> {
    match (a.support, b.support) {
        (
            Support::Line {
                origin: o1,
                unit: u1,
            },
            Support::Line {
                origin: o2,
                unit: u2,
            },
        ) if cross(u1, u2).abs() <= tolerance.angular => {
            let gap = cross(o2 - o1, u1).abs();
            (gap > tolerance.linear).then_some(gap)
        }
        _ => None,
    }
}

/// The fillet of two parallel, distinct curves: a semicircle whose diameter joins the end of the
/// first curve nearest its pick point to the foot of that end on the second line. The radius of
/// the fillet is ignored; the semicircle has half the distance between the lines. It bulges away
/// from the parts kept, which both lie on the body side of their ends. The second curve keeps the
/// part from the foot in the body direction and must contain its pick point there.
fn parallel_fillet(
    first: &Curve2,
    b: &Side,
    first_hint: Point2,
    second_hint: Point2,
    mode: CornerMode,
    tolerance: &Tolerance,
) -> Result<CornerResult> {
    let (end, body) = match first {
        Curve2::Segment(segment) => {
            if distance(first_hint, segment.start()) <= distance(first_hint, segment.end()) {
                (segment.start(), unit_of(segment))
            } else {
                (segment.end(), -unit_of(segment))
            }
        }
        Curve2::Ray(ray) => (ray.origin(), ray.unit_direction()),
        _ => return Err(Error::new(codes::CORNER_UNSUPPORTED_PAIR)),
    };
    let (Support::Line { origin, unit }, Domain::Linear { lo, hi }) = (b.support, b.domain) else {
        return Err(Error::new(codes::CORNER_UNSUPPORTED_PAIR));
    };
    let away = -body;
    let foot = origin + unit * (end - origin).dot(&unit);
    let radius = distance(end, foot) / 2.0;
    let centre = midpoint(end, foot);
    let start_angle = angle_of(end - centre);
    let toward = angle_of(away);
    let sweep =
        if (start_angle + PI / 2.0 - toward).cos() >= (start_angle - PI / 2.0 - toward).cos() {
            PI
        } else {
            -PI
        };
    let joint = Arc2::new(centre, radius, start_angle, sweep)?;
    let direction = if body.dot(&unit) > 0.0 { 1.0 } else { -1.0 };
    let foot_t = (foot - origin).dot(&unit);
    let hint_t = (second_hint - origin).dot(&unit);
    let kept = if direction > 0.0 {
        (foot_t, hi.max(foot_t))
    } else {
        (lo.min(foot_t), foot_t)
    };
    if kept.1 - kept.0 <= tolerance.linear {
        return Err(Error::new(codes::CORNER_CURVE_TOO_SHORT));
    }
    if (hint_t - foot_t) * direction <= tolerance.linear {
        return Err(Error::new(codes::CORNER_PICK_NO_SOLUTION));
    }
    let (first_replacement, second_replacement) = match mode {
        CornerMode::Trim => (
            Replacement::Trimmed(first.clone()),
            b.replacement(Some(kept))?,
        ),
        CornerMode::Keep => (Replacement::Unchanged, Replacement::Unchanged),
    };
    Ok(CornerResult {
        joint: Some(Curve2::Arc(joint)),
        first: first_replacement,
        second: second_replacement,
    })
}

/// The fillet solutions of two sides for radius `radius`: one per side combination and per
/// intersection of the offset supports. Candidates whose joint cannot be built are omitted.
fn fillet_candidates(
    a: &Side,
    b: &Side,
    radius: f64,
    corners: &[Point2],
    tolerance: &Tolerance,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for sign_a in [1.0, -1.0] {
        for sign_b in [1.0, -1.0] {
            let (Some(offset_a), Some(offset_b)) = (
                offset_support(a.support, sign_a, radius),
                offset_support(b.support, sign_b, radius),
            ) else {
                continue;
            };
            for center in support_points(offset_a, offset_b) {
                let (Some(cut_first), Some(cut_second)) =
                    (foot(a.support, center), foot(b.support, center))
                else {
                    continue;
                };
                let Some(corner) = nearest(corners, midpoint(cut_first, cut_second)) else {
                    continue;
                };
                let Some(joint) =
                    arc_joint(center, radius, cut_first, cut_second, corner, tolerance)
                else {
                    continue;
                };
                out.push(Candidate {
                    cut_first,
                    cut_second,
                    corner,
                    joint: Some(joint),
                });
            }
        }
    }
    out
}

/// The candidate with the cuts nearest to the hints.
fn best_candidate(candidates: Vec<Candidate>, hints: (Point2, Point2)) -> Result<Candidate> {
    let mut best: Option<(f64, Candidate)> = None;
    for candidate in candidates {
        let score =
            distance(hints.0, candidate.cut_first) + distance(hints.1, candidate.cut_second);
        let better = match &best {
            None => true,
            Some((best_score, _)) => score < *best_score,
        };
        if better {
            best = Some((score, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
        .ok_or_else(|| Error::new(codes::CORNER_RADIUS_TOO_LARGE))
}

/// The chamfer solution for the size, with the cut points on the sides of the hints.
fn chamfer_candidate(
    first_hint: Point2,
    second_hint: Point2,
    corner: Point2,
    size: ChamferSize,
    tolerance: &Tolerance,
) -> Result<Candidate> {
    let to_first = first_hint - corner;
    let to_second = second_hint - corner;
    if crate::length(&to_first) <= tolerance.linear || crate::length(&to_second) <= tolerance.linear
    {
        return Err(Error::new(codes::CORNER_PICK_NO_SOLUTION));
    }
    let unit_first = crate::unit(&to_first);
    let unit_second = crate::unit(&to_second);
    let (cut_first, cut_second) = match size {
        ChamferSize::Distances { first, second } => {
            (corner + unit_first * first, corner + unit_second * second)
        }
        ChamferSize::DistanceAngle { distance, angle } => {
            if distance <= 0.0 {
                (corner, corner)
            } else {
                let start = corner + unit_first * distance;
                let end = chamfer_end(start, corner, unit_second, angle)
                    .ok_or_else(|| Error::new(codes::CORNER_PICK_NO_SOLUTION))?;
                (start, end)
            }
        }
    };
    let sharp = crate::length(&(cut_second - cut_first)) <= tolerance.linear;
    let joint = if sharp {
        None
    } else {
        Some(Curve2::Segment(Segment2::new(cut_first, cut_second)?))
    };
    Ok(Candidate {
        cut_first,
        cut_second,
        corner,
        joint,
    })
}

/// The end of a chamfer that starts at `start` and makes `angle` with the direction from `start`
/// to `corner`, on the side of the second line that `unit_second` points to.
fn chamfer_end(start: Point2, corner: Point2, unit_second: Vector2, angle: f64) -> Option<Point2> {
    let normal_second = Vector2::new(-unit_second.y, unit_second.x);
    let toward_corner = crate::unit(&(corner - start));
    [1.0, -1.0].into_iter().find_map(|sign| {
        let direction = rotate(toward_corner, sign * angle);
        let denominator = normal_second.dot(&direction);
        if denominator.abs() <= 0.0 {
            return None;
        }
        let t = normal_second.dot(&(corner - start)) / denominator;
        let end = start + direction * t;
        let on_side = (end - corner).dot(&unit_second) > 0.0;
        (t > 0.0 && on_side).then_some(end)
    })
}

/// Fails with `geom.corner.radius_invalid` or `geom.corner.distance_invalid` for a size that is
/// not finite and not in its range.
fn validate_size(size: ChamferSize) -> Result<()> {
    let valid = match size {
        ChamferSize::Distances { first, second } => {
            [first, second].iter().all(|d| d.is_finite() && *d >= 0.0)
        }
        ChamferSize::DistanceAngle { distance, angle } => {
            distance.is_finite()
                && distance >= 0.0
                && angle.is_finite()
                && angle > 0.0
                && angle < PI
        }
    };
    if valid {
        Ok(())
    } else {
        Err(Error::new(codes::CORNER_DISTANCE_INVALID))
    }
}

/// Fails with `geom.corner.unsupported_pair` if two sides are parallel or coincident, the cases
/// without a corner point or with a degenerate one.
fn check_pair(a: &Side, b: &Side, tolerance: &Tolerance) -> Result<()> {
    let refusal = match (a.support, b.support) {
        (
            Support::Line {
                origin: o1,
                unit: u1,
            },
            Support::Line {
                origin: o2,
                unit: u2,
            },
        ) if cross(u1, u2).abs() <= tolerance.angular => {
            if cross(o2 - o1, u1).abs() <= tolerance.linear {
                Some(codes::CORNER_COINCIDENT)
            } else {
                Some(codes::CORNER_PARALLEL)
            }
        }
        (
            Support::Circle {
                center: c1,
                radius: r1,
            },
            Support::Circle {
                center: c2,
                radius: r2,
            },
        ) if crate::length(&(c2 - c1)) <= tolerance.linear
            && (r1 - r2).abs() <= tolerance.linear =>
        {
            Some(codes::CORNER_COINCIDENT)
        }
        _ => None,
    };
    match refusal {
        Some(code) => Err(Error::new(code)),
        None => Ok(()),
    }
}

/// The segment of a polyline at `index` as a curve.
fn segment_curve(polyline: &Polyline2, index: usize) -> Result<Curve2> {
    match polyline.segment(index) {
        Some(PolylineSegment::Line(segment)) => Ok(Curve2::Segment(segment)),
        Some(PolylineSegment::Arc(arc)) => Ok(Curve2::Arc(arc)),
        None => Err(Error::new(codes::CORNER_SEGMENT_INVALID)),
    }
}

/// The vertices of a polyline where corners are formed: all of a closed one, the inner ones of
/// an open one.
fn corner_vertices(polyline: &Polyline2) -> std::ops::Range<usize> {
    let segments = polyline.segment_count();
    if polyline.is_closed() {
        0..segments
    } else {
        1..segments
    }
}

/// The segments before and after a corner vertex.
fn neighbours(vertex: usize, segments: usize, closed: bool) -> (usize, usize) {
    if closed {
        (
            if vertex == 0 {
                segments - 1
            } else {
                vertex - 1
            },
            vertex % segments,
        )
    } else {
        (vertex - 1, vertex)
    }
}

/// The bulge of a joint from its start to its end: the tangent of a quarter of its sweep.
fn bulge_of(joint: Option<&Curve2>) -> f64 {
    match joint {
        Some(Curve2::Arc(arc)) => (arc.sweep() / 4.0).tan(),
        _ => 0.0,
    }
}

/// The fillet arc from `from` to `to` on the circle of `center`, the arc whose middle faces the
/// corner. `None` when the two points coincide.
fn arc_joint(
    center: Point2,
    radius: f64,
    from: Point2,
    to: Point2,
    corner: Point2,
    tolerance: &Tolerance,
) -> Option<Curve2> {
    if crate::length(&(to - from)) <= tolerance.linear {
        return None;
    }
    let alpha = angle_of(from - center);
    let ccw = (angle_of(to - center) - alpha).rem_euclid(TAU);
    let cw = TAU - ccw;
    let facing = angle_of(corner - center);
    let sweep = if (alpha + ccw / 2.0 - facing).cos() >= (alpha - cw / 2.0 - facing).cos() {
        ccw
    } else {
        -cw
    };
    Arc2::new(center, radius, alpha, sweep)
        .ok()
        .map(Curve2::Arc)
}

/// The support offset by `sign * distance` to the left of a line, or the circle of radius
/// `radius + sign * distance`; `None` when that circle is not positive.
fn offset_support(support: Support, sign: f64, distance: f64) -> Option<Support> {
    match support {
        Support::Line { origin, unit } => {
            let normal = Vector2::new(-unit.y, unit.x);
            Some(Support::Line {
                origin: origin + normal * (sign * distance),
                unit,
            })
        }
        Support::Circle { center, radius } => {
            let offset = radius + sign * distance;
            (offset > 0.0 && offset.is_finite()).then_some(Support::Circle {
                center,
                radius: offset,
            })
        }
    }
}

/// The point of a support nearest to `point`: the foot on a line, the radial point on a circle.
fn foot(support: Support, point: Point2) -> Option<Point2> {
    match support {
        Support::Line { origin, unit } => Some(origin + unit * (point - origin).dot(&unit)),
        Support::Circle { center, radius } => {
            let v = point - center;
            let n = crate::length(&v);
            (n > 0.0 && n.is_finite()).then(|| center + v * (radius / n))
        }
    }
}

/// The points where two supports meet, finite ones only. Parallel lines and identical circles
/// meet nowhere here; callers refuse them first.
fn support_points(a: Support, b: Support) -> Vec<Point2> {
    let points = match (a, b) {
        (
            Support::Line {
                origin: o1,
                unit: u1,
            },
            Support::Line {
                origin: o2,
                unit: u2,
            },
        ) => {
            let den = cross(u1, u2);
            if den == 0.0 {
                Vec::new()
            } else {
                vec![o1 + u1 * (cross(o2 - o1, u2) / den)]
            }
        }
        (Support::Line { origin, unit }, Support::Circle { center, radius })
        | (Support::Circle { center, radius }, Support::Line { origin, unit }) => {
            let d = origin - center;
            let b = d.dot(&unit);
            let disc = b * b - (d.dot(&d) - radius * radius);
            if disc < 0.0 {
                Vec::new()
            } else {
                let s = disc.sqrt();
                vec![origin + unit * (-b + s), origin + unit * (-b - s)]
            }
        }
        (
            Support::Circle {
                center: c1,
                radius: r1,
            },
            Support::Circle {
                center: c2,
                radius: r2,
            },
        ) => {
            let delta = c2 - c1;
            let d = crate::length(&delta);
            if d == 0.0 {
                Vec::new()
            } else {
                let along = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
                let h2 = r1 * r1 - along * along;
                if h2 < 0.0 {
                    Vec::new()
                } else {
                    let n = delta / d;
                    let base = c1 + n * along;
                    let perpendicular = Vector2::new(-n.y, n.x);
                    let h = h2.sqrt();
                    vec![base + perpendicular * h, base - perpendicular * h]
                }
            }
        }
    };
    points
        .into_iter()
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .collect()
}

/// Fails with `geom.tolerance.invalid` unless the linear tolerance is positive and finite.
fn ensure_linear(tolerance: &Tolerance) -> Result<()> {
    crate::ensure_tolerance(tolerance)?;
    if tolerance.linear > 0.0 {
        Ok(())
    } else {
        Err(Error::new(codes::TOLERANCE_INVALID))
    }
}

/// The unit direction of a segment.
fn unit_of(segment: &Segment2) -> Vector2 {
    crate::unit(&(segment.end() - segment.start()))
}

/// The cross product z component of two vectors.
fn cross(a: Vector2, b: Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// The angle of a vector, in (−π, π].
fn angle_of(v: Vector2) -> f64 {
    v.y.atan2(v.x)
}

/// The arc coordinate of an angle measured in the sweep direction from the start, reduced to
/// the window of one turn centred on the middle of the arc of length `len`.
fn arc_window(theta: f64, len: f64) -> f64 {
    let mut t = theta.rem_euclid(TAU);
    let middle = len / 2.0;
    if t - middle > PI {
        t -= TAU;
    } else if t - middle < -PI {
        t += TAU;
    }
    t
}

/// The vector rotated by `angle` radians.
fn rotate(v: Vector2, angle: f64) -> Vector2 {
    let (sin, cos) = angle.sin_cos();
    Vector2::new(cos * v.x - sin * v.y, sin * v.x + cos * v.y)
}

/// The point halfway between two points.
fn midpoint(p: Point2, q: Point2) -> Point2 {
    p + (q - p) * 0.5
}

/// The distance between two points.
fn distance(p: Point2, q: Point2) -> f64 {
    crate::length(&(q - p))
}

/// The point of `points` nearest to `target`.
fn nearest(points: &[Point2], target: Point2) -> Option<Point2> {
    points
        .iter()
        .copied()
        .min_by(|p, q| distance(*p, target).total_cmp(&distance(*q, target)))
}

#[cfg(test)]
mod tests {
    use arc_base::math::{Point2, Vector2};
    use arc_base::{Result, Tolerance};
    use proptest::prelude::*;

    use crate::curve::Curve2;
    use crate::error::codes;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::{Arc2, Circle2, Line2, Segment2};

    use super::*;

    fn tol() -> Tolerance {
        Tolerance::DEFAULT
    }

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn seg(a: Point2, b: Point2) -> Curve2 {
        Curve2::Segment(Segment2::new(a, b).expect_ok())
    }

    fn code_of<T: std::fmt::Debug>(result: Result<T>) -> &'static str {
        result.expect_err("expected a refusal").code()
    }

    /// Unwraps a test value; tests are exempt from the no-panic rules.
    trait ExpectOk<T> {
        fn expect_ok(self) -> T;
    }

    impl<T, E: std::fmt::Debug> ExpectOk<T> for std::result::Result<T, E> {
        fn expect_ok(self) -> T {
            match self {
                Ok(value) => value,
                Err(error) => panic!("unexpected error: {error:?}"),
            }
        }
    }

    fn trimmed(replacement: &Replacement) -> Curve2 {
        match replacement {
            Replacement::Trimmed(curve) => curve.clone(),
            Replacement::Unchanged => panic!("expected a trimmed curve"),
        }
    }

    fn joint_arc(result: &CornerResult) -> Arc2 {
        match &result.joint {
            Some(Curve2::Arc(arc)) => *arc,
            other => panic!("expected an arc joint, got {other:?}"),
        }
    }

    fn near(a: Point2, b: Point2) -> bool {
        (a - b).norm() <= 1e-7
    }

    fn ends(curve: &Curve2) -> (Point2, Point2) {
        (
            curve.start_point().expect_ok_option(),
            curve.end_point().expect_ok_option(),
        )
    }

    trait ExpectOption<T> {
        fn expect_ok_option(self) -> T;
    }

    impl<T> ExpectOption<T> for Option<T> {
        fn expect_ok_option(self) -> T {
            match self {
                Some(value) => value,
                None => panic!("expected a point"),
            }
        }
    }

    /// The fillet of two lines through the origin along x and y, with hints on the far parts.
    fn right_corner() -> (Curve2, Curve2) {
        (
            seg(p(0.0, 0.0), p(10.0, 0.0)),
            seg(p(0.0, 0.0), p(0.0, 10.0)),
        )
    }

    #[test]
    fn right_angle_fillet_trims_both_segments_to_the_tangent_points() {
        let (a, b) = right_corner();
        let result = fillet(
            &a,
            p(8.0, 0.0),
            &b,
            p(0.0, 8.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let arc = joint_arc(&result);
        assert!(near(arc.center(), p(1.0, 1.0)));
        assert!((arc.radius() - 1.0).abs() < 1e-12);
        assert!(near(ends(&trimmed(&result.first)).0, p(1.0, 0.0)));
        assert!(near(ends(&trimmed(&result.second)).0, p(0.0, 1.0)));
        assert!(near(ends(&trimmed(&result.first)).1, p(10.0, 0.0)));
    }

    #[test]
    fn hint_off_the_segment_is_refused() {
        let (a, b) = right_corner();
        let result = fillet(
            &a,
            p(-3.0, 0.0),
            &b,
            p(0.0, 8.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        );
        // The first hint lies behind the corner: the segment does not reach it, so the
        // first part must be the one containing the hint, which is removed.
        assert_eq!(code_of(result), codes::CORNER_PICK_OFF_CURVE.code);
    }

    #[test]
    fn keep_mode_returns_the_joint_and_unchanged_inputs() {
        let (a, b) = right_corner();
        let result = fillet(
            &a,
            p(8.0, 0.0),
            &b,
            p(0.0, 8.0),
            1.0,
            CornerMode::Keep,
            &tol(),
        )
        .expect_ok();
        assert!(result.joint.is_some());
        assert_eq!(result.first, Replacement::Unchanged);
        assert_eq!(result.second, Replacement::Unchanged);
    }

    #[test]
    fn radius_zero_is_the_sharp_corner_at_the_intersection() {
        let a = seg(p(0.0, 0.0), p(10.0, 0.0));
        let b = seg(p(4.0, -3.0), p(4.0, 5.0));
        let result = fillet(
            &a,
            p(8.0, 0.0),
            &b,
            p(4.0, 4.0),
            0.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        assert!(result.joint.is_none());
        assert!(near(ends(&trimmed(&result.first)).0, p(4.0, 0.0)));
        assert!(near(ends(&trimmed(&result.first)).1, p(10.0, 0.0)));
        assert!(near(ends(&trimmed(&result.second)).0, p(4.0, 0.0)));
        assert!(near(ends(&trimmed(&result.second)).1, p(4.0, 5.0)));
    }

    #[test]
    fn segments_that_do_not_reach_the_corner_are_extended_to_it() {
        let a = seg(p(0.0, 0.0), p(5.0, 0.0));
        let b = seg(p(20.0, -10.0), p(20.0, -6.0));
        // The lines meet at (20, 0): the first segment is extended from 5 to 20 at radius 0.
        let result = fillet(
            &a,
            p(1.0, 0.0),
            &b,
            p(20.0, -8.0),
            0.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let (_, end) = ends(&trimmed(&result.first));
        assert!(near(end, p(20.0, 0.0)));
        // The second segment does not reach the corner from below either: it is extended up.
        assert!(near(ends(&trimmed(&result.second)).1, p(20.0, 0.0)));
    }

    #[test]
    fn fillet_of_segments_that_do_not_meet_extends_them_to_the_intersection() {
        let a = seg(p(0.0, 0.0), p(4.0, 0.0));
        let b = seg(p(10.0, 1.0), p(10.0, 9.0));
        // Corner at (10, 0); the fillet of radius 1 sits in the wedge toward (4, 0) and (10, 9).
        let result = fillet(
            &a,
            p(2.0, 0.0),
            &b,
            p(10.0, 8.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let arc = joint_arc(&result);
        assert!((arc.radius() - 1.0).abs() < 1e-12);
        let first = ends(&trimmed(&result.first));
        assert!(near(first.0, p(0.0, 0.0)));
        assert!(
            first.1.x > 4.0,
            "the first segment must be extended to the fillet"
        );
    }

    #[test]
    fn line_and_circle_fillet_is_tangent_to_both() {
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::new(1.0, 0.0)).expect_ok());
        let circle = Curve2::Circle(Circle2::new(p(0.0, 1.0), 2.0).expect_ok());
        // The circle meets the line at x = ±√3; the fillet sits on the right, on the hint's side.
        let result = fillet(
            &line,
            p(9.0, 0.0),
            &circle,
            p(2.0, 1.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let arc = joint_arc(&result);
        let center = arc.center();
        assert!((center.y - 1.0).abs() < 1e-9 || (center.y + 1.0).abs() < 1e-9);
        let distance = (center - p(0.0, 1.0)).norm();
        assert!((distance - 1.0).abs() < 1e-9 || (distance - 3.0).abs() < 1e-9);
        let trimmed_line = trimmed(&result.first);
        assert!(matches!(trimmed_line, Curve2::Ray(_)));
        assert!(matches!(trimmed(&result.second), Curve2::Arc(_)));
    }

    #[test]
    fn circle_circle_fillet_has_the_requested_radius_and_tangency() {
        let a = Curve2::Circle(Circle2::new(p(0.0, 0.0), 5.0).expect_ok());
        let b = Curve2::Circle(Circle2::new(p(6.0, 0.0), 4.0).expect_ok());
        let result = fillet(
            &a,
            p(0.0, 5.0),
            &b,
            p(6.0, 4.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let arc = joint_arc(&result);
        assert!((arc.radius() - 1.0).abs() < 1e-12);
        let c = arc.center();
        let da = (c - p(0.0, 0.0)).norm();
        let db = (c - p(6.0, 0.0)).norm();
        assert!((da - 4.0).abs() < 1e-9 || (da - 6.0).abs() < 1e-9);
        assert!((db - 3.0).abs() < 1e-9 || (db - 5.0).abs() < 1e-9);
    }

    #[test]
    fn arc_and_line_fillet_trims_the_arc_to_the_tangent_point() {
        let arc = Curve2::Arc(Arc2::new(p(0.0, 0.0), 5.0, 0.0, 1.5).expect_ok());
        let line = seg(p(-10.0, 3.0), p(10.0, 3.0));
        let result = fillet(
            &arc,
            arc.point_at(0.3),
            &line,
            p(8.0, 3.0),
            0.5,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let fillet_arc = joint_arc(&result);
        assert!((fillet_arc.radius() - 0.5).abs() < 1e-12);
        assert!(matches!(trimmed(&result.first), Curve2::Arc(_)));
    }

    #[test]
    fn fillet_radius_too_large_for_the_segment_is_refused() {
        // The corner is the end of the first segment; a radius of 12 puts the tangent point
        // beyond its start, so no part of the segment remains.
        let a = seg(p(0.0, 0.0), p(10.0, 0.0));
        let b = seg(p(10.0, 0.0), p(10.0, 10.0));
        let result = fillet(
            &a,
            p(2.0, 0.0),
            &b,
            p(10.0, 8.0),
            12.0,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_CURVE_TOO_SHORT.code);
    }

    #[test]
    fn hint_on_the_removed_stretch_selects_no_solution() {
        let (a, b) = right_corner();
        // The fillet of radius 2 cuts the first segment at x = 2; the hint at x = 1 is removed.
        let result = fillet(
            &a,
            p(1.0, 0.0),
            &b,
            p(0.0, 8.0),
            2.0,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_PICK_NO_SOLUTION.code);
    }

    #[test]
    fn hint_off_its_curve_is_refused() {
        let (a, b) = right_corner();
        let result = fillet(
            &a,
            p(8.0, 1.0),
            &b,
            p(0.0, 8.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_PICK_OFF_CURVE.code);
    }

    #[test]
    fn parallel_and_coincident_lines_are_refused() {
        let a = seg(p(0.0, 0.0), p(10.0, 0.0));
        let b = seg(p(0.0, 2.0), p(10.0, 2.0));
        let semicircle = fillet(
            &a,
            p(8.0, 0.0),
            &b,
            p(2.0, 2.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        assert!((joint_arc(&semicircle).radius() - 1.0).abs() < 1e-12);
        let chamfer_refused = chamfer(
            &a,
            p(8.0, 0.0),
            &b,
            p(2.0, 2.0),
            ChamferSize::Distances {
                first: 1.0,
                second: 1.0,
            },
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(chamfer_refused), codes::CORNER_PARALLEL.code);
        let c = seg(p(5.0, 0.0), p(20.0, 0.0));
        let coincident = fillet(
            &a,
            p(2.0, 0.0),
            &c,
            p(7.0, 0.0),
            1.0,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(coincident), codes::CORNER_COINCIDENT.code);
    }

    #[test]
    fn supports_that_do_not_meet_are_refused() {
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::new(1.0, 0.0)).expect_ok());
        let circle = Curve2::Circle(Circle2::new(p(0.0, 5.0), 1.0).expect_ok());
        let result = fillet(
            &line,
            p(3.0, 0.0),
            &circle,
            p(0.0, 6.0),
            0.5,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_NO_INTERSECTION.code);
    }

    #[test]
    fn empty_candidate_list_is_a_radius_too_large_refusal() {
        let (a, b) = right_corner();
        let result = select(
            &Side::of(&a).expect_ok(),
            &Side::of(&b).expect_ok(),
            (p(8.0, 0.0), p(0.0, 8.0)),
            Vec::new(),
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_RADIUS_TOO_LARGE.code);
    }

    #[test]
    fn unsupported_curve_kinds_and_bad_arguments_are_refused() {
        let (a, b) = right_corner();
        let ellipse = Curve2::Ellipse(
            crate::primitives::Ellipse2::new(p(0.0, 0.0), Vector2::new(2.0, 0.0), 0.5).expect_ok(),
        );
        assert_eq!(
            code_of(fillet(
                &ellipse,
                p(2.0, 0.0),
                &b,
                p(0.0, 8.0),
                1.0,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_UNSUPPORTED_PAIR.code
        );
        assert_eq!(
            code_of(fillet(
                &a,
                p(8.0, 0.0),
                &b,
                p(0.0, 8.0),
                -1.0,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_RADIUS_INVALID.code
        );
        assert_eq!(
            code_of(fillet(
                &a,
                p(8.0, 0.0),
                &b,
                p(0.0, 8.0),
                f64::NAN,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_RADIUS_INVALID.code
        );
        let zero = Tolerance {
            linear: 0.0,
            ..tol()
        };
        assert_eq!(
            code_of(fillet(
                &a,
                p(8.0, 0.0),
                &b,
                p(0.0, 8.0),
                1.0,
                CornerMode::Trim,
                &zero
            )),
            codes::TOLERANCE_INVALID.code
        );
    }

    #[test]
    fn chamfer_by_two_distances_puts_the_ends_on_the_curves() {
        let (a, b) = right_corner();
        let result = chamfer(
            &a,
            p(8.0, 0.0),
            &b,
            p(0.0, 8.0),
            ChamferSize::Distances {
                first: 2.0,
                second: 3.0,
            },
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let joint = result.joint.clone().expect_ok_option();
        let (start, end) = ends(&joint);
        assert!(near(start, p(2.0, 0.0)));
        assert!(near(end, p(0.0, 3.0)));
        assert!(near(ends(&trimmed(&result.first)).0, p(2.0, 0.0)));
        assert!(near(ends(&trimmed(&result.second)).0, p(0.0, 3.0)));
    }

    #[test]
    fn chamfer_by_distance_and_right_angle_matches_equal_distances() {
        let (a, b) = right_corner();
        let by_angle = chamfer(
            &a,
            p(8.0, 0.0),
            &b,
            p(0.0, 8.0),
            ChamferSize::DistanceAngle {
                distance: 2.0,
                angle: PI / 4.0,
            },
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        let (start, end) = ends(&by_angle.joint.clone().expect_ok_option());
        assert!(near(start, p(2.0, 0.0)));
        // A chamfer at 45° on a right corner has equal legs.
        assert!(near(end, p(0.0, 2.0)));
    }

    #[test]
    fn chamfer_with_zero_distances_is_the_sharp_corner() {
        let (a, b) = right_corner();
        let result = chamfer(
            &a,
            p(8.0, 0.0),
            &b,
            p(0.0, 8.0),
            ChamferSize::Distances {
                first: 0.0,
                second: 0.0,
            },
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        assert!(result.joint.is_none());
        assert!(near(ends(&trimmed(&result.first)).0, p(0.0, 0.0)));
    }

    #[test]
    fn chamfer_longer_than_a_segment_is_refused() {
        let a = seg(p(0.0, 0.0), p(5.0, 0.0));
        let b = seg(p(0.0, 0.0), p(0.0, 10.0));
        let result = chamfer(
            &a,
            p(4.0, 0.0),
            &b,
            p(0.0, 8.0),
            ChamferSize::Distances {
                first: 7.0,
                second: 1.0,
            },
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(result), codes::CORNER_CURVE_TOO_SHORT.code);
    }

    #[test]
    fn chamfer_refuses_arcs_and_bad_sizes() {
        let arc = Curve2::Arc(Arc2::new(p(0.0, 0.0), 5.0, 0.0, 1.0).expect_ok());
        let line = seg(p(-10.0, 5.0), p(10.0, 5.0));
        let size = ChamferSize::Distances {
            first: 1.0,
            second: 1.0,
        };
        assert_eq!(
            code_of(chamfer(
                &arc,
                p(5.0, 0.0),
                &line,
                p(0.0, 5.0),
                size,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_UNSUPPORTED_PAIR.code
        );
        let (a, b) = right_corner();
        let negative = ChamferSize::Distances {
            first: -1.0,
            second: 1.0,
        };
        assert_eq!(
            code_of(chamfer(
                &a,
                p(8.0, 0.0),
                &b,
                p(0.0, 8.0),
                negative,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_DISTANCE_INVALID.code
        );
        let straight = ChamferSize::DistanceAngle {
            distance: 1.0,
            angle: PI,
        };
        assert_eq!(
            code_of(chamfer(
                &a,
                p(8.0, 0.0),
                &b,
                p(0.0, 8.0),
                straight,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_DISTANCE_INVALID.code
        );
    }

    fn square(size: f64) -> Polyline2 {
        Polyline2::new(
            vec![
                PolylineVertex::new(p(0.0, 0.0), 0.0),
                PolylineVertex::new(p(size, 0.0), 0.0),
                PolylineVertex::new(p(size, size), 0.0),
                PolylineVertex::new(p(0.0, size), 0.0),
            ],
            true,
        )
        .expect_ok()
    }

    #[test]
    fn fillet_of_two_polyline_segments_returns_each_segment_trimmed() {
        let poly = square(10.0);
        let result = fillet_polyline_segments(
            &poly,
            SegmentPick {
                index: 0,
                hint: p(5.0, 0.0),
            },
            SegmentPick {
                index: 1,
                hint: p(10.0, 5.0),
            },
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        assert!(near(ends(&trimmed(&result.first)).1, p(9.0, 0.0)));
        assert!(near(ends(&trimmed(&result.second)).0, p(10.0, 1.0)));
    }

    #[test]
    fn fillet_of_non_adjacent_parallel_segments_is_a_semicircle() {
        let poly = square(10.0);
        // Segments 0 (bottom) and 2 (top) are parallel and 10 apart: the semicircle has radius 5.
        let result = fillet_polyline_segments(
            &poly,
            SegmentPick {
                index: 0,
                hint: p(5.0, 0.0),
            },
            SegmentPick {
                index: 2,
                hint: p(5.0, 10.0),
            },
            1.0,
            CornerMode::Trim,
            &tol(),
        )
        .expect_ok();
        assert!((joint_arc(&result).radius() - 5.0).abs() < 1e-12);
        let same = fillet_polyline_segments(
            &poly,
            SegmentPick {
                index: 1,
                hint: p(10.0, 5.0),
            },
            SegmentPick {
                index: 1,
                hint: p(10.0, 5.0),
            },
            1.0,
            CornerMode::Trim,
            &tol(),
        );
        assert_eq!(code_of(same), codes::CORNER_SEGMENT_INVALID.code);
    }

    #[test]
    fn fillet_all_vertices_of_a_closed_square_gives_four_arcs() {
        let poly = square(10.0);
        let rounded = fillet_polyline_vertices(&poly, 1.0, &tol()).expect_ok();
        assert_eq!(rounded.polyline.vertices().len(), 8);
        let arcs = rounded
            .polyline
            .vertices()
            .iter()
            .filter(|v| v.bulge != 0.0)
            .count();
        assert_eq!(arcs, 4);
        for vertex in rounded
            .polyline
            .vertices()
            .iter()
            .filter(|v| v.bulge != 0.0)
        {
            // Quarter turns: bulge = tan(π/8).
            assert!((vertex.bulge - (PI / 8.0).tan()).abs() < 1e-9);
        }
    }

    #[test]
    fn fillet_all_vertices_of_an_open_polyline_keeps_its_ends() {
        let open = Polyline2::new(
            vec![
                PolylineVertex::new(p(0.0, 0.0), 0.0),
                PolylineVertex::new(p(10.0, 0.0), 0.0),
                PolylineVertex::new(p(10.0, 10.0), 0.0),
            ],
            false,
        )
        .expect_ok();
        let rounded = fillet_polyline_vertices(&open, 2.0, &tol()).expect_ok();
        assert!(near(rounded.polyline.vertices()[0].point, p(0.0, 0.0)));
        let last = rounded.polyline.vertices().last().expect_ok_option();
        assert!(near(last.point, p(10.0, 10.0)));
        assert_eq!(rounded.polyline.vertices().len(), 4);
    }

    #[test]
    fn fillet_all_vertices_skips_bulged_and_too_short_vertices() {
        let bulged = Polyline2::new(
            vec![
                PolylineVertex::new(p(0.0, 0.0), 0.5),
                PolylineVertex::new(p(10.0, 0.0), 0.0),
                PolylineVertex::new(p(10.0, 10.0), 0.0),
                PolylineVertex::new(p(0.0, 10.0), 0.0),
            ],
            true,
        )
        .expect_ok();
        let rounded = fillet_polyline_vertices(&bulged, 1.0, &tol()).expect_ok();
        let skipped: Vec<_> = rounded
            .skipped
            .iter()
            .map(|s| (s.index, s.reason))
            .collect();
        assert_eq!(
            skipped,
            vec![
                (0, SkipReason::BulgedSegment),
                (1, SkipReason::BulgedSegment)
            ]
        );
        // The bulge of the untouched segment is kept, and the two straight corners are filleted.
        assert!(rounded.polyline.vertices().iter().any(|v| v.bulge == 0.5));
        assert_eq!(
            rounded
                .polyline
                .vertices()
                .iter()
                .filter(|v| v.bulge != 0.0)
                .count(),
            3
        );

        let overlapping = fillet_polyline_vertices(&square(2.0), 1.5, &tol()).expect_ok();
        assert!(
            overlapping
                .skipped
                .iter()
                .any(|s| s.reason == SkipReason::TooShort)
        );
        assert!(overlapping.polyline.vertices().len() >= 4);
    }

    #[test]
    fn collinear_polyline_vertex_refuses_the_whole_call() {
        let line = Polyline2::new(
            vec![
                PolylineVertex::new(p(0.0, 0.0), 0.0),
                PolylineVertex::new(p(5.0, 0.0), 0.0),
                PolylineVertex::new(p(10.0, 0.0), 0.0),
                PolylineVertex::new(p(10.0, 10.0), 0.0),
            ],
            false,
        )
        .expect_ok();
        assert_eq!(
            code_of(fillet_polyline_vertices(&line, 1.0, &tol())),
            codes::CORNER_COINCIDENT.code
        );
    }

    #[test]
    fn chamfer_all_vertices_replaces_each_corner_with_a_segment() {
        let poly = square(10.0);
        let chamfered = chamfer_polyline_vertices(&poly, 1.0, &tol()).expect_ok();
        assert_eq!(chamfered.polyline.vertices().len(), 8);
        assert!(chamfered.polyline.vertices().iter().all(|v| v.bulge == 0.0));
        assert!(chamfered.polyline.is_closed());
    }

    /// The sides of a corner of two rays from the origin at angle `angle`, with length 20.
    fn rays(angle: f64) -> (Curve2, Curve2) {
        let a = seg(p(0.0, 0.0), p(20.0, 0.0));
        let direction = Vector2::new(angle.cos(), angle.sin()) * 20.0;
        let b = seg(p(0.0, 0.0), Point2::from(direction));
        (a, b)
    }

    fn rotated_point(q: Point2, angle: f64, offset: Vector2) -> Point2 {
        let (sin, cos) = angle.sin_cos();
        Point2::new(cos * q.x - sin * q.y, sin * q.x + cos * q.y) + offset
    }

    fn rotated_curve(curve: &Curve2, angle: f64, offset: Vector2) -> Curve2 {
        match curve {
            Curve2::Segment(s) => seg(
                rotated_point(s.start(), angle, offset),
                rotated_point(s.end(), angle, offset),
            ),
            other => other.clone(),
        }
    }

    proptest! {
        #[test]
        fn fillet_arc_is_tangent_to_both_trimmed_results_at_its_ends(
            angle in 0.3_f64..2.8,
            radius in 0.01_f64..1.0,
        ) {
            let (a, b) = rays(angle);
            let result = fillet(&a, p(10.0, 0.0), &b, Point2::new(10.0 * angle.cos(), 10.0 * angle.sin()), radius, CornerMode::Trim, &tol()).expect_ok();
            let arc = joint_arc(&result);
            prop_assert!((arc.radius() - radius).abs() < 1e-9);
            let (joint_start, joint_end) = ends(&Curve2::Arc(arc));
            let first = ends(&trimmed(&result.first));
            let second = ends(&trimmed(&result.second));
            prop_assert!(near(first.0, joint_start) || near(first.0, joint_end));
            prop_assert!(near(second.0, joint_start) || near(second.0, joint_end));
            // The arc is tangent to a segment at its end: the radius meets the segment at a right angle.
            let radial = joint_start - arc.center();
            let along = Vector2::new(angle.cos(), angle.sin());
            prop_assert!(radial.dot(&Vector2::new(1.0, 0.0)).abs() <= radius + 1e-9);
            prop_assert!(radial.dot(&along).abs() <= radius + 1e-9);
            let distance_a = (arc.center() - p(0.0, 0.0)).dot(&Vector2::new(0.0, 1.0)).abs();
            prop_assert!((distance_a - radius).abs() < 1e-9);
        }

        #[test]
        fn chamfer_ends_lie_on_the_curves_at_the_requested_distances(
            angle in 0.3_f64..2.8,
            d1 in 0.1_f64..4.0,
            d2 in 0.1_f64..4.0,
        ) {
            let (a, b) = rays(angle);
            let hint_b = Point2::new(10.0 * angle.cos(), 10.0 * angle.sin());
            let result = chamfer(&a, p(10.0, 0.0), &b, hint_b, ChamferSize::Distances { first: d1, second: d2 }, CornerMode::Trim, &tol()).expect_ok();
            let (start, end) = ends(&result.joint.clone().expect_ok_option());
            prop_assert!((start.coords.norm() - d1).abs() < 1e-9);
            prop_assert!((end.coords.norm() - d2).abs() < 1e-9);
            prop_assert!((start.y).abs() < 1e-9);
            prop_assert!((end.x * angle.sin() - end.y * angle.cos()).abs() < 1e-7);
        }

        #[test]
        fn fillet_is_symmetric_under_swapping_the_inputs(
            angle in 0.3_f64..2.8,
            radius in 0.01_f64..1.0,
        ) {
            let (a, b) = rays(angle);
            let hint_a = p(10.0, 0.0);
            let hint_b = Point2::new(10.0 * angle.cos(), 10.0 * angle.sin());
            let forward = fillet(&a, hint_a, &b, hint_b, radius, CornerMode::Trim, &tol()).expect_ok();
            let swapped = fillet(&b, hint_b, &a, hint_a, radius, CornerMode::Trim, &tol()).expect_ok();
            let fa = joint_arc(&forward);
            let fb = joint_arc(&swapped);
            prop_assert!(near(fa.center(), fb.center()));
            let (s0, s1) = ends(&Curve2::Arc(fa));
            let (t0, t1) = ends(&Curve2::Arc(fb));
            prop_assert!((near(s0, t0) && near(s1, t1)) || (near(s0, t1) && near(s1, t0)));
            prop_assert!(near(ends(&trimmed(&forward.first)).0, ends(&trimmed(&swapped.second)).0)
                || near(ends(&trimmed(&forward.first)).1, ends(&trimmed(&swapped.second)).1));
        }

        #[test]
        fn fillet_is_invariant_under_rigid_motion(
            angle in 0.3_f64..2.8,
            radius in 0.01_f64..1.0,
            turn in -3.0_f64..3.0,
            ox in -50.0_f64..50.0,
            oy in -50.0_f64..50.0,
        ) {
            let (a, b) = rays(angle);
            let hint_a = p(10.0, 0.0);
            let hint_b = Point2::new(10.0 * angle.cos(), 10.0 * angle.sin());
            let offset = Vector2::new(ox, oy);
            let base = fillet(&a, hint_a, &b, hint_b, radius, CornerMode::Trim, &tol()).expect_ok();
            let moved_a = rotated_curve(&a, turn, offset);
            let moved_b = rotated_curve(&b, turn, offset);
            let moved = fillet(&moved_a, rotated_point(hint_a, turn, offset), &moved_b, rotated_point(hint_b, turn, offset), radius, CornerMode::Trim, &tol()).expect_ok();
            let base_center = joint_arc(&base).center();
            let moved_center = joint_arc(&moved).center();
            prop_assert!(near(rotated_point(base_center, turn, offset), moved_center));
        }

        #[test]
        fn sharp_corner_equals_the_intersection_of_the_supports(
            angle in 0.3_f64..2.8,
        ) {
            let (a, b) = rays(angle);
            let result = fillet(&a, p(10.0, 0.0), &b, Point2::new(10.0 * angle.cos(), 10.0 * angle.sin()), 0.0, CornerMode::Trim, &tol()).expect_ok();
            prop_assert!(result.joint.is_none());
            let kept = ends(&trimmed(&result.first));
            prop_assert!(near(kept.0, p(0.0, 0.0)));
            prop_assert!(near(kept.1, p(20.0, 0.0)));
        }
    }

    #[test]
    fn parallel_line_with_an_unbounded_first_curve_is_refused() {
        let line = Curve2::Line(Line2::new(p(0.0, 0.0), Vector2::new(1.0, 0.0)).expect_ok());
        let other = seg(p(0.0, 2.0), p(10.0, 2.0));
        assert_eq!(
            code_of(fillet(
                &line,
                p(3.0, 0.0),
                &other,
                p(5.0, 2.0),
                1.0,
                CornerMode::Trim,
                &tol()
            )),
            codes::CORNER_UNSUPPORTED_PAIR.code
        );
    }

    proptest! {
        #[test]
        fn parallel_semicircle_is_tangent_to_both_lines_with_half_the_distance_as_radius(
            angle in 0.0_f64..TAU,
            gap in 0.5_f64..8.0,
            shift in -4.0_f64..4.0,
            reversed in any::<bool>(),
        ) {
            let u = Vector2::new(angle.cos(), angle.sin());
            let n = Vector2::new(-u.y, u.x);
            let end = Point2::from(u * 10.0);
            let first = Curve2::Segment(Segment2::new(Point2::origin(), end).expect_ok());
            let direction = if reversed { -u } else { u };
            let anchor = Point2::from(n * gap + u * shift);
            let second = Curve2::Line(Line2::new(anchor, direction).expect_ok());
            // The body of the first segment lies in direction -u from its end; the second pick is
            // two units from the foot of that end on the second line, in the same direction.
            let foot = anchor + direction * (end - anchor).dot(&direction);
            let second_hint = foot - u * 2.0;
            let result = fillet(
                &first,
                Point2::from(u * 8.0),
                &second,
                second_hint,
                0.7,
                CornerMode::Trim,
                &tol(),
            )
            .expect_ok();
            let arc = joint_arc(&result);
            prop_assert!((arc.radius() - gap / 2.0).abs() < 1e-9);
            let (joint_start, joint_end) = ends(&Curve2::Arc(arc));
            let (at_end, at_foot) = if near(joint_start, end) {
                (joint_start, joint_end)
            } else {
                (joint_end, joint_start)
            };
            prop_assert!(near(at_end, end));
            // Tangent to the first segment at its end and to the second line at the foot.
            prop_assert!((end - arc.center()).dot(&u).abs() < 1e-7);
            prop_assert!((at_foot - arc.center()).dot(&direction).abs() < 1e-7);
            prop_assert!((at_foot.coords.dot(&n).abs() - gap).abs() < 1e-7);
            prop_assert!(trimmed(&result.second).start_point().is_some_and(|q| near(q, at_foot)));
        }
    }
}
