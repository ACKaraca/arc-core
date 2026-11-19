//! The snap points of single curves and of pairs of curves (the computations behind
//! [`super::snap_candidates`]). Every function offers its points to the sink, which keeps those
//! within the aperture of the cursor and of an enabled kind.

use std::f64::consts::FRAC_PI_2;

use arc_base::math::{Point2, Vector2};
use arc_base::{Result, Tolerance};
use arc_geom::closest::{CurveProject, FootPoint};
use arc_geom::eval::CurveEval;
use arc_geom::{Circle2, Curve2, Intersection, Line2, Loop2};

use super::{Entry, MAX_INTERSECTION_PAIRS, Sink, SnapKind, SnapQuery, SnapSource, limit_error};

/// The samples of the parameter range in the search for tangent points.
const TANGENT_SAMPLES: usize = 64;
/// The bisection steps that refine a tangent root.
const BISECTIONS: u32 = 64;

/// The ends of an open curve with a bounded range. Closed curves have no ends.
pub(super) fn endpoints(sink: &mut Sink<'_>, curve: &Curve2, source: &SnapSource) {
    if !sink.wants(SnapKind::Endpoint) || curve.is_closed() {
        return;
    }
    for point in [curve.start_point(), curve.end_point()]
        .into_iter()
        .flatten()
    {
        sink.offer(
            SnapKind::Endpoint,
            point,
            false,
            std::slice::from_ref(source),
        );
    }
}

/// The parameter midpoint of a segment, arc or elliptical arc. Splines have none.
pub(super) fn midpoint(sink: &mut Sink<'_>, curve: &Curve2, source: &SnapSource) {
    if !sink.wants(SnapKind::Midpoint)
        || curve.is_closed()
        || !matches!(
            curve,
            Curve2::Segment(_) | Curve2::Arc(_) | Curve2::EllipticalArc(_)
        )
    {
        return;
    }
    let (start, end) = curve.parameter_range();
    if !(start.is_finite() && end.is_finite()) {
        return;
    }
    sink.offer(
        SnapKind::Midpoint,
        curve.point_at((start + end) * 0.5),
        false,
        std::slice::from_ref(source),
    );
}

/// The center of a circle, arc, ellipse or elliptical arc.
pub(super) fn centers(sink: &mut Sink<'_>, curve: &Curve2, source: &SnapSource) {
    let center = match curve {
        Curve2::Circle(circle) => circle.center(),
        Curve2::Arc(arc) => arc.center(),
        Curve2::Ellipse(ellipse) => ellipse.center(),
        Curve2::EllipticalArc(arc) => arc.ellipse().center(),
        _ => return,
    };
    sink.offer(
        SnapKind::Center,
        center,
        false,
        std::slice::from_ref(source),
    );
}

/// The area centroid of a closed polyline or closed spline. Other curves have none.
pub(super) fn geometric_center(sink: &mut Sink<'_>, curve: &Curve2, source: &SnapSource) {
    let tolerance = sink.query.tolerance;
    let curves: Vec<Curve2> = match curve {
        Curve2::Polyline(polyline) => polyline
            .segments()
            .map(|segment| match segment {
                arc_geom::PolylineSegment::Line(line) => Curve2::Segment(line),
                arc_geom::PolylineSegment::Arc(arc) => Curve2::Arc(arc),
            })
            .collect(),
        Curve2::Nurbs(_) => vec![curve.clone()],
        _ => return,
    };
    let Ok(boundary) = Loop2::new(curves, &tolerance) else {
        return;
    };
    let Ok(measure) = boundary.measure(&tolerance) else {
        return;
    };
    sink.offer(
        SnapKind::GeometricCenter,
        measure.centroid,
        false,
        std::slice::from_ref(source),
    );
}

