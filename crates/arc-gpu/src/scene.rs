//! The retained viewport scene (CORE-GPU-002) on pooled buffers (CORE-GPU-006).
//!
//! A [`Scene`] holds render items keyed by [`ItemKey`] (object identifier and sub-item index).
//! Mesh geometry is uploaded once with [`Scene::add_geometry`] into the vertex and index pools;
//! items reference it by [`GeometryId`]. [`Scene::update`] replaces the items of the objects it
//! names and writes only their instance ranges: the bytes of every other item stay untouched on
//! the GPU. [`Scene::pending_writes`] counts the bytes written since the last frame per
//! category.
//!
//! Instance data is serialized by hand as little-endian bytes; see the layouts in the
//! specification.

use std::collections::HashMap;

use arc_base::math::{Matrix4, Point3, Vector3};
use arc_base::{Color, Error, ObjectId, Result};
use arc_mesh::TriMesh;
use arc_mesh::lod::{LodChain, ViewParams};

use crate::adapter::Gpu;
use crate::camera::relative_translation;
use crate::curve::{CurveItem, CurveState, Flattened};
use crate::error::codes;
use crate::pool::{AllocId, Pool, PoolUsage};
use crate::tile::{TILE_SIZE, TiledGeometry, split_mesh};

/// Bytes per vertex: position and normal, `f32` each.
pub const VERTEX_STRIDE: u64 = 24;
/// Bytes per index (`u32`).
pub const INDEX_STRIDE: u64 = 4;
/// Bytes per mesh instance.
pub const MESH_INSTANCE_STRIDE: u64 = 80;
/// Bytes per line segment instance.
pub const SEGMENT_STRIDE: u64 = 128;
/// Bytes per camera-relative translation.
pub const TRANSLATION_STRIDE: u64 = 12;

/// The limits of a scene. Inputs beyond them fail with `gpu.limit.exceeded`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLimits {
    /// Objects with items.
    pub max_objects: u32,
    /// Items of one object.
    pub max_items_per_object: u32,
    /// Instances of one mesh item.
    pub max_instances_per_item: u32,
    /// Segments of one line item, after polylines are split into segments.
    pub max_segments_per_item: u32,
    /// Vertices of one geometry.
    pub max_geometry_vertices: u32,
    /// Triangles of one geometry.
    pub max_geometry_triangles: u32,
    /// Bytes of one pool (vertex, index, mesh instance or line segment).
    pub max_pool_bytes: u64,
    /// The largest line width in pixels, after paper widths are converted.
    pub max_line_width_px: f32,
    /// The largest side of a rendered image in pixels.
    pub max_image_side: u32,
    /// Whether geometries added from now on also get silhouette edge data (72 bytes of the
    /// vertex pool per smooth interior edge), which
    /// [`RenderSettings::silhouette_edges`](crate::RenderSettings::silhouette_edges) draws.
    pub silhouette_edges: bool,
}

impl SceneLimits {
    /// The default limits.
    pub const DEFAULT: Self = Self {
        max_objects: 10_000_000,
        max_items_per_object: 4096,
        max_instances_per_item: 1_000_000,
        max_segments_per_item: 1_000_000,
        max_geometry_vertices: 16_000_000,
        max_geometry_triangles: 16_000_000,
        max_pool_bytes: 1 << 30,
        max_line_width_px: 256.0,
        max_image_side: 8192,
        silhouette_edges: false,
    };
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The key of a render item: the object it came from and its index among that object's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemKey {
    /// The source object.
    pub object: ObjectId,
    /// The index of the item among the object's items.
    pub sub: u32,
}

/// The handle of a geometry uploaded to a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GeometryId(u64);

impl GeometryId {
    /// The bit of a handle local to one [`crate::Extraction`]; a scene never issues it.
    pub(crate) const LOCAL: u64 = 1 << 63;

    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

/// One instance of a mesh item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshInstance {
    /// The affine transform from geometry coordinates to offsets from the item origin, as the
    /// three rows of a 3×4 matrix.
    pub transform: [[f32; 4]; 3],
    /// The base color.
    pub color: Color,
    /// The material slot.
    pub material: u32,
    /// Display flags: bit 4 is [`crate::display::FLAG_SOLID`], bits 24 to 27 are the category of
    /// display overrides ([`crate::category_flags`]); bits 0 to 3 are the paint and depth flags of
    /// drawings.
    pub flags: u32,
}

impl MeshInstance {
    /// The identity transform.
    pub const IDENTITY: [[f32; 4]; 3] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ];

    /// An instance at the item origin with the identity transform.
    #[must_use]
    pub fn new(color: Color) -> Self {
        Self {
            transform: Self::IDENTITY,
            color,
            material: 0,
            flags: 0,
        }
    }
}

/// Instances of one uploaded geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshItem {
    /// The geometry.
    pub geometry: GeometryId,
    /// The model-space point the instance transforms are relative to.
    pub origin: Point3,
    /// The instances; at least one.
    pub instances: Vec<MeshInstance>,
}

/// The width of a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineWidth {
    /// A full width in pixels, independent of zoom.
    Screen(f32),
    /// A full width in paper millimetres, converted to model units through the plot scale of
    /// the render settings and to pixels through the view.
    Paper(f32),
}

/// The shape of a line end that does not join another segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    /// A half disk.
    Round,
    /// A half square: the line extends by half its width.
    Square,
}

/// The shape of the corner where two segments of a polyline meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    /// A disk around the shared vertex.
    Round,
    /// The outer edges extended until they meet, or a bevel when the miter is longer than the
    /// miter limit.
    Miter,
    /// The outer corners connected by a straight edge.
    Bevel,
}

/// The style of a line item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineStyle {
    /// The width.
    pub width: LineWidth,
    /// The caps of free ends.
    pub cap: LineCap,
    /// The joins between segments.
    pub join: LineJoin,
    /// The longest miter as a multiple of half the width, at least 1.
    pub miter_limit: f32,
    /// Whether the width is a lineweight: drawn one pixel wide while
    /// [`crate::RenderSettings::lineweights`] is off.
    pub lineweight: bool,
}

impl LineStyle {
    /// A style with round caps and joins that is not a lineweight.
    #[must_use]
    pub fn round(width: LineWidth) -> Self {
        Self {
            width,
            cap: LineCap::Round,
            join: LineJoin::Round,
            miter_limit: 4.0,
            lineweight: false,
        }
    }
}

/// A polyline of a line item, with points relative to the item origin.
#[derive(Debug, Clone, PartialEq)]
pub struct LinePath {
    /// The points; at least one. Consecutive equal points are merged. A path of one point is
    /// a dot.
    pub points: Vec<[f32; 3]>,
    /// Whether the last point connects back to the first.
    pub closed: bool,
}

/// Polylines drawn with one style and color.
#[derive(Debug, Clone, PartialEq)]
pub struct LineItem {
    /// The model-space point the path points are relative to.
    pub origin: Point3,
    /// The polylines; at least one.
    pub paths: Vec<LinePath>,
    /// The style.
    pub style: LineStyle,
    /// The color; alpha blends.
    pub color: Color,
}

/// A render item.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderItem {
    /// Mesh instances.
    Mesh(MeshItem),
    /// Lines.
    Lines(LineItem),
    /// Exact curves, flattened for the current zoom, optionally dashed and clipped.
    Curves(CurveItem),
}

/// Bytes written to GPU buffers per category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteCounters {
    /// Vertex data.
    pub vertex: u64,
    /// Index data.
    pub index: u64,
    /// Mesh instance data.
    pub mesh_instance: u64,
    /// Line segment data.
    pub line_segment: u64,
    /// Camera-relative translations.
    pub translation: u64,
}

/// GPU memory per category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryReport {
    /// The vertex pool.
    pub vertex: PoolUsage,
    /// The index pool.
    pub index: PoolUsage,
    /// The mesh instance pool.
    pub mesh_instance: PoolUsage,
    /// The line segment pool.
    pub line_segment: PoolUsage,
    /// The translation buffers of both instance pools.
    pub translation: PoolUsage,
}

impl MemoryReport {
    /// The sum over all categories.
    #[must_use]
    pub fn total(&self) -> PoolUsage {
        let all = [
            self.vertex,
            self.index,
            self.mesh_instance,
            self.line_segment,
            self.translation,
        ];
        PoolUsage {
            capacity_bytes: all.iter().map(|u| u.capacity_bytes).sum(),
            used_bytes: all.iter().map(|u| u.used_bytes).sum(),
        }
    }
}

#[derive(Debug)]
struct GeometryEntry {
    vertices: AllocId,
    indices: AllocId,
    index_count: u32,
    /// The feature edges as index pairs in the index pool, when there are any.
    edges: Option<(AllocId, u32)>,
    /// The silhouette records in the vertex pool and the edge count, when derived.
    silhouettes: Option<(AllocId, u32)>,
    radius: f64,
    /// The box of the uploaded positions, relative to the geometry origin: `[min, max]`.
    bounds: [[f64; 3]; 2],
    users: u32,
}

