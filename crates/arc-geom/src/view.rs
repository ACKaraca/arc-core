//! The view frame (CORE-GEOM-030): eye, target, up, projection and clip planes, with
//! world-to-view and view-to-world mappings in `f64`.
//!
//! # Conventions
//!
//! - **View space** is right-handed: x points right, y points up, and the camera looks along
//!   negative z. The basis `(right, up, forward)` of the frame is orthonormal and satisfies
//!   `right x up = -forward`, so `(right, up, -forward)` is right-handed.
//! - **Clip space** after the projection matrix: x and y in [-1, 1] (x right, y up), and
//!   normalized depth in [0, 1] with 0 on the near plane and 1 on the far plane. The GPU crate
//!   adapts this range to its own conventions.
//! - **Aspect ratio** is width divided by height of the target. It is not stored: one frame is
//!   shared by views of different sizes, so every function that needs it takes it as an
//!   argument. A perspective frame stores the *vertical* field of view; an orthographic frame
//!   stores the view *height* in model units, and the width is `height * aspect`.
//! - Near and far are distances along the forward direction from the eye. Orthographic frames
//!   may use a zero or negative near distance (clipping behind the eye plane); perspective
//!   frames need `near > 0`.
//!
//! Degeneracy of the eye/target distance and of the up vector is decided with the tolerance
//! given to [`ViewFrame::new`] (linear for the eye/target distance, angular for the angle
//! between up and the view direction). Deserialization validates with
//! [`Tolerance::DEFAULT`]. Everything else is exact, as in the rest of the crate.
//!
//! The mapping between world and view space is rigid and is computed directly from the basis;
//! no general matrix inversion is used.

use arc_base::math::{Matrix4, Point3, Vector3};
use arc_base::{Error, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::ensure_finite;
use crate::error::codes;

/// The projection of a view frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Projection {
    /// A perspective projection with the vertical field of view in radians, in (0, π).
    Perspective {
        /// The full vertical field of view in radians.
        vertical_fov: f64,
    },
    /// An orthographic projection.
    Orthographic {
        /// The visible height in model units (the view scale), greater than zero.
        height: f64,
    },
}

/// A validated view frame. See the [module documentation](self) for the conventions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ViewFrameRaw")]
pub struct ViewFrame {
    eye: Point3,
    target: Point3,
    #[serde(rename = "up")]
    up_hint: Vector3,
    projection: Projection,
    near: f64,
    far: f64,
    #[serde(skip_serializing)]
    right: Vector3,
    #[serde(skip_serializing)]
    up: Vector3,
    #[serde(skip_serializing)]
    forward: Vector3,
}

#[derive(Deserialize)]
struct ViewFrameRaw {
    eye: Point3,
    target: Point3,
    up: Vector3,
    projection: Projection,
    near: f64,
    far: f64,
}

impl TryFrom<ViewFrameRaw> for ViewFrame {
    type Error = Error;

    fn try_from(raw: ViewFrameRaw) -> Result<Self> {
        Self::new(
            raw.eye,
            raw.target,
            raw.up,
            raw.projection,
            raw.near,
            raw.far,
            &Tolerance::DEFAULT,
        )
    }
}

