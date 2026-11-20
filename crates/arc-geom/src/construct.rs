//! Analytic constructions of circles, arcs, circles tangent to two curves, regular polygons and
//! ellipses (CORE-GEOM-034).
//!
//! Each function checks its inputs, then returns exact primitives ([`Circle2`], [`Arc2`],
//! [`Ellipse2`]) or points. Distances are compared with `Tolerance::linear` and angles with
//! `Tolerance::angular`. Nothing here changes a document. Every construction is closed form; the
//! vertex lists are the only output whose size grows, and it is bounded by [`MAX_POLYGON_SIDES`].
//!
//! The arc constructions take the orientation from the points or from the sign of the radius, so an
//! arc never has a direction the caller did not give. Degenerate inputs are refused with a typed
//! error; a construction that has no solution returns an empty list (tangent circles) or the
//! error `geom.construct.radius_too_small` (an arc of a radius through two points).

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::primitives::{Arc2, Circle2, Ellipse2};

/// The most sides a regular polygon construction builds in one call.
pub const MAX_POLYGON_SIDES: usize = 1 << 16;

/// The side of a directed edge on which a polygon lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeSide {
    /// Left of the direction from the first point of the edge to the second.
    Left,
    /// Right of that direction.
    Right,
}

/// A circle tangent to two curves, with the tangent point on each.
#[derive(Debug, Clone, PartialEq)]
pub struct TangentCircle {
    /// The circle.
    pub circle: Circle2,
    /// The tangent point on the first curve.
    pub first: Point2,
    /// The tangent point on the second curve.
    pub second: Point2,
}

/// The circle through three points.
///
/// The center is the circumcenter of the triangle. The points must be pairwise distinct and not
/// collinear: a pair closer than `tolerance.linear` is refused with
/// `geom.construct.coincident_points`, and a third point within `tolerance.linear` of the line
/// through the other two is refused with `geom.construct.collinear_points`.
///
/// # Errors
///
/// `geom.tolerance.invalid` for a linear tolerance that is not finite and positive;
/// `geom.input.not_finite` for a non-finite point; `geom.construct.coincident_points` and
/// `geom.construct.collinear_points` as described above.
pub fn circle_through_points(
    first: Point2,
    second: Point2,
    third: Point2,
    tolerance: &Tolerance,
) -> Result<Circle2> {
    ensure_linear(tolerance)?;
    ensure_points(&[first, second, third])?;
    ensure_distinct(
        &[(first, second), (second, third), (first, third)],
        tolerance,
    )?;
    circumcircle(first, second, third, tolerance)
}

/// The circle whose diameter is the segment from `first` to `second`.
///
/// # Errors
///
/// `geom.tolerance.invalid`, `geom.input.not_finite` and `geom.construct.coincident_points` for
/// two points closer than `tolerance.linear`.
pub fn circle_on_diameter(first: Point2, second: Point2, tolerance: &Tolerance) -> Result<Circle2> {
    ensure_linear(tolerance)?;
    ensure_points(&[first, second])?;
    ensure_distinct(&[(first, second)], tolerance)?;
    let half = (second - first) * 0.5;
    Circle2::new(first + half, half.norm())
}

/// The arc from `start` to `end` that passes through `middle`.
///
/// The arc lies on the circle through the three points. Its sweep is counterclockwise when the
/// middle point is reached before the end point going counterclockwise from the start, and
/// clockwise otherwise.
///
/// # Errors
///
/// As [`circle_through_points`].
pub fn arc_through_points(
    start: Point2,
    middle: Point2,
    end: Point2,
    tolerance: &Tolerance,
) -> Result<Arc2> {
    ensure_linear(tolerance)?;
    ensure_points(&[start, middle, end])?;
    ensure_distinct(&[(start, middle), (middle, end), (start, end)], tolerance)?;
    let circle = circumcircle(start, middle, end, tolerance)?;
    let center = circle.center();
    let angle = |p: Point2| angle_of(p - center);
    let (a_start, a_middle, a_end) = (angle(start), angle(middle), angle(end));
    let to_end = (a_end - a_start).rem_euclid(TAU);
    let to_middle = (a_middle - a_start).rem_euclid(TAU);
    let sweep = if to_middle < to_end {
        to_end
    } else {
        to_end - TAU
    };
    Arc2::new(center, circle.radius(), a_start, sweep)
}

/// The arc with its center `center` and its start point `start`, swept by `sweep` radians.
///
/// The radius is the distance from `start` to `center`. A positive sweep runs counterclockwise,
/// a negative one clockwise.
///
/// # Errors
///
/// `geom.construct.coincident_points` when `start` and `center` are closer than
/// `tolerance.linear`; `geom.sweep.invalid` for a sweep that is zero or larger than a full turn in
/// magnitude; `geom.input.not_finite` for a non-finite value.
pub fn arc_from_start_center(
    start: Point2,
    center: Point2,
    sweep: f64,
    tolerance: &Tolerance,
) -> Result<Arc2> {
    ensure_linear(tolerance)?;
    ensure_points(&[start, center])?;
    let offset = start - center;
    let radius = offset.norm();
    if radius <= tolerance.linear {
        return Err(coincident());
    }
    Arc2::new(center, radius, angle_of(offset), sweep)
}

