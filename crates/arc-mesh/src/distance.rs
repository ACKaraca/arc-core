//! Distances between a point and a mesh and between two meshes (CORE-MESH-021).
//!
//! - **Point to triangle.** The closest point is found by the Voronoi regions of the triangle
//!   (vertices, edges, face). A degenerate triangle, where the face region is numerically
//!   unusable, falls back to the closest point on its three edges. Coordinates are translated
//!   to the query point and scaled by a power of two first, so any finite magnitude works.
//! - **Triangle to triangle.** Two triangles intersect when an edge of one crosses the other
//!   within the linear tolerance; the distance is then exactly 0 and the two points are the
//!   crossing point and its closest point on the other triangle (at most the tolerance apart).
//!   Otherwise the distance is the least of the six vertex-to-triangle distances and nine
//!   edge-to-edge distances, which is exact for triangles that do not intersect (coplanar
//!   overlap included, through a vertex inside the other triangle or crossing edges).
//! - **Mesh to mesh.** Both hierarchies are walked together, visiting the nearer node pair
//!   first and skipping pairs whose box distance exceeds the best distance. Among equally
//!   distant pairs of positive distance the lexicographically smallest `(triangle of a,
//!   triangle of b)` wins; once a distance of 0 is found the walk stops, so a zero result
//!   reports the first intersecting pair of the walk, which is the same for the same meshes.
//!   The number of node and triangle pairs visited is bounded by `64 · limits.array_length`.

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::bvh::MeshBvh;
use crate::error::codes;
use crate::topo::{Budget, get, power_of_two_scale};

/// The relative slack of box distance bounds.
const SLACK: f64 = 16.0 * f64::EPSILON;

/// The closest points of two meshes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshPair {
    /// The triangle of the first and of the second mesh.
    pub triangles: [u32; 2],
    /// The closest point on the first and on the second mesh.
    pub points: [Point3; 2],
    /// The distance; 0 when the meshes intersect (see the module documentation).
    pub distance: f64,
    /// Whether the meshes intersect, that is whether the distance is 0.
    pub intersecting: bool,
}

pub(crate) struct TriClosest {
    pub(crate) barycentric: [f64; 3],
    pub(crate) point: Vector3,
    pub(crate) distance: f64,
}

/// The largest power of two not above the largest absolute coordinate of the vectors; 1 for
/// all zeros; infinite when a coordinate is not finite.
pub(crate) fn scale_of(vectors: &[Vector3]) -> f64 {
    let mut m = 0.0_f64;
    for v in vectors {
        for c in [v.x, v.y, v.z] {
            if !c.is_finite() {
                return f64::INFINITY;
            }
            m = m.max(c.abs());
        }
    }
    if m == 0.0 { 1.0 } else { power_of_two_scale(m) }
}

