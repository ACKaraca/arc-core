//! Manipulators (CORE-GPU-026): move, rotate and scale handles of constant screen size with
//! their own hit testing.
//!
//! A [`Manipulator`] is a per-frame input ([`crate::FrameExtras::manipulator`]). It is resolved
//! on the CPU in `f64` and drawn through the overlay: above the scene regardless of depth and
//! not clipped by sections. Its size is a number of pixels, converted to model units at the
//! depth of its origin ([`world_per_pixel`]), so it keeps its size on the screen at any camera
//! distance.
//!
//! - **Move** and **scale** draw one handle per axis: a line from the origin to a tip
//!   `size_px` away, ending in an arrowhead (move) or a square (scale).
//! - **Rotate** draws one ring per axis: a circle of radius `size_px` in the plane
//!   perpendicular to the axis.
//! - **Hit testing** ([`Manipulator::hit_test`]) works in pixels: the distance from the pixel
//!   to the projected handle (the line, or the polyline of the ring). The nearest handle within
//!   the tolerance is hit; ties go to the lower axis. A handle or part of it behind the near
//!   plane is not drawn and not hit.
//! - The handle of [`Manipulator::active`] is drawn in [`Manipulator::active_color`].
//!
//! Plane handles and a uniform scale handle are not provided.

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Error, Result};
use arc_geom::view::{Projection, ViewFrame};

use crate::error::codes;
use crate::overlay::{Anchor, MarkerShape, Overlay, OverlayPrimitive, Projector};

/// Segments of a rotation ring.
pub const RING_SEGMENTS: usize = 48;
/// The largest handle size in pixels.
pub const MAX_SIZE_PX: f32 = 4096.0;
/// The length of a move arrowhead in pixels, and half its width.
const ARROW_PX: f64 = 14.0;
const ARROW_HALF_PX: f64 = 6.0;
/// The size of a scale square in pixels.
const SQUARE_PX: f32 = 12.0;

/// What a manipulator changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManipulatorKind {
    /// Translate along an axis.
    Move,
    /// Rotate about an axis.
    Rotate,
    /// Scale along an axis.
    Scale,
}

/// A handle's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Axis {
    /// The first axis.
    X,
    /// The second axis.
    Y,
    /// The third axis.
    Z,
}

impl Axis {
    /// The three axes in order.
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

/// Handles at a point; see the module documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Manipulator {
    /// The kind of handles.
    pub kind: ManipulatorKind,
    /// The point the handles are drawn at.
    pub origin: Point3,
    /// The directions of the x, y and z handles; each is normalized, none may be zero.
    pub axes: [Vector3; 3],
    /// The handle length (move, scale) or ring radius (rotate) in pixels, in (0, 4096].
    pub size_px: f32,
    /// The line width in pixels.
    pub width_px: f32,
    /// The colors of the x, y and z handles.
    pub colors: [Color; 3],
    /// The handle being dragged or hovered, drawn in [`Manipulator::active_color`].
    pub active: Option<Axis>,
    /// The color of the active handle.
    pub active_color: Color,
}

impl Manipulator {
    /// A manipulator on the world axes with 100-pixel handles, three pixels wide, colored red,
    /// green and blue, and a yellow active handle.
    #[must_use]
    pub fn new(kind: ManipulatorKind, origin: Point3) -> Self {
        Self {
            kind,
            origin,
            axes: [Vector3::x(), Vector3::y(), Vector3::z()],
            size_px: 100.0,
            width_px: 3.0,
            colors: [
                Color::new(0.9, 0.15, 0.15, 1.0),
                Color::new(0.15, 0.75, 0.15, 1.0),
                Color::new(0.2, 0.3, 0.95, 1.0),
            ],
            active: None,
            active_color: Color::new(1.0, 0.9, 0.1, 1.0),
        }
    }

