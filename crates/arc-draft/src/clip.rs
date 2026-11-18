//! Exact pieces of curves: the piece of a curve between two parameters, and the pieces of a
//! curve inside or outside a closed boundary. Display lists use them for dashes, viewport clips
//! and wipeout masks.

use arc_base::math::Point2;
use arc_base::{Result, Tolerance};
use arc_geom::intersect::{Intersection, intersect};
use arc_geom::region::{Containment, Loop2};
use arc_geom::{Arc2, Curve2, EllipticalArc2, Polyline2, PolylineVertex, Ray2, Segment2};

/// A drawn piece: a point (a dot, or a piece of length zero) or an exact curve.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    /// A point.
    Point(Point2),
    /// A curve.
    Curve(Curve2),
}

fn sub_polyline(polyline: &Polyline2, a: f64, b: f64) -> Result<Curve2> {
    let vertices = polyline.vertices();
    let count = polyline.segment_count();
    let last = count.saturating_sub(1);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a and b lie in [0, n]; floor and ceil are clamped to segment indices"
    )]
    let (first, end) = (
        (a.floor().max(0.0) as usize).min(last),
        ((b.ceil() - 1.0).max(0.0) as usize).min(last),
    );
    let mut out = Vec::with_capacity(end.saturating_sub(first) + 2);
    for index in first..=end {
        let Some(v) = vertices.get(index) else { break };
        #[expect(clippy::cast_precision_loss, reason = "segment indices are small")]
        let base = index as f64;
        let s = if index == first { a - base } else { 0.0 };
        let e = if index == end { b - base } else { 1.0 };
        let point = if s == 0.0 {
            v.point
        } else {
            polyline.point_at(a)
        };
        let width = |f: f64| (v.end_width - v.start_width).mul_add(f, v.start_width);
        out.push(PolylineVertex {
            point,
            bulge: ((e - s) * v.bulge.atan()).tan(),
            start_width: width(s),
            end_width: width(e),
        });
    }
    #[expect(clippy::cast_precision_loss, reason = "segment indices are small")]
    let at_vertex = b == (end + 1) as f64;
    let last_point = match vertices.get((end + 1) % vertices.len().max(1)) {
        Some(v) if at_vertex => v.point,
        _ => polyline.point_at(b),
    };
    out.push(PolylineVertex::new(last_point, 0.0));
    Ok(Curve2::Polyline(Polyline2::new(out, false)?))
}

/// The piece of `curve` between the parameters `a <= b` (either may be infinite for rays and
/// lines): a point when `a == b`, the curve itself for its whole range, and otherwise an exact
/// curve of a kind that holds the piece: a segment, ray, arc, elliptical arc or open polyline.
///
/// # Errors
///
/// The construction errors of `arc-geom` for a piece too small to represent (a zero length or
/// sweep); callers drop such pieces.
pub fn piece(curve: &Curve2, a: f64, b: f64) -> Result<Piece> {
    if curve.parameter_range() == (a, b) {
        return Ok(Piece::Curve(curve.clone()));
    }
    if a == b {
        return Ok(Piece::Point(curve.point_at(a)));
    }
    let segment = |p: Point2, q: Point2| Segment2::new(p, q).map(Curve2::Segment);
    let ray = |o: Point2, d| Ray2::new(o, d).map(Curve2::Ray);
    let c = match curve {
        Curve2::Segment(_) => segment(curve.point_at(a), curve.point_at(b))?,
        Curve2::Ray(r) if b.is_infinite() => ray(curve.point_at(a), r.direction())?,
        Curve2::Line(l) if a.is_infinite() && b.is_infinite() => Curve2::Line(*l),
        Curve2::Line(l) if a.is_infinite() => ray(curve.point_at(b), -l.direction())?,
        Curve2::Line(l) if b.is_infinite() => ray(curve.point_at(a), l.direction())?,
        Curve2::Ray(_) | Curve2::Line(_) => segment(curve.point_at(a), curve.point_at(b))?,
        Curve2::Circle(c) => Curve2::Arc(Arc2::new(c.center(), c.radius(), a, b - a)?),
        Curve2::Arc(arc) => {
            let sign = arc.sweep().signum();
            Curve2::Arc(Arc2::new(
                arc.center(),
                arc.radius(),
                sign.mul_add(a, arc.start_angle()),
                sign * (b - a),
            )?)
        }
        Curve2::Ellipse(e) => Curve2::EllipticalArc(EllipticalArc2::new(*e, a, b - a)?),
        Curve2::EllipticalArc(arc) => {
            let sign = arc.sweep().signum();
            Curve2::EllipticalArc(EllipticalArc2::new(
                *arc.ellipse(),
                sign.mul_add(a, arc.start_parameter()),
                sign * (b - a),
            )?)
        }
        Curve2::Polyline(p) => sub_polyline(p, a, b)?,
    };
    Ok(Piece::Curve(c))
}

