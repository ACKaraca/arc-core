//! Planar regions bounded by exact curves, and point containment (CORE-GEOM-021).
//!
//! A [`Loop2`] is a closed chain of bounded exact curves: segments, circular arcs, elliptical
//! arcs and polylines, or a single full circle, full ellipse or closed polyline. A [`Region`] is
//! one or more [`RegionPart`]s, each an outer loop with zero or more holes.
//!
//! - **Closure.** [`Loop2::new`] accepts consecutive curves whose end and start points differ by
//!   at most `tolerance.linear` (the last curve joins the first). A gap that is not exactly zero
//!   is closed by inserting the straight segment across it into the stored chain, so a newly
//!   built loop joins exactly. Reversing or transforming a loop recomputes the end points of
//!   arcs and can leave gaps of a few units in the last place; every algorithm treats any
//!   remaining gap between consecutive curves as a straight joint, so the boundary is always
//!   watertight. Deserialization accepts gaps up to [`Tolerance::DEFAULT`]`.linear` and stores
//!   the curves unchanged.
//! - **Area and orientation.** The signed area is computed in closed form: each curve
//!   contributes the triangle of its chord with a reference point, and each circular or
//!   elliptical arc adds its signed segment area `½·r²·(θ − sin θ)` (for an elliptical arc,
//!   `r²` is the product of the semi-axes and `θ` the eccentric sweep). A loop with an area of
//!   exactly zero is rejected, with `geom.precision.insufficient` instead when the loop is so
//!   small that the square of its size underflows. Positive area is counterclockwise.
//! - **Regions.** [`Region::new`] takes loops already grouped into parts and normalizes their
//!   orientation: outer loops counterclockwise, holes clockwise. It checks only what is cheap:
//!   at least one part, and the bounding box of each hole inside that of its outer loop. That
//!   the loops of a region neither cross nor touch one another or themselves, and that each
//!   hole lies inside its outer loop, is the caller's precondition; building regions from
//!   arbitrary curves with containment and self-intersection detection is a separate
//!   capability.
//! - **Containment.** A point is [`Containment::OnBoundary`] when its distance to some curve of
//!   some loop is at most `tolerance.linear`, measured with
//!   [`CurveProject::closest_point`]; a circular arc whose sagitta is at most `tolerance.linear`
//!   is measured to its chord instead, which differs from it by at most the sagitta, so a nearly
//!   straight arc of huge radius needs no projection onto its far-away circle. Otherwise a horizontal ray from the point toward
//!   increasing x is intersected with every piece of the boundary and the crossings are
//!   counted, with the half-open rule that an end point at the ray's height counts as below
//!   it. Straight pieces cross when their ends lie on different sides; the side of the point
//!   is decided exactly with [`orient2d`]. Arcs and elliptical arcs are split at their
//!   extrema in y into monotone stretches, whose ends decide by the same half-open rule
//!   whether they cross; where one does, the crossing is located by bisection and compared
//!   with the point, which is more than the tolerance away. Because consecutive pieces share
//!   their end points exactly, vertices and tangencies are never counted twice. An odd count
//!   is inside. A point inside a hole is outside the region.

use std::f64::consts::PI;

use arc_base::math::{Point2, Transform2};
use arc_base::{Error, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::bbox::Aabb2;
use crate::closest::CurveProject;
use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::{MAX_ROOT_ITERATIONS, Piece, Shape, Shaped, cross, ensure_tolerance};
use crate::predicates::{Sign, orient2d};
use crate::primitives::Segment2;

/// Where a point lies relative to a loop or a region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Containment {
    /// Strictly inside, farther than the linear tolerance from the boundary.
    Inside,
    /// Strictly outside, farther than the linear tolerance from the boundary.
    Outside,
    /// Within the linear tolerance of the boundary.
    OnBoundary,
}

/// A closed chain of bounded exact curves, as described in the [module documentation](self).
///
/// Serialized as its list of curves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Loop2Raw")]
pub struct Loop2 {
    curves: Vec<Curve2>,
}

#[derive(Deserialize)]
struct Loop2Raw {
    curves: Vec<Curve2>,
}

impl TryFrom<Loop2Raw> for Loop2 {
    type Error = Error;

    fn try_from(raw: Loop2Raw) -> Result<Self> {
        Self::build(raw.curves, &Tolerance::DEFAULT, false)
    }
}

impl Loop2 {
    /// A loop from stored curves, validated as deserialization validates them.
    pub(crate) fn from_stored(curves: Vec<Curve2>) -> Result<Self> {
        Self::build(curves, &Tolerance::DEFAULT, false)
    }
}

/// One smooth piece of a loop boundary with the end points the chain shares with its
/// neighbors.
#[derive(Debug, Clone, Copy)]
struct Element {
    piece: Piece,
    start: Point2,
    end: Point2,
}

