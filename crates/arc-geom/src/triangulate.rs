//! Polygon triangulation of regions for display (CORE-GEOM-022).
//!
//! [`triangulate`] flattens every loop of a [`Region`] with [`CurveFlatten`] at the supplied
//! chord and angle tolerances and returns the constrained Delaunay triangulation of the
//! flattened loops, restricted to the region.
//!
//! - **Flattening.** Each curve of a loop is flattened in order; the first vertex of each curve
//!   after the first is the previous curve's end, so it is dropped, and so is the closing
//!   vertex. Segment curves no longer than the chord tolerance, such as the joints that close
//!   construction gaps, are skipped. Where a joint or a rounding gap left by reversal or a
//!   transform separates consecutive curves, the flattened loop therefore joins at the end of
//!   the earlier curve, within the chord tolerance of the boundary. Consecutive vertices that
//!   are exactly equal are merged. A flattened loop with fewer than three vertices, or whose
//!   shoelace area is zero or has the opposite sign of the loop's exact area, fails with
//!   `geom.triangulate.loop_collapsed`: the tolerances are too coarse for that loop.
//! - **Simplicity.** Before triangulating, every pair of flattened edges whose bounding boxes
//!   meet is tested exactly with [`orient2d`]: edges that cross, edges that touch away from a
//!   shared vertex (a vertex on another edge, or two loops sharing a vertex) and adjacent edges
//!   that fold back over each other fail with `geom.triangulate.edges_cross`. This catches loops
//!   that cross, and loops that do not but whose chords cross after coarse flattening.
//! - **Triangulation.** The vertices and the loop edges as constraints form a constrained
//!   Delaunay triangulation. Among all triangulations of the flattened vertices that contain
//!   the loop edges, it maximizes the smallest angle, so no triangle is thinner than the
//!   vertices and edges force. No vertices are added, so thin triangles remain where the
//!   flattened boundary itself forces them; this is the meaning of "where the input allows".
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
use spade::{ConstrainedDelaunayTriangulation, Triangulation};

use crate::bbox::Aabb2;
use crate::curve::Curve2;
use crate::error::codes;
use crate::flatten::CurveFlatten;
use crate::index::SpatialIndex2;
use crate::predicates::{Sign, orient2d};
use crate::region::{Loop2, Region};

