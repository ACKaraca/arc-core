//! The glyph cache of outline text (CORE-GPU-012), behind the feature `draft`.
//!
//! A glyph outline of a font becomes a triangle mesh in em units once, and is kept by font and
//! glyph identifier: the extractor draws every occurrence of a glyph as an instance of that mesh
//! with the run's transform, so text is crisp at any zoom and needs no atlas.
//!
//! - Outlines are read with `skrifa` at the font's default location, in font units, and divided
//!   by the units per em. Quadratic and cubic segments are flattened once, in em units, with the
//!   chord tolerance [`GLYPH_CHORD_EM`]: the flattening cannot follow the zoom because the mesh
//!   is shared by every size and view. The chord error in pixels is the size of the em in
//!   pixels times the tolerance, 0.5 px at an em of 1024 px.
//! - The contours of a glyph become a region with `arc_geom::build_region` (nesting parity: a
//!   contour inside one contour is a hole) and are triangulated with `arc_geom::triangulate`.
//!   Contours that overlap or touch (some fonts draw a glyph from overlapping parts) make the
//!   region fail: such a glyph is skipped and counted, not drawn wrong.
//! - A glyph has at most [`MAX_GLYPH_POINTS`] flattened points. The cache holds at most
//!   [`MAX_CACHED_GLYPHS`] glyphs and is emptied when it is full.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use arc_base::math::{Point2, Point3};
use arc_base::{Limits, Tolerance};
use arc_draft::fonts::OutlineFont;
use arc_geom::{Composite2, Curve2, Polyline2, PolylineVertex, build_region, triangulate};
use arc_mesh::TriMesh;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// The chord tolerance of glyph flattening, in em units.
pub const GLYPH_CHORD_EM: f64 = 1.0 / 2048.0;
/// The most flattened points of one glyph.
pub const MAX_GLYPH_POINTS: usize = 1 << 16;
/// The most glyphs a cache holds before it is emptied.
pub const MAX_CACHED_GLYPHS: usize = 1 << 16;
/// The most segments one curve of an outline is divided into.
const MAX_CURVE_SEGMENTS: f64 = 64.0;

/// What a font gives for one glyph identifier.
#[derive(Debug, Clone)]
pub(crate) enum Glyph {
    /// No contours: a space or a missing glyph.
    Empty,
    /// The triangles of the glyph in em units, `z = 0`.
    Mesh(Arc<TriMesh>),
    /// The outline could not be drawn: unreadable, too large, or its contours overlap.
    Skipped,
}

/// A font's identity in the cache: the address of its bytes and its face index. The cache
/// keeps the bytes, so the address is not reused while the entry exists.
type FontKey = (usize, u32);

#[derive(Debug, Default)]
struct Inner {
    fonts: HashMap<FontKey, FontEntry>,
    count: usize,
}

#[derive(Debug)]
struct FontEntry {
    _data: Arc<[u8]>,
    glyphs: HashMap<u32, Glyph>,
}

/// Glyph meshes by font and glyph, shared by every extraction that uses the same cache (clones
/// of a cache share it).
#[derive(Debug, Clone, Default)]
pub struct GlyphCache(Arc<Mutex<Inner>>);

impl PartialEq for GlyphCache {
    /// Caches are equal when they are the same cache.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl GlyphCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A poisoned cache holds only finished entries: use them.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The number of glyphs held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().count
    }

    /// Whether the cache holds no glyph.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empties the cache.
    pub fn clear(&self) {
        *self.lock() = Inner::default();
    }

    /// The glyph `id` of `font`, drawn once.
    pub(crate) fn glyph(&self, font: &OutlineFont, index: u32, id: u32) -> Glyph {
        let key = (Arc::as_ptr(&font.data).cast::<u8>() as usize, index);
        if let Some(found) = self
            .lock()
            .fonts
            .get(&key)
            .and_then(|f| f.glyphs.get(&id))
            .cloned()
        {
            return found;
        }
        // Drawn outside the lock: two threads may draw the same glyph, with the same result.
        let drawn = draw(&font.data, index, id);
        let mut inner = self.lock();
        if inner.count >= MAX_CACHED_GLYPHS {
            *inner = Inner::default();
        }
        let entry = inner.fonts.entry(key).or_insert_with(|| FontEntry {
            _data: Arc::clone(&font.data),
            glyphs: HashMap::new(),
        });
        if entry.glyphs.insert(id, drawn.clone()).is_none() {
            inner.count += 1;
        }
        drawn
    }
}

/// Collects the flattened contours of an outline, in em units.
struct Contours {
    /// Em units per font unit.
    unit: f64,
    chord: f64,
    done: Vec<Vec<Point2>>,
    current: Vec<Point2>,
    points: usize,
    overflow: bool,
}

impl Contours {
    fn at(&self, x: f32, y: f32) -> Point2 {
        Point2::new(f64::from(x) * self.unit, f64::from(y) * self.unit)
    }