/// The levels of a geometry with levels of detail: the chain and the uploaded geometry of
/// each level (level 0 is the group's own handle).
#[derive(Debug)]
struct LodGroup {
    chain: LodChain,
    levels: Vec<GeometryId>,
}

/// Faces meeting at an angle above this (radians) make their shared edge a feature edge.
pub const FEATURE_EDGE_ANGLE: f64 = std::f64::consts::FRAC_PI_6;

/// `v` scaled to unit length without overflow, or `None` for a zero or non-finite vector.
pub(crate) fn unit(v: &Vector3) -> Option<Vector3> {
    let largest = v.x.abs().max(v.y.abs()).max(v.z.abs());
    if !(largest > 0.0 && largest.is_finite()) {
        return None;
    }
    let scaled = v / largest;
    Some(scaled / scaled.norm())
}

/// The length of `v` without overflow of intermediate squares.
pub(crate) fn length(v: &Vector3) -> f64 {
    let largest = v.x.abs().max(v.y.abs()).max(v.z.abs());
    if largest > 0.0 && largest.is_finite() {
        (v / largest).norm() * largest
    } else {
        largest
    }
}

/// The feature edges of `mesh` as pairs of vertex indices (boundary, sharp and non-manifold
/// edges of [`arc_mesh::features::feature_edges`] at [`FEATURE_EDGE_ANGLE`]), sorted so the
/// result is deterministic.
pub(crate) fn feature_edge_pairs(mesh: &TriMesh) -> Result<Vec<[u32; 2]>> {
    // The scene bounds the size of a geometry itself.
    let limits = arc_base::Limits {
        array_length: u64::from(u32::MAX),
        memory_bytes: None,
        ..arc_base::Limits::DEFAULT
    };
    let found = arc_mesh::features::feature_edges(
        mesh,
        FEATURE_EDGE_ANGLE,
        &arc_base::Tolerance::DEFAULT,
        &limits,
    )
    .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
    let mut pairs: Vec<[u32; 2]> = found
        .boundary
        .iter()
        .chain(&found.sharp)
        .chain(&found.non_manifold)
        .map(|e| e.vertices)
        .collect();
    pairs.sort_unstable();
    Ok(pairs)
}

/// The silhouette records of `mesh`: for every smooth edge of exactly two faces (creases are
/// feature edges and boundaries have one face), two records of nine `f32` (an end point as
/// relative to the geometry origin, then the unit normals of the two faces), one for each end.
/// Vertices at exactly equal positions are one vertex. `positions` are the uploaded vertex
/// positions. Returns the bytes and the edge count.
pub(crate) fn silhouette_records(mesh: &TriMesh, positions: &[[f32; 3]]) -> (Vec<u8>, u32) {
    struct Edge {
        ends: [u32; 2],
        faces: u32,
        normals: [Vector3; 2],
    }
    let source = mesh.positions();
    let mut welded: HashMap<[u64; 3], u32> = HashMap::new();
    let canonical: Vec<u32> = source
        .iter()
        .zip(0_u32..)
        // Adding zero turns −0 into +0 so both weld.
        .map(|(p, i)| {
            *welded
                .entry(p.coords.map(|c| (c + 0.0).to_bits()).into())
                .or_insert(i)
        })
        .collect();
    let cos_limit = FEATURE_EDGE_ANGLE.cos();
    let mut edges: HashMap<(u32, u32), Edge> = HashMap::new();
    for t in mesh.triangles() {
        let corner = |k: usize| t.get(k).and_then(|i| source.get(*i as usize));
        let (Some(a), Some(b), Some(c)) = (corner(0), corner(1), corner(2)) else {
            continue;
        };
        let Some(normal) = unit(&(b - a).cross(&(c - a))) else {
            continue;
        };
        for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let (Some(&ci), Some(&cj)) = (canonical.get(i as usize), canonical.get(j as usize))
            else {
                continue;
            };
            if ci == cj {
                continue;
            }
            let edge = edges.entry((ci.min(cj), ci.max(cj))).or_insert(Edge {
                ends: [i, j],
                faces: 0,
                normals: [normal; 2],
            });
            edge.faces += 1;
            if edge.faces == 2 {
                edge.normals[1] = normal;
            }
        }
    }
    type Smooth = ((u32, u32), [u32; 2], [Vector3; 2]);
    let mut smooth: Vec<Smooth> = edges
        .into_iter()
        .filter(|(_, e)| e.faces == 2 && e.normals[0].dot(&e.normals[1]) >= cos_limit)
        .map(|(key, e)| (key, e.ends, e.normals))
        .collect();
    smooth.sort_unstable_by_key(|(key, _, _)| *key);
    let mut bytes = Vec::with_capacity(smooth.len() * SILHOUETTE_EDGE_BYTES);
    let mut count = 0_u32;
    for (_, ends, normals) in &smooth {
        let [Some(a), Some(b)] = ends.map(|i| positions.get(i as usize)) else {
            continue;
        };
        let narrowed = normals.map(|n| [narrow(n.x), narrow(n.y), narrow(n.z)]);
        for end in [a, b] {
            put_f32(&mut bytes, end);
            put_f32(&mut bytes, &narrowed[0]);
            put_f32(&mut bytes, &narrowed[1]);
        }
        count += 1;
    }
    (bytes, count)
}

/// Bytes of the two records of one silhouette edge: three vertex units of the pool.
pub(crate) const SILHOUETTE_EDGE_BYTES: usize = 72;
/// Bytes of one silhouette record.
pub(crate) const SILHOUETTE_STRIDE: u64 = 36;

fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = v as f32;
    narrowed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemGpu {
    Mesh {
        geometry: GeometryId,
        instances: AllocId,
        count: u32,
    },
    Lines {
        segments: AllocId,
        count: u32,
    },
}

#[derive(Debug)]
pub(crate) struct ItemEntry {
    pub(crate) gpu: ItemGpu,
    pub(crate) origin: Point3,
    pub(crate) radius: f64,
    /// The box of everything the item draws, relative to its origin: `[min, max]`. For a mesh
    /// item it covers the instances of every level of its geometry.
    pub(crate) bounds: [[f64; 3]; 2],
    /// How far, in pixels, the item draws past its bounds, or `None` when that extent depends
    /// on the view (paper or item-unit widths): such an item is never culled.
    pub(crate) margin_px: Option<f64>,
    /// Whether the item is inside the view of the last frame or pick (see [`Scene::cull`]).
    pub(crate) visible: bool,
    /// The largest scale factor of the item's instance transforms (1 for lines).
    scale: f64,
    /// Whether an instance has a negative determinant (a mirror). Always false for lines.
    pub(crate) mirrored: bool,
    /// The level of detail drawn this frame, when the item's geometry has levels.
    pub(crate) shown: Option<GeometryId>,
    translation_dirty: bool,
    /// The cumulative segment count at the end of each path of a line or curve item.
    path_ends: Vec<u32>,
    /// The retained curves of a curve item and the zoom they were flattened for.
    pub(crate) curve: Option<Box<CurveState>>,
}

#[derive(Debug)]
struct ObjectEntry {
    slot: u32,
    items: Vec<ItemEntry>,
}

/// A retained viewport scene on one device.
#[derive(Debug)]
pub struct Scene {
    gpu: Gpu,
    limits: SceneLimits,
    vertices: Pool,
    indices: Pool,
    mesh_instances: Pool,
    segments: Pool,
    geometries: HashMap<GeometryId, GeometryEntry>,
    /// Geometries with levels of detail, by their level 0 handle.
    lods: HashMap<GeometryId, LodGroup>,
    next_geometry: u64,
    objects: HashMap<ObjectId, ObjectEntry>,
    free_slots: Vec<u32>,
    next_slot: u32,
    pending: WriteCounters,
    last_frame: WriteCounters,
    last_eye: Option<Point3>,
    all_translations_dirty: bool,
    /// The pixels per item unit of the last frame's orthographic view, the zoom new curve
    /// items are first flattened for.
    zoom_hint: Option<f64>,
    /// The chord tolerance in pixels of the last frame.
    curve_tolerance_px: f32,
    /// The applied extraction stamp and owned geometries of each extracted object.
    pub(crate) extracted: crate::extract::Applied,
    /// The counts of the last [`Scene::cull`].
    cull_stats: CullStats,
}

/// The items of a scene and how many the view of the last frame or pick kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CullStats {
    /// Items in the scene.
    pub items: usize,
    /// Items that are not wholly outside the view and are drawn.
    pub visible: usize,
}

/// The view that items are culled against (CORE-GPU-018, CPU path): the clip matrix of the
/// frame in `f64` without the image window, the translation origin, and the size of a pixel in
/// clip units of the image.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CullView {
    /// Projection times view rotation, applied to offsets from the translation origin.
    pub(crate) clip: Matrix4,
    /// The translation origin of the frame.
    pub(crate) eye: Point3,
    /// Clip units (normalized device units) per image pixel, horizontal and vertical.
    pub(crate) pixel: [f64; 2],
    /// Pixels added to every item's margin (the pick tolerance of a pick).
    pub(crate) extra_px: f64,
}

