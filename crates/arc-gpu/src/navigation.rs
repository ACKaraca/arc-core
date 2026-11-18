//! Navigation support (CORE-GPU-027): pure `f64` functions from a [`ViewFrame`] to a new one,
//! for orthographic and perspective frames. No GPU is involved.
//!
//! # Rules
//!
//! - **Orbit** rotates the eye and target rigidly about a pivot: `yaw` about the frame's up
//!   hint, then `pitch` about its right direction (positive pitch turns the view toward the up
//!   hint). The pitch is limited so the view direction stays at least `tolerance.angular` away
//!   from the up hint and its opposite, so the frame never degenerates. The pivot keeps its
//!   position in the image and its distance to the eye.
//! - **Pan** translates the eye and target parallel to the image plane so that `anchor` moves
//!   by the given pixel offset (x right, y down). Perspective pans need the anchor in front of
//!   the eye.
//! - **Zoom to point** scales the view by `factor` (greater than 1 zooms in) while `point`
//!   keeps its position in the image: orthographic frames divide their height by `factor` and
//!   move the eye parallel to the image plane; perspective frames move the eye along the line
//!   to `point`, to `1/factor` of its distance, and scale the eye-target distance the same way.
//! - **Zoom to a sphere** (zoom extents and zoom to selection with
//!   [`Scene::bounding_sphere`](crate::Scene::bounding_sphere)) keeps the view direction,
//!   targets the center and fits the sphere, with radius at least `tolerance.linear`, into the
//!   image in both directions. Near and far are widened to contain it (a perspective near
//!   plane only when the result stays positive); other functions keep near and far.
//! - **Interpolation** between two frames (for transitions between named views) eases with
//!   `s = 3t² − 2t³`, interpolates the target linearly, the eye-target distance geometrically
//!   and the orientation along the shortest rotation (quaternion slerp), so the view direction
//!   and up direction stay orthonormal and the frame is never degenerate. Field of view and
//!   orthographic height interpolate linearly and geometrically. Between a perspective and an
//!   orthographic frame the transition is perspective, using for the orthographic end the
//!   field of view that shows its height at its target distance, and returns the orthographic
//!   frame exactly at `t = 1` (or `t = 0`). `t` is clamped to [0, 1].
//!
//! Errors: `gpu.navigation.invalid` for invalid arguments; the view-frame errors of
//! `arc-geom` when a result cannot be represented.

use std::f64::consts::{FRAC_PI_2, PI};

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Result, Tolerance};
use arc_geom::view::{Projection, ViewFrame};

use crate::error::codes;
use crate::scene::{length, unit};

fn invalid() -> Error {
    Error::new(codes::NAVIGATION_INVALID)
}

fn finite(values: &[f64]) -> Result<()> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(invalid())
    }
}

fn rebuild(frame: &ViewFrame, eye: Point3, target: Point3) -> Result<ViewFrame> {
    ViewFrame::new(
        eye,
        target,
        frame.up_hint(),
        frame.projection(),
        frame.near(),
        frame.far(),
    )
}

/// `v` rotated by `angle` about the unit `axis` (Rodrigues' formula).
fn rotate(v: &Vector3, axis: &Vector3, angle: f64) -> Vector3 {
    let (s, c) = angle.sin_cos();
    v * c + axis.cross(v) * s + axis * (axis.dot(v) * (1.0 - c))
}

/// Orbits `frame` about `pivot` by `yaw` and `pitch` radians (see the [rules](self)).
///
/// # Errors
///
/// `gpu.navigation.invalid` for a non-finite argument or a negative angular tolerance; view
/// errors when the result cannot be represented.
pub fn orbit(
    frame: &ViewFrame,
    pivot: &Point3,
    yaw: f64,
    pitch: f64,
    tolerance: &Tolerance,
) -> Result<ViewFrame> {
    finite(&[pivot.x, pivot.y, pivot.z, yaw, pitch, tolerance.angular])?;
    if tolerance.angular < 0.0 {
        return Err(invalid());
    }
    let axis = unit(&frame.up_hint()).ok_or_else(invalid)?;
    let theta = frame.forward().dot(&axis).clamp(-1.0, 1.0).acos();
    let margin = tolerance.angular.min(FRAC_PI_2);
    let pitch = pitch.max(theta - (PI - margin)).min(theta - margin);
    let right = frame.right();
    let turn = |v: Vector3| rotate(&rotate(&v, &right, pitch), &axis, yaw);
    rebuild(
        frame,
        pivot + turn(frame.eye() - pivot),
        pivot + turn(frame.target() - pivot),
    )
}

