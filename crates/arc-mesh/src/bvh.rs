//! A bounding volume hierarchy over the triangles of a mesh, with ray queries (CORE-MESH-020)
//! and point-to-mesh distance queries (CORE-MESH-021).
//!
//! - **Build.** Triangles are split at the median of their box centers along the longest axis
//!   of the centers, ties broken by triangle index, until a node holds at most four triangles.
//!   The tree is balanced, so its depth is at most `log₂(n)`, the build needs no recursion and
//!   `O(n log n)` time and `O(n)` memory, and it is the same for the same mesh. Boxes are
//!   padded by a few units in the last place so rounding cannot hide a triangle.
//! - **Rays.** Triangles are tested with the watertight algorithm (shear the triangle into the
//!   ray's frame and evaluate three edge functions that are computed identically for the two
//!   triangles sharing an edge), so a ray through a shared edge or vertex hits at least one of
//!   the triangles and never slips between them. A ray that touches an edge exactly reports
//!   every triangle that has the edge, because their distances are equal.
//! - **Order.** Hits are ordered by distance, then triangle index; the first hit is the first
//!   of that order, so equal distances give the lowest triangle index, whatever the tree.
//! - **Magnitude.** Each triangle is translated to the query and scaled by a power of two
//!   before it is tested, so coordinates from 1e-300 to 1e300 work; a result that is not
//!   representable fails with `mesh.precision.insufficient`.

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result};

use crate::TriMesh;
use crate::distance::{closest_on_triangle, scale_of};
use crate::error::codes;
use crate::topo::{check_mesh, get, idx, pos};

/// The padding of boxes and slack of box tests, relative.
const SLACK: f64 = 16.0 * f64::EPSILON;
const LEAF_SIZE: usize = 4;

/// A half-line or segment from `origin` along a unit `direction`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    origin: Point3,
    direction: Vector3,
    max_distance: f64,
}

impl Ray {
    /// A ray of unlimited length. The direction is normalized; it need not be a unit vector.
    ///
    /// # Errors
    ///
    /// `mesh.query.invalid` when the origin or direction is not finite or the direction is zero.
    pub fn new(origin: Point3, direction: Vector3) -> Result<Self> {
        let direction = crate::unit(&direction).ok_or_else(|| Error::new(codes::QUERY_INVALID))?;
        if !origin.iter().all(|c| c.is_finite()) {
            return Err(Error::new(codes::QUERY_INVALID));
        }
        Ok(Self {
            origin,
            direction,
            max_distance: f64::INFINITY,
        })
    }

    /// The ray cut off at `max_distance` (a closed bound).
    ///
    /// # Errors
    ///
    /// `mesh.query.invalid` when the distance is NaN or negative.
    pub fn with_max_distance(mut self, max_distance: f64) -> Result<Self> {
        if max_distance.is_nan() || max_distance < 0.0 {
            return Err(Error::new(codes::QUERY_INVALID));
        }
        self.max_distance = max_distance;
        Ok(self)
    }

    /// The origin.
    #[must_use]
    pub fn origin(&self) -> Point3 {
        self.origin
    }

    /// The unit direction.
    #[must_use]
    pub fn direction(&self) -> Vector3 {
        self.direction
    }
}

/// A ray hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The index of the triangle hit.
    pub triangle: u32,
    /// The barycentric coordinates of the hit in the triangle's vertex order; they sum to 1.
    pub barycentric: [f64; 3],
    /// The distance from the ray origin.
    pub distance: f64,
}

/// The closest point of a mesh to a query point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoint {
    /// The triangle holding the point (the lowest index among equally close triangles).
    pub triangle: u32,
    /// The barycentric coordinates of the point in the triangle's vertex order.
    pub barycentric: [f64; 3],
    /// The point.
    pub point: Point3,
    /// The distance from the query point.
    pub distance: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Node {
    pub(crate) min: Vector3,
    pub(crate) max: Vector3,
    /// First entry of `order` for a leaf; the index of the right child for an inner node (the
    /// left child follows the node).
    link: u32,
    /// The number of triangles of a leaf; zero for an inner node.
    count: u32,
}

