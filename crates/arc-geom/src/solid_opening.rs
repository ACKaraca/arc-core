//! Subtracting a prism that passes through a solid: the opening in a wall.
//!
//! General booleans are not provided. [`Polyhedron::subtract_prism`] supports the case a building
//! model needs: a prism (a [`PrismTool`]) whose side surface runs between two parallel faces of
//! the solid, so the result is the solid with a through hole or a notch (or, for a full-length
//! opening, two parts).
//!
//! # The tool
//!
//! A [`PrismTool`] is a [`Region`] placed in a [`PlaneFrame`] and extruded along the frame's
//! normal `d` over the offsets `start..end` from the frame's plane. Every part of the region is
//! one opening and must have no holes; arcs are replaced by chords at the tool's `chord` as in
//! [`crate::extrude`], so the opening is the inscribed polygon (a round opening is smaller than
//! the exact circle, never larger); a straight edge of the outline is kept exactly, whatever its
//! length. The region is checked with the exact tests of [`crate::triangulate`] (crossing or
//! overlapping outlines are refused with its codes).
//!
//! # Supported case, checked
//!
//! 1. The line along `d` through a point inside the outline meets the interior of exactly two
//!    faces, `F1` and `F2`, with antiparallel normals (within `tolerance.angular`), and the solid
//!    lies between `start` and `end` there: both hits are more than `tolerance.linear` inside
//!    the tool's offsets, so the prism extends beyond both faces.
//! 2. The outline projected along `d` onto `F1` and `F2` may lie inside the face, touch its
//!    boundary or cross it (a door at floor level, a full-height opening, an opening at the end
//!    of a wall); then the faces parallel to `d` that the prism meets (the floor, the ceiling,
//!    the end of the wall) are cut by the prism's side planes, and the wall of the opening is
//!    trimmed to the solid. The outline may contain a corner of a face (it then extends beyond
//!    the face). An outline with an existing opening of the face inside it is refused.
//!    A vertex of the outline within `tolerance.linear` of the plane of a face parallel to the
//!    axis (floor, ceiling, wall end, the jamb or head of another opening) is on that plane: it
//!    is moved onto it before anything else, so a near miss is the same as a hit. An edge
//!    farther than the tolerance leaves a strip, which is kept (the planar booleans run at a
//!    fraction of the tolerance so that such a strip is not merged away).
//! 3. Every other face, not parallel to `d`, projects along `d` to a region disjoint from the
//!    outline (a conservative test: a face that projects over the outline at a different depth
//!    is refused too).
//!
//! Anything else fails with `geom.solid.opening_unsupported`; the `reason` detail names the
//! first condition that failed. A result with more connected shells than the solid (as many as
//! [`Polyhedron::shell_count`] counts) fails with `geom.solid.opening_splits` unless
//! [`PrismTool::allow_split`] is set: a profile used with an axis that runs along the wall
//! cuts it in two without any other sign, while the full-length opening of a wall that is meant
//! to be cut in two needs the flag.
//!
//! # Method
//!
//! `F1` and `F2` lose the projection of the outline: when the outline is strictly inside the face
//! it becomes a hole loop, otherwise the planar difference of [`crate::boolean`] is used in the
//! face's own coordinates. A face parallel to `d` loses the bands of the outline that cross it.
//! A wall face is made for each outline edge: the quadrilateral between the two faces when the
//! edge is strictly inside both, otherwise the part of the section of the solid by the edge's
//! plane (where material lies on both sides) that is within the edge's band, so the wall stops at
//! the solid's boundary and no wall is made where the edge lies on a face of the solid. Points
//! made by different faces for the same location are one vertex (within `tolerance.linear`), so
//! neighboring faces share vertices exactly. The result is validated by [`Polyhedron::new`].
//!
//! # Tags and cost
//!
//! Faces that lose area keep their tag (a split face yields several faces with the tag); the wall
//! of the outline of part `k` is tagged [`FaceTag::OpeningWall`] with opening number
//! `tool.opening + k` and the `curve` and `facet` of the outline edge, numbered as the tags of
//! [`crate::extrude`] are (by the curves of the profile as given). The work for an opening is linear in the number of face loop
//! entries (the test of every face against the outline is bounded by bounding boxes), plus the
//! validation of the result; the faces with many holes triangulate in `O(n log n)`. Openings are
//! applied one after the other, so `n` openings cost `n` such passes.
//!
//! Not supported: islands inside an opening, tools that end inside the solid (pockets), outlines
//! that meet a face boundary at a single point only, several faces on each side, and any
//! solid-against-solid boolean.

use std::collections::HashMap;

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::boolean::{BooleanOp, boolean};
use crate::curve::Curve2;
use crate::error::codes;
use crate::plane3::{LinePlane, Plane3, PlaneFrame};
use crate::predicates::{Sign, orient2d};
use crate::primitives::Segment2;
use crate::region::{Loop2, Region, RegionPart};
use crate::solid::{FaceTag, PolyFace, Polyhedron, newell, project_point, projection};
use crate::triangulate::{FlatLoop, Merge, flatten_region_edges, triangulate_point_loops};

/// A prism to subtract: a region extruded along the normal of its frame.
#[derive(Debug, Clone, Copy)]
pub struct PrismTool<'a> {
    /// The outline of each opening, in `frame`.
    pub profile: &'a Region,
    /// The frame that places the region; its normal is the axis of the prism.
    pub frame: PlaneFrame,
    /// The offset of the start of the prism from the plane of the frame along its normal.
    pub start: f64,
    /// The offset of the end of the prism; larger than `start`.
    pub end: f64,
    /// The chord tolerance for faceting arcs of the profile.
    pub chord: f64,
    /// The number of the first opening, carried by the walls in [`FaceTag::OpeningWall`]; part
    /// `k` of the profile is opening `opening + k`.
    pub opening: u32,
    /// Whether the opening may leave the solid in more shells than it had, as a full-length
    /// opening that cuts a wall in two does. When false such a result is refused with
    /// `geom.solid.opening_splits`: it usually means the axis does not run through the wall.
    pub allow_split: bool,
}

/// The part of the linear tolerance the planar booleans of this module run with. A planar
/// boolean treats points closer than twice its linear tolerance as one vertex and drops edges
/// shorter than four times it; running it at this fraction keeps a gap of more than the
/// linear tolerance (a strip left beside an opening) a gap, while everything within the linear
/// tolerance has been snapped before.
const BOOLEAN_FRACTION: f64 = 0.125;

fn finer(tolerance: &Tolerance) -> Tolerance {
    Tolerance {
        linear: tolerance.linear * BOOLEAN_FRACTION,
        ..*tolerance
    }
}

fn unsupported(reason: &'static str) -> Error {
    Error::new(codes::SOLID_OPENING_UNSUPPORTED).with_detail("reason", reason)
}

fn overflow() -> Error {
    Error::new(codes::SOLID_LIMIT_EXCEEDED)
}

/// A hash grid of points, to find the vertex at a location.
struct Grid {
    cell: f64,
    cells: HashMap<(i64, i64, i64), Vec<u32>>,
}

impl Grid {
    fn new(cell: f64) -> Self {
        Self {
            cell,
            cells: HashMap::new(),
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "a cell index of a finite coordinate"
    )]
    fn key(&self, p: &Point3) -> (i64, i64, i64) {
        let at = |k: usize| (p.coords.get(k).copied().unwrap_or(0.0) / self.cell).floor() as i64;
        (at(0), at(1), at(2))
    }

    fn insert(&mut self, p: &Point3, id: u32) {
        let key = self.key(p);
        self.cells.entry(key).or_default().push(id);
    }

    /// The id of the first registered point within `radius` of `p`, given the positions.
    fn find(&self, p: &Point3, radius: f64, positions: &[Point3]) -> Option<u32> {
        let (x, y, z) = self.key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let near = self.cells.get(&(x + dx, y + dy, z + dz));
                    for &id in near.into_iter().flatten() {
                        let q = positions.get(usize::try_from(id).ok()?)?;
                        if (q - p).norm() <= radius {
                            return Some(id);
                        }
                    }
                }
            }
        }
        None
    }
}

/// The vertices of the result: those of the solid, then new ones, with points of the same
/// location (within the linear tolerance) merged.
struct Vertices<'a> {
    points: Vec<Point3>,
    grid: Grid,
    tolerance: &'a Tolerance,
}

