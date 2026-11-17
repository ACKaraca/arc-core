//! Polygon triangulation of regions for display (CORE-GEOM-022).
//!
//! [`triangulate`] flattens every loop of a [`Region`] with [`CurveFlatten`] at the supplied
//! chord and angle tolerances and returns the constrained Delaunay triangulation of the
//! flattened loops, restricted to the region.
//!
//! - **Flattening.** Each curve of a loop is flattened in order; the first vertex of each curve
//!   after the first is the previous curve's end, exactly or to the rounding left by reversal or
//!   a transform, so it is dropped, and so is the closing vertex. A segment no longer than the
//!   chord tolerance whose neighbors in the loop are not such segments, such as a joint that
//!   closes a construction gap, is merged into the next curve, which then starts at the
//!   previous curve's end: the boundary moves by at most the segment's length, and a joint
//!   cannot fold back over its neighbors. A chain of two or more such segments is kept vertex
//!   by vertex, since together they may turn arbitrarily far from a straight line. Consecutive
//!   vertices that are exactly equal are merged. A flattened loop with fewer than three
//!   vertices, or whose shoelace area is zero or has the opposite sign of the loop's exact
//!   area, fails with `geom.triangulate.loop_collapsed`: the tolerances are too coarse for that
//!   loop.
//! - **Triangulation and simplicity.** The vertices and the loop edges as constraints form a
//!   constrained Delaunay triangulation, whose predicates are exact. Vertices are inserted in
//!   output order and each loop edge is added as soon as both of its ends are present. An edge
//!   that would cross an earlier edge, two equal vertices (two loops sharing a vertex), and a
//!   vertex on an edge or an edge through a vertex (touching away from a shared vertex, overlap,
//!   or adjacent edges folding back) fail with `geom.triangulate.edges_cross`. This catches loops that cross, and loops
//!   that do not but whose chords cross after coarse flattening. Among all triangulations of the
//!   flattened vertices that contain the loop edges, the result maximizes the smallest angle, so
//!   no triangle is thinner than the vertices and edges force. No vertices are added, so thin
//!   triangles remain where the flattened boundary itself forces them; this is the meaning of
//!   "where the input allows".
//! - **Cost.** Each edge is added by walking the triangles it crosses. Because an edge is added
//!   right after its end vertices, while few other vertices lie across it, the cost is close to
//!   `O(n log n)` for `n` flattened vertices on typical input: in an optimized build a
//!   314,000-vertex circle takes under 1 s and a 20,000-vertex comb of long slanted teeth under
//!   0.1 s. The worst case is `O(n²)`: many long edges of later loops that each cross the
//!   triangles of many earlier vertices, such as 2,500 long thin holes stacked inside a loop
//!   whose two long sides have 5,000 vertices each (20,000 vertices in all), take about 12 s.
//! - **Inside.** Triangles are kept by parity: a walk from the outside across the
//!   triangulation toggles at every loop edge, so a triangle is inside when it is enclosed by
//!   an odd number of loops. Outer loops, holes and parts inside holes need no centroid tests.
//!   Every loop edge must then have a kept triangle on its left (the region side of a
//!   counterclockwise outer loop or a clockwise hole) and none on its right; otherwise a hole
//!   lies outside its outer loop or parts overlap, and the call fails with
//!   `geom.triangulate.nesting_invalid` instead of returning the wrong triangles.
//! - **Output.** [`TriangleMesh2`] holds every flattened vertex, loop by loop in region order
//!   (each part's outer loop, then its holes), and counterclockwise triangles indexing them.
//!   The same region and tolerances always give the same mesh.
//!
//! Coordinates must lie within the range of the triangulation's exact arithmetic: zero or a
//! magnitude between about 1.8e-43 and 3.2e60. Others fail with `geom.precision.insufficient`.

use std::collections::VecDeque;

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result, Tolerance};
use serde::{Deserialize, Serialize};
use spade::handles::FixedVertexHandle;
use spade::{ConstrainedDelaunayTriangulation, Triangulation};

use crate::curve::Curve2;
use crate::error::codes;
use crate::flatten::CurveFlatten;
use crate::predicates::{Sign, orient2d};
use crate::region::{Loop2, Region};

/// An indexed triangle mesh in the plane: vertices and strictly counterclockwise triangles of
/// vertex indices.
///
/// Serialized as its vertices and triangles; deserialization validates as [`Self::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "TriangleMesh2Raw")]
pub struct TriangleMesh2 {
    vertices: Vec<Point2>,
    triangles: Vec<[u32; 3]>,
}

crate::deserialize_through_new!(
    TriangleMesh2,
    TriangleMesh2Raw {
        vertices: Vec<Point2>,
        triangles: Vec<[u32; 3]>,
    }
);