/// A bounding volume hierarchy over the triangles of a mesh, built once and queried many times.
#[derive(Debug, Clone)]
pub struct MeshBvh<'a> {
    mesh: &'a TriMesh,
    pub(crate) nodes: Vec<Node>,
    order: Vec<u32>,
}

fn triangle_box(mesh: &TriMesh, t: [u32; 3]) -> (Vector3, Vector3) {
    let mut lo = Vector3::repeat(f64::INFINITY);
    let mut hi = Vector3::repeat(f64::NEG_INFINITY);
    for i in t {
        let p = pos(&mesh.positions, i);
        lo = lo.inf(&p);
        hi = hi.sup(&p);
    }
    (lo, hi)
}

fn pad(lo: Vector3, hi: Vector3) -> (Vector3, Vector3) {
    let reach = lo.abs().sup(&hi.abs());
    let p = reach * SLACK;
    (lo - p, hi + p)
}

impl<'a> MeshBvh<'a> {
    /// Builds the hierarchy over every triangle of `mesh`, degenerate ones included.
    ///
    /// # Errors
    ///
    /// `mesh.query.empty` for a mesh without triangles; `mesh.limit.exceeded` beyond `limits`.
    pub fn build(mesh: &'a TriMesh, limits: &Limits) -> Result<Self> {
        check_mesh(mesh, limits, 160)?;
        if mesh.triangles.is_empty() {
            return Err(Error::new(codes::QUERY_EMPTY));
        }
        let boxes: Vec<(Vector3, Vector3)> = mesh
            .triangles
            .iter()
            .map(|&t| triangle_box(mesh, t))
            .collect();
        let center = |i: u32| {
            let (lo, hi) = get(&boxes, i as usize);
            lo / 2.0 + hi / 2.0
        };
        let mut order: Vec<u32> = (0..idx(mesh.triangles.len())).collect();
        let mut nodes: Vec<Node> = Vec::new();
        // (start, end, parent whose right link this node fills)
        let mut jobs = vec![(0_usize, order.len(), None::<usize>)];
        while let Some((start, end, parent)) = jobs.pop() {
            let here = nodes.len();
            if let Some(p) = parent
                && let Some(node) = nodes.get_mut(p)
            {
                node.link = idx(here);
            }
            let range = order.get(start..end).unwrap_or(&[]);
            let (mut lo, mut hi) = (
                Vector3::repeat(f64::INFINITY),
                Vector3::repeat(f64::NEG_INFINITY),
            );
            let (mut clo, mut chi) = (lo, hi);
            for &t in range {
                let (a, b) = get(&boxes, t as usize);
                lo = lo.inf(&a);
                hi = hi.sup(&b);
                clo = clo.inf(&center(t));
                chi = chi.sup(&center(t));
            }
            let (lo, hi) = pad(lo, hi);
            if end - start <= LEAF_SIZE {
                nodes.push(Node {
                    min: lo,
                    max: hi,
                    link: idx(start),
                    count: idx(end - start),
                });
                continue;
            }
            let extent = chi - clo;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            let mid = start + (end - start) / 2;
            if let Some(slice) = order.get_mut(start..end) {
                slice.select_nth_unstable_by(mid - start, |&a, &b| {
                    center(a)
                        .get(axis)
                        .copied()
                        .unwrap_or(0.0)
                        .total_cmp(&center(b).get(axis).copied().unwrap_or(0.0))
                        .then(a.cmp(&b))
                });
            }
            nodes.push(Node {
                min: lo,
                max: hi,
                link: 0,
                count: 0,
            });
            jobs.push((mid, end, Some(here)));
            jobs.push((start, mid, None));
        }
        Ok(Self { mesh, nodes, order })
    }