    fn push(&mut self, p: Point2) {
        self.points += 1;
        if self.points > MAX_GLYPH_POINTS || !(p.x.is_finite() && p.y.is_finite()) {
            self.overflow = true;
        } else {
            self.current.push(p);
        }
    }

    fn finish(&mut self) {
        let mut points = std::mem::take(&mut self.current);
        let tolerance = Tolerance::DEFAULT.linear;
        points.dedup_by(|b, a| (b.x - a.x).abs() <= tolerance && (b.y - a.y).abs() <= tolerance);
        while points.len() > 1
            && points.first().zip(points.last()).is_some_and(|(a, b)| {
                (b.x - a.x).abs() <= tolerance && (b.y - a.y).abs() <= tolerance
            })
        {
            points.pop();
        }
        if points.len() >= 3 {
            self.done.push(points);
        }
    }

    fn last(&self) -> Point2 {
        self.current.last().copied().unwrap_or_else(Point2::origin)
    }

    /// Divides a curve whose second differences are at most `second` long so that the chord
    /// error is within the tolerance: `error ≤ factor · second / n²`.
    fn segments(&self, second: f64, factor: f64) -> u32 {
        let n = (factor * second / self.chord).sqrt().ceil();
        if n.is_finite() {
            // Within 1..=MAX_CURVE_SEGMENTS, so the conversion is exact.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = n.clamp(1.0, MAX_CURVE_SEGMENTS) as u32;
            n
        } else {
            1
        }
    }
}

fn lerp_points(weights: &[f64], points: &[Point2]) -> Point2 {
    let (mut x, mut y) = (0.0, 0.0);
    for (w, p) in weights.iter().zip(points) {
        x += w * p.x;
        y += w * p.y;
    }
    Point2::new(x, y)
}

fn second_difference(a: Point2, b: Point2, c: Point2) -> f64 {
    (a.x - 2.0 * b.x + c.x).hypot(a.y - 2.0 * b.y + c.y)
}

impl OutlinePen for Contours {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        let p = self.at(x, y);
        self.push(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.push(p);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (p0, p1, p2) = (self.last(), self.at(cx, cy), self.at(x, y));
        let n = self.segments(second_difference(p0, p1, p2), 0.25);
        for i in 1..=n {
            let t = f64::from(i) / f64::from(n);
            let u = 1.0 - t;
            self.push(lerp_points(&[u * u, 2.0 * u * t, t * t], &[p0, p1, p2]));
        }
    }

    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        let (p0, p1, p2, p3) = (
            self.last(),
            self.at(c0x, c0y),
            self.at(c1x, c1y),
            self.at(x, y),
        );
        let second = second_difference(p0, p1, p2).max(second_difference(p1, p2, p3));
        let n = self.segments(second, 0.75);
        for i in 1..=n {
            let t = f64::from(i) / f64::from(n);
            let u = 1.0 - t;
            let weights = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            self.push(lerp_points(&weights, &[p0, p1, p2, p3]));
        }
    }

    fn close(&mut self) {
        self.finish();
    }
}

fn draw(data: &[u8], index: u32, id: u32) -> Glyph {
    let Ok(font) = FontRef::from_index(data, index) else {
        return Glyph::Skipped;
    };
    let units = font
        .metrics(Size::unscaled(), LocationRef::default())
        .units_per_em;
    let Some(outline) = font.outline_glyphs().get(GlyphId::new(id)) else {
        return Glyph::Skipped;
    };
    if units == 0 {
        return Glyph::Skipped;
    }
    let mut contours = Contours {
        unit: 1.0 / f64::from(units),
        chord: GLYPH_CHORD_EM,
        done: Vec::new(),
        current: Vec::new(),
        points: 0,
        overflow: false,
    };
    let settings = DrawSettings::unhinted(Size::unscaled(), LocationRef::default());
    if outline.draw(settings, &mut contours).is_err() {
        return Glyph::Skipped;
    }
    contours.finish();
    if contours.overflow {
        return Glyph::Skipped;
    }
    if contours.done.is_empty() {
        return Glyph::Empty;
    }
    mesh(&contours.done).map_or(Glyph::Skipped, |m| Glyph::Mesh(Arc::new(m)))
}

fn mesh(contours: &[Vec<Point2>]) -> Option<TriMesh> {
    let tolerance = Tolerance::DEFAULT;
    let limits = Limits::DEFAULT;
    let composites = contours
        .iter()
        .map(|points| {
            let vertices = points
                .iter()
                .map(|p| PolylineVertex::new(*p, 0.0))
                .collect();
            let polyline = Polyline2::new(vertices, true).ok()?;
            Composite2::new(vec![Curve2::Polyline(polyline)], &tolerance).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    let region = build_region(&composites, &tolerance, &limits).ok()?;
    let flat = triangulate(
        &region,
        tolerance.tessellation_chord,
        tolerance.tessellation_angle,
        MAX_GLYPH_POINTS,
    )
    .ok()?;
    let positions = flat
        .vertices()
        .iter()
        .map(|p| Point3::new(p.x, p.y, 0.0))
        .collect();
    TriMesh::new(positions, flat.triangles().to_vec(), &limits).ok()
}