/// The four quadrant points of a circle, arc, ellipse or elliptical arc along the UCS axes.
///
/// An ellipse has quadrant points only when its axes are parallel or perpendicular to `ucs_x`
/// within the angular tolerance; the points are then the ends of its axes.
pub(super) fn quadrants(
    sink: &mut Sink<'_>,
    curve: &Curve2,
    source: &SnapSource,
    ucs_x: Vector2,
    tolerance: &Tolerance,
) {
    if !sink.wants(SnapKind::Quadrant) {
        return;
    }
    let base = ucs_x.y.atan2(ucs_x.x);
    let points: Vec<Point2> = match curve {
        Curve2::Circle(circle) => circle_quadrants(circle, base),
        Curve2::Arc(arc) => match Circle2::new(arc.center(), arc.radius()) {
            Ok(circle) => circle_quadrants(&circle, base),
            Err(_) => return,
        },
        Curve2::Ellipse(ellipse) => {
            if !aligned(ellipse.major_axis(), base, tolerance) {
                return;
            }
            (0..4)
                .map(|k| ellipse.point_at(f64::from(k) * FRAC_PI_2))
                .collect()
        }
        Curve2::EllipticalArc(arc) => {
            if !aligned(arc.ellipse().major_axis(), base, tolerance) {
                return;
            }
            (0..4)
                .map(|k| arc.ellipse().point_at(f64::from(k) * FRAC_PI_2))
                .collect()
        }
        _ => return,
    };
    for point in points {
        // An arc or elliptical arc keeps only the quadrant points it passes through.
        if matches!(curve, Curve2::Circle(_) | Curve2::Ellipse(_))
            || on_curve(curve, point, tolerance)
        {
            sink.offer(
                SnapKind::Quadrant,
                point,
                false,
                std::slice::from_ref(source),
            );
        }
    }
}

fn circle_quadrants(circle: &Circle2, base: f64) -> Vec<Point2> {
    (0..4)
        .map(|k| {
            let angle = base + f64::from(k) * FRAC_PI_2;
            circle.center() + Vector2::new(angle.cos(), angle.sin()) * circle.radius()
        })
        .collect()
}

/// Whether a direction is parallel or perpendicular to the axis at angle `base`.
fn aligned(direction: Vector2, base: f64, tolerance: &Tolerance) -> bool {
    let turn = (direction.y.atan2(direction.x) - base).rem_euclid(FRAC_PI_2);
    turn <= tolerance.angular || FRAC_PI_2 - turn <= tolerance.angular
}

fn on_curve(curve: &Curve2, point: Point2, tolerance: &Tolerance) -> bool {
    curve
        .closest_point(point, tolerance)
        .is_ok_and(|hit| hit.distance <= tolerance.linear)
}

/// The closest point of the curve to the cursor.
pub(super) fn nearest(
    sink: &mut Sink<'_>,
    curve: &Curve2,
    source: &SnapSource,
    cursor: Point2,
    tolerance: &Tolerance,
) {
    if !sink.wants(SnapKind::Nearest) {
        return;
    }
    if let Ok(hit) = curve.closest_point(cursor, tolerance) {
        sink.offer(
            SnapKind::Nearest,
            hit.point,
            false,
            std::slice::from_ref(source),
        );
    }
}

/// The perpendicular feet of `from` on the curve. A query at the center of a circle or arc has
/// no single foot and yields nothing.
pub(super) fn perpendicular(
    sink: &mut Sink<'_>,
    curve: &Curve2,
    source: &SnapSource,
    from: Point2,
    tolerance: &Tolerance,
) {
    if !sink.wants(SnapKind::Perpendicular) {
        return;
    }
    let Ok(feet) = curve.foot_points(from, tolerance) else {
        return;
    };
    for foot in feet {
        if let FootPoint::Point(hit) = foot {
            sink.offer(
                SnapKind::Perpendicular,
                hit.point,
                false,
                std::slice::from_ref(source),
            );
        }
    }
}