impl ViewFrame {
    /// Creates a view frame looking from `eye` toward `target`. `up` is a hint: the frame's up
    /// direction is `up` made orthogonal to the view direction.
    ///
    /// # Errors
    ///
    /// - `geom.input.not_finite` for a non-finite value.
    /// - `geom.view.eye_at_target` when the eye and target are closer than `tolerance.linear`.
    /// - `geom.view.up_invalid` when `up` is zero or its angle to the view direction (or its
    ///   opposite) is at most `tolerance.angular`.
    /// - `geom.view.fov_invalid` for a perspective field of view outside (0, π).
    /// - `geom.view.scale_not_positive` for an orthographic height that is not positive.
    /// - `geom.view.clip_invalid` when `near >= far`, or `near <= 0` for a perspective frame.
    pub fn new(
        eye: Point3,
        target: Point3,
        up: Vector3,
        projection: Projection,
        near: f64,
        far: f64,
        tolerance: &Tolerance,
    ) -> Result<Self> {
        ensure_finite(&[
            eye.x, eye.y, eye.z, target.x, target.y, target.z, up.x, up.y, up.z, near, far,
        ])?;
        match projection {
            Projection::Perspective { vertical_fov } => {
                ensure_finite(&[vertical_fov])?;
                if !(vertical_fov > 0.0 && vertical_fov < std::f64::consts::PI) {
                    return Err(
                        Error::new(codes::VIEW_FOV_INVALID).with_detail("fov", vertical_fov)
                    );
                }
                if near <= 0.0 {
                    return Err(Error::new(codes::VIEW_CLIP_INVALID).with_detail("near", near));
                }
            }
            Projection::Orthographic { height } => {
                ensure_finite(&[height])?;
                if height <= 0.0 {
                    return Err(
                        Error::new(codes::VIEW_SCALE_NOT_POSITIVE).with_detail("height", height)
                    );
                }
            }
        }
        if near >= far {
            return Err(Error::new(codes::VIEW_CLIP_INVALID)
                .with_detail("near", near)
                .with_detail("far", far));
        }
        let to_target = target - eye;
        let distance = to_target.norm();
        if !distance.is_finite() || distance <= tolerance.linear.max(0.0) {
            return Err(Error::new(codes::VIEW_EYE_AT_TARGET));
        }
        let forward = to_target / distance;
        let up_norm = up.norm();
        let side = forward.cross(&up);
        let side_norm = side.norm();
        // sin(angle between forward and up); both parallel and antiparallel give ~0.
        if !up_norm.is_finite()
            || up_norm == 0.0
            || side_norm / up_norm <= tolerance.angular.max(0.0)
        {
            return Err(Error::new(codes::VIEW_UP_INVALID));
        }
        let right = side / side_norm;
        let view_up = right.cross(&forward);
        Ok(Self {
            eye,
            target,
            up_hint: up,
            projection,
            near,
            far,
            right,
            up: view_up,
            forward,
        })
    }

    /// The eye position.
    #[must_use]
    pub fn eye(&self) -> Point3 {
        self.eye
    }

    /// The target point.
    #[must_use]
    pub fn target(&self) -> Point3 {
        self.target
    }

    /// The up hint as given to the constructor.
    #[must_use]
    pub fn up_hint(&self) -> Vector3 {
        self.up_hint
    }

    /// The projection.
    #[must_use]
    pub fn projection(&self) -> Projection {
        self.projection
    }

    /// The near clip distance along the view direction.
    #[must_use]
    pub fn near(&self) -> f64 {
        self.near
    }

    /// The far clip distance along the view direction.
    #[must_use]
    pub fn far(&self) -> f64 {
        self.far
    }

    /// The unit view direction, from the eye toward the target.
    #[must_use]
    pub fn forward(&self) -> Vector3 {
        self.forward
    }

    /// The unit vector pointing right in the view.
    #[must_use]
    pub fn right(&self) -> Vector3 {
        self.right
    }

    /// The unit vector pointing up in the view, orthogonal to [`Self::forward`].
    #[must_use]
    pub fn up(&self) -> Vector3 {
        self.up
    }

    /// Maps a world point to view space (eye at the origin, looking along negative z).
    #[must_use]
    pub fn world_to_view_point(&self, p: &Point3) -> Point3 {
        Point3::from(self.world_to_view_vector(&(p - self.eye)))
    }

    /// Maps a world vector to view space (rotation only).
    #[must_use]
    pub fn world_to_view_vector(&self, v: &Vector3) -> Vector3 {
        Vector3::new(self.right.dot(v), self.up.dot(v), -self.forward.dot(v))
    }

    /// Maps a view-space point to world space.
    #[must_use]
    pub fn view_to_world_point(&self, p: &Point3) -> Point3 {
        self.eye + self.view_to_world_vector(&p.coords)
    }

    /// Maps a view-space vector to world space (rotation only).
    #[must_use]
    pub fn view_to_world_vector(&self, v: &Vector3) -> Vector3 {
        self.right * v.x + self.up * v.y - self.forward * v.z
    }