impl CullView {
    /// Whether `item` is wholly outside the view.
    ///
    /// The clip-space half-spaces are affine in the position, so an item is outside when one of
    /// them is negative at all eight corners of its box. The side planes are widened by the
    /// item's margin: a drawn pixel of the item lies within the margin of a point of the item,
    /// so a point outside the widened planes draws nothing. The near and far planes are exact.
    /// A non-finite value keeps the item.
    fn excludes(&self, item: &ItemEntry) -> bool {
        let Some(margin) = item.margin_px else {
            return false;
        };
        let margin = margin + self.extra_px;
        let (dx, dy) = (margin * self.pixel[0], margin * self.pixel[1]);
        let m = &self.clip;
        let [low, high] = item.bounds;
        let offset = item.origin - self.eye;
        // One flag per half-space: true while every corner so far is negative in it.
        let mut all_negative = [true; 6];
        for corner in 0..8 {
            let pick = |k: usize| {
                if corner & (1 << k) == 0 {
                    low[k]
                } else {
                    high[k]
                }
            };
            let p = [offset.x + pick(0), offset.y + pick(1), offset.z + pick(2)];
            let clip =
                |r: usize| m[(r, 0)] * p[0] + m[(r, 1)] * p[1] + m[(r, 2)] * p[2] + m[(r, 3)];
            let (x, y, z, w) = (clip(0), clip(1), clip(2), clip(3));
            let values = [
                x + w * (1.0 + dx),
                w * (1.0 + dx) - x,
                y + w * (1.0 + dy),
                w * (1.0 + dy) - y,
                z,
                w - z,
            ];
            if !values.iter().all(|v| v.is_finite()) {
                return false;
            }
            for (flag, value) in all_negative.iter_mut().zip(values) {
                *flag &= value < 0.0;
            }
        }
        all_negative.iter().any(|flag| *flag)
    }
}

/// Grows the box `into` to contain `other` (`[min, max]` corners).
pub(crate) fn include_box(into: &mut Option<[[f64; 3]; 2]>, other: [[f64; 3]; 2]) {
    *into = Some(match *into {
        None => other,
        Some([lo, hi]) => {
            let [olo, ohi] = other;
            [
                [0, 1, 2].map(|k| lo[k].min(olo[k])),
                [0, 1, 2].map(|k| hi[k].max(ohi[k])),
            ]
        }
    });
}

/// The box of `box_` under the affine map `rows` (a 3×4 matrix: three rows, the translation in
/// the fourth column), as the box of its eight transformed corners.
pub(crate) fn transformed_box(rows: &[[f32; 4]; 3], box_: [[f64; 3]; 2]) -> [[f64; 3]; 2] {
    let mut out = None;
    for corner in 0..8 {
        let p = [0, 1, 2].map(|k| {
            if corner & (1 << k) == 0 {
                box_[0][k]
            } else {
                box_[1][k]
            }
        });
        let q = rows.map(|r| {
            f64::from(r[0]) * p[0]
                + f64::from(r[1]) * p[1]
                + f64::from(r[2]) * p[2]
                + f64::from(r[3])
        });
        include_box(&mut out, [q, q]);
    }
    out.unwrap_or(box_)
}

fn put_f32(out: &mut Vec<u8>, values: &[f32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

fn put_u32(out: &mut Vec<u8>, values: &[u32]) {
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
}

pub(crate) fn finite(values: &[f32]) -> Result<()> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_NOT_FINITE))
    }
}

pub(crate) fn finite_point(p: &Point3) -> Result<()> {
    if p.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(Error::new(codes::VALUE_NOT_FINITE))
    }
}

pub(crate) fn limit(detail: &'static str) -> Error {
    Error::new(codes::LIMIT_EXCEEDED).with_detail("limit", detail)
}

pub(crate) fn color_array(c: Color) -> [f32; 4] {
    [c.r, c.g, c.b, c.a]
}

pub(crate) fn length3(v: [f32; 3]) -> f64 {
    let [x, y, z] = v.map(f64::from);
    x.hypot(y).hypot(z)
}

/// Style bit 7: the width is a lineweight, one pixel while lineweight display is off.
pub(crate) const BIT_LINEWEIGHT: u32 = 1 << 7;
/// Style bit 8: the segment is dashed by the pattern in its instance.
pub(crate) const BIT_DASHED: u32 = 1 << 8;
/// Style bit 9: the widths are in item units (polyline widths), converted through the view;
/// free ends are cut flat.
pub(crate) const BIT_WORLD_WIDTH: u32 = 1 << 9;

/// The style bits of a segment: bit 0 square cap; bits 1–2 join (0 round, 1 miter, 2 bevel);
/// bit 3 has a previous segment; bit 4 has a next segment; bit 5 paper width; bit 7
/// lineweight.
fn style_bits(style: &LineStyle, prev: bool, next: bool) -> u32 {
    let mut bits = u32::from(style.cap == LineCap::Square);
    bits |= match style.join {
        LineJoin::Round => 0,
        LineJoin::Miter => 1,
        LineJoin::Bevel => 2,
    } << 1;
    bits |= u32::from(prev) << 3;
    bits |= u32::from(next) << 4;
    bits |= u32::from(matches!(style.width, LineWidth::Paper(_))) << 5;
    if style.lineweight {
        bits |= BIT_LINEWEIGHT;
    }
    bits
}

/// What a segment carries beyond its end points.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct SegmentExtra {
    /// Full widths in item units at the start and end (polyline widths); `None` uses the
    /// style width.
    pub(crate) widths: Option<[f32; 2]>,
    /// The pattern positions at the start and end, the start within one period.
    pub(crate) phase: [f32; 2],
}

/// A path vertex and the extra data of the segment that starts at it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathVertex {
    pub(crate) point: [f32; 3],
    pub(crate) out: SegmentExtra,
}

/// The appearance shared by the segments of one item.
pub(crate) struct Look<'a> {
    pub(crate) style: &'a LineStyle,
    pub(crate) color: [f32; 4],
    /// The pattern: period, then four `(start, end)` intervals (unused ones negative); `None`
    /// is solid.
    pub(crate) pattern: Option<[f32; 9]>,
    pub(crate) slot: u32,
}

/// Segment instance bytes being built for one item.
#[derive(Debug, Default)]
pub(crate) struct Encoded {
    pub(crate) bytes: Vec<u8>,
    pub(crate) count: u32,
    pub(crate) radius: f64,
    /// The cumulative segment count at the end of each path.
    pub(crate) path_ends: Vec<u32>,
    /// The box of the path vertices, relative to the item origin.
    pub(crate) bounds: Option<[[f64; 3]; 2]>,
    /// The largest pixel extent past the vertices, over the paths with screen widths.
    pub(crate) margin_px: f64,
    /// Whether a path has a width that depends on the view, so no margin bounds it.
    pub(crate) unbounded: bool,
}

impl Encoded {
    /// The pixel margin for culling, or `None` when a width depends on the view.
    pub(crate) fn cull_margin(&self) -> Option<f64> {
        (!self.unbounded).then_some(self.margin_px)
    }
}

/// The bytes of one segment instance (see the layout in the specification).
#[allow(clippy::too_many_arguments)] // One field of the instance layout each.
pub(crate) fn put_segment(
    out: &mut Vec<u8>,
    ends: [[f32; 3]; 4],
    color: [f32; 4],
    widths: [f32; 2],
    miter: f32,
    bits: u32,
    slot: u32,
    phase: [f32; 2],
    pattern: &[f32; 9],
) {
    for p in ends {
        put_f32(out, &p);
    }
    put_f32(out, &color);
    put_f32(out, &[widths[0], miter]);
    put_u32(out, &[bits, slot]);
    put_f32(out, &[widths[1], phase[0], phase[1]]);
    put_f32(out, pattern);
}

/// A solid pattern: period 0 and no intervals.
pub(crate) const SOLID: [f32; 9] = [0.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0];