/// The parameter in [0, 1] of the point of segment `a..b` closest to the origin.
fn segment_parameter(a: &Vector3, b: &Vector3) -> f64 {
    let ab = b - a;
    let len2 = ab.dot(&ab);
    if len2 > 0.0 {
        (-a.dot(&ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Barycentric coordinates of the point of triangle `a b c` closest to the origin; the
/// coordinates are of moderate magnitude (at most 2 in absolute value).
fn closest_to_origin(a: &Vector3, b: &Vector3, c: &Vector3) -> [f64; 3] {
    let (ab, ac) = (b - a, c - a);
    let (d1, d2) = (ab.dot(&-a), ac.dot(&-a));
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let (d3, d4) = (ab.dot(&-b), ac.dot(&-b));
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return checked([1.0 - v, v, 0.0], a, b, c);
    }
    let (d5, d6) = (ab.dot(&-c), ac.dot(&-c));
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return checked([1.0 - w, 0.0, w], a, b, c);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return checked([0.0, 1.0 - w, w], a, b, c);
    }
    let sum = va + vb + vc;
    let (v, w) = (vb / sum, vc / sum);
    checked([1.0 - v - w, v, w], a, b, c)
}

/// `bary` when it is a valid convex combination, otherwise the closest point on the edges (for
/// degenerate triangles, where the regions are numerically unusable).
fn checked(bary: [f64; 3], a: &Vector3, b: &Vector3, c: &Vector3) -> [f64; 3] {
    let valid = bary
        .iter()
        .all(|w| w.is_finite() && *w >= -SLACK && *w <= 1.0 + SLACK);
    if valid {
        return bary.map(|w| w.clamp(0.0, 1.0));
    }
    let mut best = ([1.0, 0.0, 0.0], f64::INFINITY);
    for (i, (p, q)) in [(a, b), (b, c), (c, a)].into_iter().enumerate() {
        let t = segment_parameter(p, q);
        let d = (p + (q - p) * t).norm();
        if d < best.1 {
            let mut w = [0.0; 3];
            if let Some(x) = w.get_mut(i) {
                *x = 1.0 - t;
            }
            if let Some(x) = w.get_mut((i + 1) % 3) {
                *x = t;
            }
            best = (w, d);
        }
    }
    best.0
}

/// The point of triangle `tri` closest to `p`.
pub(crate) fn closest_on_triangle(p: &Vector3, tri: [Vector3; 3]) -> Result<TriClosest> {
    let rel = tri.map(|v| v - p);
    let scale = scale_of(&rel);
    if !scale.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let s = rel.map(|v| v / scale);
    let barycentric = closest_to_origin(&s[0], &s[1], &s[2]);
    let q = s[0] * barycentric[0] + s[1] * barycentric[1] + s[2] * barycentric[2];
    let distance = q.norm() * scale;
    let point = p + q * scale;
    if !distance.is_finite() || !point.iter().all(|c| c.is_finite()) {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    Ok(TriClosest {
        barycentric,
        point,
        distance,
    })
}

/// The points of segments `p1..q1` and `p2..q2` closest to each other.
fn segment_segment(p1: &Vector3, q1: &Vector3, p2: &Vector3, q2: &Vector3) -> (Vector3, Vector3) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.dot(&d1), d2.dot(&d2), d2.dot(&r));
    let (s, t);
    if a <= 0.0 && e <= 0.0 {
        (s, t) = (0.0, 0.0);
    } else if a <= 0.0 {
        (s, t) = (0.0, (f / e).clamp(0.0, 1.0));
    } else {
        let c = d1.dot(&r);
        if e <= 0.0 {
            (s, t) = ((-c / a).clamp(0.0, 1.0), 0.0);
        } else {
            let b = d1.dot(&d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 0.0 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t) = (s0, t0);
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

/// The point where segment `s0..s1` crosses triangle `tri` within `tol`, with the closest
/// point of the triangle to it.
fn segment_crosses(
    s0: &Vector3,
    s1: &Vector3,
    tri: &[Vector3; 3],
    tol: f64,
) -> Option<(Vector3, Vector3)> {
    let normal = (tri[1] - tri[0]).cross(&(tri[2] - tri[0]));
    let (d0, d1) = (normal.dot(&(s0 - tri[0])), normal.dot(&(s1 - tri[0])));
    if (d0 > 0.0 && d1 > 0.0) || (d0 < 0.0 && d1 < 0.0) || d0 == d1 {
        return None;
    }
    let x = s0 + (s1 - s0) * (d0 / (d0 - d1));
    let bary = closest_to_origin(&(tri[0] - x), &(tri[1] - x), &(tri[2] - x));
    let q = tri[0] * bary[0] + tri[1] * bary[1] + tri[2] * bary[2];
    ((x - q).norm() <= tol).then_some((x, q))
}

/// The distance between two triangles and a pair of closest points.
pub(crate) fn triangle_pair(
    a: [Vector3; 3],
    b: [Vector3; 3],
    tolerance: f64,
) -> Result<(f64, [Vector3; 2])> {
    let origin = a[0];
    let (ra, rb) = (a.map(|v| v - origin), b.map(|v| v - origin));
    let all: Vec<Vector3> = ra.iter().chain(&rb).copied().collect();
    let scale = scale_of(&all);
    if !scale.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let (sa, sb) = (ra.map(|v| v / scale), rb.map(|v| v / scale));
    let tol = tolerance / scale;
    let back = |p: Vector3| origin + p * scale;
    for i in 0..3 {
        let (p, q) = (sa[i], sa[(i + 1) % 3]);
        if let Some((x, y)) = segment_crosses(&p, &q, &sb, tol) {
            return Ok((0.0, [back(x), back(y)]));
        }
        let (p, q) = (sb[i], sb[(i + 1) % 3]);
        if let Some((x, y)) = segment_crosses(&p, &q, &sa, tol) {
            return Ok((0.0, [back(y), back(x)]));
        }
    }
    let mut best = (f64::INFINITY, [Vector3::zeros(); 2]);
    let mut consider = |pa: Vector3, pb: Vector3| {
        let d = (pa - pb).norm();
        if d < best.0 {
            best = (d, [pa, pb]);
        }
    };
    for v in &sa {
        let w = closest_to_origin(&(sb[0] - v), &(sb[1] - v), &(sb[2] - v));
        consider(*v, sb[0] * w[0] + sb[1] * w[1] + sb[2] * w[2]);
    }
    for v in &sb {
        let w = closest_to_origin(&(sa[0] - v), &(sa[1] - v), &(sa[2] - v));
        consider(sa[0] * w[0] + sa[1] * w[1] + sa[2] * w[2], *v);
    }
    for i in 0..3 {
        for j in 0..3 {
            let (x, y) = segment_segment(&sa[i], &sa[(i + 1) % 3], &sb[j], &sb[(j + 1) % 3]);
            consider(x, y);
        }
    }
    let distance = best.0 * scale;
    if !distance.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    Ok((distance, [back(best.1[0]), back(best.1[1])]))
}

/// A lower bound of the distance between the boxes of two nodes.
fn box_gap(a: &crate::bvh::Node, b: &crate::bvh::Node) -> f64 {
    let gap = (a.min - b.max).sup(&(b.min - a.max)).sup(&Vector3::zeros());
    crate::length(&gap) * (1.0 - SLACK)
}

/// The closest points of two meshes, by walking their hierarchies together.
///
/// # Errors
///
/// `mesh.tolerance.invalid` for an invalid tolerance; `mesh.limit.exceeded` when the walk
/// exceeds its bound; `mesh.precision.insufficient` when a distance is not representable.
pub fn closest_between(
    a: &MeshBvh<'_>,
    b: &MeshBvh<'_>,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<MeshPair> {
    crate::ensure_tolerance(tolerance)?;
    let mut budget = Budget::new(limits, 64);
    let mut best: Option<(f64, [u32; 2], [Vector3; 2])> = None;
    let mut stack = vec![(0_usize, 0_usize)];
    while let Some((i, j)) = stack.pop() {
        budget.spend(1)?;
        let (na, nb) = (a.node(i), b.node(j));
        if let Some((d, ..)) = best {
            if d == 0.0 {
                break;
            }
            if box_gap(&na, &nb) > d {
                continue;
            }
        }
        match (a.leaf(&na), b.leaf(&nb)) {
            (Some(ta), Some(tb)) => {
                for &x in ta {
                    for &y in tb {
                        budget.spend(1)?;
                        let (d, p) = triangle_pair(a.corners(x), b.corners(y), tolerance.linear)?;
                        let better = best.is_none_or(|(bd, bt, _)| (d, x, y) < (bd, bt[0], bt[1]));
                        if better {
                            best = Some((d, [x, y], p));
                        }
                    }
                }
            }
            _ => {
                // Split the node with the larger box (a leaf cannot be split).
                let split_a = match (a.children(i), b.children(j)) {
                    (Some(_), None) => true,
                    (None, _) => false,
                    (Some(_), Some(_)) => {
                        crate::length(&(na.max - na.min)) >= crate::length(&(nb.max - nb.min))
                    }
                };
                let pairs: [(usize, usize); 2] = if split_a {
                    let (l, r) = a.children(i).unwrap_or((i, i));
                    [(l, j), (r, j)]
                } else {
                    let (l, r) = b.children(j).unwrap_or((j, j));
                    [(i, l), (i, r)]
                };
                let gaps = pairs.map(|(x, y)| box_gap(&a.node(x), &b.node(y)));
                // The nearer pair is popped first; ties keep the lower node first.
                if get(&gaps, 0) <= get(&gaps, 1) {
                    stack.push(pairs[1]);
                    stack.push(pairs[0]);
                } else {
                    stack.push(pairs[0]);
                    stack.push(pairs[1]);
                }
            }
        }
    }
    let (distance, triangles, points) = best.ok_or_else(|| Error::new(codes::QUERY_EMPTY))?;
    Ok(MeshPair {
        triangles,
        points: points.map(Point3::from),
        distance,
        intersecting: distance == 0.0,
    })
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "test values are small"
)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::TriMesh;
    use crate::testing::{cube, sphere};

    const LIMITS: Limits = Limits::DEFAULT;
    const TOL: Tolerance = Tolerance::DEFAULT;

    fn v(x: f64, y: f64, z: f64) -> Vector3 {
        Vector3::new(x, y, z)
    }

    fn shifted(m: &TriMesh, d: Vector3) -> TriMesh {
        TriMesh::new(
            m.positions().iter().map(|p| p + d).collect(),
            m.triangles().to_vec(),
            &LIMITS,
        )
        .unwrap()
    }

    #[test]
    fn point_triangle_regions() {
        let tri = [v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(0.0, 2.0, 0.0)];
        let face = closest_on_triangle(&v(0.5, 0.5, 3.0), tri).unwrap();
        assert!((face.distance - 3.0).abs() < 1e-15);
        assert!((face.barycentric[1] - 0.25).abs() < 1e-15);
        let edge = closest_on_triangle(&v(3.0, 3.0, 0.0), tri).unwrap();
        assert!((edge.point - v(1.0, 1.0, 0.0)).norm() < 1e-15);
        assert!((edge.distance - 8.0_f64.sqrt()).abs() < 1e-15);
        let vertex = closest_on_triangle(&v(-1.0, -1.0, 0.0), tri).unwrap();
        assert_eq!(vertex.barycentric, [1.0, 0.0, 0.0]);
        // A degenerate triangle: three collinear points.
        let line = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(3.0, 0.0, 0.0)];
        let d = closest_on_triangle(&v(2.0, 1.0, 0.0), line).unwrap();
        assert!((d.distance - 1.0).abs() < 1e-12 && (d.point.x - 2.0).abs() < 1e-12);
    }

    #[test]
    fn triangle_pairs_with_known_distances() {
        let a = [v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)];
        // Parallel and above.
        let b = a.map(|p| p + v(0.0, 0.0, 2.0));
        assert!((triangle_pair(a, b, 1e-9).unwrap().0 - 2.0).abs() < 1e-15);
        // Skew edges: a vertical segment-like triangle above the hypotenuse.
        let c = [v(0.5, 0.5, 1.0), v(0.5, 0.5, 2.0), v(1.5, 1.5, 1.5)];
        let (d, p) = triangle_pair(a, c, 1e-9).unwrap();
        assert!((d - 1.0).abs() < 1e-12, "{d}");
        assert!((p[0] - v(0.5, 0.5, 0.0)).norm() < 1e-12);
        // Piercing: an edge of one passes through the interior of the other.
        let pierce = [v(0.2, 0.2, -1.0), v(0.2, 0.2, 1.0), v(5.0, 5.0, 3.0)];
        let (d, p) = triangle_pair(a, pierce, 1e-9).unwrap();
        assert_eq!(d, 0.0);
        assert!((p[0] - p[1]).norm() < 1e-9);
        // Coplanar overlap.
        let overlap = a.map(|p| p + v(0.25, 0.25, 0.0));
        assert_eq!(triangle_pair(a, overlap, 1e-9).unwrap().0, 0.0);
    }

    #[test]
    fn mesh_distance_and_intersection() {
        let a = cube();
        let b = shifted(&a, v(3.0, 0.0, 0.0));
        let (ba, bb) = (
            MeshBvh::build(&a, &LIMITS).unwrap(),
            MeshBvh::build(&b, &LIMITS).unwrap(),
        );
        let r = closest_between(&ba, &bb, &TOL, &LIMITS).unwrap();
        assert!((r.distance - 2.0).abs() < 1e-12 && !r.intersecting);
        assert!((r.points[0].x - 1.0).abs() < 1e-12 && (r.points[1].x - 3.0).abs() < 1e-12);
        let c = shifted(&a, v(0.5, 0.5, 0.5));
        let bc = MeshBvh::build(&c, &LIMITS).unwrap();
        let r = closest_between(&ba, &bc, &TOL, &LIMITS).unwrap();
        assert!(r.intersecting && r.distance == 0.0);
        assert!((r.points[0] - r.points[1]).norm() <= TOL.linear);
        // Nested meshes do not cross; the distance is between the surfaces, not zero.
        let inner = TriMesh::new(
            a.positions()
                .iter()
                .map(|p| Point3::from(p.coords * 0.1 + v(0.45, 0.45, 0.45)))
                .collect(),
            a.triangles().to_vec(),
            &LIMITS,
        )
        .unwrap();
        let bi = MeshBvh::build(&inner, &LIMITS).unwrap();
        let r = closest_between(&ba, &bi, &TOL, &LIMITS).unwrap();
        assert!((r.distance - 0.45).abs() < 1e-12 && !r.intersecting);
        let tiny = Limits {
            array_length: 1,
            ..LIMITS
        };
        let s = sphere(Point3::origin(), 1.0, 6, 12);
        let far = shifted(&s, v(10.0, 0.0, 0.0));
        let (b1, b2) = (
            MeshBvh::build(&s, &LIMITS).unwrap(),
            MeshBvh::build(&far, &LIMITS).unwrap(),
        );
        assert_eq!(
            closest_between(&b1, &b2, &TOL, &tiny).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
    }

    fn small_mesh() -> impl Strategy<Value = TriMesh> {
        (3_usize..12).prop_flat_map(|n| {
            (
                prop::collection::vec((-4.0..4.0_f64, -4.0..4.0_f64, -4.0..4.0_f64), n),
                prop::collection::vec([0..n as u32, 0..n as u32, 0..n as u32], 1..14),
            )
                .prop_map(|(p, t)| {
                    TriMesh::new(
                        p.into_iter()
                            .map(|(x, y, z)| Point3::new(x, y, z))
                            .collect(),
                        t,
                        &LIMITS,
                    )
                    .unwrap()
                })
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        #[test]
        fn mesh_distance_equals_brute_force(a in small_mesh(), b in small_mesh(), shift in -6.0..6.0_f64) {
            let b = shifted(&b, v(shift, 0.0, 0.0));
            let (ba, bb) = (MeshBvh::build(&a, &LIMITS).unwrap(), MeshBvh::build(&b, &LIMITS).unwrap());
            let got = closest_between(&ba, &bb, &TOL, &LIMITS).unwrap();
            let mut best = f64::INFINITY;
            let mut at = [0, 0];
            for x in 0..a.triangle_count() as u32 {
                for y in 0..b.triangle_count() as u32 {
                    let (d, _) = triangle_pair(ba.corners(x), bb.corners(y), TOL.linear).unwrap();
                    if d < best { best = d; at = [x, y]; }
                }
            }
            prop_assert_eq!(got.distance, best);
            if best > 0.0 { prop_assert_eq!(got.triangles, at); }
            prop_assert_eq!(got.intersecting, best == 0.0);
        }

        #[test]
        fn triangle_pair_distance_is_bounded_by_samples(
            a in prop::array::uniform3((-3.0..3.0_f64, -3.0..3.0_f64, -3.0..3.0_f64)),
            b in prop::array::uniform3((-3.0..3.0_f64, -3.0..3.0_f64, -3.0..3.0_f64)),
        ) {
            let ta = a.map(|p| v(p.0, p.1, p.2));
            let tb = b.map(|p| v(p.0, p.1, p.2));
            let (d, pts) = triangle_pair(ta, tb, TOL.linear).unwrap();
            // The reported points lie on the triangles and realize the distance.
            prop_assert!(((pts[0] - pts[1]).norm() - d).abs() <= 1e-9 + 2.0 * TOL.linear);
            prop_assert!(closest_on_triangle(&pts[0], ta).unwrap().distance < 1e-9);
            prop_assert!(closest_on_triangle(&pts[1], tb).unwrap().distance < 1e-9);
            // No sampled pair of points is closer.
            let sample = |t: &[Vector3; 3], i: usize, j: usize, n: usize| {
                let (x, y) = (i as f64 / n as f64, j as f64 / n as f64);
                t[0] * (1.0 - x - y) + t[1] * x + t[2] * y
            };
            let n = 6;
            for i in 0..=n { for j in 0..=(n - i) { for k in 0..=n { for l in 0..=(n - k) {
                let s = (sample(&ta, i, j, n) - sample(&tb, k, l, n)).norm();
                prop_assert!(s >= d - 1e-9, "{s} < {d}");
            }}}}
        }
    }
}