    /// The world-to-view matrix.
    #[must_use]
    pub fn world_to_view(&self) -> Matrix4 {
        let (r, u, f) = (self.right, self.up, -self.forward);
        Matrix4::new(
            r.x,
            r.y,
            r.z,
            -r.dot(&self.eye.coords),
            u.x,
            u.y,
            u.z,
            -u.dot(&self.eye.coords),
            f.x,
            f.y,
            f.z,
            -f.dot(&self.eye.coords),
            0.0,
            0.0,
            0.0,
            1.0,
        )
    }

    /// The view-to-world matrix: the transpose rotation with the eye as translation.
    #[must_use]
    pub fn view_to_world(&self) -> Matrix4 {
        let (r, u, b) = (self.right, self.up, -self.forward);
        Matrix4::new(
            r.x, u.x, b.x, self.eye.x, r.y, u.y, b.y, self.eye.y, r.z, u.z, b.z, self.eye.z, 0.0,
            0.0, 0.0, 1.0,
        )
    }

    /// The projection matrix from view space to clip space (depth 0 at near, 1 at far).
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` unless `aspect` is finite and positive.
    pub fn projection_matrix(&self, aspect: f64) -> Result<Matrix4> {
        ensure_aspect(aspect)?;
        let (n, f) = (self.near, self.far);
        Ok(match self.projection {
            Projection::Perspective { vertical_fov } => {
                let k = 1.0 / (vertical_fov / 2.0).tan();
                let a = -f / (f - n);
                Matrix4::new(
                    k / aspect,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    k,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    a,
                    a * n,
                    0.0,
                    0.0,
                    -1.0,
                    0.0,
                )
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Matrix4::new(
                    1.0 / (h * aspect),
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    1.0 / h,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    -1.0 / (f - n),
                    -n / (f - n),
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                )
            }
        })
    }

    /// The inverse of [`Self::projection_matrix`], from clip space to view space, written in
    /// closed form. Apply it to homogeneous coordinates and divide by w.
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` unless `aspect` is finite and positive.
    pub fn inverse_projection_matrix(&self, aspect: f64) -> Result<Matrix4> {
        ensure_aspect(aspect)?;
        let (n, f) = (self.near, self.far);
        Ok(match self.projection {
            Projection::Perspective { vertical_fov } => {
                let k = 1.0 / (vertical_fov / 2.0).tan();
                let a = -f / (f - n);
                let b = a * n;
                Matrix4::new(
                    aspect / k,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    1.0 / k,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    -1.0,
                    0.0,
                    0.0,
                    1.0 / b,
                    a / b,
                )
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Matrix4::new(
                    h * aspect,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    h,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    -(f - n),
                    -n,
                    0.0,
                    0.0,
                    0.0,
                    1.0,
                )
            }
        })
    }

    /// Projects a world point to normalized device coordinates (x, y in [-1, 1] inside the
    /// view, depth in [0, 1] between the clip planes). Points outside the volume give values
    /// outside these ranges.
    ///
    /// Returns `None` for an invalid `aspect`, for a perspective point at or behind the eye
    /// plane, and when the result is not finite.
    #[must_use]
    pub fn project(&self, p: &Point3, aspect: f64) -> Option<Point3> {
        ensure_aspect(aspect).ok()?;
        let v = self.world_to_view_point(p);
        let (n, f) = (self.near, self.far);
        let ndc = match self.projection {
            Projection::Perspective { vertical_fov } => {
                let d = -v.z;
                if d <= 0.0 {
                    return None;
                }
                let k = 1.0 / (vertical_fov / 2.0).tan();
                Point3::new(
                    k * v.x / (aspect * d),
                    k * v.y / d,
                    f * (d - n) / ((f - n) * d),
                )
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Point3::new(v.x / (h * aspect), v.y / h, (-v.z - n) / (f - n))
            }
        };
        (ndc.x.is_finite() && ndc.y.is_finite() && ndc.z.is_finite()).then_some(ndc)
    }

