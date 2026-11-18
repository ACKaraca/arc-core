//! Camera-relative precision and reversed-Z projection (CORE-GPU-004, CORE-GPU-007).
//!
//! # Conventions
//!
//! - The GPU never sees absolute coordinates. Each frame, the translation of every render item
//!   relative to the eye is computed in `f64` (`origin − eye`) and narrowed to `f32`; vertex
//!   positions are `f32` relative to the item origin. The view matrix on the GPU is the
//!   rotation of the [`ViewFrame`] without its translation.
//! - Depth is reversed: normalized depth is 1 on the near plane and 0 on the far plane, the
//!   opposite of the [`ViewFrame`] convention (0 at near, 1 at far). The reversed projection is
//!   built directly in `f64` rather than by flipping the frame's matrix, so no precision is lost
//!   to the subtraction `1 − depth`. The depth target is `Depth32Float`, cleared to 0 and tested
//!   with *greater*.
//! - Perspective views use the frame's finite near and far distances. Orthographic views use
//!   the frame's range intersected with the depth range of the scene's bounds, so depth
//!   precision is spent only where there is geometry, and translations are taken relative to
//!   the near end of that range rather than the eye (see [`depth_origin`]).

use arc_base::math::{Matrix4, Point3, Vector3, vector3_to_gpu};
use arc_base::{Error, Result};
use arc_geom::view::{Projection, ViewFrame};

use crate::error::codes;

/// The camera-relative translation of a point, narrowed to `f32`.
///
/// # Errors
///
/// `gpu.precision.insufficient` when the difference exceeds the range of `f32`.
pub fn relative_translation(origin: &Point3, eye: &Point3) -> Result<[f32; 3]> {
    let t = vector3_to_gpu(&(origin - eye));
    if t.is_finite() {
        Ok(t.to_array())
    } else {
        Err(Error::new(codes::PRECISION_INSUFFICIENT))
    }
}

/// The reversed-Z projection of `frame` for `aspect` and the depth range `near..far`.
#[must_use]
pub fn reversed_projection(frame: &ViewFrame, aspect: f64, near: f64, far: f64) -> Matrix4 {
    let span = far - near;
    match frame.projection() {
        Projection::Perspective { vertical_fov } => {
            let k = 1.0 / (vertical_fov / 2.0).tan();
            #[rustfmt::skip]
            let m = Matrix4::new(
                k / aspect, 0.0, 0.0, 0.0,
                0.0, k, 0.0, 0.0,
                0.0, 0.0, near / span, near * far / span,
                0.0, 0.0, -1.0, 0.0,
            );
            m
        }
        Projection::Orthographic { height } => {
            let h = height / 2.0;
            #[rustfmt::skip]
            let m = Matrix4::new(
                1.0 / (h * aspect), 0.0, 0.0, 0.0,
                0.0, 1.0 / h, 0.0, 0.0,
                0.0, 0.0, 1.0 / span, far / span,
                0.0, 0.0, 0.0, 1.0,
            );
            m
        }
    }
}

/// The world-to-view rotation of `frame`, without translation.
#[must_use]
pub fn view_rotation(frame: &ViewFrame) -> Matrix4 {
    let mut m = frame.world_to_view();
    m[(0, 3)] = 0.0;
    m[(1, 3)] = 0.0;
    m[(2, 3)] = 0.0;
    m
}

/// The depth range used for `frame`: the frame's own for perspective views; for orthographic
/// views the frame's range intersected with `bounds` (view-space distances along the forward
/// direction, with a margin of 1 % of their span), unless that intersection is empty.
#[must_use]
pub fn depth_range(frame: &ViewFrame, bounds: Option<(f64, f64)>) -> (f64, f64) {
    let (near, far) = (frame.near(), frame.far());
    match (frame.projection(), bounds) {
        (Projection::Orthographic { .. }, Some((low, high))) => {
            let margin = (high - low).abs() * 0.01 + f64::EPSILON * high.abs().max(low.abs());
            let fitted = (near.max(low - margin), far.min(high + margin));
            if fitted.0 < fitted.1 && fitted.0.is_finite() && fitted.1.is_finite() {
                fitted
            } else {
                (near, far)
            }
        }
        _ => (near, far),
    }
}

/// The point translations are taken relative to, and the depth range relative to it. For a
/// perspective view these are the eye and [`depth_range`]. An orthographic projection does not
/// change when the eye moves along the view direction, so for an orthographic view the point is
/// moved to the near end of the fitted range and the range starts at 0: depth is then computed
/// from small numbers even when the eye is far from the scene.
#[must_use]
pub fn depth_origin(frame: &ViewFrame, bounds: Option<(f64, f64)>) -> (Point3, (f64, f64)) {
    let (near, far) = depth_range(frame, bounds);
    match frame.projection() {
        Projection::Perspective { .. } => (frame.eye(), (near, far)),
        Projection::Orthographic { .. } => {
            (frame.eye() + frame.forward() * near, (0.0, far - near))
        }
    }
}