/// The pieces of `curve` inside `boundary` (`inside` set) or outside it, stretches on the
/// boundary included in both. The curve is split where it meets the boundary and each stretch
/// is classified by its middle point; consecutive kept stretches join. A curve kept whole is
/// returned unchanged.
///
/// # Errors
///
/// The errors of [`intersect`] and [`Loop2::classify`] (`geom.precision.insufficient`,
/// `geom.intersect.too_many_results`).
pub fn cut(
    curve: &Curve2,
    boundary: &Loop2,
    inside: bool,
    tolerance: &Tolerance,
) -> Result<Vec<Piece>> {
    let (lo, hi) = curve.parameter_range();
    if let Some(bounds) = curve.bounding_box()
        && !bounds
            .expanded(tolerance.linear)
            .intersects(&boundary.bounding_box())
    {
        return Ok(if inside {
            Vec::new()
        } else {
            vec![Piece::Curve(curve.clone())]
        });
    }
    let mut cuts = vec![lo, hi];
    for edge in boundary.curves() {
        for hit in intersect(curve, edge, tolerance)? {
            match hit {
                Intersection::Point { a, .. } => cuts.push(a),
                Intersection::Overlap { a, .. } => cuts.extend(a),
            }
        }
    }
    cuts.retain(|t| !t.is_nan());
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut kept: Vec<(f64, f64)> = Vec::new();
    for pair in cuts.windows(2) {
        let &[a, b] = pair else { continue };
        let side = if a.is_finite() && b.is_finite() {
            boundary.classify(curve.point_at(0.5 * (a + b)), tolerance)?
        } else {
            // An unbounded stretch without a crossing lies outside a bounded boundary.
            Containment::Outside
        };
        let keep = match side {
            Containment::OnBoundary => true,
            Containment::Inside => inside,
            Containment::Outside => !inside,
        };
        if !keep {
            continue;
        }
        match kept.last_mut() {
            Some(last) if last.1 == a => last.1 = b,
            _ => kept.push((a, b)),
        }
    }
    let mut out = Vec::with_capacity(kept.len());
    for (a, b) in kept {
        // A piece too small to construct has no extent to draw.
        if let Ok(p) = piece(curve, a, b) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Whether `point` is kept by a cut with `inside` against `boundary`.
///
/// # Errors
///
/// The errors of [`Loop2::classify`].
pub fn keeps(point: Point2, boundary: &Loop2, inside: bool, tolerance: &Tolerance) -> Result<bool> {
    Ok(match boundary.classify(point, tolerance)? {
        Containment::OnBoundary => true,
        Containment::Inside => inside,
        Containment::Outside => !inside,
    })
}

#[cfg(test)]
mod tests {
    use arc_base::math::Vector2;
    use arc_geom::{Circle2, Ellipse2};

    use super::*;

    fn square(size: f64) -> Loop2 {
        let v = |x, y| PolylineVertex::new(Point2::new(x, y), 0.0);
        let p = Polyline2::new(
            vec![v(0.0, 0.0), v(size, 0.0), v(size, size), v(0.0, size)],
            true,
        )
        .unwrap();
        Loop2::new(vec![Curve2::Polyline(p)], &Tolerance::DEFAULT).unwrap()
    }

    fn ends(p: &Piece) -> (Point2, Point2) {
        match p {
            Piece::Curve(c) => (c.start_point().unwrap(), c.end_point().unwrap()),
            Piece::Point(p) => (*p, *p),
        }
    }

    fn close(a: Point2, b: Point2) -> bool {
        (a - b).norm() < 1e-9
    }

    #[test]
    fn pieces_of_every_kind_run_between_the_points_at_their_parameters() {
        let o = Point2::new(1.0, 2.0);
        let ellipse = Ellipse2::new(o, Vector2::new(3.0, 1.0), 0.5).unwrap();
        let v = |x, y, b| PolylineVertex {
            point: Point2::new(x, y),
            bulge: b,
            start_width: 0.2,
            end_width: 0.4,
        };
        let curves = [
            Curve2::Segment(Segment2::new(o, Point2::new(5.0, 3.0)).unwrap()),
            Curve2::Circle(Circle2::new(o, 2.0).unwrap()),
            Curve2::Arc(Arc2::new(o, 2.0, 1.0, -2.5).unwrap()),
            Curve2::Ellipse(ellipse),
            Curve2::EllipticalArc(EllipticalArc2::new(ellipse, 0.5, -2.0).unwrap()),
            Curve2::Polyline(
                Polyline2::new(
                    vec![v(0.0, 0.0, 0.5), v(3.0, 0.0, -0.3), v(3.0, 2.0, 0.0)],
                    true,
                )
                .unwrap(),
            ),
        ];
        for curve in &curves {
            let (lo, hi) = curve.parameter_range();
            let (a, b) = (lo + 0.3 * (hi - lo), lo + 0.8 * (hi - lo));
            let p = piece(curve, a, b).unwrap();
            let (s, e) = ends(&p);
            assert!(close(s, curve.point_at(a)), "{curve:?}");
            assert!(close(e, curve.point_at(b)), "{curve:?}");
            if let Piece::Curve(c) = &p {
                let m = c.point_at(0.5 * (c.parameter_range().0 + c.parameter_range().1));
                // The middle of the piece lies on the curve's stretch.
                let (mid_lo, mid_hi) = (a + 0.05 * (b - a), b - 0.05 * (b - a));
                let near = (0..=100).any(|i| {
                    let t = mid_lo + (mid_hi - mid_lo) * f64::from(i) / 100.0;
                    (curve.point_at(t) - m).norm() < 0.05
                });
                assert!(near, "{curve:?}");
            }
        }
        assert!(matches!(
            piece(&curves[0], 0.5, 0.5).unwrap(),
            Piece::Point(_)
        ));
    }

    #[test]
    fn cutting_keeps_the_inside_or_the_outside() {
        let line =
            Curve2::Segment(Segment2::new(Point2::new(-1.0, 1.0), Point2::new(3.0, 1.0)).unwrap());
        let tol = Tolerance::DEFAULT;
        let inside = cut(&line, &square(2.0), true, &tol).unwrap();
        assert_eq!(inside.len(), 1);
        let (s, e) = ends(&inside[0]);
        assert!(close(s, Point2::new(0.0, 1.0)) && close(e, Point2::new(2.0, 1.0)));
        let outside = cut(&line, &square(2.0), false, &tol).unwrap();
        assert_eq!(outside.len(), 2);
        let ray = Curve2::Ray(Ray2::new(Point2::new(1.0, 1.0), Vector2::new(1.0, 0.0)).unwrap());
        let out = cut(&ray, &square(2.0), false, &tol).unwrap();
        assert!(matches!(&out[..], [Piece::Curve(Curve2::Ray(_))]));
        let far = Curve2::Circle(Circle2::new(Point2::new(50.0, 50.0), 1.0).unwrap());
        assert!(cut(&far, &square(2.0), true, &tol).unwrap().is_empty());
        assert_eq!(
            cut(&far, &square(2.0), false, &tol).unwrap(),
            vec![Piece::Curve(far)]
        );
    }
}
