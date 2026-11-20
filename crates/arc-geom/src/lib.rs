//! Exact analytic geometry of the Arc suite.
//!
//! This first part provides the exact 2D curve kinds (CORE-GEOM-001), bulge polylines
//! (CORE-GEOM-002), the [`Curve2`] sum type with its parameterization, axis-aligned bounding
//! boxes, rigid, similarity and affine transforms (CORE-GEOM-029) and robust orientation and
//! in-circle predicates (CORE-GEOM-031).
//!
//! Every value is validated when it is constructed or deserialized, so an invalid curve cannot
//! exist. Arcs and ellipses are stored exactly and are never converted to splines or polylines.
//! Comparisons that depend on a tolerance take an [`arc_base::Tolerance`].
//!
//! No function in this crate panics.

mod arrange;
pub mod bbox;
pub mod boolean;
pub mod closest;
pub mod construct;
pub mod corner;
pub mod curve;
pub mod edit;
pub mod encoding;
pub mod error;
pub mod eval;
pub mod flatten;
pub mod index;
pub mod intersect;
pub mod kdtree;
pub mod measure;
pub mod nurbs;
pub mod offset;
pub mod plane3;
pub mod polyline;
pub mod predicates;
pub mod primitives;
pub mod region;
pub mod region_build;
pub mod region_offset;
pub mod solid;
pub mod solid_clip;
pub mod solid_gen;
pub mod solid_opening;
pub mod trace;
pub mod transform;
pub mod triangulate;
pub mod view;

pub use bbox::{Aabb2, Aabb3};
pub use boolean::{BooleanOp, boolean};
pub use closest::{CurvePoint, CurveProject, FootPoint};
pub use construct::{
    EdgeSide, MAX_POLYGON_SIDES, TangentCircle, arc_from_ends_and_radius, arc_from_start_center,
    arc_through_points, circle_on_diameter, circle_through_points, ellipse_from_axis,
    polygon_circumscribed, polygon_from_edge, polygon_inscribed, tangent_circles,
};
pub use corner::{
    ChamferSize, CornerMode, CornerPolyline, CornerResult, Replacement, SegmentPick, SkipReason,
    SkippedVertex, chamfer, chamfer_polyline_segments, chamfer_polyline_vertices, fillet,
    fillet_polyline_segments, fillet_polyline_vertices,
};
pub use curve::Curve2;
pub use edit::{
    Composite2, CurveEnd, JoinGap, Joined, extend, join, split_at_parameters, split_at_points, trim,
};
pub use encoding::{GeomCodec, GeomEncoding};
pub use eval::CurveEval;
pub use flatten::CurveFlatten;
pub use index::{SpatialIndex2, SpatialIndex3};
pub use intersect::{Contact, Intersection, Orientation, intersect, intersect_with_limits};
pub use kdtree::{KdTree2, KdTree3};
pub use measure::{Measurements, SecondMoments};
pub use nurbs::Nurbs2;
pub use offset::{JoinStyle, offset, offset_composite};
pub use plane3::{LinePlane, Plane3, PlaneFrame, PlanePlane, PlaneSide, SegmentPlane};
pub use polyline::{Polyline2, PolylineSegment, PolylineVertex};
pub use predicates::{Sign, incircle, orient2d};
pub use primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2};
pub use region::{Containment, Loop2, Region, RegionPart};
pub use region_build::build_region;
pub use region_offset::offset_region;
pub use solid::{FaceTag, PLANARITY_FACTOR, PolyFace, Polyhedron};
pub use solid_clip::ClipSide;
pub use solid_gen::{extrude, extrude_between, prism};
pub use solid_opening::PrismTool;
pub use trace::{IslandMode, trace_region};
pub use triangulate::{TriangleMesh2, triangulate, triangulate_with};
pub use view::{Projection, ViewFrame};

use arc_base::math::Vector2;
use arc_base::{Error, Result, Tolerance};

/// Fails with `geom.input.not_finite` unless every value is finite.
fn ensure_finite(values: &[f64]) -> Result<()> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(error::codes::INPUT_NOT_FINITE))
    }
}

/// Fails with `geom.tolerance.invalid` unless the linear and angular tolerances are finite and
/// not negative.
pub(crate) fn ensure_tolerance(tolerance: &Tolerance) -> Result<()> {
    // Written so that NaN fails the check.
    if [tolerance.linear, tolerance.angular]
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
    {
        Ok(())
    } else {
        Err(Error::new(error::codes::TOLERANCE_INVALID))
    }
}

/// The largest component magnitude of `v`.
pub(crate) fn largest_component(v: &Vector2) -> f64 {
    v.x.abs().max(v.y.abs())
}

/// The Euclidean length of `v`, without overflow or underflow of intermediate squares.
pub(crate) fn length(v: &Vector2) -> f64 {
    v.x.hypot(v.y)
}

/// `v` scaled to unit length. It is divided by its largest component magnitude first, so no
/// intermediate value overflows or underflows. The zero vector stays zero.
pub(crate) fn unit(v: &Vector2) -> Vector2 {
    let largest = largest_component(v);
    if largest == 0.0 {
        return *v;
    }
    let scaled = v / largest;
    scaled / length(&scaled)
}

/// Generates a private raw mirror of a validated type and a `TryFrom` that runs its `new`, so
/// deserialization cannot bypass validation. Field order must match the arguments of `new`.
macro_rules! deserialize_through_new {
    ($ty:ident, $raw:ident { $($field:ident: $field_ty:ty),* $(,)? }) => {
        #[derive(serde::Deserialize)]
        struct $raw {
            $($field: $field_ty),*
        }

        impl TryFrom<$raw> for $ty {
            type Error = arc_base::Error;

            fn try_from(raw: $raw) -> arc_base::Result<Self> {
                Self::new($(raw.$field),*)
            }
        }
    };
}
pub(crate) use deserialize_through_new;

