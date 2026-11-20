//! Planes in 3D space (CORE-GEOM-012, the plane case) and the 2D frame of a plane.
//!
//! A [`Plane3`] is a point and a unit normal. Its [`PlaneFrame`] maps points of the plane to 2D
//! coordinates and back, which is how plan and section views, extrusions and the faces of
//! [`crate::Polyhedron`] are expressed with the 2D types of this crate.
//!
//! - **Construction.** The normal is normalized without overflow; a zero normal fails with
//!   `geom.direction.zero`, a non-finite value with `geom.input.not_finite`. Three points that
//!   are closer than the linear tolerance to one line fail with
//!   `geom.construct.collinear_points`.
//! - **Frame.** The first axis `u` is horizontal (`z × normal`, normalized) and the second axis
//!   `v = normal × u` points up, so a vertical plane has "up" for its second coordinate and a
//!   horizontal plane uses the world x axis (projected on the plane) as its first axis. A normal
//!   within the angular tolerance of vertical counts as horizontal. The frame is right-handed
//!   (`u × v = normal`), so counterclockwise in plane coordinates is counterclockwise seen
//!   against the normal.
//! - **Intersections** treat a line or segment within `tolerance.linear` of the plane over its
//!   whole length, and a direction within `tolerance.angular` of perpendicular to the normal,
//!   as parallel; two planes whose normals are within `tolerance.angular` of parallel are
//!   parallel, and coincident when also within `tolerance.linear` of each other.

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Error, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::error::codes;

/// The side of a plane a point lies on, decided with the linear tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneSide {
    /// More than the linear tolerance in the direction of the normal.
    Positive,
    /// Within the linear tolerance of the plane.
    On,
    /// More than the linear tolerance against the direction of the normal.
    Negative,
}

/// The intersection of a line with a plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinePlane {
    /// The line crosses the plane at `point`, at `parameter` along the given direction vector
    /// (`point = origin + parameter · direction`).
    Point {
        /// The crossing point.
        point: Point3,
        /// The parameter of the crossing point.
        parameter: f64,
    },
    /// The line is parallel to the plane and not within the linear tolerance of it.
    Parallel,
    /// The line lies in the plane within the linear tolerance.
    Contained,
}

/// The intersection of a segment with a plane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SegmentPlane {
    /// The segment meets the plane at `point`, at `parameter` in [0, 1] from the start. An end
    /// point within the linear tolerance of the plane is the meeting point, so a segment that
    /// touches the plane at an end reports that end.
    Point {
        /// The meeting point.
        point: Point3,
        /// The parameter of the meeting point on the segment.
        parameter: f64,
    },
    /// The segment lies in the plane within the linear tolerance.
    Contained,
    /// The segment and the plane do not meet.
    Disjoint,
}

/// The intersection of two planes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlanePlane {
    /// The planes cross in a line through `point` with unit `direction`, equal to the cross
    /// product of the first normal and the second, normalized.
    Line {
        /// A point of the line.
        point: Point3,
        /// The unit direction of the line.
        direction: Vector3,
    },
    /// The planes are parallel and apart.
    Parallel,
    /// The planes are parallel and within the linear tolerance of each other.
    Coincident,
}

/// A plane: a point on it and a unit normal.
///
/// Serialized as its origin and normal; deserialization validates as [`Self::new`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Plane3Raw")]
pub struct Plane3 {
    origin: Point3,
    normal: Vector3,
}

crate::deserialize_through_new!(
    Plane3,
    Plane3Raw {
        origin: Point3,
        normal: Vector3
    }
);

impl Plane3 {
    /// The plane through `origin` with the given normal, which is normalized.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite coordinate; `geom.direction.zero` for a zero
    /// normal.
    pub fn new(origin: Point3, normal: Vector3) -> Result<Self> {
        crate::ensure_finite(origin.coords.as_slice())?;
        crate::ensure_finite(normal.as_slice())?;
        let normal = unit3(&normal).ok_or_else(|| Error::new(codes::DIRECTION_ZERO))?;
        Ok(Self { origin, normal })
    }