impl Loop2 {
    /// Creates a loop from curves that join end to start in order, the last one back to the
    /// first. Gaps up to `tolerance.linear` are closed with straight segments.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a linear or angular tolerance that is NaN, infinite or
    /// negative; `geom.region.loop_empty` for no curves; `geom.curve.unbounded` for a ray or
    /// line; `geom.region.loop_open` when the end of a curve is farther than `tolerance.linear`
    /// from the start of the next; `geom.region.loop_zero_area` when the enclosed signed area is
    /// exactly zero; `geom.precision.insufficient` when the area is not finite in `f64`, or is zero
    /// because the loop is so small that the square of its size underflows.
    pub fn new(curves: Vec<Curve2>, tolerance: &Tolerance) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        Self::build(curves, tolerance, true)
    }

    fn build(curves: Vec<Curve2>, tolerance: &Tolerance, join: bool) -> Result<Self> {
        if curves.is_empty() {
            return Err(Error::new(codes::REGION_LOOP_EMPTY));
        }
        if let Some(index) = curves.iter().position(|c| c.bounding_box().is_none()) {
            return Err(Error::new(codes::CURVE_UNBOUNDED)
                .with_detail("curve", i64::try_from(index).unwrap_or(i64::MAX)));
        }
        // The joints that close gaps, each with the index of the curve it follows.
        let mut joints = Vec::new();
        for (index, curve) in curves.iter().enumerate() {
            let next = curves.get((index + 1) % curves.len());
            let (Some(end), Some(start)) = (curve.end_point(), next.and_then(Curve2::start_point))
            else {
                continue;
            };
            let gap = crate::length(&(start - end));
            if gap.is_nan() || gap > tolerance.linear {
                return Err(Error::new(codes::REGION_LOOP_OPEN)
                    .with_detail("curve", i64::try_from(index).unwrap_or(i64::MAX))
                    .with_detail("gap", gap));
            }
            if join && gap > 0.0 {
                joints.push((index, Curve2::Segment(Segment2::new(end, start)?)));
            }
        }
        let joined = Self::with_joints(curves, joints);
        let built = Self { curves: joined };
        let area = built.signed_area();
        if !area.is_finite() {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("area", area));
        }
        if area == 0.0 {
            // An area of the square of the loop's size below the smallest normal number has
            // underflowed, and its zero says nothing.
            let boxed = built.bounding_box();
            let size = crate::length(&(boxed.max() - boxed.min()));
            if size * size < f64::MIN_POSITIVE {
                return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("size", size));
            }
            return Err(Error::new(codes::REGION_LOOP_ZERO_AREA));
        }
        Ok(built)
    }

    /// The curves in order, including the segments that close construction gaps.
    #[must_use]
    pub fn curves(&self) -> &[Curve2] {
        &self.curves
    }

    /// The signed enclosed area: positive when the loop runs counterclockwise. Exact in closed
    /// form for line, arc and elliptical arc boundaries, up to the rounding of `f64`.
    #[must_use]
    pub fn signed_area(&self) -> f64 {
        let elements = self.elements();
        let Some(origin) = elements.first().map(|e| e.start) else {
            return 0.0;
        };
        0.5 * elements.iter().map(|e| area_term(e, origin)).sum::<f64>()
    }

    /// Whether the loop runs counterclockwise (its signed area is positive).
    #[must_use]
    pub fn is_counterclockwise(&self) -> bool {
        self.signed_area() > 0.0
    }

    /// `curves` with each joint inserted after the curve at its index; the joints are in
    /// ascending order of index.
    fn with_joints(curves: Vec<Curve2>, joints: Vec<(usize, Curve2)>) -> Vec<Curve2> {
        if joints.is_empty() {
            return curves;
        }
        let mut joined = Vec::with_capacity(curves.len() + joints.len());
        let mut joints = joints.into_iter().peekable();
        for (index, curve) in curves.into_iter().enumerate() {
            joined.push(curve);
            if let Some((_, joint)) = joints.next_if(|(at, _)| *at == index) {
                joined.push(joint);
            }
        }
        joined
    }

    /// The same loop traversed in the opposite direction. The reverse of an arc recomputes its
    /// end points from its angles, which can move them by the rounding of the radius; a gap
    /// wider than the default linear tolerance that this opens between two reversed curves is
    /// closed with a straight segment, so the reversed loop is valid in its stored form. The
    /// reverse of the arc of a polyline segment is the arc of the reversed segment.
    #[must_use]
    pub fn reversed(&self) -> Self {
        let curves: Vec<Curve2> = self
            .curves
            .iter()
            .rev()
            .map(|c| {
                // A chord arc is rebuilt from its reversed chord, so the reverse is the arc a
                // polyline segment in that direction has, which is the form that is stored.
                let rebuilt = match c {
                    Curve2::Arc(arc) => arc.chord().and_then(|chord| {
                        crate::encoding::chord_arc(chord.end(), chord.start(), -chord.bulge())
                            .ok()
                            .flatten()
                    }),
                    _ => None,
                };
                // Every stored curve is bounded, and only rays have no reverse.
                rebuilt
                    .map(Curve2::Arc)
                    .or_else(|| c.reversed())
                    .unwrap_or_else(|| c.clone())
            })
            .collect();
        let mut joints = Vec::new();
        for (index, curve) in curves.iter().enumerate() {
            let next = curves.get((index + 1) % curves.len());
            // A gap within the default tolerance is valid in a stored loop and stays.
            if let (Some(end), Some(start)) =
                (curve.end_point(), next.and_then(Curve2::start_point))
                && crate::length(&(start - end)) > Tolerance::DEFAULT.linear
                && let Ok(joint) = Segment2::new(end, start)
            {
                joints.push((index, Curve2::Segment(joint)));
            }
        }
        Self {
            curves: Self::with_joints(curves, joints),
        }
    }

    /// The exact tight bounding box.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        self.curves
            .iter()
            .filter_map(Curve2::bounding_box)
            .fold(Aabb2::EMPTY, |boxed, b| boxed.union(&b))
    }

    /// The image of the loop under `transform`, with each curve transformed as
    /// [`Curve2::transformed`] describes and the result joined again within
    /// `tolerance.linear`. A segment no longer than `tolerance.linear` whose image collapses to
    /// a point is dropped.
    ///
    /// # Errors
    ///
    /// As [`Curve2::transformed`] and [`Self::new`].
    pub fn transformed(&self, transform: &Transform2, tolerance: &Tolerance) -> Result<Self> {
        let mut curves = Vec::with_capacity(self.curves.len());
        for curve in &self.curves {
            match (curve.transformed(transform, tolerance), curve) {
                (Ok(image), _) => curves.push(image),
                // A joint shorter than the tolerance may collapse in the image; joining again
                // closes the gap it leaves.
                (Err(_), Curve2::Segment(s))
                    if crate::length(&(s.end() - s.start())) <= tolerance.linear => {}
                (Err(error), _) => return Err(error),
            }
        }
        Self::new(curves, tolerance)
    }

    /// Where `point` lies relative to the area this loop encloses, whatever its orientation.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite point; `geom.tolerance.invalid` when
    /// `tolerance.linear` is not finite and positive; `geom.precision.insufficient` when a
    /// distance or an orientation cannot be decided in `f64`.
    pub fn classify(&self, point: Point2, tolerance: &Tolerance) -> Result<Containment> {
        crate::ensure_finite(&[point.x, point.y])?;
        ensure_tolerance("linear", tolerance.linear)?;
        if !self
            .bounding_box()
            .expanded(tolerance.linear)
            .contains_point(&point)
        {
            return Ok(Containment::Outside);
        }
        let elements = self.elements();
        for element in &elements {
            if distance(element, point, tolerance)? <= tolerance.linear {
                return Ok(Containment::OnBoundary);
            }
        }
        let mut inside = false;
        for element in &elements {
            if crosses_ray(element, point)? {
                inside = !inside;
            }
        }
        Ok(if inside {
            Containment::Inside
        } else {
            Containment::Outside
        })
    }

    /// The smooth pieces of the boundary in order, with the straight joints across any gaps
    /// between consecutive curves.
    fn elements(&self) -> Vec<Element> {
        let mut out = Vec::new();
        let count = self.curves.len();
        for (index, curve) in self.curves.iter().enumerate() {
            let (Some(start), Some(end)) = (curve.start_point(), curve.end_point()) else {
                continue;
            };
            match curve.shape() {
                Shape::Piece(piece) => out.push(Element { piece, start, end }),
                Shape::Polyline(polyline) => {
                    let vertices = polyline.vertices();
                    out.extend((0..polyline.segment_count()).filter_map(|k| {
                        Some(Element {
                            piece: crate::eval::segment_piece(polyline.segment(k)?),
                            start: vertices.get(k)?.point,
                            end: vertices.get((k + 1) % vertices.len())?.point,
                        })
                    }));
                }
            }
            let next = self
                .curves
                .get((index + 1) % count)
                .and_then(Curve2::start_point);
            if let Some(next) = next
                && let Ok(joint) = Segment2::new(end, next)
            {
                out.push(Element {
                    piece: Piece::Segment(joint),
                    start: end,
                    end: next,
                });
            }
        }
        out
    }
}