impl Encoded {
    /// Appends the segments of one path. Consecutive equal points merge (the merged vertex
    /// keeps the extra data of the later one); a closed path whose last point repeats its
    /// first drops it.
    pub(crate) fn path(
        &mut self,
        vertices: impl IntoIterator<Item = PathVertex>,
        closed: bool,
        look: &Look<'_>,
        limits: &SceneLimits,
    ) -> Result<()> {
        let style = look.style;
        let width = match style.width {
            LineWidth::Screen(w) | LineWidth::Paper(w) => w,
        };
        // Half the width, times the longest miter (a multiple of half the width), plus one pixel
        // of coverage at the edge.
        match style.width {
            LineWidth::Screen(_) => {
                let half = f64::from(width.max(1.0)) * 0.5 * f64::from(style.miter_limit.max(1.0));
                self.margin_px = self.margin_px.max(half + 1.0);
            }
            LineWidth::Paper(_) => self.unbounded = true,
        }
        let mut points: Vec<PathVertex> = Vec::new();
        for v in vertices {
            finite(&v.point)?;
            if let Some(w) = v.out.widths {
                finite(&w)?;
                // Item-unit widths scale with the view, so no pixel margin bounds them.
                self.unbounded = true;
            }
            finite(&v.out.phase)?;
            self.radius = self.radius.max(length3(v.point));
            let p = v.point.map(f64::from);
            include_box(&mut self.bounds, [p, p]);
            match points.last_mut() {
                Some(last) if last.point == v.point => last.out = v.out,
                _ => points.push(v),
            }
        }
        if closed
            && points.len() > 2
            && points.first().map(|p| p.point) == points.last().map(|p| p.point)
        {
            points.pop();
        }
        let n = points.len();
        if n == 0 {
            return Err(Error::new(codes::ITEM_INVALID));
        }
        let closed = closed && n > 2;
        let segment_count = match n {
            1 => 1,
            _ if closed => n,
            _ => n - 1,
        };
        let at = |i: usize| points.get(i % n).copied();
        let pattern = look.pattern.unwrap_or(SOLID);
        for i in 0..segment_count {
            self.count = self
                .count
                .checked_add(1)
                .filter(|c| *c <= limits.max_segments_per_item)
                .ok_or_else(|| limit("max_segments_per_item"))?;
            let (Some(va), Some(vb)) = (at(i), at(if n == 1 { i } else { i + 1 })) else {
                return Err(Error::new(codes::ITEM_INVALID));
            };
            let (a, b) = (va.point, vb.point);
            let has_prev = closed || i > 0;
            let has_next = closed || i + 2 < n;
            let prev = if has_prev {
                at(i + n - 1).map_or(a, |v| v.point)
            } else {
                a
            };
            let next = if has_next {
                at(i + 2).map_or(b, |v| v.point)
            } else {
                b
            };
            let mut bits = style_bits(style, has_prev, has_next);
            let widths = match va.out.widths {
                Some(w) => {
                    bits = (bits & !(BIT_LINEWEIGHT | (1 << 5))) | BIT_WORLD_WIDTH;
                    w
                }
                None => [width, width],
            };
            if look.pattern.is_some() {
                bits |= BIT_DASHED;
            }
            put_segment(
                &mut self.bytes,
                [a, b, prev, next],
                look.color,
                widths,
                style.miter_limit,
                bits,
                look.slot,
                va.out.phase,
                &pattern,
            );
        }
        self.path_ends.push(self.count);
        Ok(())
    }
}

/// Validates a line style and color.
pub(crate) fn check_style(style: &LineStyle, color: Color, limits: &SceneLimits) -> Result<()> {
    let width = match style.width {
        LineWidth::Screen(w) | LineWidth::Paper(w) => w,
    };
    finite(&[width, style.miter_limit])?;
    finite(&color_array(color))?;
    if !(width > 0.0 && style.miter_limit >= 1.0) {
        return Err(Error::new(codes::ITEM_INVALID));
    }
    if matches!(style.width, LineWidth::Screen(w) if w > limits.max_line_width_px) {
        return Err(limit("max_line_width_px"));
    }
    Ok(())
}

/// The segments of a line item.
fn encode_lines(item: &LineItem, limits: &SceneLimits, slot: u32) -> Result<Encoded> {
    check_style(&item.style, item.color, limits)?;
    if item.paths.is_empty() {
        return Err(Error::new(codes::ITEM_INVALID));
    }
    let look = Look {
        style: &item.style,
        color: color_array(item.color),
        pattern: None,
        slot,
    };
    let mut out = Encoded::default();
    for path in &item.paths {
        out.path(
            path.points.iter().map(|p| PathVertex {
                point: *p,
                out: SegmentExtra::default(),
            }),
            path.closed,
            &look,
            limits,
        )?;
    }
    Ok(out)
}

/// The box of an item in scene coordinates: its bounds moved by its origin, or the box of its
/// sphere when it has no finite bounds.
fn item_box(item: &ItemEntry) -> (Point3, Point3) {
    let [lo, hi] = item.bounds;
    if lo.iter().chain(hi.iter()).all(|v| v.is_finite()) && (0..3).all(|k| lo[k] <= hi[k]) {
        (
            item.origin + Vector3::new(lo[0], lo[1], lo[2]),
            item.origin + Vector3::new(hi[0], hi[1], hi[2]),
        )
    } else {
        let r = item.radius;
        (item.origin.map(|v| v - r), item.origin.map(|v| v + r))
    }
}

impl Scene {
    /// Creates an empty scene on `gpu`.
    ///
    /// # Errors
    ///
    /// `gpu.settings.invalid` when a limit is zero or the line width limit is not positive;
    /// device errors from buffer creation.
    pub fn new(gpu: &Gpu, limits: SceneLimits) -> Result<Self> {
        let counts = [
            limits.max_objects,
            limits.max_items_per_object,
            limits.max_instances_per_item,
            limits.max_segments_per_item,
            limits.max_geometry_vertices,
            limits.max_geometry_triangles,
            limits.max_image_side,
        ];
        if counts.contains(&0)
            || limits.max_pool_bytes < 1024
            || !(limits.max_line_width_px >= 1.0 && limits.max_line_width_px.is_finite())
        {
            return Err(Error::new(codes::SETTINGS_INVALID));
        }
        let device = gpu.device();
        let max_bytes = limits.max_pool_bytes.min(device.limits().max_buffer_size);
        let vertex = wgpu::BufferUsages::VERTEX;
        gpu.scoped(|| {
            Ok(Self {
                gpu: gpu.clone(),
                limits,
                vertices: Pool::new(
                    device,
                    "arc-gpu vertices",
                    vertex,
                    VERTEX_STRIDE,
                    0,
                    max_bytes,
                ),
                indices: Pool::new(
                    device,
                    "arc-gpu indices",
                    wgpu::BufferUsages::INDEX,
                    INDEX_STRIDE,
                    0,
                    max_bytes,
                ),
                mesh_instances: Pool::new(
                    device,
                    "arc-gpu mesh instances",
                    vertex,
                    MESH_INSTANCE_STRIDE,
                    TRANSLATION_STRIDE,
                    max_bytes,
                ),
                segments: Pool::new(
                    device,
                    "arc-gpu line segments",
                    vertex,
                    SEGMENT_STRIDE,
                    TRANSLATION_STRIDE,
                    max_bytes,
                ),
                geometries: HashMap::new(),
                lods: HashMap::new(),
                next_geometry: 0,
                objects: HashMap::new(),
                free_slots: Vec::new(),
                next_slot: 0,
                pending: WriteCounters::default(),
                last_frame: WriteCounters::default(),
                last_eye: None,
                all_translations_dirty: true,
                zoom_hint: None,
                curve_tolerance_px: crate::render::RenderSettings::DEFAULT.curve_tolerance_px,
                extracted: crate::extract::Applied::default(),
                cull_stats: CullStats::default(),
            })
        })
    }

    /// The device of the scene.
    #[must_use]
    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// The limits of the scene.
    #[must_use]
    pub fn limits(&self) -> &SceneLimits {
        &self.limits
    }