impl<'a> Vertices<'a> {
    fn new(solid: &Polyhedron, tolerance: &'a Tolerance) -> Result<Self> {
        let mut grid = Grid::new(2.0 * tolerance.linear.max(f64::MIN_POSITIVE));
        for (k, p) in solid.vertices().iter().enumerate() {
            grid.insert(p, u32::try_from(k).map_err(|_| overflow())?);
        }
        Ok(Self {
            points: solid.vertices().to_vec(),
            grid,
            tolerance,
        })
    }

    fn intern(&mut self, p: Point3) -> Result<u32> {
        if let Some(id) = self.grid.find(&p, self.tolerance.linear, &self.points) {
            return Ok(id);
        }
        let id = u32::try_from(self.points.len()).map_err(|_| overflow())?;
        self.grid.insert(&p, id);
        self.points.push(p);
        Ok(id)
    }
}

/// Whether the closed segments `a`–`b` and `c`–`d` share a point.
fn segments_touch(a: &Point2, b: &Point2, c: &Point2, d: &Point2) -> Result<bool> {
    let o1 = orient2d(a, b, c)?;
    let o2 = orient2d(a, b, d)?;
    let o3 = orient2d(c, d, a)?;
    let o4 = orient2d(c, d, b)?;
    let on = |p: &Point2, q: &Point2, r: &Point2| {
        r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    Ok((o1 != o2 && o3 != o4)
        || (o1 == Sign::Zero && on(a, b, c))
        || (o2 == Sign::Zero && on(a, b, d))
        || (o3 == Sign::Zero && on(c, d, a))
        || (o4 == Sign::Zero && on(c, d, b)))
}

/// Whether `point` is inside the polygon `ring` (even-odd rule).
fn inside_polygon(point: &Point2, ring: &[Point2]) -> bool {
    inside_rings(point, std::slice::from_ref(&ring.to_vec()))
}

/// Whether `point` is inside an odd number of the rings (even-odd rule over all loops).
fn inside_rings(point: &Point2, rings: &[Vec<Point2>]) -> bool {
    let mut inside = false;
    for ring in rings {
        for (k, a) in ring.iter().enumerate() {
            let Some(b) = ring.get((k + 1) % ring.len()) else {
                continue;
            };
            if (a.y > point.y) != (b.y > point.y)
                && point.x < a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x)
            {
                inside = !inside;
            }
        }
    }
    inside
}

type Box2 = (Point2, Point2);

fn bounds(points: &[Point2]) -> Option<Box2> {
    let first = points.first()?;
    Some(points.iter().fold((*first, *first), |(lo, hi), p| {
        (
            Point2::new(lo.x.min(p.x), lo.y.min(p.y)),
            Point2::new(hi.x.max(p.x), hi.y.max(p.y)),
        )
    }))
}

fn boxes_meet(a: &Box2, b: &Box2, margin: f64) -> bool {
    a.0.x <= b.1.x + margin
        && b.0.x <= a.1.x + margin
        && a.0.y <= b.1.y + margin
        && b.0.y <= a.1.y + margin
}

/// A loop of 2D points as a closed polygon curve chain.
fn loop_of(points: &[Point2], tolerance: &Tolerance) -> Result<Loop2> {
    let mut curves = Vec::with_capacity(points.len());
    for (k, a) in points.iter().enumerate() {
        let b = points.get((k + 1) % points.len()).copied().unwrap_or(*a);
        curves.push(Curve2::Segment(Segment2::new(*a, b)?));
    }
    Loop2::new(curves, tolerance)
}

/// The region with `rings[0]` as its outer loop and the rest as holes.
fn region_of(rings: &[Vec<Point2>], tolerance: &Tolerance) -> Result<Region> {
    let mut loops = rings.iter().map(|r| loop_of(r, tolerance));
    let outer = loops.next().ok_or_else(|| unsupported("an empty face"))??;
    let holes = loops.collect::<Result<Vec<_>>>()?;
    Region::new(vec![RegionPart::new(outer, holes)])
}

/// The corner points of a loop made of segments.
fn corners(lp: &Loop2) -> Result<Vec<Point2>> {
    lp.curves()
        .iter()
        .map(|c| match c {
            Curve2::Segment(s) => Ok(s.start()),
            _ => Err(unsupported(
                "a boolean result that is not made of line segments",
            )),
        })
        .collect()
}

/// Maps the 2D points of a face's projection back to vertex ids: the face's own vertices by
/// location, others as new points on the plane of the face.
struct FaceMapper {
    frame: (usize, usize, bool),
    normal: Vector3,
    offset: f64,
    originals: Grid,
    positions: Vec<Point3>,
    ids: Vec<u32>,
}

impl FaceMapper {
    fn new(solid: &Polyhedron, index: usize, tolerance: &Tolerance) -> Result<Option<Self>> {
        let (Some(face), Some(normal)) = (solid.faces().get(index), solid.face_normal(index))
        else {
            return Ok(None);
        };
        let frame = projection(&normal);
        let mut originals = Grid::new(2.0 * tolerance.linear.max(f64::MIN_POSITIVE));
        let mut positions = Vec::new();
        let mut ids = Vec::new();
        let mut offset = 0.0;
        for (k, &id) in face.loops().flatten().enumerate() {
            let Some(p) = usize::try_from(id)
                .ok()
                .and_then(|i| solid.vertices().get(i))
            else {
                continue;
            };
            if k == 0 {
                offset = normal.dot(&p.coords);
            }
            let flat = project_point(p, frame);
            let q = Point3::new(flat.x, flat.y, 0.0);
            originals.insert(&q, u32::try_from(positions.len()).map_err(|_| overflow())?);
            positions.push(q);
            ids.push(id);
        }
        Ok(Some(Self {
            frame,
            normal,
            offset,
            originals,
            positions,
            ids,
        }))
    }

    fn map(&self, p: &Point2, vertices: &mut Vertices<'_>) -> Result<u32> {
        let flat = Point3::new(p.x, p.y, 0.0);
        if let Some(local) = self
            .originals
            .find(&flat, vertices.tolerance.linear, &self.positions)
            && let Some(&id) = usize::try_from(local).ok().and_then(|l| self.ids.get(l))
        {
            return Ok(id);
        }
        let (i, j, positive) = self.frame;
        let (a, b) = if positive { (p.x, p.y) } else { (p.y, p.x) };
        let k = 3 - i - j;
        let component = |axis: usize| self.normal.get(axis).copied().unwrap_or(0.0);
        let third = (self.offset - component(i) * a - component(j) * b) / component(k);
        let mut coords = [0.0; 3];
        for (axis, value) in [(i, a), (j, b), (k, third)] {
            if let Some(slot) = coords.get_mut(axis) {
                *slot = value;
            }
        }
        vertices.intern(Point3::new(coords[0], coords[1], coords[2]))
    }
}

/// The faces a region gives, with the loops mapped to vertex ids and the tag `tag`.
fn faces_of(
    region: &Option<Region>,
    mapper: &FaceMapper,
    vertices: &mut Vertices<'_>,
    tag: FaceTag,
) -> Result<Vec<PolyFace>> {
    let mut faces = Vec::new();
    let Some(region) = region else {
        return Ok(faces);
    };
    let ids = |lp: &Loop2, vertices: &mut Vertices<'_>| -> Result<Vec<u32>> {
        let mut out: Vec<u32> = Vec::new();
        for p in corners(lp)? {
            let id = mapper.map(&p, vertices)?;
            if out.last() != Some(&id) {
                out.push(id);
            }
        }
        while out.len() > 1 && out.first() == out.last() {
            out.pop();
        }
        Ok(out)
    };
    for part in region.parts() {
        let mut face = PolyFace::new(ids(part.outer(), vertices)?).with_tag(tag);
        for hole in part.holes() {
            face.holes.push(ids(hole, vertices)?);
        }
        faces.push(face);
    }
    Ok(faces)
}

/// A face of the solid as 2D rings in its own projection.
fn face_rings(solid: &Polyhedron, index: usize) -> Vec<Vec<Point2>> {
    let (Some(face), Some(normal)) = (solid.faces().get(index), solid.face_normal(index)) else {
        return Vec::new();
    };
    let frame = projection(&normal);
    face.loops()
        .map(|l| {
            l.iter()
                .filter_map(|&i| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| solid.vertices().get(i))
                })
                .map(|p| project_point(p, frame))
                .collect()
        })
        .collect()
}