    /// The mesh the hierarchy was built over.
    #[must_use]
    pub fn mesh(&self) -> &'a TriMesh {
        self.mesh
    }

    pub(crate) fn node(&self, i: usize) -> Node {
        self.nodes.get(i).copied().unwrap_or(Node {
            min: Vector3::zeros(),
            max: Vector3::zeros(),
            link: 0,
            count: 0,
        })
    }

    /// The triangles of a leaf, or `None` for an inner node.
    pub(crate) fn leaf(&self, node: &Node) -> Option<&[u32]> {
        (node.count > 0)
            .then(|| {
                let s = node.link as usize;
                self.order.get(s..s + node.count as usize)
            })
            .flatten()
    }

    /// The children of an inner node.
    pub(crate) fn children(&self, i: usize) -> Option<(usize, usize)> {
        let n = self.node(i);
        (n.count == 0).then_some((i + 1, n.link as usize))
    }

    pub(crate) fn corners(&self, t: u32) -> [Vector3; 3] {
        crate::topo::corners(&self.mesh.positions, get(&self.mesh.triangles, t as usize))
    }

    /// The nearest hit along the ray, or `None`.
    ///
    /// # Errors
    ///
    /// `mesh.precision.insufficient` when a distance is not representable.
    pub fn first_hit(&self, ray: &Ray) -> Result<Option<RayHit>> {
        let mut best: Option<RayHit> = None;
        self.traverse(ray, |hit| {
            if best.is_none_or(|b| (hit.distance, hit.triangle) < (b.distance, b.triangle)) {
                best = Some(hit);
            }
            best.map_or(ray.max_distance, |b| b.distance)
        })?;
        Ok(best)
    }

    /// Every hit along the ray, ordered by distance and then triangle index.
    ///
    /// # Errors
    ///
    /// `mesh.precision.insufficient` when a distance is not representable.
    pub fn all_hits(&self, ray: &Ray) -> Result<Vec<RayHit>> {
        let mut hits = Vec::new();
        self.traverse(ray, |hit| {
            hits.push(hit);
            ray.max_distance
        })?;
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.triangle.cmp(&b.triangle))
        });
        Ok(hits)
    }

    /// Calls `visit` for every hit within the current reach; it returns the new reach.
    fn traverse(&self, ray: &Ray, mut visit: impl FnMut(RayHit) -> f64) -> Result<()> {
        let o = ray.origin.coords;
        let d = ray.direction;
        let mut reach = ray.max_distance;
        let mut stack = vec![0_usize];
        while let Some(i) = stack.pop() {
            let node = self.node(i);
            if !slab(&o, &d, &node, reach) {
                continue;
            }
            if let Some((a, b)) = self.children(i) {
                stack.push(b);
                stack.push(a);
                continue;
            }
            for &t in self.leaf(&node).unwrap_or(&[]) {
                if let Some(hit) = ray_triangle(&o, &d, self.corners(t), t)?
                    && hit.distance <= reach
                {
                    reach = visit(hit);
                }
            }
        }
        Ok(())
    }

    /// The point of the mesh closest to `point`, with its triangle and barycentric
    /// coordinates; the lowest triangle index among equally close triangles.
    ///
    /// # Errors
    ///
    /// `mesh.query.invalid` for a non-finite point; `mesh.precision.insufficient` when a
    /// distance is not representable.
    pub fn closest_point(&self, point: &Point3) -> Result<ClosestPoint> {
        if !point.iter().all(|c| c.is_finite()) {
            return Err(Error::new(codes::QUERY_INVALID));
        }
        let mut best: Option<ClosestPoint> = None;
        let mut stack = vec![(0_usize, 0.0_f64)];
        while let Some((i, bound)) = stack.pop() {
            if best.is_some_and(|b| bound > b.distance) {
                continue;
            }
            if let Some((a, b)) = self.children(i) {
                let (da, db) = (self.box_distance(a, point), self.box_distance(b, point));
                // The nearer child is popped first.
                if da <= db {
                    stack.push((b, db));
                    stack.push((a, da));
                } else {
                    stack.push((a, da));
                    stack.push((b, db));
                }
                continue;
            }
            for &t in self.leaf(&self.node(i)).unwrap_or(&[]) {
                let c = closest_on_triangle(&point.coords, self.corners(t))?;
                let candidate = ClosestPoint {
                    triangle: t,
                    barycentric: c.barycentric,
                    point: Point3::from(c.point),
                    distance: c.distance,
                };
                if best.is_none_or(|b| (candidate.distance, t) < (b.distance, b.triangle)) {
                    best = Some(candidate);
                }
            }
        }
        best.ok_or_else(|| Error::new(codes::QUERY_EMPTY))
    }

    /// A lower bound of the distance from `p` to anything in the box of node `i`.
    fn box_distance(&self, i: usize, p: &Point3) -> f64 {
        let n = self.node(i);
        let gap = (n.min - p.coords)
            .sup(&(p.coords - n.max))
            .sup(&Vector3::zeros());
        crate::length(&gap) * (1.0 - SLACK)
    }
}