    /// Uploads a mesh with its positions relative to `origin` (see
    /// [`TriMesh::gpu_positions`]); items place the point `origin` at their own origin. The
    /// mesh normals are used when present; otherwise shading uses the face normal.
    ///
    /// # Errors
    ///
    /// `gpu.geometry.invalid` for a mesh without triangles; `gpu.limit.exceeded` beyond the
    /// limits; `gpu.value.not_finite` and `gpu.precision.insufficient` for an origin that is
    /// not finite or too far from the positions; device errors.
    #[allow(clippy::too_many_lines)] // One upload: validate, build the bytes, allocate the ranges.
    pub fn add_geometry(&mut self, mesh: &TriMesh, origin: &Point3) -> Result<GeometryId> {
        finite_point(origin)?;
        if mesh.triangle_count() == 0 {
            return Err(Error::new(codes::GEOMETRY_INVALID));
        }
        let vertex_count = u32::try_from(mesh.vertex_count())
            .ok()
            .filter(|n| *n <= self.limits.max_geometry_vertices)
            .ok_or_else(|| limit("max_geometry_vertices"))?;
        let triangle_count = u32::try_from(mesh.triangle_count())
            .ok()
            .filter(|n| *n <= self.limits.max_geometry_triangles)
            .ok_or_else(|| limit("max_geometry_triangles"))?;
        let positions = mesh
            .gpu_positions(origin)
            .map_err(|_| Error::new(codes::PRECISION_INSUFFICIENT))?;
        let normals = mesh.normals();
        let mut vertex_bytes = Vec::with_capacity(positions.len() * 24);
        let mut radius: f64 = 0.0;
        let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for (i, p) in positions.iter().enumerate() {
            put_f32(&mut vertex_bytes, &p.to_array());
            let n = normals.and_then(|n| n.get(i)).copied().unwrap_or([0.0; 3]);
            put_f32(&mut vertex_bytes, &n);
            radius = radius.max(length3(p.to_array()));
            for (k, v) in p.to_array().into_iter().enumerate() {
                bounds[0][k] = bounds[0][k].min(f64::from(v));
                bounds[1][k] = bounds[1][k].max(f64::from(v));
            }
        }
        let mut index_bytes = Vec::with_capacity(mesh.triangle_count() * 12);
        for t in mesh.triangles() {
            put_u32(&mut index_bytes, t);
        }
        let index_count = triangle_count * 3;
        let mut edge_bytes = Vec::new();
        for pair in feature_edge_pairs(mesh)? {
            put_u32(&mut edge_bytes, &pair);
        }
        let (silhouette_bytes, silhouette_count) = if self.limits.silhouette_edges {
            let uploaded: Vec<[f32; 3]> = positions.iter().map(|p| p.to_array()).collect();
            silhouette_records(mesh, &uploaded)
        } else {
            (Vec::new(), 0)
        };
        // At most three edges per triangle, so this fits in u32.
        let edge_index_count =
            u32::try_from(edge_bytes.len() / 4).map_err(|_| limit("max_geometry_triangles"))?;
        let (device, queue) = (self.gpu.device().clone(), self.gpu.queue().clone());
        let gpu = self.gpu.clone();
        let (vertices, indices, edges, silhouettes) = gpu.scoped(|| {
            let (vertices, _) =
                self.vertices
                    .insert(&device, &queue, u64::from(vertex_count), &vertex_bytes)?;
            let indices =
                match self
                    .indices
                    .insert(&device, &queue, u64::from(index_count), &index_bytes)
                {
                    Ok((indices, _)) => indices,
                    Err(e) => {
                        self.vertices.free(vertices);
                        return Err(e);
                    }
                };
            let mut edges = None;
            if edge_index_count > 0 {
                match self
                    .indices
                    .insert(&device, &queue, u64::from(edge_index_count), &edge_bytes)
                {
                    Ok((id, _)) => edges = Some((id, edge_index_count)),
                    Err(e) => {
                        self.vertices.free(vertices);
                        self.indices.free(indices);
                        return Err(e);
                    }
                }
            }
            let mut silhouettes = None;
            if silhouette_count > 0 {
                // Three 24-byte vertex units hold the two records of an edge.
                match self.vertices.insert(
                    &device,
                    &queue,
                    u64::from(silhouette_count) * 3,
                    &silhouette_bytes,
                ) {
                    Ok((id, _)) => silhouettes = Some((id, silhouette_count)),
                    Err(e) => {
                        self.vertices.free(vertices);
                        self.indices.free(indices);
                        if let Some((id, _)) = edges {
                            self.indices.free(id);
                        }
                        return Err(e);
                    }
                }
            }
            Ok((vertices, indices, edges, silhouettes))
        })?;
        self.pending.vertex += (vertex_bytes.len() + silhouette_bytes.len()) as u64;
        self.pending.index += (index_bytes.len() + edge_bytes.len()) as u64;
        // Cannot reach `GeometryId::LOCAL`: one geometry per call, 2^63 calls.
        let id = GeometryId(self.next_geometry);
        self.next_geometry += 1;
        self.geometries.insert(
            id,
            GeometryEntry {
                vertices,
                indices,
                index_count,
                edges,
                silhouettes,
                radius,
                bounds,
                users: 0,
            },
        );
        Ok(id)
    }

    /// Removes a geometry no item uses.
    ///
    /// # Errors
    ///
    /// `gpu.geometry.not_found` for an unknown handle; `gpu.geometry.in_use` while items use it.
    pub fn remove_geometry(&mut self, id: GeometryId) -> Result<()> {
        match self.geometries.get(&id) {
            None => Err(Error::new(codes::GEOMETRY_NOT_FOUND)),
            Some(g) if g.users > 0 => Err(Error::new(codes::GEOMETRY_IN_USE)),
            // A level of detail goes with the geometry that owns it.
            Some(_)
                if self
                    .lods
                    .values()
                    .any(|l| l.levels.iter().skip(1).any(|v| *v == id)) =>
            {
                Err(Error::new(codes::GEOMETRY_IN_USE))
            }
            Some(_) => {
                let group = self.lods.remove(&id);
                for level in group.iter().flat_map(|g| g.levels.iter().skip(1)) {
                    self.free_geometry(*level);
                }
                self.free_geometry(id);
                Ok(())
            }
        }
    }

    fn free_geometry(&mut self, id: GeometryId) {
        if let Some(g) = self.geometries.remove(&id) {
            self.vertices.free(g.vertices);
            self.indices.free(g.indices);
            if let Some((edges, _)) = g.edges {
                self.indices.free(edges);
            }
            if let Some((silhouettes, _)) = g.silhouettes {
                self.vertices.free(silhouettes);
            }
        }
    }

    /// Uploads `mesh` as tiles of at most [`crate::tile::TILE_SIZE`] on every axis, each with
    /// its own origin (CORE-GPU-005; see [`crate::tile`]). A mesh within the bound is one tile.
    /// The tiles are in model coordinates: an item per tile with the tile's origin and identity
    /// instance transforms draws the mesh where it was authored.
    ///
    /// # Errors
    ///
    /// As [`Scene::add_geometry`] and [`crate::tile::split_mesh`]; tiles uploaded before a
    /// failure are removed.
    pub fn add_tiled_geometry(&mut self, mesh: &TriMesh) -> Result<Vec<TiledGeometry>> {
        let mut added: Vec<TiledGeometry> = Vec::new();
        for tile in split_mesh(mesh, TILE_SIZE)? {
            match self.add_geometry(&tile.mesh, &tile.origin) {
                Ok(geometry) => added.push(TiledGeometry {
                    geometry,
                    origin: tile.origin,
                }),
                Err(error) => {
                    for t in added {
                        self.free_geometry(t.geometry);
                    }
                    return Err(error);
                }
            }
        }
        Ok(added)
    }

    /// Uploads every level of `chain` relative to `origin` and returns the handle of level 0,
    /// which items use. Every frame draws, for each item of the handle, the level
    /// [`LodChain::select`] chooses from the item's projected screen error with
    /// [`crate::RenderSettings::lod_budget_px`] (CORE-GPU-019). The chain is kept by the scene.
    /// Removing the handle removes all levels.
    ///
    /// # Errors
    ///
    /// As [`Scene::add_geometry`]; levels uploaded before a failure are removed.
    pub fn add_lod_geometry(&mut self, chain: &LodChain, origin: &Point3) -> Result<GeometryId> {
        let mut levels: Vec<GeometryId> = Vec::new();
        for level in chain.levels() {
            match self.add_geometry(&level.mesh, origin) {
                Ok(id) => levels.push(id),
                Err(error) => {
                    for id in levels {
                        self.free_geometry(id);
                    }
                    return Err(error);
                }
            }
        }
        self.register_lod(levels, chain)
    }

    /// Records uploaded geometries as the levels of one geometry, level 0 first.
    pub(crate) fn register_lod(
        &mut self,
        levels: Vec<GeometryId>,
        chain: &LodChain,
    ) -> Result<GeometryId> {
        let base = levels
            .first()
            .copied()
            .ok_or_else(|| Error::new(codes::GEOMETRY_INVALID))?;
        self.lods.insert(
            base,
            LodGroup {
                chain: chain.clone(),
                levels,
            },
        );
        Ok(base)
    }

    /// Chooses the level of detail every item with levels draws: the coarsest level whose
    /// projected error is at most `budget_px`. `distance` gives the viewing distance of an item
    /// from its origin and radius; `pixels_per_unit` is the size in pixels of one unit at unit
    /// distance. A budget of 0 draws level 0.
    pub(crate) fn select_levels(
        &mut self,
        distance: impl Fn(&Point3, f64) -> f64,
        pixels_per_unit: f64,
        budget_px: f64,
    ) {
        for entry in self.objects.values_mut() {
            for item in &mut entry.items {
                let ItemGpu::Mesh { geometry, .. } = item.gpu else {
                    continue;
                };
                item.shown = self.lods.get(&geometry).and_then(|group| {
                    let view = ViewParams {
                        distance: distance(&item.origin, item.radius),
                        pixels_per_unit: pixels_per_unit * item.scale,
                    };
                    let level = group.chain.select(budget_px, &view).unwrap_or(0);
                    group.levels.get(level).copied()
                });
            }
        }
    }

    /// Replaces the items of each named object with the given items; an empty list removes
    /// the object. Items of objects that are not named are not touched. Each item's index in
    /// its list is its [`ItemKey::sub`].
    ///
    /// Every object's items are validated before any change is applied. A device error while
    /// writing leaves the objects processed before it updated and the failing object without
    /// items; the scene is derived data and the caller re-extracts it.
    ///
    /// # Errors
    ///
    /// `gpu.value.not_finite`, `gpu.item.invalid`, `gpu.geometry.not_found` and
    /// `gpu.limit.exceeded` for invalid items; device errors.
    pub fn update(&mut self, changes: Vec<(ObjectId, Vec<RenderItem>)>) -> Result<()> {
        self.validate(&changes)?;
        let gpu = self.gpu.clone();
        gpu.scoped(|| {
            for (object, items) in changes {
                self.remove_object(object);
                if !items.is_empty()
                    && let Err(error) = self.insert_object(object, &items)
                {
                    self.remove_object(object);
                    return Err(error);
                }
            }
            if self.mesh_instances.wants_compaction() {
                self.mesh_instances.compact(gpu.device(), gpu.queue());
                self.all_translations_dirty = true;
            }
            if self.segments.wants_compaction() {
                self.segments.compact(gpu.device(), gpu.queue());
                self.all_translations_dirty = true;
            }
            for pool in [&mut self.vertices, &mut self.indices] {
                if pool.wants_compaction() {
                    pool.compact(gpu.device(), gpu.queue());
                }
            }
            Ok(())
        })
    }