    /// Maps normalized device coordinates (depth 0 at near, 1 at far) to a world point.
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` for an invalid `aspect`; `geom.input.not_finite` for a
    /// non-finite coordinate; `geom.precision.insufficient` for a perspective depth that maps
    /// to infinity (the horizon, beyond the far plane).
    pub fn unproject(&self, ndc: &Point3, aspect: f64) -> Result<Point3> {
        ensure_aspect(aspect)?;
        ensure_finite(&[ndc.x, ndc.y, ndc.z])?;
        let (n, f) = (self.near, self.far);
        let view = match self.projection {
            Projection::Perspective { vertical_fov } => {
                let t = (vertical_fov / 2.0).tan();
                // Solves z = f (d - n) / ((f - n) d) for the distance d along the view axis.
                let denominator = f - ndc.z * (f - n);
                let d = f * n / denominator;
                if denominator <= 0.0 || !d.is_finite() {
                    return Err(Error::new(codes::PRECISION_INSUFFICIENT));
                }
                Point3::new(ndc.x * aspect * t * d, ndc.y * t * d, -d)
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Point3::new(ndc.x * h * aspect, ndc.y * h, -(n + ndc.z * (f - n)))
            }
        };
        Ok(self.view_to_world_point(&view))
    }

    /// The pick ray through the normalized device position `(x, y)`: its origin and unit
    /// direction in world space.
    ///
    /// Orthographic: the direction is [`Self::forward`] and the origin lies on the eye plane,
    /// offset by `(x, y)`. Perspective: the origin is the eye.
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` for an invalid `aspect`; `geom.input.not_finite` for a
    /// non-finite coordinate.
    pub fn ray_through(&self, x: f64, y: f64, aspect: f64) -> Result<(Point3, Vector3)> {
        ensure_aspect(aspect)?;
        ensure_finite(&[x, y])?;
        Ok(match self.projection {
            Projection::Perspective { vertical_fov } => {
                let t = (vertical_fov / 2.0).tan();
                let direction =
                    self.view_to_world_vector(&Vector3::new(x * aspect * t, y * t, -1.0));
                (self.eye, direction.normalize())
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                let origin = self.view_to_world_point(&Point3::new(x * h * aspect, y * h, 0.0));
                (origin, self.forward)
            }
        })
    }
}