#[cfg(test)]
pub(crate) mod testing {
    //! Strategies shared by the property tests of this crate.

    use arc_base::math::{Point2, Transform2, Vector2};
    use proptest::prelude::*;

    pub fn coordinate() -> impl Strategy<Value = f64> {
        -100.0..100.0_f64
    }

    pub fn point() -> impl Strategy<Value = Point2> {
        (coordinate(), coordinate()).prop_map(|(x, y)| Point2::new(x, y))
    }

    pub fn nonzero_vector() -> impl Strategy<Value = Vector2> {
        (coordinate(), coordinate())
            .prop_map(|(x, y)| Vector2::new(x, y))
            .prop_filter("nonzero", |v| v.norm() > 0.1)
    }

    pub fn angle() -> impl Strategy<Value = f64> {
        -10.0..10.0_f64
    }

    /// A signed sweep with magnitude in (0, 2π].
    pub fn sweep() -> impl Strategy<Value = f64> {
        (0.05..=std::f64::consts::TAU, any::<bool>())
            .prop_map(|(magnitude, ccw)| if ccw { magnitude } else { -magnitude })
    }

    /// A well-conditioned general affine transform.
    pub fn affine() -> impl Strategy<Value = Transform2> {
        (
            -3.0..3.0_f64,
            -3.0..3.0_f64,
            -3.0..3.0_f64,
            -3.0..3.0_f64,
            coordinate(),
            coordinate(),
        )
            .prop_filter("well conditioned", |(a, b, c, d, _, _)| {
                (a * d - b * c).abs() > 0.5
            })
            .prop_map(|(a, b, c, d, e, f)| {
                Transform2::from_matrix_unchecked(arc_base::math::Matrix3::new(
                    a, b, e, c, d, f, 0.0, 0.0, 1.0,
                ))
            })
    }

    /// A similarity: rotation, uniform scale, optional mirror and translation.
    pub fn similarity() -> impl Strategy<Value = Transform2> {
        similarity_with_scale().prop_map(|(transform, _)| transform)
    }

    /// A similarity together with its scale factor.
    pub fn similarity_with_scale() -> impl Strategy<Value = (Transform2, f64)> {
        (
            angle(),
            0.2..5.0_f64,
            any::<bool>(),
            coordinate(),
            coordinate(),
        )
            .prop_map(|(rotation, scale, mirror, e, f)| {
                let (sin, cos) = rotation.sin_cos();
                let m = if mirror { -1.0 } else { 1.0 };
                let transform = Transform2::from_matrix_unchecked(arc_base::math::Matrix3::new(
                    scale * cos,
                    -scale * sin * m,
                    e,
                    scale * sin,
                    scale * cos * m,
                    f,
                    0.0,
                    0.0,
                    1.0,
                ));
                (transform, scale)
            })
    }

    /// A positive magnitude spread log-uniformly over [10^low, 10^high].
    pub fn log_uniform(low: f64, high: f64) -> impl Strategy<Value = f64> {
        (low..=high).prop_map(|exponent| 10.0_f64.powf(exponent))
    }

    /// The "extreme magnitude" variant of [`coordinate`]: either sign, log-uniform over
    /// roughly 1e-150 to 1e150, or zero.
    pub fn extreme_coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![
            8 => (log_uniform(-150.0, 150.0), any::<bool>())
                .prop_map(|(m, negative)| if negative { -m } else { m }),
            1 => Just(0.0),
        ]
    }

    /// A point with extreme-magnitude coordinates.
    pub fn extreme_point() -> impl Strategy<Value = Point2> {
        (extreme_coordinate(), extreme_coordinate()).prop_map(|(x, y)| Point2::new(x, y))
    }

    /// A nonzero vector with extreme-magnitude components.
    pub fn extreme_vector() -> impl Strategy<Value = Vector2> {
        (extreme_coordinate(), extreme_coordinate())
            .prop_map(|(x, y)| Vector2::new(x, y))
            .prop_filter("nonzero", |v| v.x != 0.0 || v.y != 0.0)
    }

    /// A length log-uniform over roughly 1e-150 to 1e150.
    pub fn extreme_length() -> impl Strategy<Value = f64> {
        log_uniform(-150.0, 150.0)
    }

    /// An angle up to ±1e6 radians.
    pub fn extreme_angle() -> impl Strategy<Value = f64> {
        -1e6..1e6_f64
    }

    /// A nonzero bulge of either sign: ordinary values, or a magnitude log-uniform over 1e-300
    /// to 1e15 (bulges of about 1e16 and more are rejected).
    pub fn wide_bulge() -> impl Strategy<Value = f64> {
        prop_oneof![
            1 => -3.0..3.0_f64,
            2 => (log_uniform(-300.0, 15.0), any::<bool>())
                .prop_map(|(m, negative)| if negative { -m } else { m }),
        ]
    }

    /// A minor-to-major ratio log-uniform over 1e-12 to 1.
    pub fn wide_ratio() -> impl Strategy<Value = f64> {
        log_uniform(-12.0, 0.0).prop_map(|k| k.min(1.0))
    }

    /// A query point at a distance log-uniform over 1e-3 to 1e12 from the origin.
    pub fn far_point() -> impl Strategy<Value = Point2> {
        (log_uniform(-3.0, 12.0), angle()).prop_map(|(r, a)| Point2::new(r * a.cos(), r * a.sin()))
    }
}
