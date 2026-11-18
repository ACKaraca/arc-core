//! Overlays (CORE-GPU-025): transient graphics drawn above the scene regardless of depth.
//!
//! An [`Overlay`] is a list of [`OverlayPrimitive`]s passed with each frame; it is not part of
//! the [`Scene`](crate::Scene) and writes no scene buffer. Every primitive is resolved on the
//! CPU in `f64` to pixel positions of the image (x right, y down, origin at the top-left
//! corner), then drawn by the line pipeline (exact-width segments with round caps) and a fill
//! pipeline, without depth testing and without the scene's clip planes.
//!
//! - A [`Anchor::World`] point is projected through the view frame. Perspective segments are
//!   clipped at the near plane before projection; a point behind it is not drawn (a marker or
//!   fill using it is skipped). The far plane does not clip overlays.
//! - Segments are clipped to a guard band one image side wide around the image, so points far
//!   outside never reach the GPU as huge coordinates.
//! - Polylines are drawn as separate segments with round ends, which equals round joins.
//! - Fills are simple polygons, triangulated by ear clipping in pixel space.
//! - Fills are drawn before lines; within each kind, primitives draw in list order.

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Error, Result};
use arc_geom::view::{Projection, ViewFrame};

use crate::error::codes;
use crate::scene::SceneLimits;

/// A point of an overlay primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// A model-space point, projected through the view.
    World(Point3),
    /// A pixel position: x right, y down, from the top-left corner of the image.
    Screen([f64; 2]),
}

/// The shape of a marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerShape {
    /// An image-aligned square.
    Square,
    /// A circle.
    Circle,
    /// A triangle pointing up.
    Triangle,
    /// Two diagonals; never filled.
    Cross,
}

/// A grid of lines in a plane: lines through `origin + u·i` along `v` and through
/// `origin + v·i` along `u`, for `i` in `−half_count..=half_count`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// The grid origin.
    pub origin: Point3,
    /// The spacing vector of the first family.
    pub u: Vector3,
    /// The spacing vector of the second family.
    pub v: Vector3,
    /// Lines on each side of the origin per family.
    pub half_count: u32,
    /// Every this many lines (counted from the origin) is a major line; 0 for none.
    pub major_every: u32,
    /// The color of minor lines.
    pub color: Color,
    /// The color of major lines.
    pub major_color: Color,
    /// The line width in pixels.
    pub width_px: f32,
}

/// An overlay primitive.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayPrimitive {
    /// A polyline of constant pixel width.
    Polyline {
        /// The points; at least one (a single point is a dot).
        points: Vec<Anchor>,
        /// Whether the last point connects back to the first.
        closed: bool,
        /// The full width in pixels.
        width_px: f32,
        /// The color; alpha blends.
        color: Color,
    },
    /// A filled simple polygon.
    Fill {
        /// The corners; at least three.
        points: Vec<Anchor>,
        /// The color; alpha blends.
        color: Color,
    },
    /// A marker of constant pixel size centered on a point.
    Marker {
        /// The center.
        at: Anchor,
        /// The shape.
        shape: MarkerShape,
        /// The full size in pixels.
        size_px: f32,
        /// Whether the shape is filled; otherwise it is outlined.
        filled: bool,
        /// The outline width in pixels.
        width_px: f32,
        /// The color.
        color: Color,
    },
    /// A grid.
    Grid(Grid),
    /// The world axes x, y and z seen through the view, drawn from a pixel position; axes
    /// pointing away from the eye are drawn first.
    AxisTriad {
        /// The pixel position of the origin.
        center: [f64; 2],
        /// The length in pixels of an axis parallel to the image.
        length_px: f64,
        /// The line width in pixels.
        width_px: f32,
        /// The colors of the x, y and z axes.
        colors: [Color; 3],
    },
}

/// Transient graphics drawn above the scene.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    /// The primitives.
    pub primitives: Vec<OverlayPrimitive>,
}

/// Bytes per fill vertex: pixel position (2 `f32`) and color (4 `f32`).
pub(crate) const FILL_VERTEX_STRIDE: u64 = 24;

/// An overlay resolved to segment and fill-vertex bytes for one view.
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    pub(crate) segments: Vec<u8>,
    pub(crate) segment_count: u32,
    pub(crate) fill: Vec<u8>,
    pub(crate) fill_vertices: u32,
}

/// Style bits of an overlay segment: pixel positions (bit 6), round caps, no neighbors.
const SCREEN_BITS: u32 = 64;
/// Sides of an outlined circle marker.
const CIRCLE_SIDES: u32 = 32;