/// Twice the signed area contribution of one element relative to `origin`.
fn area_term(element: &Element, origin: Point2) -> f64 {
    let chord = cross(&(element.start - origin), &(element.end - origin));
    let segment = match element.piece {
        Piece::Arc(arc) => {
            let sweep = arc.sweep();
            // Below a half turn the radius follows from the exact chord, which stays accurate
            // for nearly straight polyline arcs whose stored radius is rounded.
            let radius = if sweep.abs() < PI {
                crate::length(&(element.end - element.start)) / (2.0 * (0.5 * sweep).sin().abs())
            } else {
                arc.radius()
            };
            radius * radius * sweep_minus_sine(sweep)
        }
        Piece::Elliptical(arc) => {
            let ellipse = arc.ellipse();
            let major = crate::length(&ellipse.major_axis());
            major * (major * ellipse.ratio()) * sweep_minus_sine(arc.sweep())
        }
        Piece::Segment(_) | Piece::Ray(_) | Piece::Line(_) => 0.0,
    };
    chord + segment
}

/// `θ − sin θ`, by its Taylor series where the difference cancels.
fn sweep_minus_sine(theta: f64) -> f64 {
    if theta.abs() >= 0.5 {
        return theta - theta.sin();
    }
    // θ³/3! − θ⁵/5! + …, each term the previous times −θ²/((2k + 2)(2k + 3)); eight terms
    // reach full precision for |θ| < 0.5.
    let t2 = theta * theta;
    let mut sum = 1.0;
    for divisor in [272.0, 210.0, 156.0, 110.0, 72.0, 42.0, 20.0] {
        sum = 1.0 - t2 / divisor * sum;
    }
    theta * t2 / 6.0 * sum
}

/// The distance from `point` to an element when it can be within `tolerance.linear`, and
/// infinity otherwise. A circular arc whose sagitta is at most `tolerance.linear` is measured to
/// its chord, which differs from the arc by at most the sagitta.
fn distance(element: &Element, point: Point2, tolerance: &Tolerance) -> Result<f64> {
    let near = |boxed: Aabb2| boxed.expanded(tolerance.linear).contains_point(&point);
    let flat = |arc: &crate::primitives::Arc2| {
        let chord = crate::length(&(element.end - element.start));
        // sagitta = ½·chord·tan(|θ|/4), at most the chord below half a turn.
        arc.sweep().abs() < PI && 0.5 * chord * (0.25 * arc.sweep().abs()).tan() <= tolerance.linear
    };
    let found = match &element.piece {
        Piece::Arc(c) if near(c.bounding_box()) && flat(c) => {
            match Segment2::new(element.start, element.end) {
                Ok(chord) => chord.closest_point(point, tolerance)?,
                Err(_) => c.closest_point(point, tolerance)?,
            }
        }
        Piece::Segment(c) if near(c.bounding_box()) => c.closest_point(point, tolerance)?,
        Piece::Arc(c) if near(c.bounding_box()) => c.closest_point(point, tolerance)?,
        Piece::Elliptical(c) if near(c.bounding_box()) => c.closest_point(point, tolerance)?,
        _ => return Ok(f64::INFINITY),
    };
    Ok(found.distance)
}

/// Whether the element crosses the ray from `point` toward increasing x an odd number of
/// times, with end points at the ray's height counting as below it.
fn crosses_ray(element: &Element, point: Point2) -> Result<bool> {
    let above = |p: &Point2| p.y > point.y;
    let Some(conic) = element.piece.conic() else {
        let (a, b) = (element.start, element.end);
        if above(&a) == above(&b) {
            return Ok(false);
        }
        // Upward, the crossing is right of the point when the point is left of the piece.
        let side = orient2d(&a, &b, &point)?;
        return Ok(if above(&b) {
            side == Sign::Positive
        } else {
            side == Sign::Negative
        });
    };
    // y(θ) = c.y + a.y·cos θ + b.y·sin θ has its extrema at atan2(b.y, a.y) and half a turn on.
    let span = conic.span();
    let sign = conic.sweep.signum();
    let extremum = conic.b.y.atan2(conic.a.y);
    let mut splits: Vec<f64> = [extremum, extremum + PI]
        .iter()
        .map(|theta| ((theta - conic.start) * sign).rem_euclid(std::f64::consts::TAU))
        .filter(|t| *t > 0.0 && *t < span)
        .collect();
    splits.sort_by(f64::total_cmp);
    let mut stops = vec![(0.0, element.start)];
    stops.extend(splits.iter().map(|t| (*t, element.piece.point(*t))));
    stops.push((span, element.end));
    let mut odd = false;
    for pair in stops.windows(2) {
        let [(t0, p0), (t1, p1)] = pair else {
            continue;
        };
        let side = above(p0);
        if side == above(p1) {
            continue;
        }
        // The stretch is monotone in y, so it crosses the ray's line once; bisect for it.
        let (mut lo, mut hi) = (*t0, *t1);
        for _ in 0..MAX_ROOT_ITERATIONS {
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                break;
            }
            if above(&element.piece.point(mid)) == side {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if element.piece.point(0.5 * (lo + hi)).x > point.x {
            odd = !odd;
        }
    }
    Ok(odd)
}

/// An outer loop with its holes; a part of a [`Region`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionPart {
    outer: Loop2,
    holes: Vec<Loop2>,
}