    fn validate(&self, changes: &[(ObjectId, Vec<RenderItem>)]) -> Result<()> {
        let added = changes
            .iter()
            .filter(|(o, i)| !i.is_empty() && !self.objects.contains_key(o));
        if self.objects.len().saturating_add(added.count())
            > usize::try_from(self.limits.max_objects).unwrap_or(usize::MAX)
        {
            return Err(limit("max_objects"));
        }
        for (_, items) in changes {
            if items.len() > usize::try_from(self.limits.max_items_per_object).unwrap_or(usize::MAX)
            {
                return Err(limit("max_items_per_object"));
            }
            for item in items {
                match item {
                    RenderItem::Mesh(mesh) => {
                        finite_point(&mesh.origin)?;
                        if !self.geometries.contains_key(&mesh.geometry) {
                            return Err(Error::new(codes::GEOMETRY_NOT_FOUND));
                        }
                        if mesh.instances.is_empty() {
                            return Err(Error::new(codes::ITEM_INVALID));
                        }
                        if mesh.instances.len()
                            > usize::try_from(self.limits.max_instances_per_item)
                                .unwrap_or(usize::MAX)
                        {
                            return Err(limit("max_instances_per_item"));
                        }
                        for i in &mesh.instances {
                            finite(i.transform.as_flattened())?;
                            finite(&color_array(i.color))?;
                        }
                    }
                    RenderItem::Lines(lines) => {
                        finite_point(&lines.origin)?;
                        encode_lines(lines, &self.limits, 0)?;
                    }
                    RenderItem::Curves(curves) => crate::curve::validate(curves, &self.limits)?,
                }
            }
        }
        Ok(())
    }

    fn remove_object(&mut self, object: ObjectId) {
        let Some(entry) = self.objects.remove(&object) else {
            return;
        };
        for item in entry.items {
            match item.gpu {
                ItemGpu::Mesh {
                    geometry,
                    instances,
                    ..
                } => {
                    self.mesh_instances.free(instances);
                    if let Some(g) = self.geometries.get_mut(&geometry) {
                        g.users = g.users.saturating_sub(1);
                    }
                }
                ItemGpu::Lines { segments, .. } => self.segments.free(segments),
            }
        }
        self.free_slots.push(entry.slot);
    }

    #[allow(clippy::too_many_lines)] // One arm per item kind, each uploading its own data.
    fn insert_object(&mut self, object: ObjectId, items: &[RenderItem]) -> Result<()> {
        let slot = self.free_slots.pop().unwrap_or_else(|| {
            self.next_slot += 1;
            self.next_slot - 1
        });
        // Registered first so a failure below still releases what was allocated.
        self.objects.insert(
            object,
            ObjectEntry {
                slot,
                items: Vec::new(),
            },
        );
        let (device, queue) = (self.gpu.device().clone(), self.gpu.queue().clone());
        for item in items {
            let entry = match item {
                RenderItem::Mesh(mesh) => {
                    let geometry_radius = self
                        .geometries
                        .get(&mesh.geometry)
                        .map_or(0.0, |g| g.radius);
                    // The boxes of every level, so the bounds hold whichever level is drawn.
                    let boxes: Vec<[[f64; 3]; 2]> = match self.lods.get(&mesh.geometry) {
                        Some(group) => group
                            .levels
                            .iter()
                            .filter_map(|id| self.geometries.get(id))
                            .map(|g| g.bounds)
                            .collect(),
                        None => self
                            .geometries
                            .get(&mesh.geometry)
                            .map(|g| g.bounds)
                            .into_iter()
                            .collect(),
                    };
                    let mut bounds = None;
                    let mut bytes = Vec::with_capacity(mesh.instances.len() * 80);
                    let mut radius: f64 = 0.0;
                    let mut largest_scale: f64 = 0.0;
                    for i in &mesh.instances {
                        put_f32(&mut bytes, i.transform.as_flattened());
                        put_f32(&mut bytes, &color_array(i.color));
                        put_u32(&mut bytes, &[i.material, i.flags, slot, 0]);
                        let [r0, r1, r2] = i.transform;
                        let column = |c: usize| {
                            length3(
                                [r0.get(c), r1.get(c), r2.get(c)]
                                    .map(|v| v.copied().unwrap_or(0.0)),
                            )
                        };
                        let scale = column(0).max(column(1)).max(column(2));
                        largest_scale = largest_scale.max(scale);
                        radius = radius.max(column(3) + scale * geometry_radius);
                        for b in &boxes {
                            include_box(&mut bounds, transformed_box(&i.transform, *b));
                        }
                    }
                    let count = u32::try_from(mesh.instances.len())
                        .map_err(|_| limit("max_instances_per_item"))?;
                    let (instances, moved) =
                        self.mesh_instances
                            .insert(&device, &queue, u64::from(count), &bytes)?;
                    self.all_translations_dirty |= moved;
                    self.pending.mesh_instance += bytes.len() as u64;
                    if let Some(g) = self.geometries.get_mut(&mesh.geometry) {
                        g.users += 1;
                    }
                    ItemEntry {
                        gpu: ItemGpu::Mesh {
                            geometry: mesh.geometry,
                            instances,
                            count,
                        },
                        origin: mesh.origin,
                        mirrored: mesh
                            .instances
                            .iter()
                            .any(|i| linear_determinant(&i.transform) < 0.0),
                        radius,
                        bounds: bounds.unwrap_or([[0.0; 3]; 2]),
                        // Edges are one-pixel lines on the surface; two pixels cover them.
                        margin_px: Some(2.0),
                        visible: true,
                        scale: largest_scale,
                        shown: None,
                        translation_dirty: true,
                        path_ends: Vec::new(),
                        curve: None,
                    }
                }
                RenderItem::Lines(lines) => {
                    let encoded = encode_lines(lines, &self.limits, slot)?;
                    self.line_entry(lines.origin, encoded, None)?
                }
                RenderItem::Curves(curves) => {
                    let scale = self
                        .zoom_hint
                        .unwrap_or_else(|| crate::curve::initial_scale(curves));
                    let flattened = Flattened {
                        scale,
                        tolerance_px: self.curve_tolerance_px,
                    };
                    let encoded = crate::curve::encode(curves, flattened, &self.limits, slot)?;
                    let state = CurveState::new(curves.clone(), flattened);
                    self.line_entry(curves.origin, encoded, Some(Box::new(state)))?
                }
            };
            if let Some(o) = self.objects.get_mut(&object) {
                o.items.push(entry);
            }
        }
        Ok(())
    }

    /// Uploads the segments of a line or curve item.
    fn line_entry(
        &mut self,
        origin: Point3,
        encoded: Encoded,
        curve: Option<Box<CurveState>>,
    ) -> Result<ItemEntry> {
        let (device, queue) = (self.gpu.device().clone(), self.gpu.queue().clone());
        let (segments, moved) =
            self.segments
                .insert(&device, &queue, u64::from(encoded.count), &encoded.bytes)?;
        self.all_translations_dirty |= moved;
        self.pending.line_segment += encoded.bytes.len() as u64;
        Ok(ItemEntry {
            mirrored: false,
            gpu: ItemGpu::Lines {
                segments,
                count: encoded.count,
            },
            origin,
            radius: encoded.radius,
            bounds: encoded.bounds.unwrap_or([[0.0; 3]; 2]),
            margin_px: encoded.cull_margin(),
            visible: true,
            scale: 1.0,
            shown: None,
            translation_dirty: true,
            path_ends: encoded.path_ends,
            curve,
        })
    }

    /// Flattens again the curve items whose zoom changed by more than a factor of two since
    /// they were flattened, or whose chord tolerance changed (CORE-GPU-010). `scale` gives the
    /// pixels per item unit for an item's origin and radius; `hint` is the zoom new items are
    /// flattened for. Only the re-flattened items' segments are written.
    pub(crate) fn refresh_curves(
        &mut self,
        scale: impl Fn(&Point3, f64) -> f64,
        tolerance_px: f32,
        hint: Option<f64>,
    ) -> Result<()> {
        self.zoom_hint = hint.filter(|h| h.is_finite() && *h > 0.0);
        self.curve_tolerance_px = tolerance_px;
        let (device, queue) = (self.gpu.device().clone(), self.gpu.queue().clone());
        let limits = self.limits;
        for entry in self.objects.values_mut() {
            for item in &mut entry.items {
                let Some(state) = item.curve.as_mut() else {
                    continue;
                };
                let now = scale(&item.origin, item.radius);
                let Some(flattened) = state.refresh(now, tolerance_px) else {
                    continue;
                };
                // A flattening that fails keeps the previous segments; it is not retried until
                // the zoom leaves the band again.
                let encoded = crate::curve::encode(&state.item, flattened, &limits, entry.slot);
                state.flattened = flattened;
                let Ok(encoded) = encoded else {
                    continue;
                };
                if let ItemGpu::Lines { segments, .. } = item.gpu {
                    self.segments.free(segments);
                }
                let (segments, moved) = self.segments.insert(
                    &device,
                    &queue,
                    u64::from(encoded.count),
                    &encoded.bytes,
                )?;
                self.all_translations_dirty |= moved;
                self.pending.line_segment += encoded.bytes.len() as u64;
                item.gpu = ItemGpu::Lines {
                    segments,
                    count: encoded.count,
                };
                item.radius = item.radius.max(encoded.radius);
                item.bounds = encoded.bounds.unwrap_or([[0.0; 3]; 2]);
                item.margin_px = encoded.cull_margin();
                item.path_ends = encoded.path_ends;
                item.translation_dirty = true;
            }
        }
        Ok(())
    }