struct Projector {
    frame: ViewFrame,
    width: f64,
    height: f64,
    aspect: f64,
}

type Pixel = [f64; 2];

impl Projector {
    fn pixel(&self, v: &Point3) -> Pixel {
        let (x, y) = match self.frame.projection() {
            Projection::Perspective { vertical_fov } => {
                let k = 1.0 / (vertical_fov / 2.0).tan();
                let d = -v.z;
                (k * (v.x / d) / self.aspect, k * (v.y / d))
            }
            Projection::Orthographic { height } => {
                let h = height / 2.0;
                (v.x / h / self.aspect, v.y / h)
            }
        };
        [(x * 0.5 + 0.5) * self.width, (0.5 - y * 0.5) * self.height]
    }

    fn perspective(&self) -> bool {
        matches!(self.frame.projection(), Projection::Perspective { .. })
    }

    /// Whether a view-space point is in front of the near plane (always for orthographic).
    fn visible(&self, v: &Point3) -> bool {
        !self.perspective() || -v.z >= self.frame.near()
    }

    fn point(&self, a: &Anchor) -> Option<Pixel> {
        match a {
            Anchor::Screen(p) => Some(*p),
            Anchor::World(w) => {
                let v = self.frame.world_to_view_point(w);
                self.visible(&v).then(|| self.pixel(&v))
            }
        }
    }

    /// A segment in pixels, clipped at the near plane and to the guard band.
    fn segment(&self, a: &Anchor, b: &Anchor) -> Option<(Pixel, Pixel)> {
        let (pa, pb) = match (a, b) {
            (Anchor::World(wa), Anchor::World(wb)) => {
                let (mut va, mut vb) = (
                    self.frame.world_to_view_point(wa),
                    self.frame.world_to_view_point(wb),
                );
                if self.perspective() {
                    let near = self.frame.near();
                    let (da, db) = (-va.z - near, -vb.z - near);
                    if da < 0.0 && db < 0.0 {
                        return None;
                    }
                    if da < 0.0 {
                        va = va + (vb - va) * (da / (da - db));
                    } else if db < 0.0 {
                        vb = vb + (va - vb) * (db / (db - da));
                    }
                }
                (self.pixel(&va), self.pixel(&vb))
            }
            _ => (self.point(a)?, self.point(b)?),
        };
        self.guard(pa, pb)
    }

    /// Liang–Barsky clipping to the image grown by one image side on every edge.
    fn guard(&self, a: Pixel, b: Pixel) -> Option<(Pixel, Pixel)> {
        let g = self.width.max(self.height);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        for (p, q) in [
            (-dx, a[0] + g),
            (dx, self.width + g - a[0]),
            (-dy, a[1] + g),
            (dy, self.height + g - a[1]),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
            }
        }
        let at = |t: f64| [a[0] + dx * t, a[1] + dy * t];
        let clipped = (t0 <= t1).then(|| (at(t0), at(t1)))?;
        clipped
            .0
            .iter()
            .chain(&clipped.1)
            .all(|v| v.is_finite())
            .then_some(clipped)
    }
}

fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Pixel positions are within the guard band.
    let n = v as f32;
    n
}

struct Builder<'a> {
    limits: &'a SceneLimits,
    out: Resolved,
}

impl Builder<'_> {
    fn count(&self) -> u32 {
        self.out
            .segment_count
            .saturating_add(self.out.fill_vertices)
    }

    fn reserve(&self, n: u64) -> Result<()> {
        if u64::from(self.count()) + n > u64::from(self.limits.max_segments_per_item) {
            return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "overlay"));
        }
        Ok(())
    }

    fn segment(&mut self, a: Pixel, b: Pixel, width: f32, color: Color) -> Result<()> {
        self.reserve(1)?;
        let (a, b) = (
            [narrow(a[0]), narrow(a[1]), 0.0],
            [narrow(b[0]), narrow(b[1]), 0.0],
        );
        for v in [a, b, a, b].iter().flatten() {
            self.out.segments.extend_from_slice(&v.to_le_bytes());
        }
        for v in [color.r, color.g, color.b, color.a, width, 4.0] {
            self.out.segments.extend_from_slice(&v.to_le_bytes());
        }
        for v in [SCREEN_BITS, 0] {
            self.out.segments.extend_from_slice(&v.to_le_bytes());
        }
        self.out.segment_count += 1;
        Ok(())
    }

    fn triangles(&mut self, polygon: &[Pixel], color: Color) -> Result<()> {
        let triangles = ear_clip(polygon);
        self.reserve(triangles.len() as u64)?;
        for p in triangles {
            for v in [
                narrow(p[0]),
                narrow(p[1]),
                color.r,
                color.g,
                color.b,
                color.a,
            ] {
                self.out.fill.extend_from_slice(&v.to_le_bytes());
            }
            self.out.fill_vertices += 1;
        }
        Ok(())
    }
}