/// Pans `frame` so that `anchor` moves by `delta_px` pixels (x right, y down) in an image of
/// `width × height` pixels.
///
/// # Errors
///
/// `gpu.navigation.invalid` for a non-finite argument, a zero image side or, in perspective,
/// an anchor at or behind the eye; view errors when the result cannot be represented.
pub fn pan(
    frame: &ViewFrame,
    delta_px: [f64; 2],
    width: u32,
    height: u32,
    anchor: &Point3,
) -> Result<ViewFrame> {
    finite(&[delta_px[0], delta_px[1], anchor.x, anchor.y, anchor.z])?;
    if width == 0 || height == 0 {
        return Err(invalid());
    }
    let per_pixel = match frame.projection() {
        Projection::Orthographic { height: h } => h / f64::from(height),
        Projection::Perspective { vertical_fov } => {
            let depth = -frame.world_to_view_point(anchor).z;
            if depth.is_nan() || depth <= 0.0 {
                return Err(invalid());
            }
            2.0 * depth * (vertical_fov / 2.0).tan() / f64::from(height)
        }
    };
    let t = frame.up() * (delta_px[1] * per_pixel) - frame.right() * (delta_px[0] * per_pixel);
    rebuild(frame, frame.eye() + t, frame.target() + t)
}

/// Zooms `frame` by `factor` about `point`, which keeps its position in the image.
///
/// # Errors
///
/// `gpu.navigation.invalid` for a non-finite argument or a factor that is not positive; view
/// errors when the result cannot be represented.
pub fn zoom_at(frame: &ViewFrame, factor: f64, point: &Point3) -> Result<ViewFrame> {
    finite(&[factor, point.x, point.y, point.z])?;
    if factor <= 0.0 {
        return Err(invalid());
    }
    let keep = 1.0 - 1.0 / factor;
    let to_point = point - frame.eye();
    match frame.projection() {
        Projection::Orthographic { height } => {
            let (r, u) = (frame.right(), frame.up());
            let t = (r * to_point.dot(&r) + u * to_point.dot(&u)) * keep;
            ViewFrame::new(
                frame.eye() + t,
                frame.target() + t,
                frame.up_hint(),
                Projection::Orthographic {
                    height: height / factor,
                },
                frame.near(),
                frame.far(),
            )
        }
        Projection::Perspective { .. } => {
            let eye = frame.eye() + to_point * keep;
            rebuild(frame, eye, eye + (frame.target() - frame.eye()) / factor)
        }
    }
}

/// Fits the sphere `(center, radius)` into an image of aspect ratio `aspect` (width over
/// height), keeping the view direction (see the [rules](self)).
///
/// # Errors
///
/// `gpu.navigation.invalid` for a non-finite argument, a negative radius, an aspect that is
/// not positive or a radius and linear tolerance that are both zero; view errors when the
/// result cannot be represented.
pub fn zoom_to_sphere(
    frame: &ViewFrame,
    center: &Point3,
    radius: f64,
    aspect: f64,
    tolerance: &Tolerance,
) -> Result<ViewFrame> {
    finite(&[
        center.x,
        center.y,
        center.z,
        radius,
        aspect,
        tolerance.linear,
    ])?;
    let r = radius.max(tolerance.linear);
    if radius < 0.0 || aspect <= 0.0 || r <= 0.0 {
        return Err(invalid());
    }
    let forward = frame.forward();
    let (projection, distance) = match frame.projection() {
        Projection::Orthographic { .. } => (
            Projection::Orthographic {
                height: 2.0 * r * (1.0 / aspect).max(1.0),
            },
            length(&(frame.target() - frame.eye())),
        ),
        p @ Projection::Perspective { vertical_fov } => {
            let half_v = vertical_fov / 2.0;
            let half_h = (aspect * half_v.tan()).atan();
            (p, r / half_v.min(half_h).sin())
        }
    };
    let near = match projection {
        Projection::Perspective { .. } if distance - r <= 0.0 => frame.near(),
        _ => frame.near().min(distance - r),
    };
    ViewFrame::new(
        center - forward * distance,
        *center,
        frame.up_hint(),
        projection,
        near,
        frame.far().max(distance + r),
    )
}