    /// The path of a picked element: for a line or curve item the index of the path (curve)
    /// holding segment `hit.element`, for a mesh item the instance index. `None` for an
    /// unknown item or element.
    #[must_use]
    pub fn path_of(&self, hit: &crate::PickHit) -> Option<u32> {
        let entry = self
            .object_items(&hit.key.object)
            .get(hit.key.sub as usize)?;
        match entry.gpu {
            ItemGpu::Mesh { count, .. } => (hit.element < count).then_some(hit.element),
            ItemGpu::Lines { .. } => {
                let at = entry.path_ends.partition_point(|end| *end <= hit.element);
                (at < entry.path_ends.len()).then(|| u32::try_from(at).unwrap_or(u32::MAX))
            }
        }
    }

    /// The level of detail the last frame drew for an item whose geometry has levels (0 is
    /// the finest), or `None` for an unknown key or an item without levels.
    #[must_use]
    pub fn lod_level(&self, key: ItemKey) -> Option<usize> {
        let entry = self.object_items(&key.object).get(key.sub as usize)?;
        let ItemGpu::Mesh { geometry, .. } = entry.gpu else {
            return None;
        };
        let group = self.lods.get(&geometry)?;
        let shown = entry.shown.unwrap_or(geometry);
        group.levels.iter().position(|l| *l == shown)
    }

    /// The pixels per unit and chord tolerance a curve item is currently flattened for, or
    /// `None` for an unknown key or another kind of item.
    #[must_use]
    pub fn flattening(&self, key: ItemKey) -> Option<(f64, f32)> {
        let entry = self.object_items(&key.object).get(key.sub as usize)?;
        let f = entry.curve.as_ref()?.flattened;
        Some((f.scale, f.tolerance_px))
    }

    /// The clip rectangle of an item (curve items only), in item coordinates.
    pub(crate) fn clip_of(item: &ItemEntry) -> Option<[arc_base::math::Point2; 2]> {
        item.curve.as_ref().and_then(|c| c.item.clip)
    }

    /// Compacts every pool now, regardless of the compaction rule.
    ///
    /// # Errors
    ///
    /// Device errors.
    pub fn compact(&mut self) -> Result<()> {
        let gpu = self.gpu.clone();
        gpu.scoped(|| {
            for pool in [
                &mut self.vertices,
                &mut self.indices,
                &mut self.mesh_instances,
                &mut self.segments,
            ] {
                pool.compact(gpu.device(), gpu.queue());
            }
            self.all_translations_dirty = true;
            Ok(())
        })
    }

    /// Marks the items wholly outside `view` as not visible (CORE-GPU-018, CPU path); with
    /// `None` every item is visible. Items partly inside are always visible.
    pub(crate) fn cull(&mut self, view: Option<&CullView>) {
        let mut stats = CullStats::default();
        for entry in self.objects.values_mut() {
            for item in &mut entry.items {
                item.visible = view.is_none_or(|v| !v.excludes(item));
                stats.items += 1;
                stats.visible += usize::from(item.visible);
            }
        }
        self.cull_stats = stats;
    }

    /// The item counts of the last frame or pick: how many items the view kept (see
    /// [`CullStats`]).
    #[must_use]
    pub fn cull_stats(&self) -> CullStats {
        self.cull_stats
    }

    /// Whether an item is inside the view of the last frame or pick. `None` for an unknown key.
    /// Without culling every item is visible.
    #[must_use]
    pub fn item_visible(&self, key: ItemKey) -> Option<bool> {
        self.object_items(&key.object)
            .get(key.sub as usize)
            .map(|item| item.visible)
    }

    /// The keys of all items, sorted.
    #[must_use]
    pub fn keys(&self) -> Vec<ItemKey> {
        let mut keys: Vec<ItemKey> = self
            .objects
            .iter()
            .flat_map(|(object, entry)| {
                (0..entry.items.len()).map(move |i| ItemKey {
                    object: *object,
                    sub: u32::try_from(i).unwrap_or(u32::MAX),
                })
            })
            .collect();
        keys.sort();
        keys
    }

    /// The bytes written since the last frame, per category.
    #[must_use]
    pub fn pending_writes(&self) -> WriteCounters {
        self.pending
    }

    /// The bytes written for the last rendered frame: the writes pending when it started plus
    /// its translations.
    #[must_use]
    pub fn last_frame_writes(&self) -> WriteCounters {
        self.last_frame
    }

    /// GPU memory per category.
    #[must_use]
    pub fn memory(&self) -> MemoryReport {
        let a = self.mesh_instances.companion_usage();
        let b = self.segments.companion_usage();
        MemoryReport {
            vertex: self.vertices.usage(),
            index: self.indices.usage(),
            mesh_instance: self.mesh_instances.usage(),
            line_segment: self.segments.usage(),
            translation: PoolUsage {
                capacity_bytes: a.capacity_bytes + b.capacity_bytes,
                used_bytes: a.used_bytes + b.used_bytes,
            },
        }
    }

    /// Writes the camera-relative translations for `eye`: all of them when the eye moved or
    /// buffers were relocated, otherwise only those of changed items. Ends the frame's write
    /// accounting.
    pub(crate) fn prepare_frame(&mut self, eye: &Point3) -> Result<()> {
        let all = self.all_translations_dirty || self.last_eye != Some(*eye);
        let queue = self.gpu.queue().clone();
        let mut written = 0;
        for entry in self.objects.values_mut() {
            for item in &mut entry.items {
                if !(all || item.translation_dirty) {
                    continue;
                }
                let t = relative_translation(&item.origin, eye)?;
                let (pool, id, count) = match item.gpu {
                    ItemGpu::Mesh {
                        instances, count, ..
                    } => (&self.mesh_instances, instances, count),
                    ItemGpu::Lines { segments, count } => (&self.segments, segments, count),
                };
                let mut bytes = Vec::with_capacity(usize::try_from(count).unwrap_or(0) * 12);
                for _ in 0..count {
                    put_f32(&mut bytes, &t);
                }
                pool.write_companion(&queue, id, &bytes);
                written += bytes.len() as u64;
                item.translation_dirty = false;
            }
        }
        self.all_translations_dirty = false;
        self.last_eye = Some(*eye);
        self.pending.translation += written;
        self.last_frame = std::mem::take(&mut self.pending);
        Ok(())
    }

    /// The items, for drawing and bounds.
    pub(crate) fn items(&self) -> impl Iterator<Item = &ItemEntry> {
        self.objects.values().flat_map(|o| o.items.iter())
    }

    /// The vertex, index and index-count data of a geometry: `(first vertex, first index,
    /// index count)`.
    pub(crate) fn geometry_ranges(&self, id: GeometryId) -> Option<(u64, u64, u32)> {
        let g = self.geometries.get(&id)?;
        Some((
            self.vertices.range(g.vertices)?.start,
            self.indices.range(g.indices)?.start,
            g.index_count,
        ))
    }

    /// The feature edges of a geometry: `(first index, index count)`.
    pub(crate) fn edge_range(&self, id: GeometryId) -> Option<(u64, u32)> {
        let (edges, count) = self.geometries.get(&id)?.edges?;
        Some((self.indices.range(edges)?.start, count))
    }

    /// The silhouette records of a geometry: `(first vertex unit, edge count)`.
    pub(crate) fn silhouette_range(&self, id: GeometryId) -> Option<(u64, u32)> {
        let (records, count) = self.geometries.get(&id)?.silhouettes?;
        Some((self.vertices.range(records)?.start, count))
    }

    /// The items with the object they belong to.
    pub(crate) fn items_with_object(&self) -> impl Iterator<Item = (&ObjectId, &ItemEntry)> {
        self.objects
            .iter()
            .flat_map(|(id, o)| o.items.iter().map(move |i| (id, i)))
    }

    /// The items of one object, if it has any.
    pub(crate) fn object_items(&self, object: &ObjectId) -> &[ItemEntry] {
        self.objects.get(object).map_or(&[], |o| o.items.as_slice())
    }