    /// The plane through three points; the normal is `(b − a) × (c − a)` normalized, so the
    /// points run counterclockwise seen against the normal.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite coordinate; `geom.tolerance.invalid` for an
    /// invalid tolerance; `geom.construct.collinear_points` when the points are within
    /// `tolerance.linear` of one line (this includes coincident points).
    pub fn from_points(a: Point3, b: Point3, c: Point3, tolerance: &Tolerance) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        crate::ensure_finite(a.coords.as_slice())?;
        crate::ensure_finite(b.coords.as_slice())?;
        crate::ensure_finite(c.coords.as_slice())?;
        let ab = b - a;
        let ac = c - a;
        let cross = ab.cross(&ac);
        let base = ab.norm();
        // The distance of c from the line ab is |ab × ac| / |ab|; when ab is shorter than the
        // tolerance the points are coincident or collinear for the purpose of a plane.
        let height = if base > 0.0 { cross.norm() / base } else { 0.0 };
        if !(base > tolerance.linear && height > tolerance.linear) {
            return Err(Error::new(codes::CONSTRUCT_COLLINEAR_POINTS));
        }
        Self::new(a, cross)
    }

    /// A point of the plane.
    #[must_use]
    pub fn origin(&self) -> Point3 {
        self.origin
    }

    /// The unit normal.
    #[must_use]
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The same plane with the opposite normal.
    #[must_use]
    pub fn flipped(&self) -> Self {
        Self {
            origin: self.origin,
            normal: -self.normal,
        }
    }

    /// The signed distance of `point` from the plane, positive on the side of the normal.
    #[must_use]
    pub fn signed_distance(&self, point: &Point3) -> f64 {
        self.normal.dot(&(point - self.origin))
    }

    /// The side of the plane `point` lies on, decided with `tolerance.linear`.
    #[must_use]
    pub fn side(&self, point: &Point3, tolerance: &Tolerance) -> PlaneSide {
        let distance = self.signed_distance(point);
        if distance > tolerance.linear {
            PlaneSide::Positive
        } else if distance < -tolerance.linear {
            PlaneSide::Negative
        } else {
            PlaneSide::On
        }
    }

    /// The closest point of the plane to `point`.
    #[must_use]
    pub fn project(&self, point: &Point3) -> Point3 {
        point - self.normal * self.signed_distance(point)
    }

    /// The frame of this plane, with its origin at [`Self::origin`] and the axes described in
    /// the [module documentation](self).
    #[must_use]
    pub fn frame(&self, tolerance: &Tolerance) -> PlaneFrame {
        let up = Vector3::z();
        let horizontal = up.cross(&self.normal);
        let u = if horizontal.norm() > tolerance.angular {
            horizontal.normalize()
        } else {
            // A (nearly) vertical normal: the world x axis, projected on the plane.
            let x = Vector3::x();
            unit3(&(x - self.normal * self.normal.x)).unwrap_or(x)
        };
        let v = self.normal.cross(&u);
        PlaneFrame {
            origin: self.origin,
            u,
            v,
            normal: self.normal,
        }
    }

    /// The frame of this plane whose first axis is `x_hint` projected on the plane.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite hint; `geom.direction.zero` when the hint is zero
    /// or within `tolerance.angular` of the normal's direction.
    pub fn frame_with_x(&self, x_hint: &Vector3, tolerance: &Tolerance) -> Result<PlaneFrame> {
        crate::ensure_finite(x_hint.as_slice())?;
        let hint = unit3(x_hint).ok_or_else(|| Error::new(codes::DIRECTION_ZERO))?;
        let projected = hint - self.normal * self.normal.dot(&hint);
        if projected.norm() <= tolerance.angular {
            return Err(Error::new(codes::DIRECTION_ZERO));
        }
        let u = projected.normalize();
        Ok(PlaneFrame {
            origin: self.origin,
            u,
            v: self.normal.cross(&u),
            normal: self.normal,
        })
    }

    /// The intersection of the line through `origin` with `direction` (any nonzero length) with
    /// the plane.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.direction.zero` for a zero
    /// direction; `geom.tolerance.invalid` for an invalid tolerance.
    pub fn intersect_line(
        &self,
        origin: &Point3,
        direction: &Vector3,
        tolerance: &Tolerance,
    ) -> Result<LinePlane> {
        crate::ensure_tolerance(tolerance)?;
        crate::ensure_finite(origin.coords.as_slice())?;
        crate::ensure_finite(direction.as_slice())?;
        let unit_direction = unit3(direction).ok_or_else(|| Error::new(codes::DIRECTION_ZERO))?;
        let distance = self.signed_distance(origin);
        let slope = self.normal.dot(&unit_direction);
        if slope.abs() <= tolerance.angular {
            return Ok(if distance.abs() <= tolerance.linear {
                LinePlane::Contained
            } else {
                LinePlane::Parallel
            });
        }
        // Along the unit direction the distance changes by `slope` per unit length.
        let unit_parameter = -distance / slope;
        let length = direction.amax();
        let parameter = unit_parameter / (direction / length).norm() / length;
        Ok(LinePlane::Point {
            point: origin + unit_direction * unit_parameter,
            parameter,
        })
    }

    /// The intersection of the segment from `start` to `end` with the plane.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.segment.zero_length` when the end
    /// points are within `tolerance.linear` of each other; `geom.tolerance.invalid` for an
    /// invalid tolerance.
    pub fn intersect_segment(
        &self,
        start: &Point3,
        end: &Point3,
        tolerance: &Tolerance,
    ) -> Result<SegmentPlane> {
        crate::ensure_tolerance(tolerance)?;
        crate::ensure_finite(start.coords.as_slice())?;
        crate::ensure_finite(end.coords.as_slice())?;
        if (end - start).norm() <= tolerance.linear {
            return Err(Error::new(codes::SEGMENT_ZERO_LENGTH));
        }
        let d_start = self.signed_distance(start);
        let d_end = self.signed_distance(end);
        let on_start = d_start.abs() <= tolerance.linear;
        let on_end = d_end.abs() <= tolerance.linear;
        Ok(match (on_start, on_end) {
            (true, true) => SegmentPlane::Contained,
            (true, false) => SegmentPlane::Point {
                point: *start,
                parameter: 0.0,
            },
            (false, true) => SegmentPlane::Point {
                point: *end,
                parameter: 1.0,
            },
            (false, false) if (d_start > 0.0) != (d_end > 0.0) => {
                let parameter = d_start / (d_start - d_end);
                SegmentPlane::Point {
                    point: start + (end - start) * parameter,
                    parameter,
                }
            }
            (false, false) => SegmentPlane::Disjoint,
        })
    }

    /// The intersection of this plane with `other`.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for an invalid tolerance.
    pub fn intersect_plane(&self, other: &Self, tolerance: &Tolerance) -> Result<PlanePlane> {
        crate::ensure_tolerance(tolerance)?;
        let direction = self.normal.cross(&other.normal);
        let sine = direction.norm();
        if sine <= tolerance.angular {
            return Ok(
                if other.signed_distance(&self.origin).abs() <= tolerance.linear {
                    PlanePlane::Coincident
                } else {
                    PlanePlane::Parallel
                },
            );
        }
        // With h = n · o for each plane, the point of the line closest to the world origin is
        // (h1 (n2 × d) + h2 (d × n1)) / |d|², since n1 · it = h1 and n2 · it = h2.
        let h1 = self.normal.dot(&self.origin.coords);
        let h2 = other.normal.dot(&other.origin.coords);
        let point = (other.normal.cross(&direction) * h1 + direction.cross(&self.normal) * h2)
            / (sine * sine);
        Ok(PlanePlane::Line {
            point: Point3::from(point),
            direction: direction / sine,
        })
    }
}