/// A unit quaternion `[w, x, y, z]`.
type Quaternion = [f64; 4];

/// The rotation whose columns are right, up and back (−forward) of `frame`.
fn orientation(frame: &ViewFrame) -> Quaternion {
    let (r, u, b) = (frame.right(), frame.up(), -frame.forward());
    let (m00, m10, m20) = (r.x, r.y, r.z);
    let (m01, m11, m21) = (u.x, u.y, u.z);
    let (m02, m12, m22) = (b.x, b.y, b.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [s / 4.0, (m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [(m21 - m12) / s, s / 4.0, (m01 + m10) / s, (m02 + m20) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m02 - m20) / s, (m01 + m10) / s, s / 4.0, (m12 + m21) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m10 - m01) / s, (m02 + m20) / s, (m12 + m21) / s, s / 4.0]
    };
    normalized(q)
}

fn normalized(q: Quaternion) -> Quaternion {
    let n = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    q.map(|v| v / n)
}

/// The shortest-arc spherical interpolation from `a` to `b`.
fn slerp(a: Quaternion, mut b: Quaternion, s: f64) -> Quaternion {
    let dot: f64 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    if dot < 0.0 {
        b = b.map(|v| -v);
    }
    // The angle between the quaternions, stable for small and large angles.
    let difference = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt();
    let sum = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x + y) * (x + y))
        .sum::<f64>()
        .sqrt();
    let theta = 2.0 * difference.atan2(sum);
    let sin = theta.sin();
    if sin == 0.0 {
        return a;
    }
    let (wa, wb) = (((1.0 - s) * theta).sin() / sin, (s * theta).sin() / sin);
    let mut q = [0.0; 4];
    for (out, (x, y)) in q.iter_mut().zip(a.iter().zip(&b)) {
        *out = wa * x + wb * y;
    }
    normalized(q)
}

/// The forward and up directions of an orientation.
fn directions(q: Quaternion) -> (Vector3, Vector3) {
    let [w, x, y, z] = q;
    let up = Vector3::new(
        2.0 * (x * y - w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + w * x),
    );
    let back = Vector3::new(
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    );
    (-back, up)
}

/// The field of view that shows `frame`'s orthographic height at its target distance, or its
/// own field of view.
fn equivalent_fov(frame: &ViewFrame) -> f64 {
    match frame.projection() {
        Projection::Perspective { vertical_fov } => vertical_fov,
        Projection::Orthographic { height } => {
            let distance = length(&(frame.target() - frame.eye()));
            2.0 * (height / 2.0 / distance).atan()
        }
    }
}