impl RegionPart {
    /// Groups an outer loop with its holes. [`Region::new`] validates and orients them.
    #[must_use]
    pub fn new(outer: Loop2, holes: Vec<Loop2>) -> Self {
        Self { outer, holes }
    }

    /// The outer loop; counterclockwise in a region.
    #[must_use]
    pub fn outer(&self) -> &Loop2 {
        &self.outer
    }

    /// The holes; clockwise in a region.
    #[must_use]
    pub fn holes(&self) -> &[Loop2] {
        &self.holes
    }
}

/// A planar region: outer loops with holes, as described in the [module
/// documentation](self).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RegionRaw")]
pub struct Region {
    parts: Vec<RegionPart>,
}

crate::deserialize_through_new!(Region, RegionRaw { parts: Vec<RegionPart> });

impl Region {
    /// Creates a region from parts, turning each outer loop counterclockwise and each hole
    /// clockwise. The loops must neither cross nor touch, and each hole must lie inside its
    /// outer loop; only the bounding boxes are checked.
    ///
    /// # Errors
    ///
    /// `geom.region.empty` for no parts; `geom.region.hole_outside` when the bounding box of a
    /// hole is not inside that of its outer loop.
    pub fn new(parts: Vec<RegionPart>) -> Result<Self> {
        if parts.is_empty() {
            return Err(Error::new(codes::REGION_EMPTY));
        }
        let mut oriented = Vec::with_capacity(parts.len());
        for (index, part) in parts.into_iter().enumerate() {
            let outer = oriented_loop(part.outer, true);
            let outer_box = outer.bounding_box();
            let mut holes = Vec::with_capacity(part.holes.len());
            for (hole_index, hole) in part.holes.into_iter().enumerate() {
                let hole = oriented_loop(hole, false);
                if !outer_box.contains(&hole.bounding_box()) {
                    return Err(Error::new(codes::REGION_HOLE_OUTSIDE)
                        .with_detail("part", i64::try_from(index).unwrap_or(i64::MAX))
                        .with_detail("hole", i64::try_from(hole_index).unwrap_or(i64::MAX)));
                }
                holes.push(hole);
            }
            oriented.push(RegionPart { outer, holes });
        }
        Ok(Self { parts: oriented })
    }

    /// The parts: each an outer loop with its holes.
    #[must_use]
    pub fn parts(&self) -> &[RegionPart] {
        &self.parts
    }