/// A right-handed 2D coordinate frame on a plane: an origin, two orthonormal in-plane axes
/// `u` and `v`, and the normal `u × v`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneFrame {
    origin: Point3,
    u: Vector3,
    v: Vector3,
    normal: Vector3,
}

impl PlaneFrame {
    /// The frame at `origin` with axes `u` and `v` and normal `u × v`.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.direction.zero` when an axis is
    /// zero, the axes are not perpendicular within `tolerance.angular`, or they are not unit
    /// length within `tolerance.angular`.
    pub fn new(origin: Point3, u: Vector3, v: Vector3, tolerance: &Tolerance) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        crate::ensure_finite(origin.coords.as_slice())?;
        crate::ensure_finite(u.as_slice())?;
        crate::ensure_finite(v.as_slice())?;
        let unit_length = |a: &Vector3| (a.norm() - 1.0).abs() <= tolerance.angular;
        if !(unit_length(&u) && unit_length(&v) && u.dot(&v).abs() <= tolerance.angular) {
            return Err(Error::new(codes::DIRECTION_ZERO));
        }
        let normal = unit3(&u.cross(&v)).ok_or_else(|| Error::new(codes::DIRECTION_ZERO))?;
        Ok(Self {
            origin,
            u,
            v,
            normal,
        })
    }

    /// The origin of the frame.
    #[must_use]
    pub fn origin(&self) -> Point3 {
        self.origin
    }

    /// The first axis.
    #[must_use]
    pub fn u(&self) -> Vector3 {
        self.u
    }

    /// The second axis.
    #[must_use]
    pub fn v(&self) -> Vector3 {
        self.v
    }

    /// The unit normal `u × v`.
    #[must_use]
    pub fn normal(&self) -> Vector3 {
        self.normal
    }

    /// The plane of the frame.
    #[must_use]
    pub fn plane(&self) -> Plane3 {
        Plane3 {
            origin: self.origin,
            normal: self.normal,
        }
    }

    /// The 2D coordinates of `point` in the frame; the component of `point` along the normal is
    /// dropped, so a point off the plane maps to the coordinates of its projection.
    #[must_use]
    pub fn to_plane(&self, point: &Point3) -> Point2 {
        let offset = point - self.origin;
        Point2::new(self.u.dot(&offset), self.v.dot(&offset))
    }

    /// The point of the plane with 2D coordinates `point`.
    #[must_use]
    pub fn to_world(&self, point: &Point2) -> Point3 {
        self.origin + self.u * point.x + self.v * point.y
    }
}