/// The frame at `t` of the transition from `a` to `b` (see the [rules](self)).
///
/// # Errors
///
/// `gpu.navigation.invalid` for a non-finite `t`; view errors when an intermediate frame
/// cannot be represented (for example a target so far from the origin that the distance
/// rounds away).
pub fn interpolate(a: &ViewFrame, b: &ViewFrame, t: f64) -> Result<ViewFrame> {
    finite(&[t])?;
    if t <= 0.0 {
        return Ok(*a);
    }
    if t >= 1.0 {
        return Ok(*b);
    }
    let s = t * t * (3.0 - 2.0 * t);
    let lerp = |x: f64, y: f64| x + (y - x) * s;
    let geometric = |x: f64, y: f64| (x.ln() * (1.0 - s) + y.ln() * s).exp();
    let (forward, up) = directions(slerp(orientation(a), orientation(b), s));
    let target = Point3::from(a.target().coords.lerp(&b.target().coords, s));
    let distance = geometric(
        length(&(a.target() - a.eye())),
        length(&(b.target() - b.eye())),
    );
    let (projection, near, far) = match (a.projection(), b.projection()) {
        (Projection::Perspective { .. }, Projection::Perspective { .. }) => (
            Projection::Perspective {
                vertical_fov: lerp(equivalent_fov(a), equivalent_fov(b)),
            },
            geometric(a.near(), b.near()),
            lerp(a.far(), b.far()),
        ),
        (Projection::Orthographic { height: ha }, Projection::Orthographic { height: hb }) => (
            Projection::Orthographic {
                height: geometric(ha, hb),
            },
            lerp(a.near(), b.near()),
            lerp(a.far(), b.far()),
        ),
        (Projection::Perspective { .. }, _) | (_, Projection::Perspective { .. }) => {
            let near = if matches!(a.projection(), Projection::Perspective { .. }) {
                a.near()
            } else {
                b.near()
            };
            (
                Projection::Perspective {
                    vertical_fov: lerp(equivalent_fov(a), equivalent_fov(b)),
                },
                near,
                a.far().max(b.far()),
            )
        }
    };
    ViewFrame::new(
        target - forward * distance,
        target,
        up,
        projection,
        near,
        far,
    )
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const TOLERANCE: Tolerance = Tolerance::DEFAULT;

    fn frame(eye: [f64; 3], target: [f64; 3], projection: Projection) -> Option<ViewFrame> {
        let near = match projection {
            Projection::Perspective { .. } => 0.01,
            Projection::Orthographic { .. } => -1e3,
        };
        ViewFrame::new(
            Point3::from(eye),
            Point3::from(target),
            Vector3::z(),
            projection,
            near,
            1e9,
        )
        .ok()
    }

    fn projection() -> impl Strategy<Value = Projection> {
        prop_oneof![
            (0.05_f64..3.0).prop_map(|vertical_fov| Projection::Perspective { vertical_fov }),
            (1e-3_f64..1e4).prop_map(|height| Projection::Orthographic { height }),
        ]
    }

    fn coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![-1e3_f64..1e3, -1e7_f64..1e7]
    }

    fn point() -> impl Strategy<Value = [f64; 3]> {
        prop::array::uniform3(coordinate())
    }

    /// Normalized device x and y of `p`, which must be projectable.
    fn ndc(frame: &ViewFrame, p: &Point3) -> [f64; 2] {
        let n = frame.project(p, 1.5).unwrap();
        [n.x, n.y]
    }

    fn close(a: [f64; 2], b: [f64; 2], tolerance: f64) -> bool {
        (a[0] - b[0]).abs() <= tolerance && (a[1] - b[1]).abs() <= tolerance
    }

    #[test]
    fn invalid_arguments_are_rejected() {
        let f = frame(
            [0.0, -10.0, 0.0],
            [0.0; 3],
            Projection::Perspective { vertical_fov: 1.0 },
        )
        .unwrap();
        let o = Point3::origin();
        let code = |r: Result<ViewFrame>| r.unwrap_err().code();
        assert_eq!(code(zoom_at(&f, 0.0, &o)), "gpu.navigation.invalid");
        assert_eq!(code(zoom_at(&f, f64::NAN, &o)), "gpu.navigation.invalid");
        assert_eq!(
            code(orbit(&f, &o, f64::INFINITY, 0.0, &TOLERANCE)),
            "gpu.navigation.invalid"
        );
        assert_eq!(
            code(pan(&f, [1.0, 0.0], 10, 10, &Point3::new(0.0, -20.0, 0.0))),
            "gpu.navigation.invalid",
            "anchor behind the eye"
        );
        assert_eq!(
            code(pan(&f, [1.0, 0.0], 0, 10, &o)),
            "gpu.navigation.invalid"
        );
        assert_eq!(
            code(zoom_to_sphere(&f, &o, -1.0, 1.0, &TOLERANCE)),
            "gpu.navigation.invalid"
        );
        assert_eq!(
            code(interpolate(&f, &f, f64::NAN)),
            "gpu.navigation.invalid"
        );
        // A point sphere fits with the linear tolerance as radius.
        assert!(zoom_to_sphere(&f, &o, 0.0, 1.0, &TOLERANCE).is_ok());
    }

    #[test]
    fn orbit_past_the_pole_stops_short_of_it() {
        let f = frame(
            [0.0, -10.0, 0.0],
            [0.0; 3],
            Projection::Perspective { vertical_fov: 1.0 },
        )
        .unwrap();
        let tolerance = Tolerance {
            angular: 1e-3,
            ..TOLERANCE
        };
        let over = orbit(&f, &Point3::origin(), 0.0, -3.0, &tolerance).unwrap();
        // Looking straight down would be degenerate; the view stops 1 mrad before it.
        let angle = over.forward().dot(&-Vector3::z()).clamp(-1.0, 1.0).acos();
        assert!((angle - 1e-3).abs() < 1e-9, "{angle}");
    }

    #[test]
    fn opposite_views_interpolate_without_degenerating() {
        let projection = Projection::Perspective { vertical_fov: 1.0 };
        let a = frame([0.0, -10.0, 0.0], [0.0; 3], projection).unwrap();
        let b = frame([0.0, 10.0, 0.0], [0.0; 3], projection).unwrap();
        for i in 0..=100 {
            let f = interpolate(&a, &b, f64::from(i) / 100.0).unwrap();
            assert!((length(&(f.eye() - f.target())) - 10.0).abs() < 1e-9);
        }
    }

    proptest! {
        /// Orbiting keeps the pivot at its place in the image and at its distance from the eye.
        #[test]
        fn orbit_keeps_the_pivot_in_place(
            eye in point(),
            offset in prop::array::uniform3(-1e3_f64..1e3),
            projection in projection(),
            pivot_along in 0.1_f64..1.0,
            yaw in -10.0_f64..10.0,
            pitch in -4.0_f64..4.0,
        ) {
            let target = [eye[0] + offset[0], eye[1] + offset[1], eye[2] + offset[2]];
            let Some(f) = frame(eye, target, projection) else { return Ok(()); };
            prop_assume!(f.forward().z.abs() < 0.999);
            let pivot = f.eye() + (f.target() - f.eye()) * pivot_along;
            let tolerance = Tolerance { angular: 1e-3, ..TOLERANCE };
            let g = orbit(&f, &pivot, yaw, pitch, &tolerance).unwrap();
            let scale = length(&(f.eye() - pivot)).max(1.0);
            prop_assert!((length(&(g.eye() - pivot)) - length(&(f.eye() - pivot))).abs() <= 1e-9 * scale + 1e-9 * length(&pivot.coords));
            prop_assert!(close(ndc(&f, &pivot), ndc(&g, &pivot), 1e-6), "{:?} {:?}", ndc(&f, &pivot), ndc(&g, &pivot));
        }

        /// Panning moves the anchor by exactly the pixel offset.
        #[test]
        fn pan_moves_the_anchor_with_the_pointer(
            eye in point(),
            offset in prop::array::uniform3(-1e3_f64..1e3),
            projection in projection(),
            along in 0.1_f64..1.0,
            delta in prop::array::uniform2(-500.0_f64..500.0),
        ) {
            let target = [eye[0] + offset[0], eye[1] + offset[1], eye[2] + offset[2]];
            let Some(f) = frame(eye, target, projection) else { return Ok(()); };
            let anchor = f.eye() + (f.target() - f.eye()) * along;
            let (w, h) = (300_u32, 200_u32);
            let g = pan(&f, delta, w, h, &anchor).unwrap();
            let pixel = |frame: &ViewFrame| {
                let n = frame.project(&anchor, 1.5).unwrap();
                [(n.x * 0.5 + 0.5) * f64::from(w), (0.5 - n.y * 0.5) * f64::from(h)]
            };
            let (before, after) = (pixel(&f), pixel(&g));
            prop_assert!(close([after[0] - before[0], after[1] - before[1]], delta, 1e-6 * (1.0 + delta[0].abs() + delta[1].abs()) + 1e-6));
        }

        /// Zooming keeps the point under the pointer in place and scales the view.
        #[test]
        fn zoom_keeps_the_point_under_the_pointer(
            eye in point(),
            offset in prop::array::uniform3(-1e3_f64..1e3),
            projection in projection(),
            pixel in prop::array::uniform2(-0.9_f64..0.9),
            depth in 0.1_f64..2.0,
            factor in prop_oneof![1e-3_f64..1.0, 1.0_f64..1e3],
        ) {
            let target = [eye[0] + offset[0], eye[1] + offset[1], eye[2] + offset[2]];
            let Some(f) = frame(eye, target, projection) else { return Ok(()); };
            let (origin, direction) = f.ray_through(pixel[0], pixel[1], 1.5).unwrap();
            let point = origin + direction * (depth * length(&(f.target() - f.eye())));
            let g = zoom_at(&f, factor, &point).unwrap();
            // Coordinates carry an absolute rounding of about ε·|p|, which is large in image
            // units when the zoomed view is small compared with its distance from the origin.
            let view = match g.projection() {
                Projection::Orthographic { height } => height,
                Projection::Perspective { vertical_fov } => {
                    length(&(point - g.eye())) * (vertical_fov / 2.0).tan()
                }
            };
            let rounding = 1e-14 * (length(&f.eye().coords) + length(&point.coords)) / view;
            prop_assert!(close(ndc(&f, &point), ndc(&g, &point), 1e-6 + rounding));
            // The direction is recomputed from eye and target: rounding is relative to their
            // magnitude over their distance.
            let conditioning = 1.0 + length(&g.eye().coords) / length(&(g.target() - g.eye()));
            prop_assert!((g.forward() - f.forward()).norm() < 1e-14 * conditioning);
            if let (Projection::Orthographic { height: a }, Projection::Orthographic { height: b }) = (f.projection(), g.projection()) {
                prop_assert!((a / b / factor - 1.0).abs() < 1e-12);
            }
        }

        /// Zooming to a sphere shows all of it.
        #[test]
        fn a_fitted_sphere_is_inside_the_image(
            eye in point(),
            offset in prop::array::uniform3(-1e3_f64..1e3),
            projection in projection(),
            center in point(),
            radius in prop_oneof![Just(0.0), 1e-3_f64..1e5],
            aspect in 0.2_f64..5.0,
            directions in prop::collection::vec(prop::array::uniform3(-1.0_f64..1.0), 8),
        ) {
            let target = [eye[0] + offset[0], eye[1] + offset[1], eye[2] + offset[2]];
            let Some(f) = frame(eye, target, projection) else { return Ok(()); };
            let center = Point3::from(center);
            let g = zoom_to_sphere(&f, &center, radius, aspect, &TOLERANCE).unwrap();
            let r = radius.max(TOLERANCE.linear);
            for d in directions.iter().map(|d| Vector3::from(*d)).chain([g.right(), -g.right(), g.up(), -g.up()]) {
                let Some(d) = unit(&d) else { continue; };
                let n = g.project(&(center + d * r), aspect).unwrap();
                let slack = 1e-9 + 1e-12 * length(&center.coords) / r;
                prop_assert!(n.x.abs() <= 1.0 + slack && n.y.abs() <= 1.0 + slack, "{n:?}");
                prop_assert!((-slack..=1.0 + slack).contains(&n.z), "{n:?}");
            }
        }

        /// Transitions start and end exactly at the named views, never degenerate and move
        /// the view direction continuously.
        #[test]
        fn transitions_are_continuous_and_never_degenerate(
            ea in point(), oa in prop::array::uniform3(-1e3_f64..1e3), pa in projection(),
            eb in point(), ob in prop::array::uniform3(-1e3_f64..1e3), pb in projection(),
            t in 0.0_f64..1.0,
        ) {
            let ta = [ea[0] + oa[0], ea[1] + oa[1], ea[2] + oa[2]];
            let tb = [eb[0] + ob[0], eb[1] + ob[1], eb[2] + ob[2]];
            let (Some(a), Some(b)) = (frame(ea, ta, pa), frame(eb, tb, pb)) else { return Ok(()); };
            prop_assert_eq!(interpolate(&a, &b, 0.0).unwrap(), a);
            prop_assert_eq!(interpolate(&a, &b, 1.0).unwrap(), b);
            let dt = 1e-4;
            let f = interpolate(&a, &b, t).unwrap();
            let g = interpolate(&a, &b, (t + dt).min(1.0)).unwrap();
            // The eased rotation turns at most 1.5 π per unit of t.
            prop_assert!((g.forward() - f.forward()).norm() <= 1.5 * PI * dt + 1e-9);
            prop_assert!((g.up() - f.up()).norm() <= 1.5 * PI * dt + 1e-9);
        }
    }
}