/// The distance from `p` to the segment `a`–`b`.
fn distance_to_segment(p: &Point2, a: &Point2, b: &Point2) -> f64 {
    let (ab, ap) = (b - a, p - a);
    let square = ab.x * ab.x + ab.y * ab.y;
    let t = if square > 0.0 {
        ((ap.x * ab.x + ap.y * ab.y) / square).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (ap.x - ab.x * t, ap.y - ab.y * t);
    dx.hypot(dy)
}

/// Whether `p` is inside `poly` and farther than `margin` from its boundary.
fn strictly_inside(p: &Point2, poly: &[Point2], margin: f64) -> bool {
    inside_polygon(p, poly)
        && poly.iter().enumerate().all(|(m, c)| {
            poly.get((m + 1) % poly.len())
                .is_none_or(|d| distance_to_segment(p, c, d) > margin)
        })
}

/// Whether any edge of `rings` meets the polygon `poly`, or lies inside it; `Err` when a hole
/// of the face (every ring after the first, which is the outer loop) has a vertex strictly
/// inside the polygon: an existing opening lies in the outline. A corner of the outer loop
/// inside the polygon is not an error: the outline extends beyond the face there.
fn rings_meet_polygon(rings: &[Vec<Point2>], poly: &[Point2], margin: f64) -> Result<bool> {
    let Some(poly_box) = bounds(poly) else {
        return Ok(false);
    };
    let mut meets = false;
    for (ring_index, ring) in rings.iter().enumerate() {
        for (k, a) in ring.iter().enumerate() {
            let Some(b) = ring.get((k + 1) % ring.len()) else {
                continue;
            };
            let edge_box = (
                Point2::new(a.x.min(b.x), a.y.min(b.y)),
                Point2::new(a.x.max(b.x), a.y.max(b.y)),
            );
            if !boxes_meet(&edge_box, &poly_box, margin) {
                continue;
            }
            if ring_index > 0 && strictly_inside(a, poly, margin) {
                return Err(unsupported("the outline contains an existing opening"));
            }
            for (m, c) in poly.iter().enumerate() {
                let Some(d) = poly.get((m + 1) % poly.len()) else {
                    continue;
                };
                if segments_touch(a, b, c, d)? {
                    meets = true;
                }
            }
        }
    }
    Ok(meets)
}

/// How an edge-on face meets the prism: its plane crosses the outline in `bands`, intervals of
/// the coordinate along the line.
struct Affected {
    face: usize,
    /// Offset of the line in the frame's 2D plane: `n2 . q = c`.
    c: f64,
    n2: Point2,
    e2: Point2,
    bands: Vec<(f64, f64)>,
}

/// Everything found about one outline before the faces are rebuilt.
struct Found {
    /// The outline with every vertex within the linear tolerance of the plane of a face
    /// parallel to the axis moved onto that plane.
    outline: Vec<Point2>,
    first: usize,
    second: usize,
    h1: Vec<Point3>,
    h2: Vec<Point3>,
    affected: Vec<Affected>,
    margin: f64,
}

impl Polyhedron {
    /// Subtracts `tool` from the solid in the supported case described in the [module
    /// documentation](self).
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for an invalid tolerance or chord; `geom.solid.extrusion_invalid`
    /// when `start` is not below `end` or a value is not finite; `geom.solid.opening_unsupported`
    /// for anything outside the supported case; the errors of [`crate::triangulate`] for an
    /// invalid profile region; `geom.solid.opening_splits` when the result has more shells than the
    /// solid and `tool.allow_split` is false; `geom.solid.limit_exceeded`.
    pub fn subtract_prism(
        &self,
        tool: &PrismTool<'_>,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        let valid_chord = tool.chord.is_finite() && tool.chord > 0.0;
        if !valid_chord {
            return Err(Error::new(codes::TOLERANCE_INVALID));
        }
        let ordered = tool.start.is_finite() && tool.end.is_finite() && tool.start < tool.end;
        if !ordered {
            return Err(Error::new(codes::SOLID_EXTRUSION_INVALID));
        }
        if tool.profile.parts().iter().any(|p| !p.holes().is_empty()) {
            return Err(unsupported("the profile has holes"));
        }
        let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX) / 2;
        let angle = std::f64::consts::PI;
        let outlines = flatten_region_edges(
            tool.profile,
            tool.chord,
            angle,
            limit,
            Merge::Run(tolerance.linear),
        )?;
        let point_loops: Vec<Vec<Point2>> = outlines.iter().map(|o| o.points.clone()).collect();
        triangulate_point_loops(&point_loops)?;
        let mut current = self.clone();
        for (k, outline) in outlines.iter().enumerate() {
            let number = tool
                .opening
                .checked_add(u32::try_from(k).map_err(|_| overflow())?)
                .ok_or_else(overflow)?;
            current = current.subtract_outline(tool, outline, number, tolerance, limits)?;
        }
        if !tool.allow_split && current.shell_count() > self.shell_count() {
            return Err(Error::new(codes::SOLID_OPENING_SPLITS));
        }
        Ok(current)
    }

    /// The faces whose interior the line through `probe` along `d` meets, with the distance
    /// along the line.
    fn probe_hits(
        &self,
        probe: &Point3,
        d: &Vector3,
        tolerance: &Tolerance,
    ) -> Result<Vec<(usize, f64)>> {
        let mut hits = Vec::new();
        for (index, face) in self.faces().iter().enumerate() {
            let Some(plane) = self.face_plane(index) else {
                continue;
            };
            let LinePlane::Point { point, parameter } =
                plane.intersect_line(probe, d, tolerance)?
            else {
                continue;
            };
            let outer: Vec<Point3> = face
                .outer
                .iter()
                .filter_map(|&i| usize::try_from(i).ok().and_then(|i| self.vertices().get(i)))
                .copied()
                .collect();
            let inside_box = (0..3).all(|k| {
                let c = point.coords.get(k).copied().unwrap_or(0.0);
                let lo = outer
                    .iter()
                    .map(|p| p.coords.get(k).copied().unwrap_or(0.0))
                    .fold(f64::INFINITY, f64::min);
                let hi = outer
                    .iter()
                    .map(|p| p.coords.get(k).copied().unwrap_or(0.0))
                    .fold(f64::NEG_INFINITY, f64::max);
                c >= lo - tolerance.linear && c <= hi + tolerance.linear
            });
            if !inside_box {
                continue;
            }
            let frame = projection(
                &self
                    .face_normal(index)
                    .ok_or_else(|| unsupported("a face without a normal"))?,
            );
            if inside_rings(&project_point(&point, frame), &face_rings(self, index)) {
                hits.push((index, parameter));
            }
        }
        Ok(hits)
    }

    /// Finds the two faces, the points where the outline meets them and the faces parallel to the
    /// axis that the prism cuts, and refuses what is outside the supported case.
    #[expect(
        clippy::too_many_lines,
        reason = "the checks of the supported case, in order"
    )]
    fn find_faces(
        &self,
        tool: &PrismTool<'_>,
        outline: &[Point2],
        tolerance: &Tolerance,
    ) -> Result<Found> {
        let frame = &tool.frame;
        let d = frame.normal();
        let (_, triangles) = triangulate_point_loops(&[outline.to_vec()])?;
        let corner_of = |k: u32| usize::try_from(k).ok().and_then(|k| outline.get(k));
        // Candidate points inside the outline: centroids of triangles and a coarse grid. The
        // first one whose axis meets exactly two faces decides the pair, so an outline that
        // mostly lies outside the solid still finds its faces.
        let mut candidates: Vec<Point2> = triangles
            .iter()
            .take(16)
            .filter_map(|t| Some((corner_of(t[0])?, corner_of(t[1])?, corner_of(t[2])?)))
            .map(|(a, b, c)| Point2::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0))
            .collect();
        if let Some((lo, hi)) = bounds(outline) {
            for i in 1..8 {
                for j in 1..8 {
                    let (fx, fy) = (f64::from(i) / 8.0, f64::from(j) / 8.0);
                    let q = Point2::new(lo.x + (hi.x - lo.x) * fx, lo.y + (hi.y - lo.y) * fy);
                    if inside_polygon(&q, outline) {
                        candidates.push(q);
                    }
                }
            }
        }
        let mut found = None;
        for candidate in &candidates {
            let hits = self.probe_hits(&frame.to_world(candidate), &d, tolerance)?;
            if let [first, second] = hits.as_slice() {
                found = Some((*first, *second));
                break;
            }
        }
        let Some(((first, ta), (second, tb))) = found else {
            return Err(unsupported("the axis does not cross exactly two faces"));
        };
        let inside_extent =
            |t: f64| t > tool.start + tolerance.linear && t < tool.end - tolerance.linear;
        if !(inside_extent(ta) && inside_extent(tb)) {
            return Err(unsupported("the prism does not extend beyond both faces"));
        }
        let (Some(n1), Some(n2)) = (self.face_normal(first), self.face_normal(second)) else {
            return Err(unsupported("the faces are not known"));
        };
        if n1.cross(&n2).norm() > tolerance.angular || n1.dot(&n2) >= 0.0 {
            return Err(unsupported("the two faces are not parallel and opposite"));
        }
        // A vertex of the outline within the linear tolerance of the plane of a face parallel
        // to the axis (a floor, a ceiling, the end of a wall, the jamb of another opening) lies
        // on it: it is moved onto the plane, so that every later test sees it there.
        let mut outline = outline.to_vec();
        let (u_axis, v_axis) = (frame.u(), frame.v());
        for (index, face) in self.faces().iter().enumerate() {
            let Some(n) = self.face_normal(index) else {
                continue;
            };
            if index == first || index == second || n.dot(&d).abs() > tolerance.angular {
                continue;
            }
            let n2 = Point2::new(n.dot(&u_axis), n.dot(&v_axis));
            let square = n2.x * n2.x + n2.y * n2.y;
            let flat: Vec<Point2> = face
                .outer
                .iter()
                .filter_map(|&i| usize::try_from(i).ok().and_then(|i| self.vertices().get(i)))
                .map(|p| frame.to_plane(p))
                .collect();
            let Some(q0) = flat.first() else { continue };
            let c = n2.x * q0.x + n2.y * q0.y;
            let (u0, u1) = flat
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    let a = -n2.y * p.x + n2.x * p.y;
                    (lo.min(a), hi.max(a))
                });
            if square <= 0.0 {
                continue;
            }
            for p in &mut outline {
                let side = n2.x * p.x + n2.y * p.y - c;
                let along = -n2.y * p.x + n2.x * p.y;
                if side.abs() <= tolerance.linear
                    && along >= u0 - tolerance.linear
                    && along <= u1 + tolerance.linear
                {
                    *p = Point2::new(p.x - n2.x * side / square, p.y - n2.y * side / square);
                }
            }
        }
        let outline = outline.as_slice();
        let mut hit_points = [Vec::new(), Vec::new()];
        for (slot, face) in [first, second].into_iter().enumerate() {
            let plane = self
                .face_plane(face)
                .ok_or_else(|| unsupported("a face without a plane"))?;
            for v in outline {
                match plane.intersect_line(&frame.to_world(v), &d, tolerance)? {
                    LinePlane::Point { point, .. } => {
                        if let Some(list) = hit_points.get_mut(slot) {
                            list.push(plane.project(&point));
                        }
                    }
                    _ => return Err(unsupported("the outline is parallel to a face")),
                }
            }
        }
        let [h1, h2] = hit_points;

        let outline_box = bounds(outline).ok_or_else(|| unsupported("the profile has no area"))?;
        let bbox = self.bounding_box();
        let diagonal = (bbox.max() - bbox.min()).norm();
        let margin = 2.0 * diagonal + (tool.end - tool.start).abs() + 1.0;
        let delta = 4.0 * tolerance.linear;
        let mut affected = Vec::new();
        for (index, face) in self.faces().iter().enumerate() {
            if index == first || index == second {
                continue;
            }
            let Some(n) = self.face_normal(index) else {
                continue;
            };
            let flat: Vec<Point2> = face
                .outer
                .iter()
                .filter_map(|&i| usize::try_from(i).ok().and_then(|i| self.vertices().get(i)))
                .map(|p| frame.to_plane(p))
                .collect();
            let Some(face_box) = bounds(&flat) else {
                continue;
            };
            if n.dot(&d).abs() > tolerance.angular {
                if boxes_meet(&outline_box, &face_box, tolerance.linear)
                    && (rings_meet_polygon(std::slice::from_ref(&flat), outline, tolerance.linear)?
                        || outline.iter().any(|p| inside_polygon(p, &flat)))
                {
                    return Err(unsupported("another face meets the prism"));
                }
                continue;
            }
            // A face parallel to the axis: its projection is a segment on a line `n2 . q = c`.
            let n2 = Point2::new(n.dot(&u_axis), n.dot(&v_axis));
            let e2 = Point2::new(-n2.y, n2.x);
            let Some(q0) = flat.first() else { continue };
            let c = n2.x * q0.x + n2.y * q0.y;
            let side = |p: &Point2| n2.x * p.x + n2.y * p.y - c;
            let along = |p: &Point2| e2.x * p.x + e2.y * p.y;
            let (lo, hi) = outline
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(side(p)), hi.max(side(p)))
                });
            if lo > tolerance.linear || hi < -tolerance.linear {
                continue;
            }
            let mut breaks: Vec<f64> = Vec::new();
            for (k, p) in outline.iter().enumerate() {
                let Some(q) = outline.get((k + 1) % outline.len()) else {
                    continue;
                };
                let (sp, sq) = (side(p), side(q));
                if sp.abs() <= tolerance.linear {
                    breaks.push(along(p));
                }
                if (sp > tolerance.linear && sq < -tolerance.linear)
                    || (sp < -tolerance.linear && sq > tolerance.linear)
                {
                    let t = sp / (sp - sq);
                    breaks.push(along(&Point2::new(
                        p.x + (q.x - p.x) * t,
                        p.y + (q.y - p.y) * t,
                    )));
                }
            }
            breaks.sort_by(f64::total_cmp);
            breaks.dedup_by(|a, b| (*a - *b).abs() <= tolerance.linear);
            let (u0, u1) = flat
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(along(p)), hi.max(along(p)))
                });
            let base = Point2::new(n2.x * c, n2.y * c);
            let mut bands = Vec::new();
            for pair in breaks.windows(2) {
                let (a, b) = (pair[0].max(u0), pair[1].min(u1));
                if b - a <= tolerance.linear {
                    continue;
                }
                let mid = 0.5 * (pair[0] + pair[1]);
                let sample = Point2::new(
                    base.x + e2.x * mid - n2.x * delta,
                    base.y + e2.y * mid - n2.y * delta,
                );
                if inside_polygon(&sample, outline) {
                    bands.push((a, b));
                }
            }
            if !bands.is_empty() {
                affected.push(Affected {
                    face: index,
                    c,
                    n2,
                    e2,
                    bands,
                });
            }
        }
        Ok(Found {
            outline: outline.to_vec(),
            first,
            second,
            h1,
            h2,
            affected,
            margin,
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "find, rebuild the faces, close the walls and validate, in order"
    )]
    fn subtract_outline(
        &self,
        tool: &PrismTool<'_>,
        source_loop: &FlatLoop,
        number: u32,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Self> {
        let found = self.find_faces(tool, source_loop.points.as_slice(), tolerance)?;
        let outline = found.outline.as_slice();
        let frame = &tool.frame;
        let d = frame.normal();
        let (Some(n1), Some(n2)) = (
            self.face_normal(found.first),
            self.face_normal(found.second),
        ) else {
            return Err(unsupported("the faces are not known"));
        };
        let flat = |points: &[Point3], normal: &Vector3| -> Vec<Point2> {
            let f = projection(normal);
            points.iter().map(|p| project_point(p, f)).collect()
        };
        let (p1, p2) = (flat(&found.h1, &n1), flat(&found.h2, &n2));
        let (rings1, rings2) = (
            face_rings(self, found.first),
            face_rings(self, found.second),
        );
        let touches1 = rings_meet_polygon(&rings1, &p1, tolerance.linear)?;
        let touches2 = rings_meet_polygon(&rings2, &p2, tolerance.linear)?;
        let inside =
            |p: &[Point2], rings: &[Vec<Point2>]| p.first().is_some_and(|q| inside_rings(q, rings));
        let (inside1, inside2) = (inside(&p1, &rings1), inside(&p2, &rings2));
        if !touches1 && !inside1 || !touches2 && !inside2 {
            return Err(unsupported("the outline lies outside a face"));
        }
        let simple = found.affected.is_empty() && !touches1 && !touches2;

        let mut vertices = Vertices::new(self, tolerance)?;
        let mut faces: Vec<Option<PolyFace>> = self.faces().iter().cloned().map(Some).collect();
        let mut added: Vec<PolyFace> = Vec::new();
        let count = outline.len();
        let wall_tag = |e: usize| -> Result<FaceTag> {
            let source = source_loop.sources.get(e).ok_or_else(overflow)?;
            Ok(FaceTag::OpeningWall {
                opening: number,
                curve: source.curve,
                facet: source.facet,
            })
        };
        if simple {
            let ids1 = found
                .h1
                .iter()
                .map(|p| vertices.intern(*p))
                .collect::<Result<Vec<_>>>()?;
            let ids2 = found
                .h2
                .iter()
                .map(|p| vertices.intern(*p))
                .collect::<Result<Vec<_>>>()?;
            let reversed_1 = newell(&vertices.points, &ids1).dot(&n1) > 0.0;
            let reversed_2 = newell(&vertices.points, &ids2).dot(&n2) > 0.0;
            for k in 0..count {
                let next = (k + 1) % count;
                let at = |l: &[u32], i: usize| l.get(i).copied().unwrap_or(0);
                let (a1, b1, a2, b2) =
                    (at(&ids1, k), at(&ids1, next), at(&ids2, k), at(&ids2, next));
                added.push(
                    PolyFace::new(if reversed_1 {
                        vec![a1, b1, b2, a2]
                    } else {
                        vec![b1, a1, a2, b2]
                    })
                    .with_tag(wall_tag(k)?),
                );
            }
            let hole = |l: Vec<u32>, reversed: bool| {
                if reversed {
                    l.into_iter().rev().collect()
                } else {
                    l
                }
            };
            for (face, ids, reversed) in [
                (found.first, ids1, reversed_1),
                (found.second, ids2, reversed_2),
            ] {
                if let Some(Some(f)) = faces.get_mut(face) {
                    f.holes.push(hole(ids, reversed));
                }
            }
        } else {
            let mut rebuild =
                |face: usize, hole: &[Point2], vertices: &mut Vertices<'_>| -> Result<()> {
                    let rings = face_rings(self, face);
                    let mapper = FaceMapper::new(self, face, tolerance)?
                        .ok_or_else(|| unsupported("a face without a normal"))?;
                    let region = boolean(
                        &region_of(&rings, tolerance)?,
                        &region_of(&[hole.to_vec()], tolerance)?,
                        BooleanOp::Difference,
                        &finer(tolerance),
                        limits,
                    )?;
                    let tag = self.face_tag(face).unwrap_or_default();
                    added.extend(faces_of(&region, &mapper, vertices, tag)?);
                    if let Some(slot) = faces.get_mut(face) {
                        *slot = None;
                    }
                    Ok(())
                };
            rebuild(found.first, &p1, &mut vertices)?;
            rebuild(found.second, &p2, &mut vertices)?;
            for item in &found.affected {
                let rings = face_rings(self, item.face);
                let normal = self
                    .face_normal(item.face)
                    .ok_or_else(|| unsupported("a face without a normal"))?;
                let face_frame = projection(&normal);
                let mapper = FaceMapper::new(self, item.face, tolerance)?
                    .ok_or_else(|| unsupported("a face without a normal"))?;
                let mut region = Some(region_of(&rings, tolerance)?);
                let base = Point2::new(item.n2.x * item.c, item.n2.y * item.c);
                for &(ua, ub) in &item.bands {
                    let at = |u: f64, t: f64| -> Point2 {
                        let q = Point2::new(base.x + item.e2.x * u, base.y + item.e2.y * u);
                        project_point(&(frame.to_world(&q) + d * t), face_frame)
                    };
                    let band = [
                        at(ua, -found.margin),
                        at(ub, -found.margin),
                        at(ub, found.margin),
                        at(ua, found.margin),
                    ];
                    region = match region {
                        Some(r) => boolean(
                            &r,
                            &region_of(&[band.to_vec()], tolerance)?,
                            BooleanOp::Difference,
                            &finer(tolerance),
                            limits,
                        )?,
                        None => None,
                    };
                }
                let tag = self.face_tag(item.face).unwrap_or_default();
                added.extend(faces_of(&region, &mapper, &mut vertices, tag)?);
                if let Some(slot) = faces.get_mut(item.face) {
                    *slot = None;
                }
            }
            // The walls: a quadrilateral where the edge is clear of both faces' boundaries,
            // otherwise the part of the section by the edge's plane within the edge's band.
            let ids1 = found
                .h1
                .iter()
                .map(|p| vertices.intern(*p))
                .collect::<Result<Vec<_>>>()?;
            let ids2 = found
                .h2
                .iter()
                .map(|p| vertices.intern(*p))
                .collect::<Result<Vec<_>>>()?;
            let reversed_1 = newell(&vertices.points, &ids1).dot(&n1) > 0.0;
            for k in 0..count {
                let next = (k + 1) % count;
                let (Some(a), Some(b)) = (outline.get(k), outline.get(next)) else {
                    continue;
                };
                let edge_clear = |ring_set: &[Vec<Point2>], p: &[Point2]| -> Result<bool> {
                    let (Some(pa), Some(pb)) = (p.get(k), p.get(next)) else {
                        return Ok(false);
                    };
                    let mid = Point2::new(0.5 * (pa.x + pb.x), 0.5 * (pa.y + pb.y));
                    Ok(inside_rings(&mid, ring_set)
                        && !rings_meet_polygon(ring_set, &[*pa, *pb, *pa], tolerance.linear)?)
                };
                if edge_clear(&rings1, &p1)? && edge_clear(&rings2, &p2)? {
                    let at = |l: &[u32], i: usize| l.get(i).copied().unwrap_or(0);
                    let (a1, b1, a2, b2) =
                        (at(&ids1, k), at(&ids1, next), at(&ids2, k), at(&ids2, next));
                    added.push(
                        PolyFace::new(if reversed_1 {
                            vec![a1, b1, b2, a2]
                        } else {
                            vec![b1, a1, a2, b2]
                        })
                        .with_tag(wall_tag(k)?),
                    );
                } else {
                    added.extend(self.wall_piece(
                        tool,
                        (a, b),
                        wall_tag(k)?,
                        &mut vertices,
                        tolerance,
                        limits,
                    )?);
                }
            }
        }

        // Compact the vertices in use and validate.
        let all: Vec<PolyFace> = faces.into_iter().flatten().chain(added).collect();
        let mut used: Vec<u32> = all
            .iter()
            .flat_map(PolyFace::loops)
            .flatten()
            .copied()
            .collect();
        used.sort_unstable();
        used.dedup();
        let mut remap: HashMap<u32, u32> = HashMap::with_capacity(used.len());
        let mut points = Vec::with_capacity(used.len());
        for id in used {
            let p = usize::try_from(id)
                .ok()
                .and_then(|i| vertices.points.get(i))
                .copied()
                .ok_or_else(overflow)?;
            remap.insert(id, u32::try_from(points.len()).map_err(|_| overflow())?);
            points.push(p);
        }
        let map = |l: &Vec<u32>| {
            l.iter()
                .filter_map(|v| remap.get(v).copied())
                .collect::<Vec<u32>>()
        };
        let all = all
            .iter()
            .map(|f| PolyFace {
                outer: map(&f.outer),
                holes: f.holes.iter().map(map).collect(),
                tag: f.tag,
            })
            .collect();
        Self::new(points, all, tolerance, limits).map_err(|error| {
            if error.code() == codes::SOLID_LIMIT_EXCEEDED.code {
                error
            } else {
                unsupported("the result is not a valid solid").with_source(error)
            }
        })
    }

    /// The wall of the outline edge `a`–`b` where it is not a plain quadrilateral: the section of
    /// the solid by the edge's plane, kept within the band of the edge.
    fn wall_piece(
        &self,
        tool: &PrismTool<'_>,
        (a, b): (&Point2, &Point2),
        tag: FaceTag,
        vertices: &mut Vertices<'_>,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<Vec<PolyFace>> {
        let frame = &tool.frame;
        let d = frame.normal();
        let bbox = self.bounding_box();
        let margin = 2.0 * (bbox.max() - bbox.min()).norm() + (tool.end - tool.start).abs() + 1.0;
        let (a3, b3) = (frame.to_world(a), frame.to_world(b));
        let along = b3 - a3;
        let length = along.norm();
        let long_enough = length > tolerance.linear;
        if !long_enough {
            return Ok(Vec::new());
        }
        let normal = d.cross(&(along / length));
        let plane = Plane3::new(a3, normal)?;
        let Some(section) = self.section(&plane, tolerance, limits)? else {
            return Ok(Vec::new());
        };
        let plane_frame = plane.frame(tolerance);
        let band: Vec<Point2> = [
            a3 - d * margin,
            b3 - d * margin,
            b3 + d * margin,
            a3 + d * margin,
        ]
        .iter()
        .map(|p| plane_frame.to_plane(p))
        .collect();
        let piece = boolean(
            &section,
            &region_of(&[band], tolerance)?,
            BooleanOp::Intersection,
            &finer(tolerance),
            limits,
        )?;
        let mut faces = Vec::new();
        let Some(piece) = piece else {
            return Ok(faces);
        };
        let ids = |lp: &Loop2, vertices: &mut Vertices<'_>| -> Result<Vec<u32>> {
            let mut out: Vec<u32> = Vec::new();
            for p in corners(lp)? {
                let id = vertices.intern(plane_frame.to_world(&p))?;
                if out.last() != Some(&id) {
                    out.push(id);
                }
            }
            while out.len() > 1 && out.first() == out.last() {
                out.pop();
            }
            Ok(out)
        };
        for part in piece.parts() {
            let mut face = PolyFace::new(ids(part.outer(), vertices)?).with_tag(tag);
            for hole in part.holes() {
                face.holes.push(ids(hole, vertices)?);
            }
            faces.push(face);
        }
        Ok(faces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Curve2;
    use crate::primitives::Circle2;
    use crate::region::{Loop2, RegionPart};
    use crate::solid::tests::{TOL, cuboid, p};
    use crate::solid_clip::ClipSide;
    use crate::solid_gen::prism;
    use crate::solid_gen::tests::{polygon, region};

    const LIMITS: Limits = Limits::DEFAULT;

    /// A frame at `y = 1` looking along -y with x to the right and z up.
    fn wall_frame() -> PlaneFrame {
        PlaneFrame::new(p(0.0, 1.0, 0.0), Vector3::x(), Vector3::z(), &TOL).expect("frame")
    }

    fn wall() -> Polyhedron {
        // 6 long, 0.3 thick (y from 0 to 0.3), 3 high.
        cuboid(6.0, 0.3, 3.0)
    }

    fn door(x0: f64, x1: f64, z0: f64, z1: f64) -> Region {
        region(&[(x0, z0), (x1, z0), (x1, z1), (x0, z1)], &[])
    }

    fn tool(profile: &Region) -> PrismTool<'_> {
        PrismTool {
            profile,
            frame: wall_frame(),
            start: 0.5,
            end: 1.5,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        }
    }

    fn code<T>(result: Result<T>) -> Option<&'static str> {
        result.err().map(|e| e.code())
    }

    #[test]
    fn a_door_opening_through_a_straight_wall() {
        let profile = door(2.0, 3.0, 0.0 + 0.5, 2.5);
        let result = wall()
            .subtract_prism(&tool(&profile), &TOL, &LIMITS)
            .expect("opening");
        assert!((result.volume() - (5.4 - 2.0 * 0.3)).abs() < 1e-9);
        assert_eq!(result.faces().len(), 6 + 4);
        // The result is a solid like any other: cut through the opening.
        let level = crate::Plane3::new(p(0.0, 0.0, 1.0), Vector3::z()).expect("plane");
        let section = result
            .section(&level, &TOL, &LIMITS)
            .expect("section")
            .expect("some");
        assert_eq!(section.parts().len(), 2, "the wall is split by the opening");
        let above = result
            .clip(&level, ClipSide::Positive, 0, &TOL, &LIMITS)
            .expect("clip")
            .expect("some");
        let below = result
            .clip(&level, ClipSide::Negative, 0, &TOL, &LIMITS)
            .expect("clip")
            .expect("some");
        assert!((above.volume() + below.volume() - result.volume()).abs() < 1e-9);
        // Triangulation of the faces with holes.
        for index in 0..result.faces().len() {
            assert!(
                !result
                    .triangulate_face(index, &TOL)
                    .expect("triangles")
                    .is_empty()
            );
        }
    }

    #[test]
    fn two_openings_in_one_call_are_numbered_from_the_tool() {
        let two = Region::new(vec![
            RegionPart::new(
                polygon(&[(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]),
                vec![],
            ),
            RegionPart::new(
                polygon(&[(3.0, 1.0), (5.0, 1.0), (5.0, 2.0), (3.0, 2.0)]),
                vec![],
            ),
        ])
        .expect("region");
        let result = wall()
            .subtract_prism(
                &PrismTool {
                    opening: 7,
                    ..tool(&two)
                },
                &TOL,
                &LIMITS,
            )
            .expect("two openings");
        assert!((result.volume() - (5.4 - 0.3 * 3.0)).abs() < 1e-9);
        let numbers: std::collections::BTreeSet<u32> = result
            .faces()
            .iter()
            .filter_map(|f| match f.tag {
                crate::solid::FaceTag::OpeningWall { opening, .. } => Some(opening),
                _ => None,
            })
            .collect();
        assert_eq!(numbers.into_iter().collect::<Vec<_>>(), vec![7, 8]);
    }

    #[test]
    fn a_slanted_and_a_round_opening() {
        let (s, c) = (0.3_f64.sin(), 0.3_f64.cos());
        let frame = PlaneFrame::new(
            p(0.0, 1.0, 0.0),
            Vector3::x(),
            Vector3::new(0.0, s, c),
            &TOL,
        )
        .expect("frame");
        let profile = door(2.0, 3.0, 1.0, 2.0);
        let slanted = PrismTool {
            profile: &profile,
            frame,
            start: 0.0,
            end: 3.0,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        let result = wall()
            .subtract_prism(&slanted, &TOL, &LIMITS)
            .expect("slanted");
        // The profile has area 1 in the frame's plane, which is the cross section of the prism.
        assert!(
            (result.volume() - (5.4 - 0.3 / c)).abs() < 1e-9,
            "volume {}",
            result.volume()
        );
        let round = Region::new(vec![RegionPart::new(
            Loop2::new(
                vec![Curve2::Circle(
                    Circle2::new(arc_base::math::Point2::new(3.0, 1.5), 0.5).expect("circle"),
                )],
                &TOL,
            )
            .expect("loop"),
            vec![],
        )])
        .expect("region");
        let result = wall()
            .subtract_prism(&tool(&round), &TOL, &LIMITS)
            .expect("round");
        let removed = 5.4 - result.volume();
        let exact = std::f64::consts::PI * 0.25 * 0.3;
        assert!(removed < exact && exact - removed < 2.0 * std::f64::consts::PI * 0.5 * 1e-3 * 0.3);
    }

    #[test]
    fn anything_outside_the_supported_case_is_refused_with_a_reason() {
        let profile = door(2.0, 3.0, 0.5, 2.5);
        let unsupported_code = Some("geom.solid.opening_unsupported");
        // Pocket: the prism stops inside the wall.
        let pocket = PrismTool {
            end: 0.85,
            ..tool(&profile)
        };
        assert_eq!(
            code(wall().subtract_prism(&pocket, &TOL, &LIMITS)),
            unsupported_code
        );
        // Outline entirely outside the face.
        let outside = door(10.0, 12.0, 0.5, 2.5);
        assert_eq!(
            code(wall().subtract_prism(&tool(&outside), &TOL, &LIMITS)),
            unsupported_code
        );
        // Axis along the wall, wider than the wall: the prism cuts it into two slabs.
        let along = PrismTool {
            frame: PlaneFrame::new(p(-1.0, 0.15, 0.0), Vector3::y(), Vector3::z(), &TOL)
                .expect("frame"),
            start: 0.0,
            end: 8.0,
            ..tool(&profile)
        };
        let narrow = door(-0.2, 0.2, 1.0, 2.0);
        let along = PrismTool {
            profile: &narrow,
            ..along
        };
        assert_eq!(
            code(wall().subtract_prism(&along, &TOL, &LIMITS)),
            Some("geom.solid.opening_splits")
        );
        let along = PrismTool {
            allow_split: true,
            ..along
        };
        let slabs = wall()
            .subtract_prism(&along, &TOL, &LIMITS)
            .expect("two slabs");
        assert!((slabs.volume() - (5.4 - 0.3 * 6.0)).abs() < 1e-9);
        assert!(watertight(&slabs));
        // A profile with a hole.
        let holed = region(
            &[(1.0, 0.5), (4.0, 0.5), (4.0, 2.5), (1.0, 2.5)],
            &[&[(2.0, 1.0), (2.0, 2.0), (3.0, 2.0), (3.0, 1.0)]],
        );
        assert_eq!(
            code(wall().subtract_prism(&tool(&holed), &TOL, &LIMITS)),
            unsupported_code
        );
        // Misordered extent and bad chord.
        let backwards = PrismTool {
            start: 2.0,
            ..tool(&profile)
        };
        assert_eq!(
            code(wall().subtract_prism(&backwards, &TOL, &LIMITS)),
            Some("geom.solid.extrusion_invalid")
        );
        let no_chord = PrismTool {
            chord: 0.0,
            ..tool(&profile)
        };
        assert_eq!(
            code(wall().subtract_prism(&no_chord, &TOL, &LIMITS)),
            Some("geom.tolerance.invalid")
        );
    }

    #[test]
    fn a_tube_that_would_cross_a_third_face_is_refused() {
        // An L-shaped wall in plan, 3 high; a prism along y over x in 0.5..2.5 would leave the
        // solid through two different faces (the step at y = 1 and the end at y = 3).
        let l = region(
            &[
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 1.0),
                (1.0, 1.0),
                (1.0, 3.0),
                (0.0, 3.0),
            ],
            &[],
        );
        let solid = prism(&l, 0.0, 3.0, 1e-3, &TOL, &LIMITS).expect("l wall");
        let profile = door(0.5, 2.5, 1.0, 2.0);
        let frame =
            PlaneFrame::new(p(0.0, 5.0, 0.0), Vector3::x(), Vector3::z(), &TOL).expect("frame");
        let across = PrismTool {
            profile: &profile,
            frame,
            start: 0.0,
            end: 10.0,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        assert_eq!(
            code(solid.subtract_prism(&across, &TOL, &LIMITS)),
            Some("geom.solid.opening_unsupported")
        );
        // Through the leg along x it works: faces x = 0 and x = 1.
        let leg = region(&[(1.5, 1.0), (2.5, 1.0), (2.5, 2.0), (1.5, 2.0)], &[]);
        let frame =
            PlaneFrame::new(p(-1.0, 0.0, 0.0), Vector3::y(), Vector3::z(), &TOL).expect("frame");
        let through = PrismTool {
            profile: &leg,
            frame,
            start: 0.0,
            end: 3.5,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        let result = solid
            .subtract_prism(&through, &TOL, &LIMITS)
            .expect("through the leg");
        assert!((result.volume() - (solid.volume() - 1.0 * 1.0 * 1.0)).abs() < 1e-9);
    }

    // ---- openings that touch or cross the boundary, tags, meshes, cost.

    use crate::solid::FaceTag;
    use crate::solid::tests::watertight;
    use crate::solid_clip::tests::elevation_wall;

    fn plain_wall() -> Polyhedron {
        let footprint = region(&[(0.0, 0.0), (5.0, 0.0), (5.0, 0.2), (0.0, 0.2)], &[]);
        prism(&footprint, 0.0, 3.0, 1e-3, &TOL, &LIMITS).expect("wall")
    }

    fn wall_with_openings(rects: &[(f64, f64, f64, f64)]) -> Polyhedron {
        openings_in(plain_wall(), rects, false)
    }

    fn openings_in(
        mut solid: Polyhedron,
        rects: &[(f64, f64, f64, f64)],
        allow_split: bool,
    ) -> Polyhedron {
        for (k, &(x0, z0, x1, z1)) in rects.iter().enumerate() {
            let profile = door(x0, x1, z0, z1);
            let t = PrismTool {
                opening: u32::try_from(k).expect("small"),
                allow_split,
                ..tool(&profile)
            };
            solid = solid.subtract_prism(&t, &TOL, &LIMITS).expect("opening");
        }
        solid
    }

    #[test]
    fn a_door_at_floor_level_equals_the_wall_extruded_from_its_elevation() {
        let wall = wall_with_openings(&[(1.0, 0.0, 1.9, 2.1), (3.0, 0.9, 4.2, 2.4)]);
        let reference = elevation_wall();
        assert!((wall.volume() - reference.volume()).abs() < 1e-12);
        assert!((wall.surface_area() - reference.surface_area()).abs() < 1e-9);
        assert!(watertight(&wall));
        assert!(wall.faces().iter().all(|f| f.tag != FaceTag::Untagged));
        // The floor is split in two by the door, with the same tag; the walls carry numbers.
        let bottoms = wall
            .faces()
            .iter()
            .filter(|f| f.tag == FaceTag::Bottom)
            .count();
        assert_eq!(bottoms, 2);
        let mut numbers: Vec<u32> = wall
            .faces()
            .iter()
            .filter_map(|f| match f.tag {
                FaceTag::OpeningWall { opening, .. } => Some(opening),
                _ => None,
            })
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers, vec![0, 1]);
    }

    #[test]
    fn full_height_end_of_wall_and_crossing_openings() {
        let full = openings_in(plain_wall(), &[(2.0, 0.0, 3.0, 3.0)], true);
        assert!((full.volume() - (3.0 - 3.0) * 0.2 - 4.0 * 3.0 * 0.2).abs() < 1e-12);
        assert!(watertight(&full));
        let at_end = wall_with_openings(&[(0.0, 0.5, 1.0, 2.5)]);
        assert!((at_end.volume() - (3.0 - 2.0) * 0.2 * 1.0 - 4.0 * 3.0 * 0.2).abs() < 1e-12);
        assert!(watertight(&at_end));
        let crossing = wall_with_openings(&[(1.0, -0.5, 1.9, 2.1)]);
        let at_floor = wall_with_openings(&[(1.0, 0.0, 1.9, 2.1)]);
        assert!((crossing.volume() - at_floor.volume()).abs() < 1e-12);
        assert!(watertight(&crossing));
        let past_end = wall_with_openings(&[(4.5, 1.0, 6.0, 2.0)]);
        assert!((past_end.volume() - (15.0 - 0.5 * 1.0) * 0.2).abs() < 1e-12);
        assert!(watertight(&past_end));
    }

    #[test]
    fn a_window_wall_clipped_through_its_openings_meshes_on_both_sides() {
        let wall = wall_with_openings(&[(1.0, 0.0, 1.9, 2.1), (3.0, 0.9, 4.2, 2.4)]);
        for z in [1.2, 0.9, 2.1, 2.4] {
            let cut = Plane3::new(p(0.0, 0.0, z), Vector3::z()).expect("plane");
            let above = wall
                .clip(&cut, ClipSide::Positive, 0, &TOL, &LIMITS)
                .expect("above")
                .expect("some");
            let below = wall
                .clip(&cut, ClipSide::Negative, 0, &TOL, &LIMITS)
                .expect("below")
                .expect("some");
            assert!(
                (above.volume() + below.volume() - wall.volume()).abs() < 1e-9,
                "z {z}"
            );
            assert!(watertight(&above) && watertight(&below), "z {z}");
        }
    }

    #[test]
    fn tags_survive_extrude_then_subtract_then_clip_on_a_far_slanted_wall() {
        let angle = 0.5235987755982988_f64;
        let far = p(512_345.678, 4_423_456.789, 1_234.5);
        let dir = Vector3::new(angle.cos(), angle.sin(), 0.0);
        let frame = PlaneFrame::new(far, dir, Vector3::z(), &TOL).expect("frame");
        let footprint = region(&[(0.0, 0.0), (5.0, 0.0), (5.0, 3.0), (0.0, 3.0)], &[]);
        let wall =
            crate::solid_gen::extrude(&footprint, &frame, -0.2, 1e-3, &TOL, &LIMITS).expect("wall");
        let window = door(3.0, 4.2, 0.9, 2.4);
        let tool_frame =
            PlaneFrame::new(far + frame.normal() * 1.0, dir, Vector3::z(), &TOL).expect("frame");
        let cutter = PrismTool {
            profile: &window,
            frame: tool_frame,
            start: -3.0,
            end: 0.0,
            chord: 1e-3,
            opening: 4,
            allow_split: false,
        };
        let holed = wall.subtract_prism(&cutter, &TOL, &LIMITS).expect("window");
        assert!((wall.volume() - holed.volume() - 1.2 * 1.5 * 0.2).abs() < 1e-6);
        assert!(watertight(&holed));
        let cut = Plane3::new(Point3::new(far.x, far.y, far.z + 1.2), Vector3::z()).expect("plane");
        let top = holed
            .clip(&cut, ClipSide::Positive, 0, &TOL, &LIMITS)
            .expect("clip")
            .expect("some");
        assert!(watertight(&top));
        let tags: Vec<FaceTag> = top.faces().iter().map(|f| f.tag).collect();
        assert!(tags.contains(&FaceTag::Top) || tags.contains(&FaceTag::Bottom));
        assert!(tags.iter().any(|t| matches!(t, FaceTag::Side { .. })));
        assert!(
            tags.iter()
                .any(|t| matches!(t, FaceTag::OpeningWall { opening: 4, .. }))
        );
        assert!(tags.contains(&FaceTag::Cap { cut: 0 }));
        assert!(!tags.contains(&FaceTag::Untagged));
    }

    #[test]
    fn fifty_openings_and_round_openings_stay_fast() {
        let rects: Vec<(f64, f64, f64, f64)> = (0..50)
            .map(|k| {
                let x = 0.1 + 0.095 * f64::from(k);
                (x, 1.0 + 0.01 * f64::from(k % 5), x + 0.06, 2.0)
            })
            .collect();
        let wall = wall_with_openings(&rects);
        // Each opening removes 0.06 x (2 - sill) x 0.2; the sills repeat in steps of 0.01.
        assert!((wall.volume() - (3.0 - 0.012 * 10.0 * 4.9)).abs() < 1e-9);
        let long = cuboid(60.0, 0.2, 3.0);
        let mut solid = long;
        for k in 0..6 {
            let circle = Region::new(vec![RegionPart::new(
                Loop2::new(
                    vec![Curve2::Circle(
                        Circle2::new(
                            arc_base::math::Point2::new(3.0 + 9.0 * f64::from(k), 1.5),
                            0.5,
                        )
                        .expect("circle"),
                    )],
                    &TOL,
                )
                .expect("loop"),
                vec![],
            )])
            .expect("region");
            let t = PrismTool {
                chord: 1e-5,
                ..tool(&circle)
            };
            solid = solid.subtract_prism(&t, &TOL, &LIMITS).expect("round");
        }
        // Six inscribed polygons: each is smaller than the circle by less than the chord times
        // its circumference.
        let circle = std::f64::consts::PI * 0.25 * 0.2;
        let removed = 36.0 - solid.volume();
        assert!(removed < 6.0 * circle);
        assert!(6.0 * circle - removed < 6.0 * 2.0 * std::f64::consts::PI * 0.5 * 1e-5 * 0.2);
    }

    // ---- openings at corners and near boundaries, the axis check.

    /// The wall of the qualification cases: 5 long, 0.2 thick, 3 high, with the profile frame at
    /// `y = -1` looking along +y (the profile's x is the wall's x, its y the elevation).
    fn case_frame() -> PlaneFrame {
        PlaneFrame::new(p(0.0, -1.0, 0.0), Vector3::x(), Vector3::z(), &TOL).expect("frame")
    }

    fn cut(wall: &Polyhedron, x0: f64, z0: f64, x1: f64, z1: f64) -> Result<Polyhedron> {
        let profile = door(x0, x1, z0, z1);
        let t = PrismTool {
            profile: &profile,
            frame: case_frame(),
            start: -2.0,
            end: 0.5,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        wall.subtract_prism(&t, &TOL, &LIMITS)
    }

    fn cut_ok(wall: &Polyhedron, x0: f64, z0: f64, x1: f64, z1: f64, volume: f64) -> Polyhedron {
        let solid = cut(wall, x0, z0, x1, z1).expect("opening");
        assert!(
            (solid.volume() - volume).abs() < 1e-9,
            "({x0}, {z0}, {x1}, {z1}): volume {} instead of {volume}",
            solid.volume()
        );
        assert!(watertight(&solid));
        solid
    }

    #[test]
    fn a_door_at_the_floor_and_the_wall_end_at_once_is_cut() {
        let wall = plain_wall();
        let removed = 0.8 * 2.1 * 0.2;
        for (x0, z0) in [(-0.5, 0.0), (0.0, 0.0), (-0.5, -0.5), (0.0, -0.5)] {
            let solid = cut_ok(&wall, x0, z0, 0.8, 2.1, 3.0 - removed);
            assert_eq!(solid.shell_count(), 1);
        }
        // The same at the far end, and across the vertical cut of a clipped wall.
        cut_ok(&wall, 4.2, 0.0, 5.5, 2.1, 3.0 - removed);
        let half = wall
            .clip(
                &Plane3::new(p(2.5, 0.0, 0.0), Vector3::x()).expect("plane"),
                ClipSide::Positive,
                0,
                &TOL,
                &LIMITS,
            )
            .expect("clip")
            .expect("some");
        cut_ok(&half, 2.0, 0.0, 3.0, 2.1, 1.5 - 0.5 * 2.1 * 0.2);
        // A second door that shares the jamb of the first, and one above its head.
        let first = cut_ok(&wall, 1.0, 0.0, 1.9, 2.1, 3.0 - 0.378);
        cut_ok(&first, 1.9, 0.0, 2.5, 1.0, 3.0 - 0.378 - 0.6 * 0.2);
        cut_ok(&first, 1.2, 2.1, 1.7, 2.6, 3.0 - 0.378 - 0.5 * 0.5 * 0.2);
    }

    #[test]
    fn an_edge_within_the_linear_tolerance_lies_on_the_boundary_and_one_beyond_is_a_gap() {
        let wall = plain_wall();
        let first = cut_ok(&wall, 1.0, 0.0, 1.9, 2.1, 3.0 - 0.378);
        let window = cut_ok(&wall, 1.0, 1.0, 2.0, 2.0, 2.8);
        for gap in [3e-7, 8e-7] {
            // On the wall end, the head, the floor, the jamb of an opening and its neighbor.
            cut_ok(&wall, gap, 0.0, 0.8, 2.1, 3.0 - 0.8 * 2.1 * 0.2);
            cut_ok(&wall, 3.0, 2.0, 4.0, 3.0 - gap, 3.0 - 0.2);
            cut_ok(&wall, 1.0, gap, 1.9, 2.1, 3.0 - 0.378);
            cut_ok(&first, 1.9 + gap, 0.0, 2.5, 1.0, 3.0 - 0.378 - 0.12);
            cut_ok(&window, 2.0 + gap, 1.0, 3.0, 2.0, 2.6);
            cut_ok(&window, 1.2, 2.0 + gap, 1.8, 2.5, 2.8 - 0.06);
        }
        // Beyond the tolerance a strip stays: a normal gap, never refused.
        for gap in [1.5e-6, 2e-6, 1e-4] {
            let strip = 0.2 * gap * 2.1;
            let end = cut_ok(&wall, gap, 0.0, 0.8, 2.1, 3.0 - 0.336 + strip);
            assert_eq!(end.shell_count(), 1);
            cut_ok(
                &first,
                1.9 + gap,
                0.0,
                2.5,
                1.0,
                3.0 - 0.378 - 0.12 + 0.2 * gap,
            );
            cut_ok(&window, 2.0 + gap, 1.0, 3.0, 2.0, 2.6 + 0.2 * gap);
            cut_ok(&wall, 3.0, 2.0, 4.0, 3.0 - gap, 3.0 - 0.2 + 0.2 * gap);
        }
    }

    #[test]
    fn an_opening_that_encloses_an_existing_one_is_still_refused() {
        let window = cut_ok(&plain_wall(), 1.0, 1.0, 2.0, 2.0, 2.8);
        assert_eq!(
            code(cut(&window, 0.5, 0.5, 2.5, 2.5)),
            Some("geom.solid.opening_unsupported")
        );
    }

    #[test]
    fn a_wrong_axis_that_splits_the_wall_is_refused_unless_allowed() {
        // A window profile in the y-z plane with the axis along the wall: it cuts the 3 m
        // block into two shells (volume 2.0 instead of 3.0 minus nothing).
        let frame =
            PlaneFrame::new(p(-1.0, 0.0, 0.0), Vector3::y(), Vector3::z(), &TOL).expect("frame");
        let profile = door(-0.5, 0.7, 1.0, 2.0);
        let tool = PrismTool {
            profile: &profile,
            frame,
            start: -1.0,
            end: 8.0,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        let wall = plain_wall();
        assert_eq!(
            code(wall.subtract_prism(&tool, &TOL, &LIMITS)),
            Some("geom.solid.opening_splits")
        );
        let allowed = PrismTool {
            allow_split: true,
            ..tool
        };
        let split = wall.subtract_prism(&allowed, &TOL, &LIMITS).expect("split");
        assert_eq!(split.shell_count(), 2);
        assert!((split.volume() - 2.0).abs() < 1e-12);
        assert!(watertight(&split));
        // A full-length opening that cuts the wall in two is the intended use of the flag.
        let full = door(0.0, 5.0, 1.0, 2.0);
        let across = PrismTool {
            profile: &full,
            frame: case_frame(),
            start: -2.0,
            end: 0.5,
            chord: 1e-3,
            opening: 0,
            allow_split: false,
        };
        assert_eq!(
            code(wall.subtract_prism(&across, &TOL, &LIMITS)),
            Some("geom.solid.opening_splits")
        );
        let across = PrismTool {
            allow_split: true,
            ..across
        };
        let two = wall
            .subtract_prism(&across, &TOL, &LIMITS)
            .expect("two parts");
        assert_eq!(two.shell_count(), 2);
        assert!((two.volume() - 2.0).abs() < 1e-12);
    }
}