/// The interval of ray parameters inside the box, widened for rounding, intersected with
/// `[0, reach]`; whether it is not empty.
fn slab(o: &Vector3, d: &Vector3, node: &Node, reach: f64) -> bool {
    let (mut near, mut far) = (0.0_f64, reach);
    for k in 0..3 {
        let (lo, hi) = (get(node.min.as_slice(), k), get(node.max.as_slice(), k));
        let (ok, dk) = (get(o.as_slice(), k), get(d.as_slice(), k));
        if dk == 0.0 {
            let slack = SLACK * (lo.abs() + hi.abs());
            if ok < lo - slack || ok > hi + slack {
                return false;
            }
            continue;
        }
        let (t1, t2) = ((lo - ok) / dk, (hi - ok) / dk);
        let widen = SLACK * (t1.abs() + t2.abs() + (lo.abs() + hi.abs() + ok.abs()) / dk.abs());
        // NaN (infinite minus infinite) is ignored by `max` and `min`: the box stays possible.
        near = near.max(t1.min(t2) - widen);
        far = far.min(t1.max(t2) + widen);
    }
    near <= far
}

/// The watertight ray–triangle test, after translating to the ray origin and scaling by a power
/// of two. `None` for a miss, a hit behind the origin or a hit at infinity.
fn ray_triangle(o: &Vector3, d: &Vector3, tri: [Vector3; 3], index: u32) -> Result<Option<RayHit>> {
    let shifted = tri.map(|v| v - o);
    let scale = scale_of(&shifted);
    if !scale.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    let a = shifted.map(|v| v / scale);
    let ax = |v: &Vector3, k: usize| get(v.as_slice(), k);
    let mut kz = 0;
    for k in 1..3 {
        if ax(d, k).abs() > ax(d, kz).abs() {
            kz = k;
        }
    }
    let (mut kx, mut ky) = ((kz + 1) % 3, (kz + 2) % 3);
    if ax(d, kz) < 0.0 {
        std::mem::swap(&mut kx, &mut ky);
    }
    let (sx, sy, sz) = (
        ax(d, kx) / ax(d, kz),
        ax(d, ky) / ax(d, kz),
        1.0 / ax(d, kz),
    );
    let sheared = a.map(|v| {
        (
            ax(&v, kx) - sx * ax(&v, kz),
            ax(&v, ky) - sy * ax(&v, kz),
            sz * ax(&v, kz),
        )
    });
    let [(ax_, ay, az), (bx, by, bz), (cx, cy, cz)] = sheared;
    let u = cx * by - cy * bx;
    let v = ax_ * cy - ay * cx;
    let w = bx * ay - by * ax_;
    if (u < 0.0 || v < 0.0 || w < 0.0) && (u > 0.0 || v > 0.0 || w > 0.0) {
        return Ok(None);
    }
    let det = u + v + w;
    if det == 0.0 {
        return Ok(None);
    }
    let t = (u * az + v * bz + w * cz) / det;
    if t < 0.0 || !t.is_finite() {
        return Ok(None);
    }
    let distance = t * scale;
    if !distance.is_finite() {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    Ok(Some(RayHit {
        triangle: index,
        barycentric: [u / det, v / det, w / det],
        distance,
    }))
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
    use crate::testing::{cube, sphere};

    const LIMITS: Limits = Limits::DEFAULT;

    fn ray(o: [f64; 3], d: [f64; 3]) -> Ray {
        Ray::new(
            Point3::new(o[0], o[1], o[2]),
            Vector3::new(d[0], d[1], d[2]),
        )
        .unwrap()
    }

    fn brute_hits(mesh: &TriMesh, r: &Ray) -> Vec<RayHit> {
        let mut hits: Vec<RayHit> = (0..mesh.triangle_count() as u32)
            .filter_map(|t| {
                let c = crate::topo::corners(&mesh.positions, mesh.triangles[t as usize]);
                ray_triangle(&r.origin.coords, &r.direction, c, t).unwrap()
            })
            .filter(|h| h.distance <= r.max_distance)
            .collect();
        hits.sort_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then(a.triangle.cmp(&b.triangle))
        });
        hits
    }

    #[test]
    fn rays_hit_a_cube_with_distance_and_barycentrics() {
        let m = cube();
        let bvh = MeshBvh::build(&m, &LIMITS).unwrap();
        let r = ray([0.25, 0.25, -3.0], [0.0, 0.0, 1.0]);
        let hit = bvh.first_hit(&r).unwrap().unwrap();
        assert!((hit.distance - 3.0).abs() < 1e-12);
        let c = crate::topo::corners(&m.positions, m.triangles[hit.triangle as usize]);
        let p = c[0] * hit.barycentric[0] + c[1] * hit.barycentric[1] + c[2] * hit.barycentric[2];
        assert!((p - Vector3::new(0.25, 0.25, 0.0)).norm() < 1e-12);
        let all = bvh.all_hits(&r).unwrap();
        assert_eq!(all.len(), 2);
        assert!((all[1].distance - 4.0).abs() < 1e-12);
        assert!(
            bvh.first_hit(&r.with_max_distance(2.0).unwrap())
                .unwrap()
                .is_none()
        );
        assert!(
            bvh.first_hit(&ray([0.5, 0.5, 5.0], [0.0, 0.0, 1.0]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_ray_through_a_shared_edge_or_vertex_is_never_lost() {
        let m = sphere(Point3::origin(), 1.0, 8, 16);
        let bvh = MeshBvh::build(&m, &LIMITS).unwrap();
        // Through every vertex of the mesh from outside.
        for p in m.positions() {
            let r = Ray::new(p * 2.0, -p.coords).unwrap();
            let hit = bvh.first_hit(&r).unwrap().unwrap();
            assert!((hit.distance - p.coords.norm()).abs() < 1e-9);
        }
        // Through the diagonal of the cube's faces (a shared edge).
        let cube_mesh = cube();
        let c = MeshBvh::build(&cube_mesh, &LIMITS).unwrap();
        let r = ray([0.5, 0.5, -1.0], [0.0, 0.0, 1.0]);
        assert!(c.first_hit(&r).unwrap().is_some());
        let diagonal = ray([2.0, 2.0, 0.5], [-1.0, -1.0, 0.0]);
        assert!(c.first_hit(&diagonal).unwrap().is_some());
    }

    #[test]
    fn ties_give_the_lowest_triangle_and_input_is_validated() {
        let cube_mesh = cube();
        let c = MeshBvh::build(&cube_mesh, &LIMITS).unwrap();
        // Along the diagonal of the bottom face the ray touches an edge shared by two triangles.
        let r = ray([-1.0, -1.0, 0.0], [1.0, 1.0, 0.0]);
        let hits = c.all_hits(&r).unwrap();
        let first = c.first_hit(&r).unwrap().unwrap();
        assert_eq!(hits[0].triangle, first.triangle);
        for bad in [
            [f64::NAN, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            [f64::INFINITY, 0.0, 1.0],
        ] {
            let e = Ray::new(Point3::origin(), Vector3::new(bad[0], bad[1], bad[2])).unwrap_err();
            assert_eq!(e.code(), "mesh.query.invalid");
        }
        assert!(Ray::new(Point3::new(f64::NAN, 0.0, 0.0), Vector3::x()).is_err());
        assert!(
            ray([0.0; 3], [1.0, 0.0, 0.0])
                .with_max_distance(-1.0)
                .is_err()
        );
        assert_eq!(
            MeshBvh::build(&TriMesh::default(), &LIMITS)
                .unwrap_err()
                .code(),
            "mesh.query.empty"
        );
        assert!(c.closest_point(&Point3::new(f64::NAN, 0.0, 0.0)).is_err());
        let tight = Limits {
            array_length: 3,
            ..LIMITS
        };
        assert_eq!(
            MeshBvh::build(&cube(), &tight).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
    }

    #[test]
    fn extreme_magnitudes_hit_or_fail_with_a_code() {
        for k in [1e-200, 1e-8, 1.0, 1e8, 1e200, 1e300] {
            let m = TriMesh::new(
                cube().positions().iter().map(|p| p * k).collect(),
                cube().triangles().to_vec(),
                &LIMITS,
            )
            .unwrap();
            let bvh = MeshBvh::build(&m, &LIMITS).unwrap();
            let r = ray([0.25 * k, 0.25 * k, -3.0 * k], [0.0, 0.0, 1.0]);
            match bvh.first_hit(&r) {
                Ok(Some(h)) => assert!((h.distance / k - 3.0).abs() < 1e-9, "{k}"),
                Ok(None) => panic!("missed at {k}"),
                Err(e) => assert_eq!(e.code(), "mesh.precision.insufficient"),
            }
            match bvh.closest_point(&Point3::new(0.5 * k, 0.5 * k, 2.0 * k)) {
                Ok(c) => assert!((c.distance / k - 1.0).abs() < 1e-9, "{k}"),
                Err(e) => assert_eq!(e.code(), "mesh.precision.insufficient"),
            }
        }
    }

    fn random_mesh() -> impl Strategy<Value = TriMesh> {
        (4_usize..40).prop_flat_map(|n| {
            (
                prop::collection::vec((-5.0..5.0_f64, -5.0..5.0_f64, -5.0..5.0_f64), n),
                prop::collection::vec([0..n as u32, 0..n as u32, 0..n as u32], 1..60),
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
        fn rays_equal_brute_force(
            mesh in random_mesh(),
            o in (-8.0..8.0_f64, -8.0..8.0_f64, -8.0..8.0_f64),
            d in (-1.0..1.0_f64, -1.0..1.0_f64, -1.0..1.0_f64),
            limit in prop::option::of(0.0..20.0_f64),
        ) {
            let Ok(mut r) = Ray::new(Point3::new(o.0, o.1, o.2), Vector3::new(d.0, d.1, d.2)) else {
                return Ok(());
            };
            if let Some(l) = limit { r = r.with_max_distance(l).unwrap(); }
            let bvh = MeshBvh::build(&mesh, &LIMITS).unwrap();
            let expected = brute_hits(&mesh, &r);
            prop_assert_eq!(bvh.all_hits(&r).unwrap(), expected.clone());
            prop_assert_eq!(bvh.first_hit(&r).unwrap(), expected.first().copied());
        }

        #[test]
        fn closest_points_equal_brute_force(
            mesh in random_mesh(),
            p in (-8.0..8.0_f64, -8.0..8.0_f64, -8.0..8.0_f64),
        ) {
            let p = Point3::new(p.0, p.1, p.2);
            let bvh = MeshBvh::build(&mesh, &LIMITS).unwrap();
            let got = bvh.closest_point(&p).unwrap();
            let mut want: Option<ClosestPoint> = None;
            for t in 0..mesh.triangle_count() as u32 {
                let c = closest_on_triangle(&p.coords, crate::topo::corners(&mesh.positions, mesh.triangles[t as usize])).unwrap();
                if want.is_none_or(|w| c.distance < w.distance) {
                    want = Some(ClosestPoint { triangle: t, barycentric: c.barycentric, point: Point3::from(c.point), distance: c.distance });
                }
            }
            prop_assert_eq!(Some(got), want);
        }
    }
}