impl TriangleMesh2 {
    /// Creates a mesh from vertices and triangles of vertex indices.
    ///
    /// # Errors
    ///
    /// `geom.mesh.invalid` when a vertex is not finite, a triangle refers to an index outside
    /// the vertex list, or a triangle is not strictly counterclockwise as decided exactly by
    /// [`orient2d`] (clockwise, degenerate, with a repeated index, or with coordinates whose
    /// orientation cannot be decided).
    pub fn new(vertices: Vec<Point2>, triangles: Vec<[u32; 3]>) -> Result<Self> {
        let finite = vertices.iter().all(|p| p.x.is_finite() && p.y.is_finite());
        let counterclockwise = || {
            triangles.iter().all(|t| {
                let [a, b, c] = t.map(|i| usize::try_from(i).ok().and_then(|i| vertices.get(i)));
                matches!((a, b, c), (Some(a), Some(b), Some(c))
                    if orient2d(a, b, c).ok() == Some(Sign::Positive))
            })
        };
        if finite && counterclockwise() {
            Ok(Self {
                vertices,
                triangles,
            })
        } else {
            Err(Error::new(codes::MESH_INVALID))
        }
    }

    /// The vertices.
    #[must_use]
    pub fn vertices(&self) -> &[Point2] {
        &self.vertices
    }

    /// The triangles, each three indices into [`Self::vertices`] in counterclockwise order.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }
}

/// Triangulates `region` for display after flattening its loops within `chord` (a distance)
/// and `angle` (radians of turning per segment), with at most `max_vertices` vertices in all.
///
/// # Errors
///
/// `geom.tolerance.invalid` when `chord` or `angle` is not finite and positive;
/// `geom.flatten.too_many_vertices` when the flattened loops need more than `max_vertices`
/// vertices (or more than `u32` indices can address); `geom.triangulate.loop_collapsed` when a
/// loop degenerates at these tolerances; `geom.triangulate.edges_cross` when flattened edges
/// cross, touch or fold back; `geom.triangulate.nesting_invalid` when a hole lies outside its
/// outer loop or parts overlap; `geom.precision.insufficient` when a coordinate is outside the
/// range given in the [module documentation](self) or an orientation cannot be decided.
pub fn triangulate(
    region: &Region,
    chord: f64,
    angle: f64,
    max_vertices: usize,
) -> Result<TriangleMesh2> {
    let limit = max_vertices.min(usize::try_from(u32::MAX).unwrap_or(usize::MAX));
    let loops = flatten_region(region, chord, angle, limit)?;
    let mut vertices = Vec::new();
    let mut edges = Vec::new();
    for points in &loops {
        let first = vertices.len();
        let count = points.len();
        for k in 0..count {
            edges.push(Edge {
                from: first + k,
                to: first + (k + 1) % count,
            });
        }
        vertices.extend_from_slice(points);
    }
    let triangles = constrained_delaunay(&vertices, &edges)?;
    Ok(TriangleMesh2 {
        vertices,
        triangles,
    })
}