    fn validate(&self) -> Result<[Vector3; 3]> {
        let finite = self.origin.iter().all(|v| v.is_finite())
            && self.axes.iter().all(|a| a.iter().all(|v| v.is_finite()))
            && self
                .colors
                .iter()
                .chain([&self.active_color])
                .all(|c| [c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()));
        if !finite {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
        let units = self.axes.map(|a| crate::scene::unit(&a));
        let [Some(x), Some(y), Some(z)] = units else {
            return Err(Error::new(codes::SETTINGS_INVALID));
        };
        let size_ok = self.size_px > 0.0 && self.size_px <= MAX_SIZE_PX;
        let width_ok = self.width_px > 0.0 && self.width_px.is_finite();
        if size_ok && width_ok {
            Ok([x, y, z])
        } else {
            Err(Error::new(codes::SETTINGS_INVALID))
        }
    }

    /// The handle geometry of axis `axis` in model space: a segment origin to tip, or the
    /// closed ring as consecutive points; `None` when the origin is not in front of the eye.
    fn handle(&self, axes: &[Vector3; 3], axis: Axis, unit: f64) -> Vec<Point3> {
        let i = axis.index();
        let get = |k: usize| axes.get(k % 3).copied().unwrap_or_else(Vector3::x);
        let length = f64::from(self.size_px) * unit;
        match self.kind {
            ManipulatorKind::Move | ManipulatorKind::Scale => {
                vec![self.origin, self.origin + get(i) * length]
            }
            ManipulatorKind::Rotate => {
                let (u, v) = (get(i + 1), get(i + 2));
                (0..=RING_SEGMENTS)
                    .map(|k| {
                        let a = std::f64::consts::TAU * k as f64 / RING_SEGMENTS as f64;
                        self.origin + (u * a.cos() + v * a.sin()) * length
                    })
                    .collect()
            }
        }
    }

    /// Appends the handles to `overlay` for an image of `size` pixels.
    ///
    /// # Errors
    ///
    /// `gpu.value.not_finite`, `gpu.settings.invalid` for an invalid manipulator.
    pub fn append_to(
        &self,
        overlay: &mut Overlay,
        frame: &ViewFrame,
        size: (u32, u32),
    ) -> Result<()> {
        let axes = self.validate()?;
        let Some(unit) = world_per_pixel(frame, &self.origin, size.1) else {
            return Ok(());
        };
        let projector = Projector::new(frame, size.0, size.1);
        for axis in Axis::ALL {
            let color = if self.active == Some(axis) {
                self.active_color
            } else {
                self.colors
                    .get(axis.index())
                    .copied()
                    .unwrap_or(Color::WHITE)
            };
            let points = self.handle(&axes, axis, unit);
            overlay.primitives.push(OverlayPrimitive::Polyline {
                points: points.iter().map(|p| Anchor::World(*p)).collect(),
                closed: false,
                width_px: self.width_px,
                color,
            });
            let (Some(first), Some(tip)) = (points.first(), points.last()) else {
                continue;
            };
            match self.kind {
                ManipulatorKind::Move => {
                    if let Some(head) = arrowhead(&projector, first, tip) {
                        overlay.primitives.push(OverlayPrimitive::Fill {
                            points: head,
                            color,
                        });
                    }
                }
                ManipulatorKind::Scale => overlay.primitives.push(OverlayPrimitive::Marker {
                    at: Anchor::World(*tip),
                    shape: MarkerShape::Square,
                    size_px: SQUARE_PX,
                    filled: true,
                    width_px: 1.0,
                    color,
                }),
                ManipulatorKind::Rotate => {}
            }
        }
        Ok(())
    }

    /// The distance in pixels from `pixel` (x right, y down) to each handle, `None` for a
    /// handle that is not visible.
    fn distances(
        &self,
        frame: &ViewFrame,
        size: (u32, u32),
        pixel: [f64; 2],
    ) -> Result<[Option<f64>; 3]> {
        let axes = self.validate()?;
        let Some(unit) = world_per_pixel(frame, &self.origin, size.1) else {
            return Ok([None; 3]);
        };
        let projector = Projector::new(frame, size.0, size.1);
        Ok(Axis::ALL.map(|axis| {
            self.handle(&axes, axis, unit)
                .windows(2)
                .filter_map(|w| {
                    let (a, b) = (w.first()?, w.get(1)?);
                    let (pa, pb) = projector.segment(&Anchor::World(*a), &Anchor::World(*b))?;
                    Some(distance_to_segment(pixel, pa, pb))
                })
                .reduce(f64::min)
        }))
    }

    /// The handle under `pixel` (x right, y down) in an image of `size` pixels: the nearest
    /// handle within `tolerance_px` of the pixel, the lower axis on a tie.
    ///
    /// # Errors
    ///
    /// `gpu.value.not_finite`, `gpu.settings.invalid` for an invalid manipulator or a pixel or
    /// tolerance that is not finite or negative.
    pub fn hit_test(
        &self,
        frame: &ViewFrame,
        size: (u32, u32),
        pixel: [f64; 2],
        tolerance_px: f64,
    ) -> Result<Option<Axis>> {
        if !(pixel.iter().all(|v| v.is_finite()) && tolerance_px.is_finite() && tolerance_px >= 0.0)
        {
            return Err(Error::new(codes::SETTINGS_INVALID));
        }
        let distances = self.distances(frame, size, pixel)?;
        let mut best: Option<(Axis, f64)> = None;
        for (axis, d) in Axis::ALL.into_iter().zip(distances) {
            if let Some(d) = d.filter(|d| *d <= tolerance_px)
                && best.is_none_or(|(_, b)| d < b)
            {
                best = Some((axis, d));
            }
        }
        Ok(best.map(|(axis, _)| axis))
    }
}

/// The model-space length of one pixel at `point` in an image `image_height` pixels tall:
/// constant for orthographic views, proportional to the depth for perspective views. `None`
/// for a point not in front of the near plane or an image without height.
#[must_use]
pub fn world_per_pixel(frame: &ViewFrame, point: &Point3, image_height: u32) -> Option<f64> {
    if image_height == 0 {
        return None;
    }
    let pixels = f64::from(image_height);
    match frame.projection() {
        Projection::Orthographic { height } => Some(height / pixels),
        Projection::Perspective { vertical_fov } => {
            let depth = (point - frame.eye()).dot(&frame.forward());
            (depth >= frame.near()).then(|| 2.0 * depth * (vertical_fov / 2.0).tan() / pixels)
        }
    }
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ab, ap) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
    let length2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if length2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (ap[0] - t * ab[0]).hypot(ap[1] - t * ab[1])
}