/// `v` scaled to unit length without overflow or underflow of intermediate squares, or `None`
/// for the zero vector.
pub(crate) fn unit3(v: &Vector3) -> Option<Vector3> {
    let largest = v.amax();
    if largest > 0.0 && largest.is_finite() {
        let scaled = v / largest;
        Some(scaled / scaled.norm())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TOL: Tolerance = Tolerance::DEFAULT;

    fn p(x: f64, y: f64, z: f64) -> Point3 {
        Point3::new(x, y, z)
    }

    fn v(x: f64, y: f64, z: f64) -> Vector3 {
        Vector3::new(x, y, z)
    }

    fn plane(origin: Point3, normal: Vector3) -> Plane3 {
        Plane3::new(origin, normal).expect("valid plane")
    }

    #[test]
    fn construction_normalizes_and_validates() {
        let plane = plane(p(1.0, 2.0, 3.0), v(0.0, 0.0, 5.0));
        assert_eq!(plane.normal(), v(0.0, 0.0, 1.0));
        assert_eq!(
            Plane3::new(p(0.0, 0.0, 0.0), v(0.0, 0.0, 0.0))
                .err()
                .map(|e| e.code()),
            Some("geom.direction.zero")
        );
        assert_eq!(
            Plane3::new(p(f64::NAN, 0.0, 0.0), v(0.0, 0.0, 1.0))
                .err()
                .map(|e| e.code()),
            Some("geom.input.not_finite")
        );
        let huge = Plane3::new(p(0.0, 0.0, 0.0), v(1e300, 1e300, 0.0)).expect("no overflow");
        assert!((huge.normal().norm() - 1.0).abs() < 1e-15);
    }

    #[test]
    fn three_points_give_a_counterclockwise_normal_and_collinear_points_fail() {
        let plane = Plane3::from_points(p(0.0, 0.0, 1.0), p(1.0, 0.0, 1.0), p(0.0, 1.0, 1.0), &TOL)
            .expect("plane");
        assert_eq!(plane.normal(), v(0.0, 0.0, 1.0));
        for (a, b, c) in [
            (p(0.0, 0.0, 0.0), p(1.0, 1.0, 1.0), p(2.0, 2.0, 2.0)),
            (p(0.0, 0.0, 0.0), p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0)),
            (p(0.0, 0.0, 0.0), p(1.0, 0.0, 0.0), p(2.0, 5e-7, 0.0)),
        ] {
            assert_eq!(
                Plane3::from_points(a, b, c, &TOL).err().map(|e| e.code()),
                Some("geom.construct.collinear_points")
            );
        }
    }

    #[test]
    fn distance_side_and_projection() {
        let plane = plane(p(0.0, 0.0, 2.0), v(0.0, 0.0, 1.0));
        assert_eq!(plane.signed_distance(&p(5.0, 5.0, 5.0)), 3.0);
        assert_eq!(plane.side(&p(0.0, 0.0, 5.0), &TOL), PlaneSide::Positive);
        assert_eq!(plane.side(&p(0.0, 0.0, 2.0 + 5e-7), &TOL), PlaneSide::On);
        assert_eq!(plane.side(&p(0.0, 0.0, 0.0), &TOL), PlaneSide::Negative);
        assert_eq!(plane.project(&p(3.0, 4.0, 9.0)), p(3.0, 4.0, 2.0));
        assert_eq!(plane.flipped().signed_distance(&p(0.0, 0.0, 5.0)), -3.0);
    }

    #[test]
    fn frames_follow_the_documented_axes() {
        let horizontal = plane(p(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0)).frame(&TOL);
        assert_eq!(
            (horizontal.u(), horizontal.v()),
            (v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0))
        );
        let down = plane(p(0.0, 0.0, 0.0), v(0.0, 0.0, -1.0)).frame(&TOL);
        assert_eq!((down.u(), down.v()), (v(1.0, 0.0, 0.0), v(0.0, -1.0, 0.0)));
        let wall = plane(p(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0)).frame(&TOL);
        assert_eq!((wall.u(), wall.v()), (v(-1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)));
        let hinted = plane(p(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0))
            .frame_with_x(&v(0.0, 2.0, 1.0), &TOL)
            .expect("frame");
        assert_eq!(hinted.u(), v(0.0, 1.0, 0.0));
        assert_eq!(
            plane(p(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0))
                .frame_with_x(&v(0.0, 0.0, 3.0), &TOL)
                .err()
                .map(|e| e.code()),
            Some("geom.direction.zero")
        );
    }

    #[test]
    fn frame_new_validates_axes() {
        let frame = PlaneFrame::new(p(1.0, 1.0, 1.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), &TOL)
            .expect("frame");
        assert_eq!(frame.normal(), v(0.0, 0.0, 1.0));
        assert!(
            PlaneFrame::new(p(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(1.0, 0.0, 0.0), &TOL).is_err()
        );
        assert!(
            PlaneFrame::new(p(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(0.0, 1.0, 0.0), &TOL).is_err()
        );
    }

    #[test]
    fn line_and_segment_intersections() {
        let plane = plane(p(0.0, 0.0, 1.0), v(0.0, 0.0, 1.0));
        let hit = plane
            .intersect_line(&p(1.0, 1.0, 0.0), &v(0.0, 0.0, 4.0), &TOL)
            .expect("line");
        assert_eq!(
            hit,
            LinePlane::Point {
                point: p(1.0, 1.0, 1.0),
                parameter: 0.25
            }
        );
        assert_eq!(
            plane
                .intersect_line(&p(0.0, 0.0, 2.0), &v(1.0, 0.0, 0.0), &TOL)
                .expect("line"),
            LinePlane::Parallel
        );
        assert_eq!(
            plane
                .intersect_line(&p(0.0, 0.0, 1.0), &v(1.0, 0.0, 0.0), &TOL)
                .expect("line"),
            LinePlane::Contained
        );
        assert_eq!(
            plane
                .intersect_line(&p(0.0, 0.0, 0.0), &v(0.0, 0.0, 0.0), &TOL)
                .err()
                .map(|e| e.code()),
            Some("geom.direction.zero")
        );
        let segment =
            |a: Point3, b: Point3| plane.intersect_segment(&a, &b, &TOL).expect("segment");
        assert_eq!(
            segment(p(0.0, 0.0, 0.0), p(0.0, 0.0, 4.0)),
            SegmentPlane::Point {
                point: p(0.0, 0.0, 1.0),
                parameter: 0.25
            }
        );
        assert_eq!(
            segment(p(0.0, 0.0, 2.0), p(0.0, 0.0, 4.0)),
            SegmentPlane::Disjoint
        );
        assert_eq!(
            segment(p(0.0, 0.0, 1.0), p(3.0, 0.0, 1.0)),
            SegmentPlane::Contained
        );
        assert_eq!(
            segment(p(0.0, 0.0, 1.0), p(0.0, 0.0, 3.0)),
            SegmentPlane::Point {
                point: p(0.0, 0.0, 1.0),
                parameter: 0.0
            }
        );
        assert_eq!(
            segment(p(0.0, 0.0, -1.0), p(0.0, 0.0, 1.0)),
            SegmentPlane::Point {
                point: p(0.0, 0.0, 1.0),
                parameter: 1.0
            }
        );
        assert_eq!(
            plane
                .intersect_segment(&p(0.0, 0.0, 0.0), &p(0.0, 0.0, 0.0), &TOL)
                .err()
                .map(|e| e.code()),
            Some("geom.segment.zero_length")
        );
    }

    #[test]
    fn plane_plane_intersections() {
        let floor = plane(p(0.0, 0.0, 0.0), v(0.0, 0.0, 1.0));
        let wall = plane(p(0.0, 3.0, 0.0), v(0.0, 1.0, 0.0));
        let PlanePlane::Line { point, direction } =
            floor.intersect_plane(&wall, &TOL).expect("line")
        else {
            panic!("planes cross");
        };
        assert_eq!(direction, v(-1.0, 0.0, 0.0));
        assert!((point - p(0.0, 3.0, 0.0)).norm() < 1e-12);
        assert_eq!(
            floor
                .intersect_plane(&plane(p(0.0, 0.0, 1.0), v(0.0, 0.0, -1.0)), &TOL)
                .expect("parallel"),
            PlanePlane::Parallel
        );
        assert_eq!(
            floor
                .intersect_plane(&plane(p(5.0, 5.0, 0.0), v(0.0, 0.0, -1.0)), &TOL)
                .expect("coincident"),
            PlanePlane::Coincident
        );
    }

    fn plane_strategy() -> impl Strategy<Value = Plane3> {
        (
            crate::testing::coordinate(),
            crate::testing::coordinate(),
            crate::testing::coordinate(),
            -1.0..1.0_f64,
            -1.0..1.0_f64,
            -1.0..1.0_f64,
        )
            .prop_filter_map("normal", |(x, y, z, a, b, c)| {
                Plane3::new(p(x, y, z), v(a, b, c))
                    .ok()
                    .filter(|_| a * a + b * b + c * c > 0.05)
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn frame_round_trips_and_is_orthonormal(plane in plane_strategy(), x in -50.0..50.0_f64, y in -50.0..50.0_f64) {
            let frame = plane.frame(&TOL);
            prop_assert!((frame.u().norm() - 1.0).abs() < 1e-12);
            prop_assert!(frame.u().dot(&frame.v()).abs() < 1e-12);
            prop_assert!((frame.u().cross(&frame.v()) - plane.normal()).norm() < 1e-12);
            let world = frame.to_world(&Point2::new(x, y));
            prop_assert!(plane.signed_distance(&world).abs() < 1e-10);
            let back = frame.to_plane(&world);
            prop_assert!((back - Point2::new(x, y)).norm() < 1e-10);
        }

        #[test]
        fn projection_lies_on_the_plane_at_the_signed_distance(plane in plane_strategy(), q in (-50.0..50.0_f64, -50.0..50.0_f64, -50.0..50.0_f64)) {
            let point = p(q.0, q.1, q.2);
            let foot = plane.project(&point);
            prop_assert!(plane.signed_distance(&foot).abs() < 1e-10);
            prop_assert!(((point - foot).norm() - plane.signed_distance(&point).abs()).abs() < 1e-10);
        }

        #[test]
        fn plane_intersection_lies_on_both_planes(a in plane_strategy(), b in plane_strategy()) {
            if let PlanePlane::Line { point, direction } = a.intersect_plane(&b, &TOL).expect("ok") {
                prop_assert!(a.signed_distance(&point).abs() < 1e-8);
                prop_assert!(b.signed_distance(&point).abs() < 1e-8);
                prop_assert!(a.normal().dot(&direction).abs() < 1e-9);
                prop_assert!(b.normal().dot(&direction).abs() < 1e-9);
            }
        }
    }
}