/// The triangles of a simple polygon by ear clipping, as consecutive vertex triples.
/// Repeated points are dropped; a polygon left without an ear (self-intersecting or
/// degenerate) yields the triangles found so far.
fn ear_clip(polygon: &[Pixel]) -> Vec<Pixel> {
    let mut points: Vec<Pixel> = Vec::with_capacity(polygon.len());
    for p in polygon {
        if points.last() != Some(p) {
            points.push(*p);
        }
    }
    if points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    let cross = |o: Pixel, a: Pixel, b: Pixel| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let area: f64 = (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum();
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    let mut out = Vec::new();
    while points.len() > 3 {
        let n = points.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            sign * cross(a, b, c) > 0.0
                && points.iter().enumerate().all(|(j, p)| {
                    j == i
                        || j == (i + 1) % n
                        || j == (i + n - 1) % n
                        || !(sign * cross(a, b, *p) >= 0.0
                            && sign * cross(b, c, *p) >= 0.0
                            && sign * cross(c, a, *p) >= 0.0)
                })
        });
        let Some(i) = ear else {
            return out;
        };
        out.extend([points[(i + n - 1) % n], points[i], points[(i + 1) % n]]);
        points.remove(i);
    }
    if points.len() == 3 {
        out.extend_from_slice(&points);
    }
    out
}

fn check_width(width: f32, limits: &SceneLimits) -> Result<()> {
    if !width.is_finite() {
        return Err(Error::new(codes::VALUE_NOT_FINITE));
    }
    if width <= 0.0 {
        return Err(Error::new(codes::ITEM_INVALID));
    }
    if width > limits.max_line_width_px {
        return Err(Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", "max_line_width_px"));
    }
    Ok(())
}

fn check_color(c: Color) -> Result<()> {
    if [c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_NOT_FINITE))
    }
}

fn check_anchor(a: &Anchor) -> Result<()> {
    let finite = match a {
        Anchor::World(p) => p.iter().all(|v| v.is_finite()),
        Anchor::Screen(p) => p.iter().all(|v| v.is_finite()),
    };
    if finite {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_NOT_FINITE))
    }
}

/// The outline points of a marker around `c` with half size `s`.
fn marker_outline(shape: MarkerShape, c: Pixel, s: f64) -> Vec<Pixel> {
    match shape {
        MarkerShape::Square => vec![
            [c[0] - s, c[1] - s],
            [c[0] + s, c[1] - s],
            [c[0] + s, c[1] + s],
            [c[0] - s, c[1] + s],
        ],
        MarkerShape::Triangle => {
            let h = s * 3_f64.sqrt() / 2.0;
            vec![
                [c[0], c[1] - s],
                [c[0] + h, c[1] + s / 2.0],
                [c[0] - h, c[1] + s / 2.0],
            ]
        }
        MarkerShape::Circle | MarkerShape::Cross => (0..CIRCLE_SIDES)
            .map(|i| {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(CIRCLE_SIDES);
                [c[0] + s * a.cos(), c[1] + s * a.sin()]
            })
            .collect(),
    }
}