/// The corners (in pixels) of an arrowhead at `tip` pointing away from `from`.
fn arrowhead(projector: &Projector, from: &Point3, tip: &Point3) -> Option<Vec<Anchor>> {
    let a = projector.point(&Anchor::World(*from))?;
    let b = projector.point(&Anchor::World(*tip))?;
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0].hypot(d[1]);
    if length < 1e-6 {
        return None;
    }
    let u = [d[0] / length, d[1] / length];
    let n = [-u[1], u[0]];
    let corner = |along: f64, across: f64| {
        Anchor::Screen([
            b[0] + u[0] * along + n[0] * across,
            b[1] + u[1] * along + n[1] * across,
        ])
    };
    Some(vec![
        corner(ARROW_PX, 0.0),
        corner(0.0, ARROW_HALF_PX),
        corner(0.0, -ARROW_HALF_PX),
    ])
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests fail by panicking")]

    use proptest::prelude::*;

    use super::*;

    const SIZE: (u32, u32) = (400, 300);

    fn view(distance: f64, orthographic: bool) -> ViewFrame {
        looking_from(Vector3::new(1.0, -1.2, 0.9), distance, orthographic)
    }

    fn looking_from(direction: Vector3, distance: f64, orthographic: bool) -> ViewFrame {
        ViewFrame::new(
            Point3::from(direction.normalize() * distance),
            Point3::origin(),
            Vector3::y(),
            if orthographic {
                Projection::Orthographic {
                    height: distance / 2.0,
                }
            } else {
                Projection::Perspective { vertical_fov: 1.0 }
            },
            0.1,
            10_000.0,
        )
        .unwrap()
    }

    fn pixel_of(frame: &ViewFrame, p: &Point3) -> [f64; 2] {
        Projector::new(frame, SIZE.0, SIZE.1)
            .point(&Anchor::World(*p))
            .unwrap()
    }

    #[test]
    fn handles_keep_their_pixel_length_at_any_distance() {
        // Looking down z, the x handle lies across the image at the depth of its origin, so its
        // projected length is exactly its size at every distance.
        for orthographic in [false, true] {
            for distance in [5.0, 50.0, 5000.0] {
                let frame = looking_from(Vector3::z(), distance, orthographic);
                let m = Manipulator::new(ManipulatorKind::Move, Point3::origin());
                let mut overlay = Overlay::default();
                m.append_to(&mut overlay, &frame, SIZE).unwrap();
                let OverlayPrimitive::Polyline { points, .. } = &overlay.primitives[0] else {
                    panic!("the first primitive is the x line");
                };
                let [Anchor::World(a), Anchor::World(b)] = points.as_slice() else {
                    panic!("a line has two points");
                };
                let (pa, pb) = (pixel_of(&frame, a), pixel_of(&frame, b));
                let length = (pa[0] - pb[0]).hypot(pa[1] - pb[1]);
                assert!((length - 100.0).abs() < 0.01, "{length} at {distance}");
            }
        }
    }

    #[test]
    fn world_per_pixel_is_none_behind_the_near_plane() {
        let frame = view(10.0, false);
        assert!(world_per_pixel(&frame, &Point3::origin(), 300).is_some());
        let behind = Point3::from(frame.eye().coords * 3.0);
        assert!(world_per_pixel(&frame, &behind, 300).is_none());
        assert!(world_per_pixel(&frame, &Point3::origin(), 0).is_none());
    }

    #[test]
    fn invalid_manipulators_and_pixels_are_rejected() {
        let frame = view(10.0, false);
        let mut m = Manipulator::new(ManipulatorKind::Scale, Point3::origin());
        m.size_px = 0.0;
        assert_eq!(
            m.hit_test(&frame, SIZE, [1.0, 1.0], 3.0)
                .unwrap_err()
                .code(),
            codes::SETTINGS_INVALID.code
        );
        m.size_px = 50.0;
        m.axes[1] = Vector3::zeros();
        assert_eq!(
            m.append_to(&mut Overlay::default(), &frame, SIZE)
                .unwrap_err()
                .code(),
            codes::SETTINGS_INVALID.code
        );
        let ok = Manipulator::new(ManipulatorKind::Move, Point3::origin());
        assert!(ok.hit_test(&frame, SIZE, [f64::NAN, 0.0], 3.0).is_err());
        assert!(ok.hit_test(&frame, SIZE, [0.0, 0.0], -1.0).is_err());
    }

    proptest! {
        /// A pixel on an axis handle hits that axis, and one far from every handle hits nothing.
        #[test]
        fn pixels_on_axis_handles_hit_them(t in 0.5_f64..1.0, axis in 0_usize..3, scale in prop::sample::select(vec![false, true])) {
            let frame = view(20.0, false);
            let kind = if scale { ManipulatorKind::Scale } else { ManipulatorKind::Move };
            let m = Manipulator::new(kind, Point3::origin());
            let unit = world_per_pixel(&frame, &m.origin, SIZE.1).unwrap();
            let direction = m.axes[axis];
            let on = m.origin + direction * (100.0 * unit * t);
            let pixel = pixel_of(&frame, &on);
            let hit = m.hit_test(&frame, SIZE, pixel, 4.0).unwrap();
            prop_assert_eq!(hit, Some(Axis::ALL[axis]));
            let far = [pixel[0] + 200.0, pixel[1] + 150.0];
            prop_assert_eq!(m.hit_test(&frame, SIZE, far, 4.0).unwrap(), None);
        }

        /// A pixel on a ring is at distance zero from that ring, and the hit is the nearest.
        #[test]
        fn pixels_on_rings_hit_the_nearest_ring(angle in 0.0_f64..std::f64::consts::TAU, axis in 0_usize..3) {
            let frame = view(20.0, false);
            let m = Manipulator::new(ManipulatorKind::Rotate, Point3::origin());
            let unit = world_per_pixel(&frame, &m.origin, SIZE.1).unwrap();
            let (u, v) = (m.axes[(axis + 1) % 3], m.axes[(axis + 2) % 3]);
            let on = m.origin + (u * angle.cos() + v * angle.sin()) * (100.0 * unit);
            let pixel = pixel_of(&frame, &on);
            let distances = m.distances(&frame, SIZE, pixel).unwrap();
            // A point on the polyline between vertices is at most the sagitta from it.
            prop_assert!(distances[axis].unwrap() < 0.5);
            let nearest = distances.iter().flatten().copied().fold(f64::INFINITY, f64::min);
            let hit = m.hit_test(&frame, SIZE, pixel, 4.0).unwrap().unwrap();
            prop_assert_eq!(distances[hit.index()], Some(nearest));
        }
    }
}