    /// Every item's instance range with its key and whether it is a line item, sorted by
    /// kind and range start, for mapping identifier-pass values back to items.
    pub(crate) fn instance_table(&self) -> Vec<(bool, std::ops::Range<u64>, ItemKey)> {
        let mut table: Vec<_> = self
            .objects
            .iter()
            .flat_map(|(object, entry)| {
                entry.items.iter().zip(0_u32..).filter_map(|(item, sub)| {
                    let line = matches!(item.gpu, ItemGpu::Lines { .. });
                    let range = self.instance_range(item.gpu)?;
                    Some((
                        line,
                        range,
                        ItemKey {
                            object: *object,
                            sub,
                        },
                    ))
                })
            })
            .collect();
        table.sort_unstable_by_key(|(line, range, _)| (*line, range.start));
        table
    }

    /// A sphere `(center, radius)` enclosing the items of `objects`, or of every item for
    /// `None`; `None` when there is no such item. The sphere is centered on the box of the
    /// items' bounds, and its radius reaches the box corners (so a drawing that is wide and flat
    /// is framed about its middle). An item without finite bounds counts as a sphere around its
    /// origin.
    #[must_use]
    pub fn bounding_sphere(&self, objects: Option<&[ObjectId]>) -> Option<(Point3, f64)> {
        let boxes: Vec<(Point3, Point3)> = match objects {
            None => self.items().map(item_box).collect(),
            Some(ids) => ids
                .iter()
                .flat_map(|id| self.object_items(id))
                .map(item_box)
                .collect(),
        };
        let (first, rest) = boxes.split_first()?;
        let (mut low, mut high) = *first;
        for (l, h) in rest {
            low = low.inf(l);
            high = high.sup(h);
        }
        // Halving first keeps the midpoint finite for any finite corners.
        let center = Point3::from(low.coords * 0.5 + high.coords * 0.5);
        let radius = length(&(high - center));
        Some((center, radius))
    }

    pub(crate) fn instance_range(&self, gpu: ItemGpu) -> Option<std::ops::Range<u64>> {
        match gpu {
            ItemGpu::Mesh { instances, .. } => self.mesh_instances.range(instances),
            ItemGpu::Lines { segments, .. } => self.segments.range(segments),
        }
    }

    pub(crate) fn buffers(&self) -> SceneBuffers<'_> {
        SceneBuffers {
            vertices: self.vertices.buffer(),
            indices: self.indices.buffer(),
            mesh_instances: self.mesh_instances.buffer(),
            mesh_translations: self.mesh_instances.companion(),
            segments: self.segments.buffer(),
            segment_translations: self.segments.companion(),
        }
    }
}

pub(crate) struct SceneBuffers<'a> {
    pub(crate) vertices: &'a wgpu::Buffer,
    pub(crate) indices: &'a wgpu::Buffer,
    pub(crate) mesh_instances: &'a wgpu::Buffer,
    pub(crate) mesh_translations: Option<&'a wgpu::Buffer>,
    pub(crate) segments: &'a wgpu::Buffer,
    pub(crate) segment_translations: Option<&'a wgpu::Buffer>,
}

/// The determinant of the linear part of an instance transform, given as the rows of a 3×4
/// matrix. A negative value marks a mirrored instance.
fn linear_determinant(t: &[[f32; 4]; 3]) -> f64 {
    let [[a0, a1, a2, _], [b0, b1, b2, _], [c0, c1, c2, _]] = *t;
    let (a, b, c) = (
        [f64::from(a0), f64::from(a1), f64::from(a2)],
        [f64::from(b0), f64::from(b1), f64::from(b2)],
        [f64::from(c0), f64::from(c1), f64::from(c2)],
    );
    a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature_edges(mesh: &TriMesh) -> Vec<[u32; 2]> {
        feature_edge_pairs(mesh).unwrap()
    }

    fn item(paths: Vec<LinePath>) -> LineItem {
        LineItem {
            origin: Point3::origin(),
            paths,
            style: LineStyle::round(LineWidth::Screen(2.0)),
            color: Color::WHITE,
        }
    }

    #[test]
    fn polylines_split_into_segments_with_neighbors() {
        let open = LinePath {
            points: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            closed: false,
        };
        let e = encode_lines(&item(vec![open]), &SceneLimits::DEFAULT, 7).unwrap();
        let (bytes, count) = (e.bytes, e.count);
        assert_eq!(count, 2, "the repeated point is merged");
        assert_eq!(bytes.len(), 256);
        let bits = |segment: usize| {
            let at = segment * 128 + 72;
            u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
        };
        assert_eq!(bits(0) & 0b11000, 0b10000, "first: next only");
        assert_eq!(bits(1) & 0b11000, 0b01000, "last: previous only");
        let closed = LinePath {
            points: vec![[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0; 3]],
            closed: true,
        };
        let count = encode_lines(&item(vec![closed]), &SceneLimits::DEFAULT, 0)
            .unwrap()
            .count;
        assert_eq!(count, 3, "a repeated closing point is dropped");
        let dot = LinePath {
            points: vec![[2.0; 3]],
            closed: false,
        };
        let e = encode_lines(&item(vec![dot]), &SceneLimits::DEFAULT, 0).unwrap();
        assert_eq!((e.count, e.path_ends), (1, vec![1]));
        assert!((e.radius - 12_f64.sqrt()).abs() < 1e-9);
    }

    fn mesh(positions: &[[f64; 3]], triangles: Vec<[u32; 3]>) -> TriMesh {
        let positions = positions.iter().map(|p| Point3::from(*p)).collect();
        TriMesh::new(positions, triangles, &arc_base::Limits::DEFAULT).unwrap()
    }

    /// Two triangles sharing the edge from (0,0,0) to (0,1,0), the second folded up by
    /// `angle` radians.
    fn folded(angle: f64) -> TriMesh {
        let (s, c) = angle.sin_cos();
        mesh(
            &[[0.0; 3], [0.0, 1.0, 0.0], [-1.0, 0.5, 0.0], [c, 0.5, s]],
            vec![[0, 1, 2], [1, 0, 3]],
        )
    }

    #[test]
    fn feature_edges_are_boundaries_creases_and_non_manifold_edges() {
        let corners: Vec<[f64; 3]> = (0..8)
            .map(|i| [f64::from(i & 1), f64::from((i >> 1) & 1), f64::from(i >> 2)])
            .collect();
        let faces: [[u32; 4]; 6] = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        let quads = |q: [u32; 4]| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]];
        let welded: Vec<[u32; 3]> = faces.iter().flat_map(|f| quads(*f)).collect();
        assert_eq!(feature_edges(&mesh(&corners, welded)).len(), 12, "a cube");
        // The same cube with every face on its own vertices, as with split normals.
        let split_positions: Vec<[f64; 3]> = faces
            .iter()
            .flat_map(|f| f.map(|i| corners[i as usize]))
            .collect();
        let split: Vec<[u32; 3]> = (0..6_u32)
            .flat_map(|f| quads([0, 1, 2, 3].map(|k| 4 * f + k)))
            .collect();
        assert_eq!(feature_edges(&mesh(&split_positions, split)).len(), 12);
        // A flat fold keeps its shared edge smooth; a steep one makes it a crease.
        assert_eq!(feature_edges(&folded(0.0)).len(), 4);
        assert_eq!(feature_edges(&folded(FEATURE_EDGE_ANGLE * 0.9)).len(), 4);
        assert_eq!(feature_edges(&folded(FEATURE_EDGE_ANGLE * 1.1)).len(), 5);
        // A third face on the shared edge makes it non-manifold; a degenerate face is skipped.
        let fan = mesh(
            &[
                [0.0; 3],
                [0.0, 1.0, 0.0],
                [-1.0, 0.5, 0.0],
                [1.0, 0.5, 0.0],
                [0.0, 0.5, 1.0],
                [0.0, 2.0, 0.0],
            ],
            vec![[0, 1, 2], [1, 0, 3], [0, 1, 4], [0, 1, 5]],
        );
        assert_eq!(feature_edges(&fan).len(), 7);
    }

    #[test]
    fn invalid_lines_are_rejected() {
        let path = || LinePath {
            points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
            closed: false,
        };
        let code = |i: LineItem| {
            encode_lines(&i, &SceneLimits::DEFAULT, 0)
                .unwrap_err()
                .code()
        };
        assert_eq!(code(item(vec![])), "gpu.item.invalid");
        assert_eq!(
            code(item(vec![LinePath {
                points: vec![],
                closed: false
            }])),
            "gpu.item.invalid"
        );
        let mut wide = item(vec![path()]);
        wide.style.width = LineWidth::Screen(1000.0);
        assert_eq!(code(wide), "gpu.limit.exceeded");
        let mut zero = item(vec![path()]);
        zero.style.width = LineWidth::Paper(0.0);
        assert_eq!(code(zero), "gpu.item.invalid");
        let mut nan = item(vec![path()]);
        nan.paths[0].points[1][2] = f32::NAN;
        assert_eq!(code(nan), "gpu.value.not_finite");
        let limits = SceneLimits {
            max_segments_per_item: 1,
            ..SceneLimits::DEFAULT
        };
        let two = item(vec![path(), path()]);
        assert_eq!(
            encode_lines(&two, &limits, 0).unwrap_err().code(),
            "gpu.limit.exceeded"
        );
    }
}