/// The minor arc from `start` to `end` with radius `radius`.
///
/// The sign of `radius` selects the side of the center: a positive radius puts the center on the
/// left of the direction from `start` to `end`, so the arc runs counterclockwise; a negative radius
/// puts it on the right, so the arc runs clockwise. The arc is the shorter one, with a sweep of
/// magnitude at most π. A chord of length exactly twice the radius gives a semicircle, on which the
/// sign still selects the side.
///
/// # Errors
///
/// `geom.radius.not_positive` for a zero radius; `geom.construct.coincident_points` when the ends
/// are closer than `tolerance.linear`; `geom.construct.radius_too_small` when the radius magnitude
/// is less than half the chord (no arc of that radius joins the ends); `geom.input.not_finite` for
/// a non-finite value.
pub fn arc_from_ends_and_radius(
    start: Point2,
    end: Point2,
    radius: f64,
    tolerance: &Tolerance,
) -> Result<Arc2> {
    ensure_linear(tolerance)?;
    ensure_points(&[start, end])?;
    ensure_finite(&[radius])?;
    if radius == 0.0 {
        return Err(Error::new(codes::RADIUS_NOT_POSITIVE).with_detail("radius", radius));
    }
    let chord = end - start;
    let length = chord.norm();
    if length <= tolerance.linear {
        return Err(coincident());
    }
    let magnitude = radius.abs();
    let half = length * 0.5;
    if half > magnitude + tolerance.linear {
        return Err(Error::new(codes::CONSTRUCT_RADIUS_TOO_SMALL)
            .with_detail("radius", magnitude)
            .with_detail("half_chord", half));
    }
    let height = (magnitude * magnitude - half * half).max(0.0).sqrt();
    let left = Vector2::new(-chord.y, chord.x) / length;
    let sign = if radius > 0.0 { 1.0 } else { -1.0 };
    let center = start + chord * 0.5 + left * (sign * height);
    let sweep = 2.0 * (half / magnitude).min(1.0).asin();
    Arc2::new(center, magnitude, angle_of(start - center), sign * sweep)
}