/// The points of a smooth curve where a line from `from` touches it: the roots of
/// `cross(C'(t), C(t) - from)`, found by sign changes over sampled parameters and bisection.
///
/// Roots where the function touches zero without changing sign are not found, so a tangent
/// point where the curve is touched twice at one parameter is missed.
pub(super) fn tangent(
    sink: &mut Sink<'_>,
    curve: &Curve2,
    source: &SnapSource,
    from: Point2,
    tolerance: &Tolerance,
) {
    if !sink.wants(SnapKind::Tangent)
        || !matches!(
            curve,
            Curve2::Circle(_)
                | Curve2::Arc(_)
                | Curve2::Ellipse(_)
                | Curve2::EllipticalArc(_)
                | Curve2::Nurbs(_)
        )
    {
        return;
    }
    let (start, end) = curve.parameter_range();
    if !(start.is_finite() && end.is_finite()) || end <= start {
        return;
    }
    let value = |t: f64| {
        let direction = curve.derivative(t);
        let offset = curve.point_at(t) - from;
        direction.x * offset.y - direction.y * offset.x
    };
    let step = (end - start) / TANGENT_SAMPLES as f64;
    let mut roots = Vec::new();
    let mut zeros = 0usize;
    let mut previous_t = start;
    let mut previous = value(start);
    for i in 1..=TANGENT_SAMPLES {
        let t = if i == TANGENT_SAMPLES {
            end
        } else {
            start + step * i as f64
        };
        let current = value(t);
        if previous == 0.0 {
            roots.push(previous_t);
            zeros += 1;
        } else if current != 0.0 && previous.signum() != current.signum() {
            roots.push(bisect(&value, previous_t, t));
        }
        previous_t = t;
        previous = current;
    }
    if previous == 0.0 {
        roots.push(end);
        zeros += 1;
    }
    if zeros > TANGENT_SAMPLES {
        return;
    }
    for t in roots {
        let point = curve.point_at(t);
        if (point - from).norm() > tolerance.linear {
            sink.offer(
                SnapKind::Tangent,
                point,
                false,
                std::slice::from_ref(source),
            );
        }
    }
}