/// An indexed triangle mesh in the plane: vertices and counterclockwise triangles of vertex
/// indices.
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
    /// `geom.mesh.invalid` when a vertex is not finite or a triangle refers to an index
    /// outside the vertex list.
    pub fn new(vertices: Vec<Point2>, triangles: Vec<[u32; 3]>) -> Result<Self> {
        let finite = vertices.iter().all(|p| p.x.is_finite() && p.y.is_finite());
        let indexed = triangles
            .iter()
            .flatten()
            .all(|&i| usize::try_from(i).is_ok_and(|i| i < vertices.len()));
        if finite && indexed {
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
        for (k, (&a, &b)) in points.iter().zip(points.iter().cycle().skip(1)).enumerate() {
            edges.push(Edge {
                from: first + k,
                to: first + (k + 1) % count,
                a,
                b,
            });
        }
        vertices.extend_from_slice(points);
    }
    ensure_simple(&edges)?;
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
    let mut out: Vec<Point2> = Vec::new();
    for curve in lp.curves() {
        // A straight curve within the chord tolerance, such as a joint across a rounding gap,
        // is dropped: the next curve then starts at the previous curve's end.
        if let Curve2::Segment(segment) = curve
            && crate::length(&(segment.end() - segment.start())) <= chord
        {
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
    a: Point2,
    b: Point2,
}

impl Edge {
    fn bounding_box(&self) -> Aabb2 {
        Aabb2::from_corners(self.a, self.b)
    }
}

/// Fails with `geom.triangulate.edges_cross` unless the edges meet only at the shared vertices
/// of consecutive edges.
fn ensure_simple(edges: &[Edge]) -> Result<()> {
    let index =
        SpatialIndex2::from_entries(edges.iter().enumerate().map(|(k, e)| (k, e.bounding_box())))?;
    for (k, e) in edges.iter().enumerate() {
        for j in index.crossing(&e.bounding_box())? {
            let Some(f) = edges.get(j).filter(|_| j > k) else {
                continue;
            };
            if conflict(e, f)? {
                return Err(Error::new(codes::TRIANGULATE_EDGES_CROSS)
                    .with_detail("edge", i64::try_from(k).unwrap_or(i64::MAX))
                    .with_detail("other", i64::try_from(j).unwrap_or(i64::MAX)));
            }
        }
    }
    Ok(())
}

/// Whether two edges meet anywhere except at a vertex they share.
fn conflict(e: &Edge, f: &Edge) -> Result<bool> {
    let shared = if e.to == f.from {
        Some((e.a, e.b, f.b))
    } else if f.to == e.from {
        Some((f.a, f.b, e.b))
    } else {
        None
    };
    match shared {
        // Consecutive edges u→v→w meet elsewhere only when they fold back along one line.
        Some((u, v, w)) => Ok(orient2d(&u, &v, &w)? == Sign::Zero
            && (within_box(&v, &u, &w) || within_box(&v, &w, &u))),
        None => segments_meet(&e.a, &e.b, &f.a, &f.b),
    }
}

/// Whether `p` lies in the closed bounding box of `a` and `b`.
fn within_box(a: &Point2, b: &Point2, p: &Point2) -> bool {
    a.x.min(b.x) <= p.x && p.x <= a.x.max(b.x) && a.y.min(b.y) <= p.y && p.y <= a.y.max(b.y)
}

/// Whether the closed segments `ab` and `cd` have a point in common, decided exactly.
fn segments_meet(a: &Point2, b: &Point2, c: &Point2, d: &Point2) -> Result<bool> {
    let (o1, o2) = (orient2d(a, b, c)?, orient2d(a, b, d)?);
    let (o3, o4) = (orient2d(c, d, a)?, orient2d(c, d, b)?);
    let opposite = |x: Sign, y: Sign| x != Sign::Zero && y != Sign::Zero && x != y;
    Ok((opposite(o1, o2) && opposite(o3, o4))
        || (o1 == Sign::Zero && within_box(a, b, c))
        || (o2 == Sign::Zero && within_box(a, b, d))
        || (o3 == Sign::Zero && within_box(c, d, a))
        || (o4 == Sign::Zero && within_box(c, d, b)))
}

/// The triangles inside the loops of the constrained Delaunay triangulation of `vertices` with
/// `edges`, which [`ensure_simple`] has accepted.
fn constrained_delaunay(vertices: &[Point2], edges: &[Edge]) -> Result<Vec<[u32; 3]>> {
    let crossing = || Error::new(codes::TRIANGULATE_EDGES_CROSS);
    let mut cdt = ConstrainedDelaunayTriangulation::<spade::Point2<f64>>::new();
    let mut handles = Vec::with_capacity(vertices.len());
    for p in vertices {
        let handle = cdt.insert(spade::Point2::new(p.x, p.y)).map_err(|error| {
            Error::new(codes::PRECISION_INSUFFICIENT)
                .with_detail("triangulation", error.to_string())
        })?;
        handles.push(handle);
    }
    // Equal vertices would be merged into one; the simplicity check excludes them.
    if cdt.num_vertices() != vertices.len() {
        return Err(crossing());
    }
    for edge in edges {
        let (Some(&from), Some(&to)) = (handles.get(edge.from), handles.get(edge.to)) else {
            return Err(crossing());
        };
        // An edge that would cross a constraint is refused, and one through a vertex is split.
        if cdt.try_add_constraint(from, to).is_empty() || !cdt.exists_constraint(from, to) {
            return Err(crossing());
        }
    }
    let mut index_of = vec![0_u32; cdt.num_vertices()];
    for (i, handle) in handles.iter().enumerate() {
        if let Some(slot) = index_of.get_mut(handle.index()) {
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
    for edge in edges {
        let (Some(&from), Some(&to)) = (handles.get(edge.from), handles.get(edge.to)) else {
            return Err(crossing());
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

    /// Exact even-odd containment in the flattened loops, with the half-open rule.
    fn inside(loops: &[Vec<Point2>], p: &Point2) -> bool {
        let mut odd = false;
        for points in loops {
            for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
                if (a.y > p.y) != (b.y > p.y) {
                    let side = orient2d(a, b, p).unwrap();
                    let up = b.y > a.y;
                    if (up && side == Sign::Positive) || (!up && side == Sign::Negative) {
                        odd = !odd;
                    }
                }
            }
        }
        odd
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
        // Counterclockwise, non-degenerate triangles whose centroids are inside.
        let mut directed = BTreeMap::new();
        for t in mesh.triangles() {
            let [a, b, c] = corners(mesh, t);
            assert_eq!(orient2d(&a, &b, &c).unwrap(), Sign::Positive);
            let centroid = Point2::from((a.coords + b.coords + c.coords) / 3.0);
            assert!(inside(loops, &centroid));
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
        // Constrained Delaunay: no interior edge has the opposite vertex inside the circle.
        for (&(a, b), &c) in &directed {
            if boundary.contains(&(a, b)) || boundary.contains(&(b, a)) {
                continue;
            }
            let d = directed[&(b, a)];
            let p = |i: u32| mesh.vertices()[i as usize];
            assert_ne!(
                incircle(&p(a), &p(b), &p(c), &p(d)).unwrap(),
                Sign::Positive
            );
        }
    }

    fn run(region: &Region, chord: f64, angle: f64) -> TriangleMesh2 {
        let mesh = triangulate(region, chord, angle, 1_000_000).unwrap();
        let loops = flatten_region(region, chord, angle, 1_000_000).unwrap();
        check(&mesh, &loops);
        mesh
    }

    proptest! {
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
}