impl Overlay {
    /// An empty overlay.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves the overlay for `frame` and an image of `width × height` pixels.
    ///
    /// # Errors
    ///
    /// `gpu.value.not_finite` for a non-finite point, vector, color or width;
    /// `gpu.item.invalid` for an empty polyline, a fill with fewer than three points or a
    /// width or size that is not positive; `gpu.limit.exceeded` when a width exceeds
    /// `max_line_width_px` or the segments and fill vertices together exceed
    /// `max_segments_per_item`.
    #[allow(clippy::too_many_lines)] // One arm per primitive kind.
    pub(crate) fn resolve(
        &self,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        limits: &SceneLimits,
    ) -> Result<Resolved> {
        let projector = Projector {
            frame: *frame,
            width: f64::from(width),
            height: f64::from(height),
            aspect: f64::from(width) / f64::from(height),
        };
        let mut b = Builder {
            limits,
            out: Resolved::default(),
        };
        for primitive in &self.primitives {
            match primitive {
                OverlayPrimitive::Polyline {
                    points,
                    closed,
                    width_px,
                    color,
                } => {
                    check_width(*width_px, limits)?;
                    check_color(*color)?;
                    points.iter().try_for_each(check_anchor)?;
                    let n = points.len();
                    if n == 0 {
                        return Err(Error::new(codes::ITEM_INVALID));
                    }
                    let segments = if *closed && n > 2 { n } else { n.max(2) - 1 };
                    for i in 0..segments {
                        let (Some(p), Some(q)) = (points.get(i), points.get((i + 1) % n)) else {
                            continue;
                        };
                        if let Some((a, c)) = projector.segment(p, q) {
                            b.segment(a, c, *width_px, *color)?;
                        }
                    }
                }
                OverlayPrimitive::Fill { points, color } => {
                    check_color(*color)?;
                    points.iter().try_for_each(check_anchor)?;
                    if points.len() < 3 {
                        return Err(Error::new(codes::ITEM_INVALID));
                    }
                    let pixels: Option<Vec<Pixel>> =
                        points.iter().map(|p| projector.point(p)).collect();
                    if let Some(pixels) = pixels {
                        b.triangles(&pixels, *color)?;
                    }
                }
                OverlayPrimitive::Marker {
                    at,
                    shape,
                    size_px,
                    filled,
                    width_px,
                    color,
                } => {
                    check_width(*width_px, limits)?;
                    check_color(*color)?;
                    check_anchor(at)?;
                    if !size_px.is_finite() {
                        return Err(Error::new(codes::VALUE_NOT_FINITE));
                    }
                    if *size_px <= 0.0 {
                        return Err(Error::new(codes::ITEM_INVALID));
                    }
                    let Some(c) = projector.point(at) else {
                        continue;
                    };
                    let s = f64::from(*size_px) / 2.0;
                    match (shape, filled) {
                        (MarkerShape::Cross, _) => {
                            for (dx, dy) in [(1.0, 1.0), (1.0, -1.0)] {
                                let a = [c[0] - s * dx, c[1] - s * dy];
                                let e = [c[0] + s * dx, c[1] + s * dy];
                                if let Some((a, e)) = projector.guard(a, e) {
                                    b.segment(a, e, *width_px, *color)?;
                                }
                            }
                        }
                        (MarkerShape::Circle, true) => {
                            // A dot with round ends is an exact disk.
                            if size_px > &limits.max_line_width_px {
                                return Err(Error::new(codes::LIMIT_EXCEEDED)
                                    .with_detail("limit", "max_line_width_px"));
                            }
                            if let Some((a, e)) = projector.guard(c, c) {
                                b.segment(a, e, *size_px, *color)?;
                            }
                        }
                        (_, true) => b.triangles(&marker_outline(*shape, c, s), *color)?,
                        (_, false) => {
                            let outline = marker_outline(*shape, c, s);
                            for (i, p) in outline.iter().enumerate() {
                                let q = outline.get((i + 1) % outline.len()).unwrap_or(p);
                                if let Some((a, e)) = projector.guard(*p, *q) {
                                    b.segment(a, e, *width_px, *color)?;
                                }
                            }
                        }
                    }
                }
                OverlayPrimitive::Grid(grid) => {
                    check_width(grid.width_px, limits)?;
                    check_color(grid.color)?;
                    check_color(grid.major_color)?;
                    check_anchor(&Anchor::World(grid.origin))?;
                    if !grid.u.iter().chain(grid.v.iter()).all(|v| v.is_finite()) {
                        return Err(Error::new(codes::VALUE_NOT_FINITE));
                    }
                    let n = i64::from(grid.half_count);
                    b.reserve((2 * u64::from(grid.half_count) + 1) * 2)?;
                    #[allow(clippy::cast_precision_loss)] // n is at most u32::MAX.
                    let reach = n as f64;
                    for (along, across) in [(grid.u, grid.v), (grid.v, grid.u)] {
                        for i in -n..=n {
                            let major =
                                grid.major_every > 0 && i % i64::from(grid.major_every) == 0;
                            #[allow(clippy::cast_precision_loss)]
                            let base = grid.origin + along * (i as f64);
                            let (p, q) = (base - across * reach, base + across * reach);
                            let color = if major { grid.major_color } else { grid.color };
                            if let Some((a, e)) =
                                projector.segment(&Anchor::World(p), &Anchor::World(q))
                            {
                                b.segment(a, e, grid.width_px, color)?;
                            }
                        }
                    }
                }
                OverlayPrimitive::AxisTriad {
                    center,
                    length_px,
                    width_px,
                    colors,
                } => {
                    check_width(*width_px, limits)?;
                    colors.iter().try_for_each(|c| check_color(*c))?;
                    check_anchor(&Anchor::Screen(*center))?;
                    if !length_px.is_finite() {
                        return Err(Error::new(codes::VALUE_NOT_FINITE));
                    }
                    let (right, up, forward) = (frame.right(), frame.up(), frame.forward());
                    let mut axes: Vec<(f64, Pixel, Color)> =
                        [Vector3::x(), Vector3::y(), Vector3::z()]
                            .iter()
                            .zip(colors)
                            .map(|(axis, color)| {
                                let end = [
                                    center[0] + axis.dot(&right) * length_px,
                                    center[1] - axis.dot(&up) * length_px,
                                ];
                                (axis.dot(&forward), end, *color)
                            })
                            .collect();
                    // Farthest first, so nearer axes are drawn over them.
                    axes.sort_by(|a, b| b.0.total_cmp(&a.0));
                    for (_, end, color) in axes {
                        if let Some((a, e)) = projector.guard(*center, end) {
                            b.segment(a, e, *width_px, color)?;
                        }
                    }
                }
            }
        }
        Ok(b.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(triangles: &[Pixel]) -> f64 {
        triangles
            .chunks(3)
            .map(|t| {
                ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1])
                    - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]))
                    .abs()
                    / 2.0
            })
            .sum()
    }

    #[test]
    fn ear_clipping_covers_concave_polygons_exactly() {
        // An L shape of area 3, in both orientations, with a repeated point.
        let l = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        assert_eq!(ear_clip(&l).len(), 12);
        assert!((area(&ear_clip(&l)) - 3.0).abs() < 1e-12);
        let reversed: Vec<_> = l.iter().rev().copied().collect();
        assert!((area(&ear_clip(&reversed)) - 3.0).abs() < 1e-12);
        assert!(ear_clip(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
    }

    #[test]
    fn guard_band_clips_far_segments_and_rejects_outside_ones() {
        let projector = Projector {
            frame: ViewFrame::new(
                Point3::new(0.0, 0.0, 10.0),
                Point3::origin(),
                Vector3::y(),
                Projection::Orthographic { height: 10.0 },
                1.0,
                100.0,
            )
            .unwrap(),
            width: 100.0,
            height: 50.0,
            aspect: 2.0,
        };
        let (a, b) = projector.guard([50.0, 25.0], [1e30, 25.0]).unwrap();
        assert_eq!((a, b), ([50.0, 25.0], [200.0, 25.0]));
        assert!(projector.guard([-500.0, 0.0], [-300.0, 10.0]).is_none());
        // The view's center maps to the image center.
        let c = projector.point(&Anchor::World(Point3::origin())).unwrap();
        assert_eq!(c, [50.0, 25.0]);
    }

    #[test]
    fn invalid_primitives_are_errors() {
        let frame = ViewFrame::new(
            Point3::new(0.0, 0.0, 10.0),
            Point3::origin(),
            Vector3::y(),
            Projection::Perspective { vertical_fov: 1.0 },
            1.0,
            100.0,
        )
        .unwrap();
        let code = |p: OverlayPrimitive| {
            Overlay {
                primitives: vec![p],
            }
            .resolve(&frame, 64, 64, &SceneLimits::DEFAULT)
            .err()
            .map(|e| e.code())
        };
        let line = |points: Vec<Anchor>, width_px: f32| OverlayPrimitive::Polyline {
            points,
            closed: false,
            width_px,
            color: Color::WHITE,
        };
        assert_eq!(code(line(vec![], 1.0)), Some("gpu.item.invalid"));
        assert_eq!(
            code(line(vec![Anchor::Screen([f64::NAN, 0.0])], 1.0)),
            Some("gpu.value.not_finite")
        );
        assert_eq!(
            code(line(vec![Anchor::Screen([0.0, 0.0])], 1000.0)),
            Some("gpu.limit.exceeded")
        );
        assert_eq!(
            code(OverlayPrimitive::Fill {
                points: vec![Anchor::Screen([0.0, 0.0]); 2],
                color: Color::WHITE
            }),
            Some("gpu.item.invalid")
        );
        let grid = Grid {
            origin: Point3::origin(),
            u: Vector3::x(),
            v: Vector3::y(),
            half_count: u32::MAX,
            major_every: 0,
            color: Color::WHITE,
            major_color: Color::WHITE,
            width_px: 1.0,
        };
        assert_eq!(
            code(OverlayPrimitive::Grid(grid)),
            Some("gpu.limit.exceeded")
        );
        // A world point behind the eye is skipped, not an error.
        assert_eq!(
            code(line(
                vec![
                    Anchor::World(Point3::new(0.0, 0.0, 20.0)),
                    Anchor::World(Point3::new(1.0, 0.0, 20.0))
                ],
                1.0
            )),
            None
        );
    }
}