fn ensure_aspect(aspect: f64) -> Result<()> {
    if aspect.is_finite() && aspect > 0.0 {
        Ok(())
    } else {
        Err(Error::new(codes::VIEW_ASPECT_INVALID))
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, PI};

    use proptest::prelude::*;

    use super::*;

    const TOL: Tolerance = Tolerance::DEFAULT;

    fn frame(projection: Projection) -> ViewFrame {
        ViewFrame::new(
            Point3::new(1.0, 2.0, 3.0),
            Point3::new(1.0, 12.0, 3.0),
            Vector3::z(),
            projection,
            0.5,
            100.0,
            &TOL,
        )
        .unwrap()
    }

    fn persp() -> Projection {
        Projection::Perspective {
            vertical_fov: FRAC_PI_2,
        }
    }

    fn code(r: Result<ViewFrame>) -> &'static str {
        r.unwrap_err().code()
    }

    fn new(
        eye: Point3,
        target: Point3,
        up: Vector3,
        p: Projection,
        n: f64,
        f: f64,
    ) -> Result<ViewFrame> {
        ViewFrame::new(eye, target, up, p, n, f, &TOL)
    }

    #[test]
    fn degenerate_frames_have_stable_codes() {
        let o = Point3::origin();
        let x = Point3::new(1.0, 0.0, 0.0);
        let ortho = Projection::Orthographic { height: 2.0 };
        assert_eq!(
            code(new(o, o, Vector3::z(), persp(), 1.0, 2.0)),
            "geom.view.eye_at_target"
        );
        assert_eq!(
            code(new(o, x, Vector3::zeros(), persp(), 1.0, 2.0)),
            "geom.view.up_invalid"
        );
        assert_eq!(
            code(new(o, x, Vector3::x(), persp(), 1.0, 2.0)),
            "geom.view.up_invalid"
        );
        assert_eq!(
            code(new(o, x, -Vector3::x() * 3.0, persp(), 1.0, 2.0)),
            "geom.view.up_invalid"
        );
        let bad_fov = |fov| Projection::Perspective { vertical_fov: fov };
        for fov in [0.0, -1.0, PI, 4.0] {
            assert_eq!(
                code(new(o, x, Vector3::z(), bad_fov(fov), 1.0, 2.0)),
                "geom.view.fov_invalid"
            );
        }
        for height in [0.0, -2.0] {
            let p = Projection::Orthographic { height };
            assert_eq!(
                code(new(o, x, Vector3::z(), p, 1.0, 2.0)),
                "geom.view.scale_not_positive"
            );
        }
        assert_eq!(
            code(new(o, x, Vector3::z(), persp(), 0.0, 2.0)),
            "geom.view.clip_invalid"
        );
        assert_eq!(
            code(new(o, x, Vector3::z(), persp(), 2.0, 2.0)),
            "geom.view.clip_invalid"
        );
        assert_eq!(
            code(new(o, x, Vector3::z(), ortho, 3.0, 2.0)),
            "geom.view.clip_invalid"
        );
        assert_eq!(
            code(new(o, x, Vector3::z(), persp(), 1.0, f64::NAN)),
            "geom.input.not_finite"
        );
        assert_eq!(
            code(new(
                Point3::new(f64::INFINITY, 0.0, 0.0),
                x,
                Vector3::z(),
                ortho,
                1.0,
                2.0
            )),
            "geom.input.not_finite"
        );
        // Orthographic frames may clip behind the eye plane.
        assert!(new(o, x, Vector3::z(), ortho, -5.0, 2.0).is_ok());
    }

    #[test]
    fn invalid_aspect_is_rejected() {
        let f = frame(persp());
        for aspect in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(f.projection_matrix(aspect).is_err());
            assert!(f.project(&Point3::new(1.0, 5.0, 3.0), aspect).is_none());
            assert!(f.ray_through(0.0, 0.0, aspect).is_err());
        }
    }

    #[test]
    fn project_behind_eye_is_none() {
        let f = frame(persp());
        assert!(f.project(&Point3::new(1.0, -5.0, 3.0), 1.0).is_none());
        assert!(f.project(&Point3::new(1.0, 2.0, 3.0), 1.0).is_none());
        assert!(
            frame(Projection::Orthographic { height: 4.0 })
                .project(&Point3::new(1.0, -5.0, 3.0), 1.0)
                .is_some()
        );
    }

    #[test]
    fn known_projection_values() {
        let f = frame(persp());
        // Target direction is +y; the point on the axis at the near and far planes.
        let near = f.project(&Point3::new(1.0, 2.5, 3.0), 1.0).unwrap();
        let far = f.project(&Point3::new(1.0, 102.0, 3.0), 1.0).unwrap();
        assert!((near - Point3::new(0.0, 0.0, 0.0)).norm() < 1e-12);
        assert!((far - Point3::new(0.0, 0.0, 1.0)).norm() < 1e-12);
        // 90 degree field of view: a point at distance d, d above the axis is at the top edge.
        let edge = f.project(&Point3::new(1.0, 12.0, 13.0), 1.0).unwrap();
        assert!((edge.y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn serde_round_trip_and_validation() {
        let f = frame(persp());
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<ViewFrame>(&json).unwrap(), f);
        let bad = json.replace("\"near\":0.5", "\"near\":-1.0");
        assert!(serde_json::from_str::<ViewFrame>(&bad).is_err());
    }

    fn arb_frame() -> impl Strategy<Value = ViewFrame> {
        let p = || (-50.0..50.0_f64, -50.0..50.0_f64, -50.0..50.0_f64);
        let projection = prop_oneof![
            (0.2..3.0_f64).prop_map(|vertical_fov| Projection::Perspective { vertical_fov }),
            (0.5..50.0_f64).prop_map(|height| Projection::Orthographic { height }),
        ];
        (p(), p(), p(), projection, 0.1..5.0_f64, 1.0..200.0_f64).prop_filter_map(
            "valid frame",
            |(e, t, u, projection, near, span)| {
                ViewFrame::new(
                    Point3::new(e.0, e.1, e.2),
                    Point3::new(t.0, t.1, t.2),
                    Vector3::new(u.0, u.1, u.2),
                    projection,
                    near,
                    near + span,
                    &TOL,
                )
                .ok()
                .filter(|f| {
                    // Keep well-conditioned frames: target away from the eye, up not near parallel.
                    (f.target() - f.eye()).norm() > 0.5
                        && f.forward().cross(&f.up_hint()).norm() > 0.1 * f.up_hint().norm()
                })
            },
        )
    }

    fn arb_point() -> impl Strategy<Value = Point3> {
        (-100.0..100.0_f64, -100.0..100.0_f64, -100.0..100.0_f64)
            .prop_map(|(x, y, z)| Point3::new(x, y, z))
    }

    fn arb_ndc() -> impl Strategy<Value = Point3> {
        (-1.0..1.0_f64, -1.0..1.0_f64, 0.0..1.0_f64).prop_map(|(x, y, z)| Point3::new(x, y, z))
    }

    proptest! {
        #[test]
        fn basis_is_orthonormal_and_right_handed(f in arb_frame()) {
            let (r, u, fw) = (f.right(), f.up(), f.forward());
            for v in [r, u, fw] {
                prop_assert!((v.norm() - 1.0).abs() < 1e-12);
            }
            prop_assert!(r.dot(&u).abs() < 1e-12);
            prop_assert!(r.dot(&fw).abs() < 1e-12);
            prop_assert!(u.dot(&fw).abs() < 1e-12);
            prop_assert!((r.cross(&u) + fw).norm() < 1e-12);
            // The hint's side of the view direction is kept.
            prop_assert!(u.dot(&f.up_hint()) > 0.0);
            // The matrices agree with the mappings and are mutual inverses.
            prop_assert!((f.world_to_view() * f.view_to_world() - Matrix4::identity()).norm() < 1e-12);
            let w = f.eye() + Vector3::new(1.0, -2.0, 0.5);
            let via_matrix = f.world_to_view().transform_point(&w);
            prop_assert!((via_matrix - f.world_to_view_point(&w)).norm() < 1e-10);
            prop_assert!(f.world_to_view_point(&f.target()).x.abs() < 1e-9);
        }

        #[test]
        fn world_view_round_trip(f in arb_frame(), p in arb_point(), v in arb_point()) {
            let back = f.view_to_world_point(&f.world_to_view_point(&p));
            prop_assert!((back - p).norm() < 1e-9);
            let back_v = f.view_to_world_vector(&f.world_to_view_vector(&v.coords));
            prop_assert!((back_v - v.coords).norm() < 1e-9);
        }

        #[test]
        fn project_unproject_round_trip(f in arb_frame(), ndc in arb_ndc(), aspect in 0.2..5.0_f64) {
            let world = f.unproject(&ndc, aspect).unwrap();
            let again = f.project(&world, aspect).unwrap();
            prop_assert!((again - ndc).norm() < 1e-6, "{ndc:?} -> {again:?}");
            let world_again = f.unproject(&again, aspect).unwrap();
            prop_assert!((world_again - world).norm() < 1e-6 * (1.0 + world.coords.norm()));
        }

        #[test]
        fn matrices_agree_with_functions(f in arb_frame(), ndc in arb_ndc(), aspect in 0.2..5.0_f64) {
            let world = f.unproject(&ndc, aspect).unwrap();
            let view = f.world_to_view_point(&world);
            let clip = f.projection_matrix(aspect).unwrap() * view.to_homogeneous();
            let from_matrix = Point3::new(clip.x / clip.w, clip.y / clip.w, clip.z / clip.w);
            prop_assert!((from_matrix - ndc).norm() < 1e-6);
            let inverse = f.inverse_projection_matrix(aspect).unwrap() * clip;
            let back = Point3::new(inverse.x / inverse.w, inverse.y / inverse.w, inverse.z / inverse.w);
            prop_assert!((back - view).norm() < 1e-6 * (1.0 + view.coords.norm()));
        }

        #[test]
        fn ray_passes_through_unprojected_point(f in arb_frame(), ndc in arb_ndc(), aspect in 0.2..5.0_f64) {
            let world = f.unproject(&ndc, aspect).unwrap();
            let (origin, direction) = f.ray_through(ndc.x, ndc.y, aspect).unwrap();
            prop_assert!((direction.norm() - 1.0).abs() < 1e-12);
            let offset = world - origin;
            let distance = offset.cross(&direction).norm();
            prop_assert!(distance < 1e-6 * (1.0 + offset.norm()), "distance {distance}");
            match f.projection() {
                Projection::Perspective { .. } => {
                    prop_assert_eq!(origin, f.eye());
                    prop_assert!(offset.dot(&direction) > 0.0);
                }
                Projection::Orthographic { .. } => prop_assert_eq!(direction, f.forward()),
            }
        }
    }
}