/// The circles of radius `radius` tangent to both `first` and `second`, each a line or a circle.
///
/// A circle is tangent to a line when its center is at distance `radius` from the line, on either
/// side, and to a circle when the distance between the centers is the sum of the radii (outside)
/// or the difference of the radii (inside). Every solution is returned, sorted by the center's x
/// and then y coordinate. A solution that would coincide with the given circle (the inside case of
/// equal radii, whose center is the circle's center) is not a tangency and is omitted.
///
/// Two lines that are parallel and distinct give no solution unless their distance is `2 · radius`,
/// which gives a family of solutions. The inputs that give a family of solutions, namely two
/// coincident lines, parallel lines at distance `2 · radius`, and two concentric circles whose
/// radii allow a common distance, are refused.
///
/// # Errors
///
/// `geom.tolerance.invalid` and `geom.input.not_finite` as for the other constructions;
/// `geom.radius.not_positive` for a radius that is not positive; `geom.curve.kind_unsupported`
/// for a curve other than a line (infinite) or a circle; `geom.construct.degenerate` for the
/// families of solutions described above.
pub fn tangent_circles(
    first: &Curve2,
    second: &Curve2,
    radius: f64,
    tolerance: &Tolerance,
) -> Result<Vec<TangentCircle>> {
    ensure_linear(tolerance)?;
    ensure_finite(&[radius])?;
    if radius <= 0.0 {
        return Err(Error::new(codes::RADIUS_NOT_POSITIVE).with_detail("radius", radius));
    }
    let (a, b) = (Support::of(first)?, Support::of(second)?);
    let (loci_a, loci_b) = (a.loci(radius, tolerance), b.loci(radius, tolerance));
    let mut centers: Vec<Point2> = Vec::new();
    for la in &loci_a {
        for lb in &loci_b {
            for center in intersect(la, lb, tolerance)? {
                let known = centers
                    .iter()
                    .any(|c| (c - center).norm() <= tolerance.linear);
                if !known {
                    centers.push(center);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(centers.len());
    for center in centers {
        let (Some(first_point), Some(second_point)) =
            (a.tangent_point(center), b.tangent_point(center))
        else {
            continue;
        };
        out.push(TangentCircle {
            circle: Circle2::new(center, radius)?,
            first: first_point,
            second: second_point,
        });
    }
    out.sort_by(|p, q| {
        p.circle
            .center()
            .x
            .total_cmp(&q.circle.center().x)
            .then(p.circle.center().y.total_cmp(&q.circle.center().y))
    });
    Ok(out)
}

/// The vertices of the regular polygon with `sides` sides inscribed in `circle`, counterclockwise,
/// the first at angle `start_angle`.
///
/// # Errors
///
/// `geom.construct.sides_invalid` for fewer than three or more than [`MAX_POLYGON_SIDES`] sides;
/// `geom.input.not_finite` for a non-finite angle.
pub fn polygon_inscribed(circle: &Circle2, sides: usize, start_angle: f64) -> Result<Vec<Point2>> {
    let sides = check_sides(sides)?;
    ensure_finite(&[start_angle])?;
    Ok(polygon_vertices(
        circle.center(),
        circle.radius(),
        sides,
        start_angle,
    ))
}

/// The vertices of the regular polygon with `sides` sides circumscribed about `circle`, its edges
/// tangent to the circle, counterclockwise, the first vertex at angle `start_angle`.
///
/// The vertices lie at distance `radius / cos(π / sides)` from the center.
///
/// # Errors
///
/// As [`polygon_inscribed`].
pub fn polygon_circumscribed(
    circle: &Circle2,
    sides: usize,
    start_angle: f64,
) -> Result<Vec<Point2>> {
    let sides = check_sides(sides)?;
    ensure_finite(&[start_angle])?;
    let reach = circle.radius() / (PI / index_f64(sides)).cos();
    Ok(polygon_vertices(circle.center(), reach, sides, start_angle))
}

/// The vertices of the regular polygon with `sides` sides that has the segment from `first` to
/// `second` as an edge, on the given side. The vertices run from `first` to `second` and on, in
/// counterclockwise order for [`EdgeSide::Left`] and clockwise for [`EdgeSide::Right`].
///
/// # Errors
///
/// `geom.construct.sides_invalid` as for [`polygon_inscribed`]; `geom.segment.zero_length` for two
/// equal points; `geom.input.not_finite` for a non-finite point.
pub fn polygon_from_edge(
    first: Point2,
    second: Point2,
    sides: usize,
    side: EdgeSide,
) -> Result<Vec<Point2>> {
    let sides = check_sides(sides)?;
    ensure_points(&[first, second])?;
    let chord = second - first;
    let length = chord.norm();
    if length == 0.0 {
        return Err(Error::new(codes::SEGMENT_ZERO_LENGTH));
    }
    let turn = match side {
        EdgeSide::Left => 1.0,
        EdgeSide::Right => -1.0,
    };
    // The center lies on the chosen side of the edge, at the apothem from its midpoint.
    let left = Vector2::new(-chord.y, chord.x) / length;
    let apothem = length * 0.5 / (PI / index_f64(sides)).tan();
    let center = first + chord * 0.5 + left * (turn * apothem);
    let reach = length * 0.5 / (PI / index_f64(sides)).sin();
    // Stepping by `turn` steps from `first` to `second` around the center: counterclockwise for
    // the left side, clockwise for the right side.
    let start = angle_of(first - center);
    let step = turn * TAU / index_f64(sides);
    let mut vertices: Vec<Point2> = (0..sides)
        .map(|k| center + unit_at(start + step * index_f64(k)) * reach)
        .collect();
    vertices[0] = first;
    vertices[1] = second;
    Ok(vertices)
}

/// The ellipse whose axis is the segment from `first` to `second` and whose other semi-axis has
/// length `other`.
///
/// The segment is the major axis when `other` is at most half its length, and the minor axis
/// otherwise. The center is the midpoint of the segment; a length equal to the half length gives a
/// circle.
///
/// # Errors
///
/// `geom.segment.zero_length` for two equal points; `geom.ellipse.invalid_axis` for an `other`
/// that is not positive; `geom.input.not_finite` for a non-finite value.
pub fn ellipse_from_axis(first: Point2, second: Point2, other: f64) -> Result<Ellipse2> {
    ensure_points(&[first, second])?;
    ensure_finite(&[other])?;
    let half = (second - first) * 0.5;
    let semi = half.norm();
    if semi == 0.0 {
        return Err(Error::new(codes::SEGMENT_ZERO_LENGTH));
    }
    if other <= 0.0 {
        return Err(Error::new(codes::ELLIPSE_INVALID_AXIS).with_detail("other", other));
    }
    let center = first + half;
    if other <= semi {
        return Ellipse2::new(center, half, other / semi);
    }
    let across = Vector2::new(-half.y, half.x) / semi * other;
    Ellipse2::new(center, across, semi / other)
}

/// A line or a circle, as an input of a tangency construction.
#[derive(Clone, Copy)]
enum Support {
    Line { point: Point2, unit: Vector2 },
    Circle { center: Point2, radius: f64 },
}

/// The locus of the centers of the circles of one radius tangent to one input.
#[derive(Clone, Copy)]
enum Locus {
    /// A line through `point` with direction `unit`.
    Line { point: Point2, unit: Vector2 },
    /// A circle.
    Circle { center: Point2, radius: f64 },
}

impl Support {
    fn of(curve: &Curve2) -> Result<Self> {
        match curve {
            Curve2::Line(line) => Ok(Self::Line {
                point: line.point(),
                unit: line.unit_direction(),
            }),
            Curve2::Circle(circle) => Ok(Self::Circle {
                center: circle.center(),
                radius: circle.radius(),
            }),
            _ => Err(Error::new(codes::CURVE_KIND_UNSUPPORTED)),
        }
    }

    /// The loci of the centers at distance `radius` from this input: the two lines parallel to a
    /// line at `radius` on either side, and the circles outside and inside a circle.
    fn loci(self, radius: f64, tolerance: &Tolerance) -> Vec<Locus> {
        match self {
            Self::Line { point, unit } => {
                let normal = Vector2::new(-unit.y, unit.x);
                [radius, -radius]
                    .into_iter()
                    .map(|offset| Locus::Line {
                        point: point + normal * offset,
                        unit,
                    })
                    .collect()
            }
            Self::Circle {
                center,
                radius: reach,
            } => {
                let mut distances = vec![reach + radius];
                let inside = (reach - radius).abs();
                if inside > tolerance.linear {
                    distances.push(inside);
                }
                distances
                    .into_iter()
                    .map(|distance| Locus::Circle {
                        center,
                        radius: distance,
                    })
                    .collect()
            }
        }
    }

    /// The point of this input nearest to a center of a tangent circle.
    fn tangent_point(self, center: Point2) -> Option<Point2> {
        match self {
            Self::Line { point, unit } => Some(point + unit * (center - point).dot(&unit)),
            Self::Circle {
                center: circle,
                radius,
            } => {
                let offset = center - circle;
                let distance = offset.norm();
                (distance > 0.0).then(|| circle + offset * (radius / distance))
            }
        }
    }
}

/// The centers at which two loci meet, or an error for a family of centers.
fn intersect(a: &Locus, b: &Locus, tolerance: &Tolerance) -> Result<Vec<Point2>> {
    let lin = tolerance.linear;
    Ok(match (*a, *b) {
        (Locus::Line { point: p, unit: u }, Locus::Line { point: q, unit: v }) => {
            let denominator = cross(u, v);
            if denominator.abs() <= tolerance.angular.max(f64::EPSILON) {
                let gap = cross(u, q - p).abs();
                if gap <= lin {
                    return Err(degenerate());
                }
                Vec::new()
            } else {
                let t = cross(q - p, v) / denominator;
                vec![p + u * t]
            }
        }
        (Locus::Line { point, unit }, Locus::Circle { center, radius })
        | (Locus::Circle { center, radius }, Locus::Line { point, unit }) => {
            line_circle(point, unit, center, radius, lin)
        }
        (
            Locus::Circle {
                center: c1,
                radius: r1,
            },
            Locus::Circle {
                center: c2,
                radius: r2,
            },
        ) => {
            let offset = c2 - c1;
            let d = offset.norm();
            if d <= lin {
                if (r1 - r2).abs() <= lin {
                    return Err(degenerate());
                }
                Vec::new()
            } else if d > r1 + r2 + lin || d < (r1 - r2).abs() - lin {
                Vec::new()
            } else {
                let dir = offset / d;
                let along = (r1 * r1 - r2 * r2 + d * d) / (2.0 * d);
                let base = c1 + dir * along;
                let height = (r1 * r1 - along * along).max(0.0).sqrt();
                if height <= lin {
                    vec![base]
                } else {
                    let side = Vector2::new(-dir.y, dir.x) * height;
                    vec![base + side, base - side]
                }
            }
        }
    })
}

/// The points where the line through `point` with direction `unit` meets the circle.
fn line_circle(point: Point2, unit: Vector2, center: Point2, radius: f64, lin: f64) -> Vec<Point2> {
    let foot = point + unit * (center - point).dot(&unit);
    let distance = (center - foot).norm();
    if (distance - radius).abs() <= lin {
        vec![foot]
    } else if distance > radius {
        Vec::new()
    } else {
        let half = (radius * radius - distance * distance).sqrt();
        vec![foot + unit * half, foot - unit * half]
    }
}

/// The circle through three distinct points, refused when they are collinear.
fn circumcircle(a: Point2, b: Point2, c: Point2, tolerance: &Tolerance) -> Result<Circle2> {
    let (u, v) = (b - a, c - a);
    let area2 = cross(u, v);
    // The distance of `c` from the line through `a` and `b` is |area2| / |u|.
    if area2.abs() <= tolerance.linear * u.norm() {
        return Err(Error::new(codes::CONSTRUCT_COLLINEAR_POINTS));
    }
    let denominator = 2.0 * area2;
    let (uu, vv) = (u.norm_squared(), v.norm_squared());
    let offset = Vector2::new(
        (v.y * uu - u.y * vv) / denominator,
        (u.x * vv - v.x * uu) / denominator,
    );
    Circle2::new(a + offset, offset.norm())
}

/// The vertices of a regular polygon of `sides` sides and circumradius `reach`, the first at
/// `start`, counterclockwise.
fn polygon_vertices(center: Point2, reach: f64, sides: usize, start: f64) -> Vec<Point2> {
    let step = TAU / index_f64(sides);
    (0..sides)
        .map(|k| center + unit_at(start + step * index_f64(k)) * reach)
        .collect()
}

fn check_sides(sides: usize) -> Result<usize> {
    if (3..=MAX_POLYGON_SIDES).contains(&sides) {
        Ok(sides)
    } else {
        Err(Error::new(codes::CONSTRUCT_SIDES_INVALID).with_detail("sides", sides.to_string()))
    }
}

/// The value of a count below [`MAX_POLYGON_SIDES`] plus the index range, as `f64`.
fn index_f64(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

fn angle_of(v: Vector2) -> f64 {
    v.y.atan2(v.x)
}

fn unit_at(angle: f64) -> Vector2 {
    Vector2::new(angle.cos(), angle.sin())
}

fn cross(a: Vector2, b: Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn coincident() -> Error {
    Error::new(codes::CONSTRUCT_COINCIDENT_POINTS)
}

fn degenerate() -> Error {
    Error::new(codes::CONSTRUCT_DEGENERATE)
}

fn ensure_linear(tolerance: &Tolerance) -> Result<()> {
    crate::ensure_tolerance(tolerance)?;
    if tolerance.linear > 0.0 {
        Ok(())
    } else {
        Err(Error::new(codes::TOLERANCE_INVALID))
    }
}

fn ensure_finite(values: &[f64]) -> Result<()> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(codes::INPUT_NOT_FINITE))
    }
}

fn ensure_points(points: &[Point2]) -> Result<()> {
    ensure_finite(&points.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<_>>())
}

/// Fails with `geom.construct.coincident_points` when two of the pairs are closer than the linear
/// tolerance.
fn ensure_distinct(pairs: &[(Point2, Point2)], tolerance: &Tolerance) -> Result<()> {
    if pairs
        .iter()
        .any(|(p, q)| (q - p).norm() <= tolerance.linear)
    {
        Err(coincident())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, FRAC_PI_6};

    use arc_base::math::{Point2, Vector2};
    use arc_base::{Result, Tolerance};
    use proptest::prelude::*;

    use crate::curve::Curve2;
    use crate::error::codes;
    use crate::primitives::{Circle2, Line2};

    use super::*;

    fn tol() -> Tolerance {
        Tolerance::DEFAULT
    }

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    fn close(a: Point2, b: Point2) -> bool {
        (a - b).norm() < 1e-9
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

    fn line(point: Point2, direction: Vector2) -> Curve2 {
        Curve2::Line(Line2::new(point, direction).expect_ok())
    }

    fn circle(center: Point2, radius: f64) -> Curve2 {
        Curve2::Circle(Circle2::new(center, radius).expect_ok())
    }

    #[test]
    fn circle_through_three_points_is_the_circumcircle() {
        let c = circle_through_points(p(0.0, 0.0), p(2.0, 0.0), p(0.0, 2.0), &tol()).expect_ok();
        assert!(close(c.center(), p(1.0, 1.0)));
        assert!((c.radius() - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn collinear_and_coincident_points_are_refused() {
        assert_eq!(
            code_of(circle_through_points(
                p(0.0, 0.0),
                p(1.0, 1.0),
                p(2.0, 2.0),
                &tol()
            )),
            codes::CONSTRUCT_COLLINEAR_POINTS.code
        );
        assert_eq!(
            code_of(circle_through_points(
                p(0.0, 0.0),
                p(0.0, 0.0),
                p(2.0, 2.0),
                &tol()
            )),
            codes::CONSTRUCT_COINCIDENT_POINTS.code
        );
        assert_eq!(
            code_of(circle_through_points(
                p(f64::NAN, 0.0),
                p(1.0, 0.0),
                p(0.0, 1.0),
                &tol()
            )),
            codes::INPUT_NOT_FINITE.code
        );
    }

    #[test]
    fn circle_on_diameter_has_the_midpoint_as_center() {
        let c = circle_on_diameter(p(0.0, 0.0), p(4.0, 0.0), &tol()).expect_ok();
        assert!(close(c.center(), p(2.0, 0.0)));
        assert!((c.radius() - 2.0).abs() < 1e-12);
        assert_eq!(
            code_of(circle_on_diameter(p(1.0, 1.0), p(1.0, 1.0), &tol())),
            codes::CONSTRUCT_COINCIDENT_POINTS.code
        );
    }

    #[test]
    fn arc_through_points_runs_through_the_middle_point_in_its_direction() {
        let ccw = arc_through_points(p(1.0, 0.0), p(0.0, 1.0), p(-1.0, 0.0), &tol()).expect_ok();
        assert!(close(ccw.center(), p(0.0, 0.0)));
        assert!((ccw.sweep() - PI).abs() < 1e-12);
        assert!(close(ccw.point_at(ccw.sweep() * 0.5), p(0.0, 1.0)));
        let cw = arc_through_points(p(-1.0, 0.0), p(0.0, 1.0), p(1.0, 0.0), &tol()).expect_ok();
        assert!((cw.sweep() + PI).abs() < 1e-12);
        assert!(close(cw.point_at(cw.sweep().abs() * 0.5), p(0.0, 1.0)));
        assert!(close(cw.point_at(cw.sweep().abs()), p(1.0, 0.0)));
    }

    #[test]
    fn arc_from_start_center_and_sweep() {
        let arc = arc_from_start_center(p(1.0, 0.0), p(0.0, 0.0), FRAC_PI_2, &tol()).expect_ok();
        assert!((arc.radius() - 1.0).abs() < 1e-12);
        assert!(close(arc.point_at(arc.sweep()), p(0.0, 1.0)));
        let cw = arc_from_start_center(p(1.0, 0.0), p(0.0, 0.0), -FRAC_PI_2, &tol()).expect_ok();
        assert!(close(cw.point_at(cw.sweep().abs()), p(0.0, -1.0)));
        assert_eq!(
            code_of(arc_from_start_center(p(0.0, 0.0), p(0.0, 0.0), 1.0, &tol())),
            codes::CONSTRUCT_COINCIDENT_POINTS.code
        );
        assert_eq!(
            code_of(arc_from_start_center(p(1.0, 0.0), p(0.0, 0.0), 0.0, &tol())),
            codes::SWEEP_INVALID.code
        );
    }

    #[test]
    fn arc_from_ends_and_radius_selects_the_side_and_the_minor_arc() {
        // Half chord 1, radius 2: the center is 3^(1/2) from the chord midpoint.
        let left = arc_from_ends_and_radius(p(-1.0, 0.0), p(1.0, 0.0), 2.0, &tol()).expect_ok();
        assert!(close(left.center(), p(0.0, 3f64.sqrt())));
        assert!((left.sweep() - PI / 3.0).abs() < 1e-12);
        assert!(close(left.point_at(left.sweep()), p(1.0, 0.0)));
        let right = arc_from_ends_and_radius(p(-1.0, 0.0), p(1.0, 0.0), -2.0, &tol()).expect_ok();
        assert!(close(right.center(), p(0.0, -3f64.sqrt())));
        assert!(right.sweep() < 0.0);
        assert!(close(right.point_at(right.sweep().abs()), p(1.0, 0.0)));
        let semi = arc_from_ends_and_radius(p(-1.0, 0.0), p(1.0, 0.0), 1.0, &tol()).expect_ok();
        assert!(close(semi.center(), p(0.0, 0.0)));
        assert!((semi.sweep().abs() - PI).abs() < 1e-9);
        assert_eq!(
            code_of(arc_from_ends_and_radius(
                p(-1.0, 0.0),
                p(1.0, 0.0),
                0.9,
                &tol()
            )),
            codes::CONSTRUCT_RADIUS_TOO_SMALL.code
        );
        assert_eq!(
            code_of(arc_from_ends_and_radius(
                p(-1.0, 0.0),
                p(1.0, 0.0),
                0.0,
                &tol()
            )),
            codes::RADIUS_NOT_POSITIVE.code
        );
    }

    #[test]
    fn tangent_circles_to_two_crossing_lines_are_the_four_quadrant_circles() {
        let x_axis = line(p(0.0, 0.0), Vector2::new(1.0, 0.0));
        let y_axis = line(p(0.0, 0.0), Vector2::new(0.0, 1.0));
        let solutions = tangent_circles(&x_axis, &y_axis, 1.0, &tol()).expect_ok();
        assert_eq!(solutions.len(), 4);
        for s in &solutions {
            assert!((s.circle.center().x.abs() - 1.0).abs() < 1e-9);
            assert!((s.circle.center().y.abs() - 1.0).abs() < 1e-9);
            assert!(close(s.first, p(s.circle.center().x, 0.0)));
            assert!(close(s.second, p(0.0, s.circle.center().y)));
        }
    }

    #[test]
    fn tangent_circles_to_a_line_and_a_circle() {
        // Circle of radius 2 at (0, 3). Radius 1 tangent to y = 1 and outside the circle: center
        // at distance 3 from (0, 3), so x = ±√5. Tangent to y = -1 is out of reach.
        let solutions = tangent_circles(
            &line(p(0.0, 0.0), Vector2::new(1.0, 0.0)),
            &circle(p(0.0, 3.0), 2.0),
            1.0,
            &tol(),
        )
        .expect_ok();
        assert_eq!(solutions.len(), 2);
        assert!(close(solutions[0].circle.center(), p(-5f64.sqrt(), 1.0)));
        assert!(close(solutions[1].circle.center(), p(5f64.sqrt(), 1.0)));
    }

    #[test]
    fn tangent_circles_refuse_families_and_unsupported_kinds() {
        let x_axis = line(p(0.0, 0.0), Vector2::new(1.0, 0.0));
        // Parallel lines at distance 2 · radius: centers on the midline, a family.
        assert_eq!(
            code_of(tangent_circles(
                &x_axis,
                &line(p(0.0, 2.0), Vector2::new(1.0, 0.0)),
                1.0,
                &tol()
            )),
            codes::CONSTRUCT_DEGENERATE.code
        );
        // Parallel lines farther apart: no solution, not an error.
        assert!(
            tangent_circles(
                &x_axis,
                &line(p(0.0, 3.0), Vector2::new(1.0, 0.0)),
                1.0,
                &tol()
            )
            .expect_ok()
            .is_empty()
        );
        let segment =
            Curve2::Segment(crate::primitives::Segment2::new(p(0.0, 0.0), p(1.0, 0.0)).expect_ok());
        assert_eq!(
            code_of(tangent_circles(&segment, &x_axis, 1.0, &tol())),
            codes::CURVE_KIND_UNSUPPORTED.code
        );
        assert_eq!(
            code_of(tangent_circles(&x_axis, &x_axis, 0.0, &tol())),
            codes::RADIUS_NOT_POSITIVE.code
        );
    }

    #[test]
    fn inscribed_and_circumscribed_squares_and_hexagons() {
        let unit = Circle2::new(p(0.0, 0.0), 1.0).expect_ok();
        let square = polygon_inscribed(&unit, 4, 0.0).expect_ok();
        assert!(close(square[0], p(1.0, 0.0)) && close(square[1], p(0.0, 1.0)));
        let outer = polygon_circumscribed(&unit, 4, FRAC_PI_4).expect_ok();
        // Vertices at distance √2 along the diagonals.
        assert!((outer[0].coords.norm() - 2f64.sqrt()).abs() < 1e-12);
        let hexagon = polygon_circumscribed(&unit, 6, 0.0).expect_ok();
        assert!((hexagon[0].coords.norm() - 1.0 / FRAC_PI_6.cos()).abs() < 1e-12);
        assert_eq!(
            code_of(polygon_inscribed(&unit, 2, 0.0)),
            codes::CONSTRUCT_SIDES_INVALID.code
        );
    }

    #[test]
    fn polygon_from_an_edge_lies_on_the_chosen_side() {
        let left = polygon_from_edge(p(0.0, 0.0), p(1.0, 0.0), 4, EdgeSide::Left).expect_ok();
        assert!(close(left[0], p(0.0, 0.0)) && close(left[1], p(1.0, 0.0)));
        assert!(close(left[2], p(1.0, 1.0)) && close(left[3], p(0.0, 1.0)));
        let right = polygon_from_edge(p(0.0, 0.0), p(1.0, 0.0), 4, EdgeSide::Right).expect_ok();
        assert!(close(right[2], p(1.0, -1.0)) && close(right[3], p(0.0, -1.0)));
        assert_eq!(
            code_of(polygon_from_edge(
                p(1.0, 1.0),
                p(1.0, 1.0),
                5,
                EdgeSide::Left
            )),
            codes::SEGMENT_ZERO_LENGTH.code
        );
    }

    #[test]
    fn ellipse_from_axis_takes_the_axis_as_major_or_minor() {
        let major = ellipse_from_axis(p(-2.0, 0.0), p(2.0, 0.0), 1.0).expect_ok();
        assert!(close(major.center(), p(0.0, 0.0)));
        assert!((major.major_axis() - Vector2::new(2.0, 0.0)).norm() < 1e-12);
        assert!((major.ratio() - 0.5).abs() < 1e-12);
        let minor = ellipse_from_axis(p(-2.0, 0.0), p(2.0, 0.0), 3.0).expect_ok();
        assert!((minor.major_axis().norm() - 3.0).abs() < 1e-12);
        assert!((minor.ratio() - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(
            code_of(ellipse_from_axis(p(0.0, 0.0), p(2.0, 0.0), 0.0)),
            codes::ELLIPSE_INVALID_AXIS.code
        );
    }

    proptest! {
        #[test]
        fn circle_through_three_points_passes_through_them(
            radius in 0.5_f64..50.0,
            a in 0.0_f64..TAU,
            gap1 in 0.3_f64..2.5,
            gap2 in 0.3_f64..2.5,
            cx in -20.0_f64..20.0,
            cy in -20.0_f64..20.0,
        ) {
            let center = p(cx, cy);
            let at = |t: f64| center + unit_at(t) * radius;
            let (x, y, z) = (at(a), at(a + gap1), at(a + gap1 + gap2));
            let circle = circle_through_points(x, y, z, &tol()).expect_ok();
            prop_assert!((circle.center() - center).norm() < 1e-7 * radius.max(1.0));
            prop_assert!((circle.radius() - radius).abs() < 1e-7 * radius.max(1.0));
        }

        #[test]
        fn circle_on_diameter_has_both_ends_on_it_and_its_center_between_them(
            ax in -20.0_f64..20.0,
            ay in -20.0_f64..20.0,
            length in 0.5_f64..30.0,
            angle in 0.0_f64..TAU,
        ) {
            let a = p(ax, ay);
            let b = a + unit_at(angle) * length;
            let c = circle_on_diameter(a, b, &tol()).expect_ok();
            prop_assert!(((a - c.center()).norm() - c.radius()).abs() < 1e-9 * length);
            prop_assert!(((b - c.center()).norm() - c.radius()).abs() < 1e-9 * length);
            prop_assert!((c.radius() - length * 0.5).abs() < 1e-9 * length);
        }

        #[test]
        fn arc_from_start_center_and_sweep_ends_at_the_swept_angle(
            cx in -20.0_f64..20.0,
            cy in -20.0_f64..20.0,
            radius in 0.5_f64..30.0,
            start in 0.0_f64..TAU,
            sweep in 0.05_f64..TAU,
            clockwise in any::<bool>(),
        ) {
            let center = p(cx, cy);
            let start_point = center + unit_at(start) * radius;
            let signed = if clockwise { -sweep } else { sweep };
            let arc = arc_from_start_center(start_point, center, signed, &tol()).expect_ok();
            prop_assert!((arc.radius() - radius).abs() < 1e-9 * radius);
            let end = center + unit_at(start + signed) * radius;
            prop_assert!(close_to(arc.point_at(arc.sweep().abs()), end, radius));
            prop_assert_eq!(arc.is_counterclockwise(), !clockwise);
        }

        #[test]
        fn arc_through_points_passes_through_its_three_points(
            radius in 0.5_f64..50.0,
            a in 0.0_f64..TAU,
            gap1 in 0.3_f64..2.5,
            gap2 in 0.3_f64..2.5,
        ) {
            let at = |t: f64| Point2::from(unit_at(t) * radius);
            let (s, m, e) = (at(a), at(a + gap1), at(a + gap1 + gap2));
            let arc = arc_through_points(s, m, e, &tol()).expect_ok();
            prop_assert!(close_to(arc.point_at(0.0), s, radius));
            prop_assert!(close_to(arc.point_at(arc.sweep().abs()), e, radius));
            // The middle point lies on the arc at the angle of the middle parameter.
            prop_assert!(close_to(arc.point_at(gap1), m, radius));
            prop_assert!(arc.sweep().abs() <= TAU);
        }

        #[test]
        fn arc_from_ends_and_radius_has_the_radius_and_the_minor_sweep(
            half in 0.5_f64..10.0,
            fraction in 1.05_f64..1.99,
            negative in any::<bool>(),
            angle in 0.0_f64..TAU,
        ) {
            let direction = unit_at(angle);
            let start = Point2::from(direction * (-half));
            let end = Point2::from(direction * half);
            let radius = half * fraction * if negative { -1.0 } else { 1.0 };
            let arc = arc_from_ends_and_radius(start, end, radius, &tol()).expect_ok();
            prop_assert!((arc.radius() - radius.abs()).abs() < 1e-9 * radius.abs().max(1.0));
            prop_assert!(arc.sweep().abs() <= PI + 1e-9);
            prop_assert!(close_to(arc.point_at(0.0), start, radius.abs()));
            prop_assert!(close_to(arc.point_at(arc.sweep().abs()), end, radius.abs()));
            // The sign puts the center on the left (positive) or right of start to end.
            let side = cross(end - start, arc.center() - start);
            prop_assert!(side * radius > 0.0);
        }

        #[test]
        fn tangent_circles_are_tangent_to_both_inputs(
            radius in 0.2_f64..3.0,
            angle in 0.0_f64..TAU,
            offset in -5.0_f64..5.0,
            cx in -6.0_f64..6.0,
            cy in -6.0_f64..6.0,
            rc in 0.5_f64..4.0,
        ) {
            let u = unit_at(angle);
            let base = p(offset * (-u.y), offset * u.x);
            let first = line(base, u);
            let second = circle(p(cx, cy), rc);
            let Ok(solutions) = tangent_circles(&first, &second, radius, &tol()) else {
                // Parallel-family refusals are not expected for a line and a circle.
                return Err(TestCaseError::fail("unexpected refusal"));
            };
            for s in solutions {
                let center = s.circle.center();
                let to_line = cross(u, center - base).abs();
                prop_assert!((to_line - radius).abs() < 1e-7);
                let d = (center - p(cx, cy)).norm();
                prop_assert!((d - (rc + radius)).abs() < 1e-7 || (d - (rc - radius).abs()).abs() < 1e-7);
                prop_assert!(((s.second - p(cx, cy)).norm() - rc).abs() < 1e-7);
            }
        }

        #[test]
        fn regular_polygon_vertices_lie_on_the_circle_and_edges_are_equal(
            sides in 3usize..40,
            radius in 0.5_f64..20.0,
            start in 0.0_f64..TAU,
        ) {
            let c = Circle2::new(p(1.0, -2.0), radius).expect_ok();
            let vertices = polygon_inscribed(&c, sides, start).expect_ok();
            prop_assert_eq!(vertices.len(), sides);
            let edge = (vertices[1] - vertices[0]).norm();
            for (a, b) in vertices.iter().zip(vertices.iter().cycle().skip(1)) {
                prop_assert!(((a - c.center()).norm() - radius).abs() < 1e-9 * radius);
                prop_assert!(((b - a).norm() - edge).abs() < 1e-9 * radius);
            }
            let outer = polygon_circumscribed(&c, sides, start).expect_ok();
            // Each edge midpoint is at distance `radius` from the center: the edge is tangent.
            for (a, b) in outer.iter().zip(outer.iter().cycle().skip(1)) {
                let mid = (a + (b - a) * 0.5) - c.center();
                prop_assert!((mid.norm() - radius).abs() < 1e-9 * radius);
            }
        }

        #[test]
        fn polygon_from_edge_has_equal_sides_on_the_chosen_side(
            sides in 3usize..40,
            length in 0.5_f64..10.0,
            angle in 0.0_f64..TAU,
            left in any::<bool>(),
        ) {
            let a = p(3.0, -1.0);
            let b = a + unit_at(angle) * length;
            let side = if left { EdgeSide::Left } else { EdgeSide::Right };
            let v = polygon_from_edge(a, b, sides, side).expect_ok();
            prop_assert_eq!(v.len(), sides);
            for (a, b) in v.iter().zip(v.iter().cycle().skip(1)) {
                prop_assert!(((b - a).norm() - length).abs() < 1e-9 * length);
            }
            // The signed area is positive for the left side and negative for the right side.
            let area: f64 = v
                .iter()
                .zip(v.iter().cycle().skip(1))
                .map(|(a, b)| cross(a.coords, b.coords))
                .sum();
            let expected = if left { 1.0 } else { -1.0 };
            prop_assert!(area * expected > 0.0);
        }

        #[test]
        fn ellipse_from_axis_passes_through_the_axis_ends_and_the_other_semi_axis(
            half in 0.5_f64..10.0,
            other in 0.2_f64..20.0,
            angle in 0.0_f64..TAU,
        ) {
            let dir = unit_at(angle);
            let e = ellipse_from_axis(
                Point2::from(-dir * half),
                Point2::from(dir * half),
                other,
            )
            .expect_ok();
            let semi_major = e.major_axis().norm();
            let semi_minor = semi_major * e.ratio();
            prop_assert!((semi_major.max(semi_minor) - half.max(other)).abs() < 1e-9 * half.max(other));
            prop_assert!((semi_major.min(semi_minor) - half.min(other)).abs() < 1e-9 * half.max(other));
            // The point at the parameter of the axis end lies on the segment's line.
            let on_axis = e.point_at(0.0);
            prop_assert!(((on_axis - e.center()).norm() - semi_major).abs() < 1e-9 * half.max(other));
        }
    }

    fn close_to(a: Point2, b: Point2, scale: f64) -> bool {
        (a - b).norm() < 1e-9 * scale.max(1.0)
    }
}