/// The view-space distance range along `forward` of a sphere.
#[must_use]
pub fn sphere_depth(frame: &ViewFrame, center: &Point3, radius: f64) -> (f64, f64) {
    let d = (center - frame.eye()).dot(&frame.forward());
    (d - radius, d + radius)
}

/// The per-frame camera data uploaded to the GPU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CameraData {
    /// Projection times rotation, column-major `f32`.
    pub view_proj: [f32; 16],
    /// The direction toward the headlight (toward the eye), world space.
    pub light: [f32; 3],
    /// `P[1][1]`: pixels per model unit at view distance 1, divided by half the image height.
    pub y_scale: f64,
}

pub(crate) fn camera_data(frame: &ViewFrame, aspect: f64, depth: (f64, f64)) -> Result<CameraData> {
    let p = reversed_projection(frame, aspect, depth.0, depth.1);
    let m = p * view_rotation(frame);
    if !m.iter().all(|v| v.is_finite()) {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let gpu = arc_base::math::matrix4_to_gpu(&m);
    if !gpu.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let back: Vector3 = -frame.forward();
    Ok(CameraData {
        view_proj: gpu.to_cols_array(),
        light: vector3_to_gpu(&back).to_array(),
        y_scale: p[(1, 1)],
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn frame(projection: Projection, near: f64, far: f64) -> ViewFrame {
        ViewFrame::new(
            Point3::new(1.0, 2.0, 3.0),
            Point3::new(1.0, 2.0, -10.0),
            Vector3::y(),
            projection,
            near,
            far,
        )
        .unwrap()
    }

    fn depth_at(frame: &ViewFrame, distance: f64) -> f64 {
        let m = reversed_projection(frame, 1.5, frame.near(), frame.far());
        let z = m[(2, 2)] * -distance + m[(2, 3)];
        let w = m[(3, 2)] * -distance + m[(3, 3)];
        z / w
    }

    #[test]
    fn near_maps_to_one_and_far_to_zero() {
        for projection in [
            Projection::Perspective { vertical_fov: 1.0 },
            Projection::Orthographic { height: 4.0 },
        ] {
            let f = frame(projection, 0.5, 100.0);
            assert!((depth_at(&f, 0.5) - 1.0).abs() < 1e-12);
            assert!(depth_at(&f, 100.0).abs() < 1e-12);
        }
    }

    #[test]
    fn depth_order_holds_at_a_ratio_of_a_million() {
        let f = frame(Projection::Perspective { vertical_fov: 1.0 }, 0.01, 1e4);
        // Neighbors 1 mm apart at 5 km stay distinct and ordered in f32.
        let narrow = |v: f64| arc_base::math::vector3_to_gpu(&Vector3::new(v, 0.0, 0.0)).x;
        let a = narrow(depth_at(&f, 5000.0));
        let b = narrow(depth_at(&f, 5000.001));
        assert!(a > b, "{a} {b}");
    }

    #[test]
    fn orthographic_range_fits_the_bounds() {
        let f = frame(Projection::Orthographic { height: 4.0 }, -1e6, 1e6);
        let (n, fa) = depth_range(&f, Some((10.0, 20.0)));
        assert!(n < 10.0 && n > 9.8 && fa > 20.0 && fa < 20.2);
        let p = frame(Projection::Perspective { vertical_fov: 1.0 }, 0.1, 50.0);
        assert_eq!(depth_range(&p, Some((10.0, 20.0))), (0.1, 50.0));
        assert_eq!(depth_range(&f, Some((2e6, 3e6))), (-1e6, 1e6));
    }

    proptest! {
        /// CORE-GPU-004: a vertex of a 1 km item (within 500 m of its origin on each axis), with
        /// the item up to 10,000 km from the model origin and the eye inside the item's extent,
        /// lands within 0.1 mm of its exact camera-relative position after the `f32` narrowing
        /// of translation and vertex and their `f32` sum.
        #[test]
        fn camera_relative_vertex_error_is_below_a_tenth_of_a_millimetre(
            origin in prop::array::uniform3(-1e7_f64..1e7),
            eye_offset in prop::array::uniform3(-500.0_f64..500.0),
            vertex in prop::array::uniform3(-500.0_f64..500.0),
        ) {
            let origin = Point3::from(origin);
            let eye = origin + Vector3::from(eye_offset);
            let vertex = Vector3::from(vertex);
            let t = relative_translation(&origin, &eye).unwrap();
            let v = vector3_to_gpu(&vertex);
            for axis in 0..3 {
                // The GPU adds the two f32 values in f32.
                let gpu = f64::from(t[axis] + v[axis]);
                let exact = origin[axis] + vertex[axis] - eye[axis];
                prop_assert!((gpu - exact).abs() <= 1e-4, "axis {axis}: {gpu} vs {exact}");
            }
        }
    }
}