    /// The image of the region under `transform`; a mirror is undone by the orientation rule.
    ///
    /// # Errors
    ///
    /// As [`Loop2::transformed`] and [`Self::new`].
    pub fn transformed(&self, transform: &Transform2, tolerance: &Tolerance) -> Result<Self> {
        let parts = self
            .parts
            .iter()
            .map(|part| {
                Ok(RegionPart::new(
                    part.outer.transformed(transform, tolerance)?,
                    part.holes
                        .iter()
                        .map(|hole| hole.transformed(transform, tolerance))
                        .collect::<Result<Vec<_>>>()?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::new(parts)
    }

    /// Where `point` lies relative to the region: inside an outer loop and outside all of its
    /// holes is [`Containment::Inside`]; within `tolerance.linear` of any loop is
    /// [`Containment::OnBoundary`].
    ///
    /// # Errors
    ///
    /// As [`Loop2::classify`].
    pub fn classify(&self, point: Point2, tolerance: &Tolerance) -> Result<Containment> {
        crate::ensure_finite(&[point.x, point.y])?;
        ensure_tolerance("linear", tolerance.linear)?;
        'parts: for part in &self.parts {
            match part.outer.classify(point, tolerance)? {
                Containment::Outside => continue,
                Containment::OnBoundary => return Ok(Containment::OnBoundary),
                Containment::Inside => {}
            }
            for hole in &part.holes {
                match hole.classify(point, tolerance)? {
                    Containment::Outside => {}
                    Containment::OnBoundary => return Ok(Containment::OnBoundary),
                    Containment::Inside => continue 'parts,
                }
            }
            return Ok(Containment::Inside);
        }
        Ok(Containment::Outside)
    }
}

fn oriented_loop(lp: Loop2, counterclockwise: bool) -> Loop2 {
    if lp.is_counterclockwise() == counterclockwise {
        lp
    } else {
        lp.reversed()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::f64::consts::{FRAC_PI_2, TAU};

    use arc_base::math::{Matrix3, Vector2};
    use proptest::prelude::*;

    use super::*;
    use crate::flatten::CurveFlatten;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2};

    pub(crate) const TOL: Tolerance = Tolerance::DEFAULT;

    fn code(result: Result<impl std::fmt::Debug>) -> &'static str {
        result.unwrap_err().code()
    }

    pub(crate) fn polygon(points: &[(f64, f64)], bulge: f64) -> Loop2 {
        let vertices = points
            .iter()
            .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), bulge))
            .collect();
        Loop2::new(
            vec![Curve2::Polyline(Polyline2::new(vertices, true).unwrap())],
            &TOL,
        )
        .unwrap()
    }

    pub(crate) fn square(cx: f64, cy: f64, half: f64) -> Loop2 {
        polygon(
            &[
                (cx - half, cy - half),
                (cx + half, cy - half),
                (cx + half, cy + half),
                (cx - half, cy + half),
            ],
            0.0,
        )
    }

    pub(crate) fn circle(cx: f64, cy: f64, r: f64) -> Loop2 {
        Loop2::new(
            vec![Curve2::Circle(
                Circle2::new(Point2::new(cx, cy), r).unwrap(),
            )],
            &TOL,
        )
        .unwrap()
    }

    pub(crate) fn ellipse(c: Point2, major: Vector2, ratio: f64) -> Loop2 {
        Loop2::new(
            vec![Curve2::Ellipse(Ellipse2::new(c, major, ratio).unwrap())],
            &TOL,
        )
        .unwrap()
    }

    /// A stadium of separate segments and arcs: straight sides 2w long, round ends of radius h.
    pub(crate) fn stadium(c: Point2, w: f64, h: f64) -> Loop2 {
        let p = |x: f64, y: f64| c + Vector2::new(x, y);
        Loop2::new(
            vec![
                Curve2::Segment(Segment2::new(p(-w, -h), p(w, -h)).unwrap()),
                Curve2::Arc(Arc2::new(p(w, 0.0), h, -FRAC_PI_2, PI).unwrap()),
                Curve2::Segment(Segment2::new(p(w, h), p(-w, h)).unwrap()),
                Curve2::Arc(Arc2::new(p(-w, 0.0), h, FRAC_PI_2, PI).unwrap()),
            ],
            &TOL,
        )
        .unwrap()
    }

    pub(crate) fn part(outer: Loop2, holes: Vec<Loop2>) -> RegionPart {
        RegionPart::new(outer, holes)
    }

    /// An outer loop around `c` whose inscribed radius is above 5.
    fn outer_loop(c: Point2) -> impl Strategy<Value = Loop2> {
        let star = (
            5_usize..9,
            prop::collection::vec((-0.1..0.1_f64, 8.0..10.0_f64, 0.0..0.4_f64), 9),
        )
            .prop_map(move |(n, v)| {
                let vertices = v
                    .iter()
                    .take(n)
                    .enumerate()
                    .map(|(k, (jitter, radius, bulge))| {
                        let angle = TAU * k as f64 / n as f64 + jitter;
                        PolylineVertex::new(
                            c + Vector2::new(angle.cos(), angle.sin()) * *radius,
                            *bulge,
                        )
                    })
                    .collect();
                Loop2::new(
                    vec![Curve2::Polyline(Polyline2::new(vertices, true).unwrap())],
                    &TOL,
                )
                .unwrap()
            });
        prop_oneof![
            star,
            (4.0..8.0_f64, 6.0..9.0_f64).prop_map(move |(w, h)| stadium(c, w, h)),
            (8.0..10.0_f64).prop_map(move |r| circle(c.x, c.y, r)),
            (-PI..PI, 9.0..10.0_f64, 0.7..=1.0_f64).prop_map(move |(a, len, k)| ellipse(
                c,
                Vector2::new(a.cos(), a.sin()) * len,
                k
            )),
        ]
    }

    /// A hole within 1.5 of `c`.
    fn hole_loop(c: Point2) -> impl Strategy<Value = Loop2> {
        prop_oneof![
            (0.5..1.5_f64).prop_map(move |r| circle(c.x, c.y, r)),
            (0.5..1.0_f64).prop_map(move |h| square(c.x, c.y, h)),
            (-PI..PI, 1.0..1.5_f64, 0.3..=1.0_f64, 0.0..PI).prop_map(move |(a, len, k, s)| {
                // Half an ellipse closed by its diameter.
                let e = Ellipse2::new(c, Vector2::new(a.cos(), a.sin()) * len, k).unwrap();
                let arc = EllipticalArc2::new(e, s, PI).unwrap();
                let (start, end) = (arc.point_at(0.0), arc.point_at(PI));
                Loop2::new(
                    vec![
                        Curve2::EllipticalArc(arc),
                        Curve2::Segment(Segment2::new(end, start).unwrap()),
                    ],
                    &TOL,
                )
                .unwrap()
            }),
        ]
    }

    fn maybe_reversed(lp: impl Strategy<Value = Loop2>) -> impl Strategy<Value = Loop2> {
        (lp, any::<bool>()).prop_map(|(lp, flip)| if flip { lp.reversed() } else { lp })
    }

    fn region_part(c: Point2) -> impl Strategy<Value = RegionPart> {
        let holes = prop::collection::vec(any::<bool>(), 2).prop_flat_map(move |which| {
            let centers: Vec<Point2> = [(-3.0, 0.0), (3.0, 0.0)]
                .iter()
                .zip(which)
                .filter(|(_, used)| *used)
                .map(|((x, y), _)| c + Vector2::new(*x, *y))
                .collect();
            centers
                .into_iter()
                .map(|h| maybe_reversed(hole_loop(h)))
                .collect::<Vec<_>>()
        });
        (maybe_reversed(outer_loop(c)), holes).prop_map(|(o, h)| part(o, h))
    }

    /// A region of one or two parts with up to two holes each, of every boundary kind.
    pub(crate) fn region() -> impl Strategy<Value = Region> {
        (
            region_part(Point2::origin()),
            prop::option::of(region_part(Point2::new(40.0, 0.0))),
        )
            .prop_map(|(a, b)| Region::new(std::iter::once(a).chain(b).collect()).unwrap())
    }

    /// Every loop of the region flattened within `chord`.
    pub(crate) fn dense(region: &Region, chord: f64) -> Vec<Vec<Point2>> {
        region
            .parts()
            .iter()
            .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
            .map(|lp| {
                lp.curves()
                    .iter()
                    .flat_map(|c| c.flatten(chord, 0.1, 1_000_000).unwrap())
                    .collect()
            })
            .collect()
    }

    /// The heights at which a horizontal ray meets the boundary in a special way: curve and
    /// polyline vertices, and the extrema in y of every piece.
    fn special_heights(region: &Region) -> Vec<f64> {
        let mut heights = Vec::new();
        for lp in region
            .parts()
            .iter()
            .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
        {
            for element in lp.elements() {
                let boxed = match &element.piece {
                    Piece::Segment(c) => c.bounding_box(),
                    Piece::Arc(c) => c.bounding_box(),
                    Piece::Elliptical(c) => c.bounding_box(),
                    Piece::Ray(_) | Piece::Line(_) => Aabb2::EMPTY,
                };
                heights.extend([element.start.y, element.end.y, boxed.min().y, boxed.max().y]);
            }
        }
        heights
    }

    /// The default number of cases of the properties that flatten a reference densely: fewer
    /// than the usual 256 regions, each queried at many points; `PROPTEST_CASES` overrides it.
    fn dense_cases() -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(48)
    }

    fn translation(x: f64, y: f64) -> Transform2 {
        Transform2::from_matrix_unchecked(Matrix3::new(1.0, 0.0, x, 0.0, 1.0, y, 0.0, 0.0, 1.0))
    }

    /// Even-odd containment in polygons, and the distance to their edges.
    pub(crate) fn reference(polygons: &[Vec<Point2>], p: Point2) -> (bool, f64) {
        let mut inside = false;
        let mut distance = f64::INFINITY;
        for polygon in polygons {
            for (a, b) in polygon.iter().zip(polygon.iter().cycle().skip(1)) {
                if (a.y > p.y) != (b.y > p.y) {
                    let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
                    if x > p.x {
                        inside = !inside;
                    }
                }
                let ab = b - a;
                let t = ((p - a).dot(&ab) / ab.norm_squared()).clamp(0.0, 1.0);
                distance = distance.min((p - (a + ab * t)).norm());
            }
        }
        (inside, distance)
    }

    fn rigid() -> impl Strategy<Value = Transform2> {
        (-PI..PI, any::<bool>(), -50.0..50.0_f64, -50.0..50.0_f64).prop_map(|(r, m, e, f)| {
            let (sin, cos) = r.sin_cos();
            let m = if m { -1.0 } else { 1.0 };
            Transform2::from_matrix_unchecked(Matrix3::new(
                cos,
                -sin * m,
                e,
                sin,
                cos * m,
                f,
                0.0,
                0.0,
                1.0,
            ))
        })
    }

    fn query() -> impl Strategy<Value = Point2> {
        (-20.0..60.0_f64, -13.0..13.0_f64).prop_map(|(x, y)| Point2::new(x, y))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(dense_cases()))]

        #[test]
        fn classification_matches_a_dense_reference(
            r in region(),
            queries in prop::collection::vec((query(), any::<prop::sample::Index>()), 16),
            offset in prop_oneof![Just(0.0), Just(1e6)],
        ) {
            // Each query once at a random point, and once at the same x but snapped to the
            // height of a vertex or an extremum of the boundary; the region is also moved far
            // from the origin, with millimeter-scale differences between the heights.
            let polygons = dense(&r, 1e-3);
            let moved = r.transformed(&translation(offset, offset), &TOL).unwrap();
            let heights = special_heights(&moved);
            for (p, pick) in queries {
                let snapped = Point2::new(p.x, pick.get(&heights) - offset);
                for q in [p, snapped] {
                    let (inside, distance) = reference(&polygons, q);
                    if distance > 1e-2 {
                        let expected =
                            if inside { Containment::Inside } else { Containment::Outside };
                        let at = Point2::new(q.x + offset, q.y + offset);
                        prop_assert_eq!(moved.classify(at, &TOL).unwrap(), expected, "{:?}", at);
                        if offset == 0.0 {
                            prop_assert_eq!(r.classify(q, &TOL).unwrap(), expected);
                        }
                    }
                }
            }
        }

        #[test]
        fn classification_is_invariant_under_rigid_transforms(
            r in region(),
            queries in prop::collection::vec(query(), 16),
            t in rigid(),
        ) {
            let moved = r.transformed(&t, &TOL).unwrap();
            let polygons = dense(&r, 1e-3);
            for p in queries {
                let (_, distance) = reference(&polygons, p);
                let before = r.classify(p, &TOL).unwrap();
                let after = moved.classify(t * p, &TOL).unwrap();
                if distance > 1e-2 || before == Containment::OnBoundary && distance < 1e-7 {
                    prop_assert_eq!(before, after);
                }
            }
        }

        #[test]
        fn signed_area_matches_the_dense_polygon(r in region()) {
            let polygons = dense(&r, 2e-4);
            let loops = r
                .parts()
                .iter()
                .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()));
            for (lp, polygon) in loops.zip(&polygons) {
                let shoelace: f64 = polygon
                    .iter()
                    .zip(polygon.iter().cycle().skip(1))
                    .map(|(a, b)| a.x * b.y - a.y * b.x)
                    .sum::<f64>()
                    * 0.5;
                // The dense polygon is within 2e-4 of the boundary, which is at most ~100 long.
                prop_assert!((lp.signed_area() - shoelace).abs() < 2e-2);
            }
        }
    }

    proptest! {
        #[test]
        fn boundary_points_are_on_the_boundary(
            r in region(),
            pick in any::<prop::sample::Index>(),
            f in 0.0..=1.0_f64,
        ) {
            let curves: Vec<&Curve2> = r
                .parts()
                .iter()
                .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
                .flat_map(Loop2::curves)
                .collect();
            let curve = pick.get(&curves);
            let (a, b) = curve.parameter_range();
            let p = curve.point_at(a + (b - a) * f);
            prop_assert_eq!(r.classify(p, &TOL).unwrap(), Containment::OnBoundary);
        }

        #[test]
        fn regions_are_oriented_and_round_trip_through_serde(r in region()) {
            for p in r.parts() {
                prop_assert!(p.outer().is_counterclockwise());
                prop_assert!(p.holes().iter().all(|h| !h.is_counterclockwise()));
            }
            let json = serde_json::to_string(&r).unwrap();
            prop_assert_eq!(serde_json::from_str::<Region>(&json).unwrap(), r);
        }
    }

    #[test]
    fn areas_are_exact_for_lines_and_arcs() {
        assert_eq!(square(0.0, 0.0, 1.0).signed_area(), 4.0);
        assert_eq!(square(0.0, 0.0, 1.0).reversed().signed_area(), -4.0);
        let c = circle(5.0, -3.0, 2.0);
        assert!((c.signed_area() - 4.0 * PI).abs() < 1e-14);
        assert!((c.reversed().signed_area() + 4.0 * PI).abs() < 1e-14);
        // Two half circles as bulge 1 polyline segments.
        let disc = polygon(&[(1.0, 0.0), (-1.0, 0.0)], 1.0);
        assert!((disc.signed_area() - PI).abs() < 1e-15);
        let s = stadium(Point2::new(1e3, 1e3), 3.0, 2.0);
        assert!((s.signed_area() - (24.0 + 4.0 * PI)).abs() < 1e-9);
        let e = ellipse(Point2::new(1.0, 2.0), Vector2::new(3.0, 4.0), 0.5);
        assert!((e.signed_area() - PI * 5.0 * 2.5).abs() < 1e-12);
        // A nearly straight polyline arc keeps an accurate segment area.
        let sliver = polygon(&[(0.0, 0.0), (1.0, 0.0)], 1e-6);
        let expected = 2.0 * 1e-6 / 3.0 * 0.5 * 2.0; // ≈ (2/3)·chord·sagitta, both arcs
        assert!((sliver.signed_area() / expected - 1.0).abs() < 1e-6);
    }

    #[test]
    fn construction_closes_small_gaps_and_rejects_degenerate_loops() {
        let half = |start: f64, nudge: f64| {
            Curve2::Arc(Arc2::new(Point2::new(nudge, 0.0), 1.0, start, PI).unwrap())
        };
        let joined = Loop2::new(vec![half(0.0, 0.0), half(PI, 1e-7)], &TOL).unwrap();
        assert_eq!(joined.curves().len(), 4);
        let exact = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(
            code(Loop2::new(vec![half(0.0, 0.0), half(PI, 1e-7)], &exact)),
            "geom.region.loop_open"
        );
        assert_eq!(
            code(Loop2::new(vec![half(0.0, 0.0), half(PI, 1e-3)], &TOL)),
            "geom.region.loop_open"
        );
        assert_eq!(code(Loop2::new(vec![], &TOL)), "geom.region.loop_empty");
        let line = crate::primitives::Line2::new(Point2::origin(), Vector2::x()).unwrap();
        assert_eq!(
            code(Loop2::new(vec![Curve2::Line(line)], &TOL)),
            "geom.curve.unbounded"
        );
        let back_and_forth = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::origin(), 0.0),
                PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            code(Loop2::new(vec![Curve2::Polyline(back_and_forth)], &TOL)),
            "geom.region.loop_zero_area"
        );
        let bad = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(
            code(Loop2::new(vec![half(0.0, 0.0)], &bad)),
            "geom.tolerance.invalid"
        );
    }

    #[test]
    fn regions_validate_parts_and_holes() {
        assert_eq!(code(Region::new(vec![])), "geom.region.empty");
        let outside = Region::new(vec![part(
            square(0.0, 0.0, 1.0),
            vec![square(0.5, 0.0, 1.0)],
        )]);
        assert_eq!(code(outside), "geom.region.hole_outside");
        let open = r#"{"parts":[{"outer":{"curves":[{"kind":"segment","start":[0.0,0.0],"end":[1.0,0.0]}]},"holes":[]}]}"#;
        assert!(serde_json::from_str::<Region>(open).is_err());
    }

    #[test]
    fn square_with_hole_classifies_every_case() {
        let r = Region::new(vec![part(
            square(0.0, 0.0, 4.0),
            vec![square(0.0, 0.0, 1.0)],
        )])
        .unwrap();
        let at = |x: f64, y: f64| r.classify(Point2::new(x, y), &TOL).unwrap();
        assert_eq!(at(0.0, 0.0), Containment::Outside);
        assert_eq!(at(2.0, 0.0), Containment::Inside);
        // Rays through vertices and along horizontal edges.
        assert_eq!(at(-2.0, 1.0), Containment::Inside);
        assert_eq!(at(-2.0, -1.0), Containment::Inside);
        assert_eq!(at(-5.0, 4.0), Containment::Outside);
        assert_eq!(at(-5.0, -4.0), Containment::Outside);
        assert_eq!(at(5.0, 0.0), Containment::Outside);
        assert_eq!(at(4.0, 4.0), Containment::OnBoundary);
        assert_eq!(at(1.0, 0.5), Containment::OnBoundary);
        assert_eq!(at(4.0 + 5e-7, 0.0), Containment::OnBoundary);
        assert_eq!(at(4.0 + 2e-6, 0.0), Containment::Outside);
        assert_eq!(
            code(r.classify(Point2::new(f64::NAN, 0.0), &TOL)),
            "geom.input.not_finite"
        );
        let zero = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(
            code(r.classify(Point2::origin(), &zero)),
            "geom.tolerance.invalid"
        );
    }

    #[test]
    fn tangent_rays_and_arc_ends_are_counted_once() {
        let c = Region::new(vec![part(circle(0.0, 0.0, 1.0), vec![])]).unwrap();
        let at = |r: &Region, x: f64, y: f64| r.classify(Point2::new(x, y), &TOL).unwrap();
        assert_eq!(at(&c, -5.0, 1.0), Containment::Outside);
        assert_eq!(at(&c, -5.0, -1.0), Containment::Outside);
        assert_eq!(at(&c, -5.0, 0.0), Containment::Outside);
        assert_eq!(at(&c, 0.0, 0.999), Containment::Inside);
        assert_eq!(at(&c, 0.0, -0.999), Containment::Inside);
        // A stadium whose arc ends sit at the heights of the queries.
        let s = Region::new(vec![part(stadium(Point2::origin(), 2.0, 1.0), vec![])]).unwrap();
        assert_eq!(at(&s, -10.0, 1.0), Containment::Outside);
        assert_eq!(at(&s, -10.0, -1.0), Containment::Outside);
        assert_eq!(at(&s, -10.0, 0.0), Containment::Outside);
        assert_eq!(at(&s, 2.5, 0.0), Containment::Inside);
        assert_eq!(at(&s, 0.0, 1.0), Containment::OnBoundary);
        assert_eq!(at(&s, 3.0, 0.0), Containment::OnBoundary);
    }

    #[test]
    fn islands_in_holes_are_inside() {
        let r = Region::new(vec![
            part(circle(0.0, 0.0, 10.0), vec![circle(0.0, 0.0, 5.0)]),
            part(square(0.0, 0.0, 1.0), vec![]),
        ])
        .unwrap();
        let at = |x: f64| r.classify(Point2::new(x, 0.0), &TOL).unwrap();
        assert_eq!(at(0.0), Containment::Inside);
        assert_eq!(at(3.0), Containment::Outside);
        assert_eq!(at(7.0), Containment::Inside);
        assert_eq!(at(11.0), Containment::Outside);
    }

    #[test]
    fn nearly_straight_arcs_classify_without_projection() {
        // One side bulged by 1e-10: the arc's radius is about 1.25e9.
        let vertices = [
            (0.0, 0.0, 1e-10),
            (1.0, 0.0, 0.0),
            (1.0, 1.0, 0.0),
            (0.0, 1.0, 0.0),
        ]
        .iter()
        .map(|&(x, y, b)| PolylineVertex::new(Point2::new(x, y), b))
        .collect();
        let lp = Loop2::new(
            vec![Curve2::Polyline(Polyline2::new(vertices, true).unwrap())],
            &TOL,
        )
        .unwrap();
        let at = |x: f64, y: f64| lp.classify(Point2::new(x, y), &TOL).unwrap();
        assert_eq!(at(0.5, 0.0), Containment::OnBoundary);
        assert_eq!(at(0.5, 5e-7), Containment::OnBoundary);
        assert_eq!(at(0.5, -5e-7), Containment::OnBoundary);
        assert_eq!(at(0.5, 2e-6), Containment::Inside);
        assert_eq!(at(0.5, -2e-6), Containment::Outside);
        // An arc of radius 1e10 whose sagitta is 5e-11.
        let r = 1e10;
        let arc = Arc2::new(Point2::new(0.0, -r), r, FRAC_PI_2 - 1.0 / r, 2.0 / r).unwrap();
        let (start, end) = (
            Curve2::Arc(arc).start_point().unwrap(),
            Curve2::Arc(arc).end_point().unwrap(),
        );
        let p = |x: f64, y: f64| Point2::new(x, y);
        let lp = Loop2::new(
            vec![
                Curve2::Arc(arc),
                Curve2::Segment(Segment2::new(end, p(end.x, -1.0)).unwrap()),
                Curve2::Segment(Segment2::new(p(end.x, -1.0), p(start.x, -1.0)).unwrap()),
                Curve2::Segment(Segment2::new(p(start.x, -1.0), start).unwrap()),
            ],
            &TOL,
        )
        .unwrap();
        assert_eq!(
            lp.classify(p(0.0, 0.0), &TOL).unwrap(),
            Containment::OnBoundary
        );
        assert_eq!(
            lp.classify(p(0.0, 2e-6), &TOL).unwrap(),
            Containment::Outside
        );
        assert_eq!(
            lp.classify(p(0.0, -2e-6), &TOL).unwrap(),
            Containment::Inside
        );
    }

    #[test]
    fn tolerance_band_and_extreme_magnitudes() {
        let c = circle(0.0, 0.0, 1.0);
        let e = ellipse(Point2::origin(), Vector2::new(1.0, 0.0), 0.5);
        for (lp, top) in [(&c, 1.0), (&e, 0.5)] {
            let at = |y: f64| lp.classify(Point2::new(0.0, y), &TOL).unwrap();
            assert_eq!(at(top - 0.999e-6), Containment::OnBoundary);
            assert_eq!(at(top + 0.999e-6), Containment::OnBoundary);
            assert_eq!(at(top - 1.001e-6), Containment::Inside);
            assert_eq!(at(top + 1.001e-6), Containment::Outside);
        }
        for scale in [1e-150, 1e150] {
            let tol = Tolerance {
                linear: scale * 1e-10,
                ..TOL
            };
            let lp = Loop2::new(
                vec![Curve2::Circle(
                    Circle2::new(Point2::origin(), scale).unwrap(),
                )],
                &tol,
            )
            .unwrap();
            assert!((lp.signed_area() / (PI * scale * scale) - 1.0).abs() < 1e-14);
            let at = |x: f64, y: f64| {
                lp.classify(Point2::new(x * scale, y * scale), &tol)
                    .unwrap()
            };
            assert_eq!(at(0.0, 1.0), Containment::OnBoundary);
            assert_eq!(at(0.0, 0.9999), Containment::Inside);
            assert_eq!(at(-2.0, 1.0), Containment::Outside);
            assert_eq!(at(0.0, 1.5), Containment::Outside);
        }
        // An area that underflows is not a zero area.
        let tiny = Loop2::new(
            vec![Curve2::Circle(
                Circle2::new(Point2::origin(), 1e-300).unwrap(),
            )],
            &TOL,
        );
        assert_eq!(code(tiny), "geom.precision.insufficient");
        let huge = Loop2::new(
            vec![Curve2::Circle(
                Circle2::new(Point2::origin(), 1e300).unwrap(),
            )],
            &TOL,
        );
        assert_eq!(code(huge), "geom.precision.insufficient");
    }

    #[test]
    fn gap_joints_count_once_at_vertex_heights() {
        // Gaps of 0.05 closed by joints, built with a linear tolerance of 0.1.
        let p = |x: f64, y: f64| Point2::new(x, y);
        let seg = |a: Point2, b: Point2| Curve2::Segment(Segment2::new(a, b).unwrap());
        let tol = Tolerance { linear: 0.1, ..TOL };
        let lp = Loop2::new(
            vec![
                seg(p(0.0, 0.0), p(2.0, 0.0)),
                Curve2::Arc(Arc2::new(p(2.0, 1.0), 1.0, -FRAC_PI_2, PI).unwrap()),
                seg(p(2.0, 2.05), p(0.0, 2.0)),
                seg(p(0.0, 2.0), p(0.0, 0.05)),
            ],
            &tol,
        )
        .unwrap();
        let polygon: Vec<Point2> = lp
            .curves()
            .iter()
            .flat_map(|c| c.flatten(1e-5, 0.01, 10_000_000).unwrap())
            .collect();
        let fine = Tolerance {
            linear: 1e-9,
            ..TOL
        };
        for lp in [lp.clone(), lp.reversed()] {
            for y in [0.0, 0.05, 1.0, 2.0, 2.025, 2.05, -1e-8, 2.05 + 1e-8] {
                for k in -10..40 {
                    let q = p(f64::from(k) * 0.1 + 0.0123, y);
                    let (inside, distance) = reference(std::slice::from_ref(&polygon), q);
                    if distance > 1e-4 {
                        let expected = if inside {
                            Containment::Inside
                        } else {
                            Containment::Outside
                        };
                        assert_eq!(lp.classify(q, &fine).unwrap(), expected, "{q:?}");
                    }
                }
            }
        }
    }
}
