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
//! Degeneracy is decided exactly, as in the rest of the crate, so the validity of a frame never
//! depends on the tolerance of a document and deserialization validates exactly as
//! [`ViewFrame::new`] does: the eye must differ from the target, and the up hint must be nonzero
//! and not parallel to the view direction (its cross product with the computed unit view
//! direction is not exactly zero). Lengths and unit vectors are computed without squaring
//! unscaled components, so every finite magnitude is accepted.
//!
//! A frame whose matrices cannot be represented in `f64` (for example a field of view or height
//! so small that its reciprocal overflows, a depth range whose span overflows, or an eye so far
//! out that the world-to-view translation overflows) is rejected with
//! `geom.precision.insufficient`. Functions that return a `Result` fail with the same code
//! instead of returning a non-finite value; [`ViewFrame::project`] returns `None`. The point
//! and vector mappings return plain values and are non-finite only when the exact result lies
//! outside the range of `f64`.
//!
//! The mapping between world and view space is rigid and is computed directly from the basis;
//! no general matrix inversion is used.

use arc_base::math::{Matrix4, Point3, Vector3};
use arc_base::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::{deserialize_through_new, ensure_finite};

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

deserialize_through_new!(
    ViewFrame,
    ViewFrameRaw {
        eye: Point3,
        target: Point3,
        up: Vector3,
        projection: Projection,
        near: f64,
        far: f64,
    }
);

/// `a·b − c·d` to within two units in the last place, and exactly zero only when the exact
/// value is zero (Kahan's algorithm with fused multiply-adds).
fn difference_of_products(a: f64, b: f64, c: f64, d: f64) -> f64 {
    let w = c * d;
    // The rounding error of w, exactly.
    let error = (-c).mul_add(d, w);
    a.mul_add(b, -w) + error
}

/// The cross product with every component computed as an accurate difference of products.
fn cross(a: &Vector3, b: &Vector3) -> Vector3 {
    Vector3::new(
        difference_of_products(a.y, b.z, a.z, b.y),
        difference_of_products(a.z, b.x, a.x, b.z),
        difference_of_products(a.x, b.y, a.y, b.x),
    )
}

/// `v` scaled to unit length, or `None` for a zero or non-finite vector. It is divided by its
/// largest component magnitude first, so no intermediate value overflows or underflows.
fn unit(v: &Vector3) -> Option<Vector3> {
    let largest = v.x.abs().max(v.y.abs()).max(v.z.abs());
    if !(largest > 0.0 && largest.is_finite()) {
        return None;
    }
    let scaled = v / largest;
    Some(scaled / scaled.norm())
}

fn all_finite(m: &Matrix4) -> bool {
    m.iter().all(|value| value.is_finite())
}

fn insufficient() -> Error {
    Error::new(codes::PRECISION_INSUFFICIENT)
}