/// [`triangulate`] with `tolerance.tessellation_chord`, `tolerance.tessellation_angle` and at
/// most `limits.array_length` vertices.
///
/// # Errors
///
/// As [`triangulate`].
pub fn triangulate_with(
    region: &Region,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<TriangleMesh2> {
    triangulate(
        region,
        tolerance.tessellation_chord,
        tolerance.tessellation_angle,
        usize::try_from(limits.array_length).unwrap_or(usize::MAX),
    )
}

fn too_many(limit: usize) -> Error {
    Error::new(codes::FLATTEN_TOO_MANY_VERTICES)
        .with_detail("limit", i64::try_from(limit).unwrap_or(i64::MAX))
}

/// The flattened loops of the region in order, with at most `limit` vertices in all.
pub(crate) fn flatten_region(
    region: &Region,
    chord: f64,
    angle: f64,
    limit: usize,
) -> Result<Vec<Vec<Point2>>> {
    let mut loops = Vec::new();
    let mut used = 0_usize;
    let all = region
        .parts()
        .iter()
        .flat_map(|part| std::iter::once(part.outer()).chain(part.holes()));
    for (index, lp) in all.enumerate() {
        let points = flatten_loop(lp, chord, angle, limit.saturating_sub(used)).map_err(
            |error| match error.code() {
                code if code == codes::FLATTEN_TOO_MANY_VERTICES.code => too_many(limit),
                _ => error,
            },
        )?;
        if points.len() < 3 || !same_orientation(&points, lp.signed_area()) {
            return Err(Error::new(codes::TRIANGULATE_LOOP_COLLAPSED)
                .with_detail("loop", i64::try_from(index).unwrap_or(i64::MAX)));
        }
        used += points.len();
        loops.push(points);
    }
    Ok(loops)
}

fn flatten_loop(lp: &Loop2, chord: f64, angle: f64, budget: usize) -> Result<Vec<Point2>> {
    let curves = lp.curves();
    let short = |index: usize| {
        matches!(curves.get(index % curves.len()), Some(Curve2::Segment(s))
            if crate::length(&(s.end() - s.start())) <= chord)
    };
    let mut out: Vec<Point2> = Vec::new();
    for (index, curve) in curves.iter().enumerate() {
        // An isolated segment within the chord tolerance, such as a joint closing a gap or a
        // rounding gap, is merged into the next curve, which starts within its length of the
        // previous curve's end; a chain of short segments is kept, as it may turn arbitrarily.
        if short(index) && !short(index + 1) && !short(index + curves.len() - 1) {
            continue;
        }
        // Room for the shared first vertex, which is dropped, and one more because flattening
        // checks a circular arc's count with one vertex to spare; the loop's own count is
        // enforced below.
        let room = budget.saturating_sub(out.len()).saturating_add(2);
        let points = curve.flatten(chord, angle, room)?;
        // The first vertex is the previous curve's end, exactly or to rounding.
        for p in points.into_iter().skip(usize::from(!out.is_empty())) {
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
        if out.len() > budget.saturating_add(1) {
            return Err(too_many(budget));
        }
    }
    // The last vertex is the end of the last curve, which closes onto the first vertex.
    out.pop();
    while out.len() > 1 && out.last() == out.first() {
        out.pop();
    }
    if out.len() > budget {
        return Err(too_many(budget));
    }
    Ok(out)
}

/// Whether the shoelace area of `points` is nonzero with the sign of `area`.
fn same_orientation(points: &[Point2], area: f64) -> bool {
    let Some(&origin) = points.first() else {
        return false;
    };
    let twice: f64 = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| crate::eval::cross(&(a - origin), &(b - origin)))
        .sum();
    twice != 0.0 && (twice > 0.0) == (area > 0.0)
}

/// A flattened loop edge between global vertex indices.
#[derive(Debug, Clone, Copy)]
struct Edge {
    from: usize,
    to: usize,
}

/// The triangles inside the loops of the constrained Delaunay triangulation of `vertices` with
/// the loop `edges` as constraints, after checking that the edges meet only at the shared
/// vertices of consecutive edges.
fn constrained_delaunay(vertices: &[Point2], edges: &[Edge]) -> Result<Vec<[u32; 3]>> {
    let crossing = |edge: usize| {
        Error::new(codes::TRIANGULATE_EDGES_CROSS)
            .with_detail("edge", i64::try_from(edge).unwrap_or(i64::MAX))
    };
    // Edge k starts at vertex k; `incoming[v]` is the edge that ends at vertex v.
    let mut incoming = vec![0_usize; vertices.len()];
    for (k, edge) in edges.iter().enumerate() {
        if let Some(slot) = incoming.get_mut(edge.to) {
            *slot = k;
        }
    }
    let mut cdt = ConstrainedDelaunayTriangulation::<spade::Point2<f64>>::new();
    let mut handles: Vec<Option<FixedVertexHandle>> = vec![None; vertices.len()];
    let mut inserted = 0_usize;
    // Vertices are inserted in order, so the result is deterministic, and each loop edge is
    // added as soon as both of its ends are present, so that it crosses few triangles.
    for (v, p) in vertices.iter().enumerate() {
        let handle = cdt.insert(spade::Point2::new(p.x, p.y)).map_err(|error| {
            Error::new(codes::PRECISION_INSUFFICIENT)
                .with_detail("triangulation", error.to_string())
        })?;
        // An equal vertex is merged into the earlier one: two loops share a vertex, or a loop
        // passes through one of its own vertices twice.
        inserted += 1;
        if cdt.num_vertices() != inserted {
            return Err(crossing(v));
        }
        if let Some(slot) = handles.get_mut(v) {
            *slot = Some(handle);
        }
        for k in [v, incoming.get(v).copied().unwrap_or(v)] {
            let Some(edge) = edges.get(k) else {
                continue;
            };
            let (Some(Some(from)), Some(Some(to))) = (handles.get(edge.from), handles.get(edge.to))
            else {
                continue;
            };
            // A constraint that would cross an earlier one is refused.
            if cdt.try_add_constraint(*from, *to).is_empty() {
                return Err(crossing(k));
            }
        }
    }
    let fixed = |i: usize| handles.get(i).copied().flatten();
    // A vertex on a loop edge, or an edge through a vertex, splits the edge; so does an edge
    // folding back over its neighbor or overlapping another edge.
    for (k, edge) in edges.iter().enumerate() {
        match (fixed(edge.from), fixed(edge.to)) {
            (Some(from), Some(to)) if cdt.exists_constraint(from, to) => {}
            _ => return Err(crossing(k)),
        }
    }
    let mut index_of = vec![0_u32; cdt.num_vertices()];
    for (i, handle) in handles.iter().enumerate() {
        if let Some(slot) = handle.and_then(|h| index_of.get_mut(h.index())) {
            *slot = u32::try_from(i).map_err(|_| too_many(i))?;
        }
    }
    // Flood fill from the outside, toggling the parity at every constraint edge.
    let mut inside: Vec<Option<bool>> = vec![None; cdt.num_all_faces()];
    let mut queue = VecDeque::new();
    for edge in cdt.convex_hull() {
        if let Some(face) = edge.rev().face().as_inner()
            && let Some(slot) = inside.get_mut(face.fix().index())
            && slot.is_none()
        {
            *slot = Some(edge.as_undirected().data().is_constraint_edge());
            queue.push_back(face.fix());
        }
    }
    while let Some(face) = queue.pop_front() {
        let parity = inside.get(face.index()).copied().flatten().unwrap_or(false);
        for edge in cdt.face(face).adjacent_edges() {
            if let Some(next) = edge.rev().face().as_inner()
                && let Some(slot) = inside.get_mut(next.fix().index())
                && slot.is_none()
            {
                *slot = Some(parity != edge.as_undirected().data().is_constraint_edge());
                queue.push_back(next.fix());
            }
        }
    }
    // Each loop edge must have the region on its left and the outside on its right; otherwise a
    // hole lies outside its outer loop or parts overlap, and the parity would be wrong.
    let is_inside = |face: spade::handles::FaceHandle<'_, _, _, _, _, _>| {
        face.as_inner()
            .and_then(|inner| inside.get(inner.fix().index()).copied().flatten())
            == Some(true)
    };
    for (k, edge) in edges.iter().enumerate() {
        let (Some(from), Some(to)) = (fixed(edge.from), fixed(edge.to)) else {
            return Err(crossing(k));
        };
        let nested = cdt
            .get_edge_from_neighbors(from, to)
            .is_some_and(|e| is_inside(e.face()) && !is_inside(e.rev().face()));
        if !nested {
            return Err(Error::new(codes::TRIANGULATE_NESTING_INVALID));
        }
    }
    Ok(cdt
        .inner_faces()
        .filter(|face| inside.get(face.fix().index()).copied().flatten() == Some(true))
        .map(|face| {
            face.vertices()
                .map(|v| index_of.get(v.fix().index()).copied().unwrap_or(0))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::f64::consts::PI;

    use arc_base::math::Vector2;
    use proptest::prelude::*;

    use super::*;
    use crate::closest::CurveProject;
    use crate::polyline::{Polyline2, PolylineSegment, PolylineVertex};
    use crate::predicates::incircle;
    use crate::primitives::{Arc2, Segment2};
    use crate::region::RegionPart;
    use crate::region::tests::{TOL, circle, part, polygon, region, square, stadium};

    fn code(result: Result<impl std::fmt::Debug>) -> &'static str {
        result.unwrap_err().code()
    }

    fn twice_area(a: &Point2, b: &Point2, c: &Point2) -> f64 {
        crate::eval::cross(&(b - a), &(c - a))
    }

    fn corners(mesh: &TriangleMesh2, t: &[u32; 3]) -> [Point2; 3] {
        t.map(|i| mesh.vertices()[i as usize])
    }

    /// Checks every acceptance property of a triangulation of `loops`.
    fn check(mesh: &TriangleMesh2, loops: &[Vec<Point2>]) {
        let expected_vertices: Vec<Point2> = loops.iter().flatten().copied().collect();
        assert_eq!(mesh.vertices(), expected_vertices.as_slice());
        // Area equality with the flattened region.
        let region_area: f64 = loops
            .iter()
            .map(|l| {
                l.iter()
                    .zip(l.iter().cycle().skip(1))
                    .map(|(a, b)| twice_area(&l[0], a, b))
                    .sum::<f64>()
            })
            .sum();
        let mesh_area: f64 = mesh
            .triangles()
            .iter()
            .map(|t| {
                let [a, b, c] = corners(mesh, t);
                twice_area(&a, &b, &c)
            })
            .sum();
        assert!(
            (mesh_area - region_area).abs() <= 1e-9 * region_area.abs(),
            "{mesh_area} != {region_area}"
        );
        // Counterclockwise, non-degenerate triangles, each directed edge used once.
        let mut directed = BTreeMap::new();
        for t in mesh.triangles() {
            let [a, b, c] = corners(mesh, t);
            assert_eq!(orient2d(&a, &b, &c).unwrap(), Sign::Positive);
            for k in 0..3 {
                let edge = (t[k], t[(k + 1) % 3]);
                assert!(directed.insert(edge, t[(k + 2) % 3]).is_none());
            }
        }
        // Every loop edge borders exactly one triangle, on the region side.
        let mut boundary = BTreeSet::new();
        let mut first = 0_u32;
        for l in loops {
            let n = u32::try_from(l.len()).unwrap();
            for k in 0..n {
                let edge = (first + k, first + (k + 1) % n);
                assert!(directed.contains_key(&edge), "missing {edge:?}");
                assert!(!directed.contains_key(&(edge.1, edge.0)));
                boundary.insert(edge);
            }
            first += n;
        }
        // Every other edge is shared by two triangles. With the loop edges bordered on their
        // region side only, the triangles form a surface whose whole boundary is the loops, so
        // no triangle lies in a hole or outside, decided exactly rather than by centroids; with
        // counterclockwise triangles and the area equality, none overlap.
        // Constrained Delaunay: no interior edge has the opposite vertex inside the circle.
        for (&(a, b), &c) in &directed {
            if boundary.contains(&(a, b)) || boundary.contains(&(b, a)) {
                continue;
            }
            let d = *directed
                .get(&(b, a))
                .expect("an interior edge has two triangles");
            let p = |i: u32| mesh.vertices()[i as usize];
            assert_ne!(
                incircle(&p(a), &p(b), &p(c), &p(d)).unwrap(),
                Sign::Positive
            );
        }
    }

    /// Checks independently of the flattening code that every flattened vertex and edge
    /// midpoint lies within `chord` of the region's boundary, by projecting onto every curve.
    fn check_flattening(region: &Region, loops: &[Vec<Point2>], chord: f64) {
        // Polylines are split into their segments, so the bounding boxes select few curves.
        let curves: Vec<Curve2> = region
            .parts()
            .iter()
            .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
            .flat_map(Loop2::curves)
            .flat_map(|c| match c {
                Curve2::Polyline(p) => (0..p.segment_count())
                    .map(|k| match p.segment(k).unwrap() {
                        PolylineSegment::Line(s) => Curve2::Segment(s),
                        PolylineSegment::Arc(a) => Curve2::Arc(a),
                    })
                    .collect(),
                other => vec![other.clone()],
            })
            .collect();
        for l in loops {
            for (a, b) in l.iter().zip(l.iter().cycle().skip(1)) {
                for p in [*a, Point2::from((a.coords + b.coords) * 0.5)] {
                    let distance = curves
                        .iter()
                        .filter(|c| {
                            c.bounding_box()
                                .is_some_and(|b| b.expanded(chord).contains_point(&p))
                        })
                        .map(|c| c.closest_point(p, &TOL).unwrap().distance)
                        .fold(f64::INFINITY, f64::min);
                    assert!(distance <= chord * (1.0 + 1e-9), "{p:?} is {distance} away");
                }
            }
        }
    }

    fn run(region: &Region, chord: f64, angle: f64) -> TriangleMesh2 {
        let mesh = triangulate(region, chord, angle, 1_000_000).unwrap();
        let loops = flatten_region(region, chord, angle, 1_000_000).unwrap();
        check(&mesh, &loops);
        check_flattening(region, &loops, chord);
        let json = serde_json::to_string(&mesh).unwrap();
        assert_eq!(serde_json::from_str::<TriangleMesh2>(&json).unwrap(), mesh);
        mesh
    }

    /// The pairwise reference for simplicity: whether two flattened edges meet anywhere except
    /// at the vertex that consecutive edges share, decided exactly.
    fn pairwise_conflict(loops: &[Vec<Point2>]) -> bool {
        let within = |a: &Point2, b: &Point2, p: &Point2| {
            a.x.min(b.x) <= p.x && p.x <= a.x.max(b.x) && a.y.min(b.y) <= p.y && p.y <= a.y.max(b.y)
        };
        let o = |a: &Point2, b: &Point2, c: &Point2| orient2d(a, b, c).unwrap();
        let mut edges = Vec::new();
        for l in loops {
            let first = edges.len();
            for k in 0..l.len() {
                edges.push((
                    first + k,
                    first + (k + 1) % l.len(),
                    l[k],
                    l[(k + 1) % l.len()],
                ));
            }
        }
        for (i, e) in edges.iter().enumerate() {
            for f in &edges[i + 1..] {
                let shared = if e.1 == f.0 {
                    Some((e.2, e.3, f.3))
                } else if f.1 == e.0 {
                    Some((f.2, f.3, e.3))
                } else {
                    None
                };
                let meet = match shared {
                    Some((u, v, w)) => {
                        o(&u, &v, &w) == Sign::Zero && (within(&v, &u, &w) || within(&v, &w, &u))
                    }
                    None => {
                        let (a, b, c, d) = (e.2, e.3, f.2, f.3);
                        let (o1, o2, o3, o4) =
                            (o(&a, &b, &c), o(&a, &b, &d), o(&c, &d, &a), o(&c, &d, &b));
                        let opposite =
                            |x: Sign, y: Sign| x != Sign::Zero && y != Sign::Zero && x != y;
                        (opposite(o1, o2) && opposite(o3, o4))
                            || (o1 == Sign::Zero && within(&a, &b, &c))
                            || (o2 == Sign::Zero && within(&a, &b, &d))
                            || (o3 == Sign::Zero && within(&c, &d, &a))
                            || (o4 == Sign::Zero && within(&c, &d, &b))
                    }
                };
                if meet {
                    return true;
                }
            }
        }
        false
    }

    /// A closed polyline loop through integer points, if they form one.
    fn grid_loop(points: &[(i32, i32)], bulge: f64) -> Option<Loop2> {
        let mut vertices: Vec<PolylineVertex> = points
            .iter()
            .map(|&(x, y)| PolylineVertex::new(Point2::new(f64::from(x), f64::from(y)), bulge))
            .collect();
        vertices.dedup_by(|a, b| a.point == b.point);
        let polyline = Polyline2::new(vertices, true).ok()?;
        Loop2::new(vec![Curve2::Polyline(polyline)], &TOL).ok()
    }

    /// Regions on a small integer grid, where collinear and coincident vertices, touching loops
    /// and holes outside their outer loop are common.
    fn grid_region() -> impl Strategy<Value = Region> {
        let outer = (
            prop::collection::vec(4_i32..12, 3..14),
            prop_oneof![Just(0.0), Just(0.25), Just(-0.25)],
        )
            .prop_filter_map("not a loop", |(radii, bulge)| {
                let n = radii.len();
                let points: Vec<(i32, i32)> = radii
                    .iter()
                    .enumerate()
                    .map(|(k, &r)| {
                        let a = std::f64::consts::TAU * k as f64 / n as f64;
                        let (sin, cos) = a.sin_cos();
                        #[expect(clippy::cast_possible_truncation, reason = "small values")]
                        (
                            (f64::from(r) * cos).round() as i32,
                            (f64::from(r) * sin).round() as i32,
                        )
                    })
                    .collect();
                grid_loop(&points, bulge)
            });
        let hole = (-4_i32..4, -4_i32..4, 1_i32..3, any::<bool>()).prop_map(|(x, y, s, round)| {
            if round {
                circle(f64::from(x), f64::from(y), f64::from(s) * 0.5)
            } else {
                square(f64::from(x), f64::from(y), f64::from(s) * 0.5)
            }
        });
        (outer, prop::collection::vec(hole, 0..4))
            .prop_filter_map("hole outside", |(o, h)| Region::new(vec![part(o, h)]).ok())
    }

    proptest! {
        #[test]
        fn simplicity_matches_the_pairwise_reference(
            r in grid_region(),
            chord in prop_oneof![Just(0.02), Just(0.1), Just(0.5), Just(2.0)],
        ) {
            let Ok(loops) = flatten_region(&r, chord, 0.4, 100_000) else {
                return Ok(());
            };
            let result = triangulate(&r, chord, 0.4, 100_000);
            let crosses = matches!(&result, Err(e) if e.code() == "geom.triangulate.edges_cross");
            prop_assert_eq!(crosses, pairwise_conflict(&loops), "{:?}", result.as_ref().err());
            if let Ok(mesh) = result {
                check(&mesh, &loops);
            }
        }

        #[test]
        fn triangulations_cover_the_flattened_region(
            r in region(),
            chord in 0.01..0.5_f64,
            angle in 0.1..0.8_f64,
        ) {
            let mesh = run(&r, chord, angle);
            prop_assert_eq!(triangulate(&r, chord, angle, 1_000_000).unwrap(), mesh);
        }
    }

    #[test]
    fn simple_regions_triangulate() {
        let one = |outer, holes| Region::new(vec![part(outer, holes)]).unwrap();
        let mesh = run(
            &one(square(0.0, 0.0, 2.0), vec![square(0.0, 0.0, 1.0)]),
            0.1,
            0.5,
        );
        assert_eq!((mesh.vertices().len(), mesh.triangles().len()), (8, 8));
        run(&one(circle(1.0, 2.0, 3.0), vec![]), 1e-3, 0.2);
        run(
            &one(circle(0.0, 0.0, 3.0), vec![circle(0.0, 0.0, 2.9)]),
            1e-3,
            0.2,
        );
        run(
            &one(circle(0.0, 0.0, 3.0), vec![circle(1.0, 0.5, 0.5)]),
            1e-2,
            0.3,
        );
        run(
            &one(stadium(Point2::new(5.0, 5.0), 3.0, 1.0), vec![]),
            1e-2,
            0.3,
        );
        run(
            &Region::new(vec![
                part(square(0.0, 0.0, 1.0), vec![]),
                part(circle(5.0, 0.0, 1.0), vec![square(5.0, 0.0, 0.25)]),
            ])
            .unwrap(),
            1e-2,
            0.3,
        );
        // An island inside a hole.
        run(
            &Region::new(vec![
                part(circle(0.0, 0.0, 10.0), vec![circle(0.0, 0.0, 5.0)]),
                part(square(0.0, 0.0, 1.0), vec![]),
            ])
            .unwrap(),
            1e-2,
            0.3,
        );
        // A loop of separate arcs and segments with joints across rounding gaps.
        let lens = Loop2::new(
            vec![
                Curve2::Arc(
                    Arc2::new(Point2::new(0.0, -1.0), 2.0, PI / 6.0, 2.0 * PI / 3.0).unwrap(),
                ),
                Curve2::Segment(
                    Segment2::new(Point2::new(-3.0_f64.sqrt(), 0.0), Point2::new(0.0, -0.5))
                        .unwrap(),
                ),
                Curve2::Segment(
                    Segment2::new(Point2::new(0.0, -0.5), Point2::new(3.0_f64.sqrt(), 0.0))
                        .unwrap(),
                ),
            ],
            &Tolerance {
                linear: 1e-9,
                ..TOL
            },
        )
        .unwrap();
        run(&one(lens.clone(), vec![]), 1e-3, 0.1);
        run(&one(lens.reversed(), vec![]), 1e-3, 0.1);
    }

    #[test]
    fn degenerate_inputs_fail_with_specific_errors() {
        let one = |outer, holes| Region::new(vec![part(outer, holes)]).unwrap();
        let ring = one(circle(0.0, 0.0, 10.0), vec![circle(5.586, 5.586, 2.0)]);
        // Fine flattening keeps the loops apart; coarse chords cross.
        run(&ring, 1e-3, 0.2);
        assert_eq!(
            code(triangulate(&ring, 3.0, 2.0, 1000)),
            "geom.triangulate.edges_cross"
        );
        // A hole touching the outer loop at a vertex.
        let touching = one(
            square(0.0, 0.0, 2.0),
            vec![polygon(&[(-2.0, -2.0), (0.0, -1.0), (-1.0, 0.0)], 0.0)],
        );
        assert_eq!(
            code(triangulate(&touching, 0.1, 0.5, 1000)),
            "geom.triangulate.edges_cross"
        );
        // A hole whose bounding box is inside but which lies in the notch of an L.
        let l = polygon(
            &[
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 4.0),
                (4.0, 4.0),
                (4.0, 10.0),
                (0.0, 10.0),
            ],
            0.0,
        );
        let notch = one(l, vec![square(7.0, 7.0, 1.0)]);
        assert_eq!(
            code(triangulate(&notch, 0.1, 0.5, 1000)),
            "geom.triangulate.nesting_invalid"
        );
        // A circle flattened coarser than its diameter.
        let tiny = one(square(0.0, 0.0, 2.0), vec![circle(0.0, 0.0, 0.1)]);
        assert_eq!(
            code(triangulate(&tiny, 1.0, 10.0, 1000)),
            "geom.triangulate.loop_collapsed"
        );
        assert_eq!(
            code(triangulate(&tiny, 0.0, 0.5, 1000)),
            "geom.tolerance.invalid"
        );
        assert_eq!(
            code(triangulate(&tiny, 0.1, f64::NAN, 1000)),
            "geom.tolerance.invalid"
        );
        // Vertex limits.
        let disc = one(circle(0.0, 0.0, 1.0), vec![]);
        let needed = triangulate(&disc, 1e-3, 0.2, 1000)
            .unwrap()
            .vertices()
            .len();
        assert!(triangulate(&disc, 1e-3, 0.2, needed).is_ok());
        assert_eq!(
            code(triangulate(&disc, 1e-3, 0.2, needed - 1)),
            "geom.flatten.too_many_vertices"
        );
        let two = one(square(0.0, 0.0, 2.0), vec![square(0.0, 0.0, 1.0)]);
        assert_eq!(
            code(triangulate(&two, 0.1, 0.5, 7)),
            "geom.flatten.too_many_vertices"
        );
        // Coordinates beyond the range of the triangulation's arithmetic.
        let huge = one(square(0.0, 0.0, 1e61), vec![]);
        assert_eq!(
            code(triangulate(&huge, 1e58, 0.5, 1000)),
            "geom.precision.insufficient"
        );
        let defaults = triangulate_with(&disc, &TOL, &Limits::DEFAULT).unwrap();
        assert_eq!(
            defaults,
            triangulate(&disc, 1e-3, PI / 12.0, 50_000_000).unwrap()
        );
    }

    #[test]
    fn meshes_validate_and_round_trip() {
        let p = |x, y| Point2::new(x, y);
        let vertices = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)];
        let mesh = TriangleMesh2::new(vertices.clone(), vec![[0, 1, 2]]).unwrap();
        let json = serde_json::to_string(&mesh).unwrap();
        assert_eq!(serde_json::from_str::<TriangleMesh2>(&json).unwrap(), mesh);
        assert_eq!(
            code(TriangleMesh2::new(vertices, vec![[0, 1, 3]])),
            "geom.mesh.invalid"
        );
        assert_eq!(
            code(TriangleMesh2::new(vec![p(f64::NAN, 0.0)], vec![])),
            "geom.mesh.invalid"
        );
        let bad = r#"{"vertices":[[0.0,0.0]],"triangles":[[0,0,1]]}"#;
        assert!(serde_json::from_str::<TriangleMesh2>(bad).is_err());
        let _ = Vector2::x();
        let _: Option<RegionPart> = None;
    }

    #[test]
    fn short_segment_curves_are_kept() {
        let p = |x: f64, y: f64| Point2::new(x, y);
        let segments = |points: &[Point2]| {
            let curves = (0..points.len())
                .map(|k| {
                    let next = points[(k + 1) % points.len()];
                    Curve2::Segment(Segment2::new(points[k], next).unwrap())
                })
                .collect();
            Loop2::new(curves, &TOL).unwrap()
        };
        let one = |outer| Region::new(vec![part(outer, vec![])]).unwrap();
        // Segments no longer than the chord.
        let unit = segments(&[p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)]);
        assert_eq!(run(&one(unit), 1.0, 0.5).vertices().len(), 4);
        // A staircase of 0.5 mm steps at the default chord of 1 mm.
        let mut stairs = vec![p(0.0, 0.0)];
        for k in 1..=200 {
            let step = 5e-4 * f64::from(k);
            stairs.extend([p(step, step - 5e-4), p(step, step)]);
        }
        stairs.push(p(0.0, 0.1));
        let mesh = run(&one(segments(&stairs)), 1e-3, PI / 12.0);
        // Joints of a unit in the last place between arcs and segments are merged, so they
        // cannot fold back over their neighbors.
        run(&one(stadium(Point2::origin(), 4.0, 6.0)), 0.05, 0.3);
        run(
            &one(stadium(Point2::origin(), 4.0, 6.0).reversed()),
            0.05,
            0.3,
        );
        assert_eq!(mesh.vertices().len(), stairs.len());
        // A 9 mm bump of 0.9 mm segments on the side of a square, at a chord of 1 mm.
        let mut bump = vec![p(0.0, 0.0), p(5.0, 0.0)];
        bump.extend((1..=10).map(|k| p(5.0, 9e-4 * f64::from(k))));
        bump.extend((0..10).rev().map(|k| p(5.005, 9e-4 * f64::from(k))));
        bump.extend([p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]);
        let bumped = segments(&bump);
        let area = bumped.signed_area();
        let mesh = run(&one(bumped), 1e-3, PI / 12.0);
        let meshed: f64 = mesh
            .triangles()
            .iter()
            .map(|t| {
                let [a, b, c] = corners(&mesh, t);
                0.5 * twice_area(&a, &b, &c)
            })
            .sum();
        assert!((meshed - area).abs() < 1e-12);
    }

    /// A comb of `teeth` long slanted teeth: each tooth edge's bounding box overlaps those of
    /// all the other teeth.
    fn slanted_comb(teeth: usize) -> Region {
        let n = teeth as f64;
        let mut points = vec![(0.0, -1.0), (n, -1.0)];
        for i in (0..teeth).rev() {
            let x = i as f64;
            points.extend([
                (x + 0.6, 0.0),
                (x + 0.6 + n, 100.0),
                (x + 0.2 + n, 100.0),
                (x + 0.2, 0.0),
            ]);
        }
        Region::new(vec![part(polygon(&points, 0.0), vec![])]).unwrap()
    }

    #[test]
    fn long_slanted_edges_are_cheap() {
        let comb = slanted_comb(250);
        let mesh = triangulate(&comb, 0.01, 0.3, 10_000).unwrap();
        check(&mesh, &flatten_region(&comb, 0.01, 0.3, 10_000).unwrap());
        assert_eq!(mesh.triangles().len(), 1000);
    }

    /// A 20,002-vertex slanted comb took minutes when every pair of edges with overlapping
    /// bounding boxes was tested and every edge was added after all vertices; it now takes about
    /// 2 s in a debug build, so the bound is generous.
    #[test]
    fn a_large_slanted_comb_triangulates_quickly() {
        let comb = slanted_comb(5000);
        let start = std::time::Instant::now();
        let mesh = triangulate(&comb, 0.01, 0.3, 1_000_000).unwrap();
        assert_eq!(mesh.triangles().len(), 20_000);
        assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
    }

    #[test]
    fn hostile_meshes_are_rejected() {
        let p = |x, y| Point2::new(x, y);
        let vertices = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0), p(2.0, 0.0)];
        for t in [[0, 2, 1], [0, 0, 1], [0, 1, 3], [1, 1, 1]] {
            assert_eq!(
                code(TriangleMesh2::new(vertices.clone(), vec![[0, 1, 2], t])),
                "geom.mesh.invalid",
                "{t:?}"
            );
        }
        let json = r#"{"vertices":[[0.0,0.0],[1.0,0.0],[0.0,1.0]],"triangles":[[0,2,1]]}"#;
        assert!(serde_json::from_str::<TriangleMesh2>(json).is_err());
    }
}