/// A root of `f` between `lo` and `hi`, where `f` changes sign.
fn bisect(f: &impl Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> f64 {
    let sign = f(lo).signum();
    for _ in 0..BISECTIONS {
        let mid = 0.5 * (lo + hi);
        if mid == lo || mid == hi {
            break;
        }
        let value = f(mid);
        if value == 0.0 {
            return mid;
        }
        if value.signum() == sign {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// The parallel through `from` in the direction of a straight curve, offered at the projection of
/// the cursor on that path when the cursor lies within the aperture of the path. Every straight
/// entry of the space contributes its direction; no hover state is kept.
pub(super) fn parallels(sink: &mut Sink<'_>, entries: &[Entry], from: Point2) {
    if !sink.wants(SnapKind::Parallel) {
        return;
    }
    let cursor = sink.query.cursor;
    for entry in entries {
        if !matches!(
            entry.curve,
            Curve2::Segment(_) | Curve2::Line(_) | Curve2::Ray(_)
        ) {
            continue;
        }
        let direction = entry.curve.tangent(0.0);
        if direction.norm() == 0.0 {
            continue;
        }
        let point = from + direction * (cursor - from).dot(&direction);
        sink.offer(
            SnapKind::Parallel,
            point,
            false,
            std::slice::from_ref(&entry.source),
        );
    }
}

/// The projection of the cursor on the extension path of an end of a segment, arc or elliptical
/// arc, when the cursor hovers that end. The path leaves the curve tangentially at the end.
pub(super) fn extension(sink: &mut Sink<'_>, curve: &Curve2, source: &SnapSource) {
    if !sink.wants(SnapKind::Extension)
        || curve.is_closed()
        || !matches!(
            curve,
            Curve2::Segment(_) | Curve2::Arc(_) | Curve2::EllipticalArc(_)
        )
    {
        return;
    }
    let (start, end) = curve.parameter_range();
    let cursor = sink.query.cursor;
    let aperture = sink.query.aperture;
    let linear = sink.query.tolerance.linear;
    let ends = [
        (curve.start_point(), -curve.tangent(start)),
        (curve.end_point(), curve.tangent(end)),
    ];
    for (end_point, outward) in ends {
        let Some(origin) = end_point else {
            continue;
        };
        if (cursor - origin).norm() > aperture || outward.norm() == 0.0 {
            continue;
        }
        let along = (cursor - origin).dot(&outward);
        if along <= linear {
            continue;
        }
        sink.offer(
            SnapKind::Extension,
            origin + outward * along,
            false,
            std::slice::from_ref(source),
        );
    }
}

/// The natural extension of a curve as a curve without ends, or `None` for a spline.
fn natural(curve: &Curve2) -> Option<Curve2> {
    match curve {
        Curve2::Segment(segment) => Line2::new(segment.start(), segment.end() - segment.start())
            .ok()
            .map(Curve2::Line),
        Curve2::Arc(arc) => Circle2::new(arc.center(), arc.radius())
            .ok()
            .map(Curve2::Circle),
        Curve2::EllipticalArc(arc) => Some(Curve2::Ellipse(*arc.ellipse())),
        Curve2::Ray(ray) => Line2::new(ray.origin(), ray.direction())
            .ok()
            .map(Curve2::Line),
        Curve2::Line(_) | Curve2::Circle(_) | Curve2::Ellipse(_) => Some(curve.clone()),
        Curve2::Polyline(_) | Curve2::Nurbs(_) => None,
    }
}

/// Whether a curve's natural extension differs from the curve.
fn extends(curve: &Curve2) -> bool {
    matches!(
        curve,
        Curve2::Segment(_) | Curve2::Arc(_) | Curve2::EllipticalArc(_)
    )
}

fn pair_limit(count: usize) -> Result<()> {
    let pairs = count.saturating_mul(count.saturating_sub(1)) / 2;
    if pairs > MAX_INTERSECTION_PAIRS {
        Err(limit_error(
            u64::try_from(MAX_INTERSECTION_PAIRS).unwrap_or(u64::MAX),
        ))
    } else {
        Ok(())
    }
}

/// The actual intersections of the curves near the cursor, from different entities. Splines
/// and overlaps yield nothing.
pub(super) fn intersections(
    sink: &mut Sink<'_>,
    pieces: &[&Entry],
    query: &SnapQuery,
) -> Result<()> {
    pair_limit(pieces.len())?;
    for (i, a) in pieces.iter().enumerate() {
        for b in &pieces[i + 1..] {
            if a.source == b.source || is_spline(&a.curve) || is_spline(&b.curve) {
                continue;
            }
            let Ok(hits) = arc_geom::intersect_with_limits(
                &a.curve,
                &b.curve,
                &query.tolerance,
                &query.limits,
            ) else {
                continue;
            };
            for hit in hits {
                if let Intersection::Point { point, .. } = hit {
                    sink.offer(
                        SnapKind::Intersection,
                        point,
                        false,
                        &[a.source.clone(), b.source.clone()],
                    );
                }
            }
        }
    }
    Ok(())
}

/// The intersections of the natural extensions of the curves, where the extensions pass near
/// the cursor and the point is not on both curves: an extended intersection.
pub(super) fn extended_intersections(
    sink: &mut Sink<'_>,
    entries: &[Entry],
    query: &SnapQuery,
) -> Result<()> {
    let cursor = query.cursor;
    let mut near: Vec<(&Entry, Curve2)> = Vec::new();
    for entry in entries {
        let Some(extension) = natural(&entry.curve) else {
            continue;
        };
        let hovered = extension
            .closest_point(cursor, &query.tolerance)
            .is_ok_and(|hit| hit.distance <= query.aperture);
        if hovered {
            near.push((entry, extension));
        }
    }
    pair_limit(near.len())?;
    for (i, (a, ext_a)) in near.iter().enumerate() {
        for (b, ext_b) in &near[i + 1..] {
            if a.source == b.source || (!extends(&a.curve) && !extends(&b.curve)) {
                continue;
            }
            let Ok(hits) =
                arc_geom::intersect_with_limits(ext_a, ext_b, &query.tolerance, &query.limits)
            else {
                continue;
            };
            for hit in hits {
                let Intersection::Point { point, .. } = hit else {
                    continue;
                };
                let on_both = on_curve(&a.curve, point, &query.tolerance)
                    && on_curve(&b.curve, point, &query.tolerance);
                if !on_both {
                    sink.offer(
                        SnapKind::Intersection,
                        point,
                        true,
                        &[a.source.clone(), b.source.clone()],
                    );
                }
            }
        }
    }
    Ok(())
}

fn is_spline(curve: &Curve2) -> bool {
    matches!(curve, Curve2::Nurbs(_))
}