impl ViewFrame {
    /// Creates a view frame looking from `eye` toward `target`. `up` is a hint: the frame's up
    /// direction is `up` made orthogonal to the view direction.
    ///
    /// # Errors
    ///
    /// - `geom.input.not_finite` for a non-finite value.
    /// - `geom.view.eye_at_target` when the eye and target are the same point.
    /// - `geom.view.up_invalid` when `up` is zero or parallel to the view direction.
    /// - `geom.view.fov_invalid` for a perspective field of view outside (0, π).
    /// - `geom.view.scale_not_positive` for an orthographic height that is not positive.
    /// - `geom.view.clip_invalid` when `near >= far`, or `near <= 0` for a perspective frame.
    /// - `geom.precision.insufficient` when the frame's matrices cannot be represented in
    ///   `f64` (see the [module documentation](self)).
    pub fn new(
        eye: Point3,
        target: Point3,
        up: Vector3,
        projection: Projection,
        near: f64,
        far: f64,
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
        // Halving is exact for coordinates large enough to overflow the difference.
        let mut to_target = target - eye;
        if !to_target.iter().all(|c| c.is_finite()) {
            to_target = target * 0.5 - eye * 0.5;
        }
        // Distinct finite values never have a zero difference.
        let forward = unit(&to_target).ok_or_else(|| Error::new(codes::VIEW_EYE_AT_TARGET))?;
        let right = unit(&up)
            .and_then(|up| unit(&cross(&forward, &up)))
            .ok_or_else(|| Error::new(codes::VIEW_UP_INVALID))?;
        let frame = Self {
            eye,
            target,
            up_hint: up,
            projection,
            near,
            far,
            right,
            up: right.cross(&forward),
            forward,
        };
        // Aspect 1 gives exactly the entries that do not depend on the aspect; if one of them is
        // not finite, no aspect gives finite matrices.
        if all_finite(&frame.world_to_view())
            && all_finite(&frame.raw_projection(1.0))
            && all_finite(&frame.raw_inverse_projection(1.0))
        {
            Ok(frame)
        } else {
            Err(insufficient())
        }
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

    /// The world-to-view matrix. Its entries are finite for every valid frame.
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
    /// `geom.view.aspect_invalid` unless `aspect` is finite and positive;
    /// `geom.precision.insufficient` when an entry that depends on `aspect` is not finite.
    pub fn projection_matrix(&self, aspect: f64) -> Result<Matrix4> {
        ensure_aspect(aspect)?;
        let m = self.raw_projection(aspect);
        if all_finite(&m) {
            Ok(m)
        } else {
            Err(insufficient())
        }
    }

    fn raw_projection(&self, aspect: f64) -> Matrix4 {
        let (n, f) = (self.near, self.far);
        match self.projection {
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
        }
    }

    /// The inverse of [`Self::projection_matrix`], from clip space to view space, written in
    /// closed form. Apply it to homogeneous coordinates and divide by w.
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` unless `aspect` is finite and positive;
    /// `geom.precision.insufficient` when an entry that depends on `aspect` is not finite.
    pub fn inverse_projection_matrix(&self, aspect: f64) -> Result<Matrix4> {
        ensure_aspect(aspect)?;
        let m = self.raw_inverse_projection(aspect);
        if all_finite(&m) {
            Ok(m)
        } else {
            Err(insufficient())
        }
    }

    fn raw_inverse_projection(&self, aspect: f64) -> Matrix4 {
        let (n, f) = (self.near, self.far);
        match self.projection {
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
        }
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
                // The plain quotient is exactly 1 at the far plane; the factored one cannot
                // overflow or underflow, so it is used when a product of the plain one would.
                let (numerator, denominator) = (f * (d - n), (f - n) * d);
                let depth =
                    if denominator.is_normal() && (numerator == 0.0 || numerator.is_normal()) {
                        numerator / denominator
                    } else {
                        f / (f - n) * ((d - n) / d)
                    };
                Point3::new(k * (v.x / d) / aspect, k * (v.y / d), depth)
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Point3::new(v.x / h / aspect, v.y / h, (-v.z - n) / (f - n))
            }
        };
        ndc.iter().all(|c| c.is_finite()).then_some(ndc)
    }

    /// Maps normalized device coordinates (depth 0 at near, 1 at far) to a world point.
    ///
    /// # Errors
    ///
    /// `geom.view.aspect_invalid` for an invalid `aspect`; `geom.input.not_finite` for a
    /// non-finite coordinate; `geom.precision.insufficient` for a perspective depth that maps
    /// to infinity (the horizon, beyond the far plane) or a point outside the range of `f64`.
    pub fn unproject(&self, ndc: &Point3, aspect: f64) -> Result<Point3> {
        ensure_aspect(aspect)?;
        ensure_finite(&[ndc.x, ndc.y, ndc.z])?;
        let (n, f) = (self.near, self.far);
        let view = match self.projection {
            Projection::Perspective { vertical_fov } => {
                let t = (vertical_fov / 2.0).tan();
                // Solves z = f (d - n) / ((f - n) d) for the distance d along the view axis.
                let denominator = f - ndc.z * (f - n);
                if denominator <= 0.0 {
                    return Err(insufficient());
                }
                let d = f / denominator * n;
                Point3::new(ndc.x * aspect * t * d, ndc.y * t * d, -d)
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                Point3::new(ndc.x * h * aspect, ndc.y * h, -(n + ndc.z * (f - n)))
            }
        };
        let world = self.view_to_world_point(&view);
        if view.iter().chain(world.iter()).all(|c| c.is_finite()) {
            Ok(world)
        } else {
            Err(insufficient())
        }
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
    /// non-finite coordinate; `geom.precision.insufficient` when the direction or origin is
    /// outside the range of `f64`.
    pub fn ray_through(&self, x: f64, y: f64, aspect: f64) -> Result<(Point3, Vector3)> {
        ensure_aspect(aspect)?;
        ensure_finite(&[x, y])?;
        match self.projection {
            Projection::Perspective { vertical_fov } => {
                let t = (vertical_fov / 2.0).tan();
                let view =
                    unit(&Vector3::new(x * aspect * t, y * t, -1.0)).ok_or_else(insufficient)?;
                Ok((self.eye, self.view_to_world_vector(&view)))
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                let origin = self.view_to_world_point(&Point3::new(x * h * aspect, y * h, 0.0));
                if origin.iter().all(|c| c.is_finite()) {
                    Ok((origin, self.forward))
                } else {
                    Err(insufficient())
                }
            }
        }
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

    fn frame(projection: Projection) -> ViewFrame {
        ViewFrame::new(
            Point3::new(1.0, 2.0, 3.0),
            Point3::new(1.0, 12.0, 3.0),
            Vector3::z(),
            projection,
            0.5,
            100.0,
        )
        .unwrap()
    }

    fn persp() -> Projection {
        Projection::Perspective {
            vertical_fov: FRAC_PI_2,
        }
    }

    fn code<T>(r: Result<T>) -> &'static str {
        r.err().map_or("ok", |e| e.code())
    }

    fn new(
        eye: Point3,
        target: Point3,
        up: Vector3,
        p: Projection,
        n: f64,
        f: f64,
    ) -> Result<ViewFrame> {
        ViewFrame::new(eye, target, up, p, n, f)
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

    fn x_frame(up: Vector3, projection: Projection, near: f64, far: f64) -> Result<ViewFrame> {
        new(
            Point3::origin(),
            Point3::new(1.0, 0.0, 0.0),
            up,
            projection,
            near,
            far,
        )
    }

    fn assert_orthonormal(f: &ViewFrame) {
        let (r, u, fw) = (f.right(), f.up(), f.forward());
        for v in [r, u, fw] {
            assert!((v.norm() - 1.0).abs() < 1e-15, "{f:?}");
        }
        assert!(r.dot(&u).abs() < 1e-15 && r.dot(&fw).abs() < 1e-15 && u.dot(&fw).abs() < 1e-15);
        assert!((r.cross(&u) + fw).norm() < 1e-15);
    }

    #[test]
    fn validity_does_not_depend_on_a_tolerance_so_serde_round_trips() {
        // Eye and target 1e-7 apart, and up 1e-10 rad from the view direction, were rejected
        // on deserialization while a tighter tolerance accepted them on construction.
        let close = new(
            Point3::origin(),
            Point3::new(1e-7, 0.0, 0.0),
            Vector3::z(),
            persp(),
            0.1,
            1.0,
        )
        .unwrap();
        let nearly_parallel = x_frame(Vector3::new(1.0, 1e-10, 0.0), persp(), 0.1, 1.0).unwrap();
        for f in [close, nearly_parallel] {
            let json = serde_json::to_string(&f).unwrap();
            assert_eq!(serde_json::from_str::<ViewFrame>(&json).unwrap(), f);
            assert_orthonormal(&f);
        }
    }

    #[test]
    fn extreme_magnitudes_are_accepted() {
        let far_apart = |e: f64, t: f64| {
            new(
                Point3::new(e, 0.0, 0.0),
                Point3::new(t, 0.0, 0.0),
                Vector3::z(),
                persp(),
                1.0,
                2.0,
            )
        };
        for (e, t) in [(-1e200, 1e200), (-f64::MAX, f64::MAX), (0.0, 5e-324)] {
            let f = far_apart(e, t).unwrap();
            assert_eq!(f.forward(), Vector3::x());
            assert_orthonormal(&f);
        }
        for up in [
            Vector3::z() * 1e-170,
            Vector3::z() * 1e170,
            Vector3::new(5e-324, 0.0, 5e-324),
            Vector3::new(f64::MAX, -f64::MAX, f64::MAX),
            // Within 1e-300 rad of parallel.
            Vector3::new(1.0, 1e-300, 0.0),
            Vector3::new(-1.0, 0.0, 1e-300),
        ] {
            let f = x_frame(up, persp(), 1.0, 2.0).unwrap();
            assert_orthonormal(&f);
            assert!(f.up().dot(&(up / up.amax())) > 0.0);
        }
        // Nearly parallel off the axes.
        let d = Vector3::new(1.0, 2.0, 3.0);
        let f = new(
            Point3::origin(),
            Point3::from(d),
            d + Vector3::new(3.0, 0.0, -1.0) * 1e-14,
            persp(),
            1.0,
            2.0,
        )
        .unwrap();
        assert_orthonormal(&f);
    }

    #[test]
    fn unrepresentable_frames_are_rejected() {
        let ortho = |height| Projection::Orthographic { height };
        let insufficient = "geom.precision.insufficient";
        assert_eq!(
            code(x_frame(Vector3::z(), ortho(1e-320), 0.1, 1.0)),
            insufficient
        );
        assert_eq!(
            code(x_frame(Vector3::z(), persp(), 1e-320, 1.0)),
            insufficient
        );
        let tiny_fov = Projection::Perspective {
            vertical_fov: 5e-324,
        };
        assert_eq!(
            code(x_frame(Vector3::z(), tiny_fov, 0.1, 1.0)),
            insufficient
        );
        // The depth span overflows.
        assert_eq!(
            code(x_frame(Vector3::z(), ortho(1.0), -1.7e308, 1.7e308)),
            insufficient
        );
        // f n / (f - n) overflows.
        let n = 1e300;
        assert_eq!(
            code(x_frame(Vector3::z(), persp(), n, n.next_up())),
            insufficient
        );
        // The world-to-view translation overflows.
        let m = f64::MAX;
        let eye = Point3::new(m, m, m);
        assert_eq!(
            code(new(eye, Point3::origin(), Vector3::z(), persp(), 1.0, 2.0)),
            insufficient
        );
    }

    #[test]
    fn non_finite_results_are_errors() {
        let insufficient = "geom.precision.insufficient";
        let wide = x_frame(
            Vector3::z(),
            Projection::Perspective { vertical_fov: 3.0 },
            0.1,
            1.0,
        )
        .unwrap();
        // Was Ok with a zero direction.
        let (_, direction) = wide.ray_through(1.0, 0.0, 1e300).unwrap();
        assert!((direction.norm() - 1.0).abs() < 1e-15);
        assert_eq!(code(wide.ray_through(1e300, 0.0, 1e300)), insufficient);
        // Was Ok with NaN and infinite coordinates.
        let far_out = Point3::new(1e300, 0.0, 0.5);
        assert_eq!(code(wide.unproject(&far_out, 1e300)), insufficient);
        let ortho = x_frame(
            Vector3::z(),
            Projection::Orthographic { height: 1.0 },
            0.1,
            1.0,
        )
        .unwrap();
        assert_eq!(code(ortho.projection_matrix(1e-310)), insufficient);
        assert_eq!(code(ortho.inverse_projection_matrix(1e300)), "ok");
        assert_eq!(code(ortho.ray_through(1e300, 0.0, 1e300)), insufficient);
        // Was None and precision.insufficient: f (d - n) overflowed.
        let deep = x_frame(Vector3::z(), persp(), 1e200, 1e300).unwrap();
        let depth = deep.project(&Point3::new(1e250, 0.0, 0.0), 1.0).unwrap().z;
        assert!((0.0..=1.0).contains(&depth));
        let middle = deep.unproject(&Point3::new(0.0, 0.0, 0.5), 1.0).unwrap();
        assert!((deep.project(&middle, 1.0).unwrap().z - 0.5).abs() < 1e-12);
        assert_eq!(
            deep.project(&Point3::new(1e300, 0.0, 0.0), 1.0).unwrap().z,
            1.0
        );
    }

    #[test]
    fn large_coordinates_keep_millimeter_detail() {
        let eye = Point3::new(1e7 + 0.123, -2e7 + 0.456, 3e6);
        let ahead = Vector3::new(3.0, 4.0, -1.0);
        let f = new(
            eye,
            eye + ahead,
            Vector3::new(0.1, 0.2, 1.0),
            persp(),
            0.01,
            1e5,
        )
        .unwrap();
        for i in 0..200 {
            let i = f64::from(i);
            let offset = Vector3::new((i * 0.37).sin(), (i * 0.91).cos(), (i * 0.13).sin());
            let p = eye + ahead * (0.1 + i) + offset * 1e-3;
            let back = f.view_to_world_point(&f.world_to_view_point(&p));
            assert!((back - p).norm() < 1e-8);
            let via_matrix = f.world_to_view().transform_point(&p);
            assert!((via_matrix - f.world_to_view_point(&p)).norm() < 1e-7);
            let ndc = f.project(&p, 1.5).unwrap();
            assert!((f.unproject(&ndc, 1.5).unwrap() - p).norm() < 1e-5);
        }
    }

    #[test]
    fn depth_is_exact_at_the_clip_planes_for_extreme_ratios() {
        for (n, far) in [(0.5, 100.0), (1e-3, 1e9), (1e-6, 1e12), (1.0, 1.0 + 1e-9)] {
            for p in [persp(), Projection::Orthographic { height: 3.0 }] {
                let f = new(
                    Point3::origin(),
                    Point3::new(0.0, 0.0, -1.0),
                    Vector3::y(),
                    p,
                    n,
                    far,
                )
                .unwrap();
                let depth = |d: f64| f.project(&Point3::new(0.0, 0.0, -d), 1.3).unwrap().z;
                assert_eq!((depth(n), depth(far)), (0.0, 1.0));
                let product =
                    f.projection_matrix(1.3).unwrap() * f.inverse_projection_matrix(1.3).unwrap();
                assert!((product - Matrix4::identity()).norm() < 1e-12);
            }
        }
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

        #[test]
        fn scaled_frames_are_accepted_and_consistent(
            f in arb_frame(),
            scale in -300_i32..=300,
            up_scale in -300_i32..=300,
            p in arb_point(),
            ndc in arb_ndc(),
            aspect in 0.2..5.0_f64,
        ) {
            let s = 10_f64.powi(scale);
            let projection = match f.projection() {
                Projection::Orthographic { height } => Projection::Orthographic { height: height * s },
                perspective @ Projection::Perspective { .. } => perspective,
            };
            let g = ViewFrame::new(
                f.eye() * s,
                f.target() * s,
                f.up_hint() * 10_f64.powi(up_scale),
                projection,
                f.near() * s,
                f.far() * s,
            )
            .unwrap();
            for (a, b) in [(g.right(), f.right()), (g.up(), f.up()), (g.forward(), f.forward())] {
                prop_assert!((a - b).norm() < 1e-9);
            }
            let q = p * s;
            let back = g.view_to_world_point(&g.world_to_view_point(&q));
            prop_assert!((back - q).amax() <= 1e-12 * s * (p.coords.norm() + f.eye().coords.norm()));
            let world = g.unproject(&ndc, aspect).unwrap();
            prop_assert!((g.project(&world, aspect).unwrap() - ndc).norm() < 1e-6);
            let product = g.projection_matrix(aspect).unwrap() * g.inverse_projection_matrix(aspect).unwrap();
            prop_assert!((product - Matrix4::identity()).norm() < 1e-9);
            let (_, direction) = g.ray_through(ndc.x, ndc.y, aspect).unwrap();
            prop_assert!((direction.norm() - 1.0).abs() < 1e-12);
        }
    }
}
