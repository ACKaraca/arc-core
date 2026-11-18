//! Mesh measurements: area, volume, centroid, bounds and genus (CORE-MESH-007).
//!
//! Topology is taken after welding vertices at exactly equal positions, so a mesh whose
//! vertices are split for texture seams or split normals measures like the welded mesh.
//! Triangles with a repeated vertex index are ignored by the topological rules.

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result};
use arc_geom::Aabb3;

use crate::TriMesh;
use crate::error::codes;
use crate::topo::{
    Adj, Dsu, Edges, Sum, check_mesh, detail, exact_canon, get, idx, index_degenerate,
    power_of_two_scale,
};

/// The volume and centroid of a closed mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidProperties {
    /// The signed volume: positive when the triangles face outward.
    pub volume: f64,
    /// The centroid of the enclosed volume.
    pub centroid: Point3,
}

/// The topology of one connected component of a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentTopology {
    /// The lowest triangle index of the component.
    pub first_triangle: u32,
    /// The number of triangles.
    pub triangles: u32,
    /// The number of distinct (welded) vertices.
    pub vertices: u32,
    /// The number of distinct edges.
    pub edges: u32,
    /// The number of boundary loops; zero for a closed component.
    pub boundary_loops: u32,
    /// The Euler characteristic `V − E + F`.
    pub euler: i64,
    /// Whether the triangles can be oriented consistently.
    pub orientable: bool,
    /// The genus (number of handles) of an orientable component, `None` otherwise.
    pub genus: Option<u32>,
}

/// The coordinates of the vertices scaled by an exact power of two so the largest is below 2,
/// with that scale.
pub(crate) fn scaled(mesh: &TriMesh) -> (Vec<Vector3>, f64) {
    let m = mesh
        .positions
        .iter()
        .flat_map(|p| p.iter().map(|c| c.abs()))
        .fold(0.0_f64, f64::max);
    let scale = power_of_two_scale(m);
    let divisor = if scale > 0.0 { scale } else { 1.0 };
    (
        mesh.positions.iter().map(|p| p.coords / divisor).collect(),
        divisor,
    )
}

/// Applies `scale` to a scaled quantity, failing when the result overflows or loses all its
/// digits to underflow.
fn unscale(value: f64, factors: [f64; 3]) -> Result<f64> {
    let result = factors.iter().fold(value, |v, f| v * f);
    let underflow = value != 0.0 && result.abs() < f64::MIN_POSITIVE;
    if result.is_finite() && !underflow {
        Ok(result)
    } else {
        Err(Error::new(codes::PRECISION_INSUFFICIENT))
    }
}

/// The surface area: the sum of the triangle areas, accumulated with compensation.
///
/// # Errors
///
/// `mesh.limit.exceeded` beyond `limits`; `mesh.precision.insufficient` when the area overflows
/// or underflows `f64`.
pub fn surface_area(mesh: &TriMesh, limits: &Limits) -> Result<f64> {
    check_mesh(mesh, limits, 0)?;
    let (q, scale) = scaled(mesh);
    let mut sum = Sum::default();
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| q.get(i as usize).copied().unwrap_or_default());
        sum.add((b - a).cross(&(c - a)).norm() / 2.0);
    }
    unscale(sum.value(), [scale, scale, 1.0])
}

/// Checks that every edge of the welded mesh is used equally often in both directions.
fn ensure_closed(mesh: &TriMesh, limits: &Limits) -> Result<()> {
    check_mesh(mesh, limits, 64)?;
    let canon = exact_canon(&mesh.positions);
    let edges = Edges::build(
        (0..)
            .zip(&mesh.triangles)
            .map(|(i, t)| (i, t.map(|v| get(&canon, v as usize)))),
    );
    let open = edges
        .groups()
        .filter(|g| g.iter().filter(|e| e.forward).count() * 2 != g.len())
        .count();
    if open > 0 || edges.uses.is_empty() {
        return Err(Error::new(codes::MEASURE_NOT_CLOSED).with_detail("edges", detail(open)));
    }
    Ok(())
}

/// The signed volume and centroid of a closed mesh by the divergence theorem, summing the
/// signed tetrahedra from a reference point near the mesh.
///
/// The mesh must be closed and consistently oriented after welding exactly equal positions:
/// every edge is used as often in one direction as in the other. Edges shared by four or more
/// triangles are allowed.
///
/// # Errors
///
/// `mesh.measure.not_closed` when the mesh is not closed (or has no triangles);
/// `mesh.measure.zero_volume` when the volume is exactly zero; `mesh.precision.insufficient`
/// when the volume overflows or underflows; `mesh.limit.exceeded` beyond `limits`.
pub fn volume_and_centroid(mesh: &TriMesh, limits: &Limits) -> Result<SolidProperties> {
    ensure_closed(mesh, limits)?;
    let (q, scale) = scaled(mesh);
    let at = |i: u32| q.get(i as usize).copied().unwrap_or_default();
    let origin = {
        let used = mesh.triangles.iter().flatten().map(|&i| at(i));
        let (mut lo, mut hi) = (
            Vector3::repeat(f64::INFINITY),
            Vector3::repeat(f64::NEG_INFINITY),
        );
        for p in used {
            lo = lo.inf(&p);
            hi = hi.sup(&p);
        }
        (lo + hi) / 2.0
    };
    let (mut volume, mut cx, mut cy, mut cz) = (
        Sum::default(),
        Sum::default(),
        Sum::default(),
        Sum::default(),
    );
    for t in &mesh.triangles {
        let [a, b, c] = t.map(|i| at(i) - origin);
        let v = a.dot(&b.cross(&c)) / 6.0;
        volume.add(v);
        let s = (a + b + c) / 4.0 * v;
        cx.add(s.x);
        cy.add(s.y);
        cz.add(s.z);
    }
    let v = volume.value();
    if v == 0.0 {
        return Err(Error::new(codes::MEASURE_ZERO_VOLUME));
    }
    let centroid = Vector3::new(cx.value(), cy.value(), cz.value()) / v + origin;
    let volume = unscale(v, [scale, scale, scale])?;
    let centroid = centroid * scale;
    if !centroid.iter().all(|c| c.is_finite()) {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT));
    }
    Ok(SolidProperties {
        volume,
        centroid: Point3::from(centroid),
    })
}

/// The signed volume of a closed mesh; see [`volume_and_centroid`].
///
/// # Errors
///
/// As [`volume_and_centroid`], except `mesh.measure.zero_volume` is not raised: a zero volume
/// is returned.
pub fn volume(mesh: &TriMesh, limits: &Limits) -> Result<f64> {
    match volume_and_centroid(mesh, limits) {
        Ok(p) => Ok(p.volume),
        Err(e) if e.code() == codes::MEASURE_ZERO_VOLUME.code => Ok(0.0),
        Err(e) => Err(e),
    }
}

/// The bounding box of the vertices that triangles use; empty when there are none. Unlike
/// [`TriMesh::bounding_box`] it ignores unreferenced vertices.
#[must_use]
pub fn bounds(mesh: &TriMesh) -> Aabb3 {
    let used: Vec<Point3> = mesh
        .triangles
        .iter()
        .flatten()
        .filter_map(|&i| mesh.positions.get(i as usize).copied())
        .collect();
    Aabb3::from_points(&used)
}

/// For every vertex whether the triangles around it form more than one fan.
pub(crate) fn pinched_flags(tris: &[[u32; 3]], edges: &Edges, vertices: usize) -> Vec<bool> {
    // Corners of one vertex joined across two-triangle edges form its fans.
    let mut corner_sets = Dsu::new(tris.len() * 3);
    for g in edges.groups() {
        if let [a, b] = g {
            for vertex in [a.lo, a.hi] {
                let corner = |e: &crate::topo::EdgeUse| {
                    let t = get(tris, e.tri as usize);
                    let k = t.iter().position(|&v| v == vertex).unwrap_or(0);
                    e.tri * 3 + idx(k)
                };
                corner_sets.union(corner(a), corner(b));
            }
        }
    }
    let mut fans = vec![0_u32; vertices];
    for (t, tri) in tris.iter().enumerate() {
        if index_degenerate(*tri) {
            continue;
        }
        for (k, &v) in tri.iter().enumerate() {
            let c = idx(t * 3 + k);
            if corner_sets.find(c) == c
                && let Some(n) = fans.get_mut(v as usize)
            {
                *n += 1;
            }
        }
    }
    fans.iter().map(|&n| n > 1).collect()
}

/// Marks (at the component's root vertex) every component whose triangles cannot be oriented
/// consistently, by propagating a flip flag across two-triangle edges.
fn non_orientable_roots(tris: &[[u32; 3]], adj: &[[Adj; 3]], parts: &mut Dsu) -> Vec<bool> {
    let mut flip: Vec<Option<bool>> = vec![None; tris.len()];
    let mut bad = vec![false; tris.len().max(1) * 3];
    for (start, first) in tris.iter().enumerate() {
        if index_degenerate(*first) || flip.get(start).copied().flatten().is_some() {
            continue;
        }
        let component = parts.find(first[0]);
        if let Some(f) = flip.get_mut(start) {
            *f = Some(false);
        }
        let mut stack = vec![idx(start)];
        while let Some(t) = stack.pop() {
            let mine = flip.get(t as usize).copied().flatten().unwrap_or(false);
            for a in get(adj, t as usize) {
                let Adj::Pair { tri, same } = a else { continue };
                let want = mine ^ same;
                match flip.get(tri as usize).copied().flatten() {
                    None => {
                        if let Some(f) = flip.get_mut(tri as usize) {
                            *f = Some(want);
                        }
                        stack.push(tri);
                    }
                    Some(have) if have != want => {
                        if let Some(n) = bad.get_mut(component as usize) {
                            *n = true;
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }
    bad
}

/// The Euler characteristic, boundary loops and genus of every connected component, ordered by
/// lowest triangle index. Components are connected through shared (welded) vertices.
///
/// The genus of an orientable component is `(2 − χ − b) / 2` for Euler characteristic `χ` and
/// `b` boundary loops.
///
/// # Errors
///
/// `mesh.topology.non_manifold` (details `edges`, `vertices`) when an edge is used by more than
/// two triangles or the triangles around a vertex form more than one fan;
/// `mesh.limit.exceeded` beyond `limits`.
pub fn component_topology(mesh: &TriMesh, limits: &Limits) -> Result<Vec<ComponentTopology>> {
    check_mesh(mesh, limits, 160)?;
    let n = mesh.positions.len();
    let canon = exact_canon(&mesh.positions);
    let tris: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .map(|t| t.map(|v| get(&canon, v as usize)))
        .collect();
    let edges = Edges::build((0..).zip(tris.iter().copied()));
    let bad_edges = edges.groups().filter(|g| g.len() > 2).count();
    let bad_vertices = pinched_flags(&tris, &edges, n)
        .iter()
        .filter(|b| **b)
        .count();
    if bad_edges > 0 || bad_vertices > 0 {
        return Err(Error::new(codes::TOPOLOGY_NON_MANIFOLD)
            .with_detail("edges", detail(bad_edges))
            .with_detail("vertices", detail(bad_vertices)));
    }
    let mut parts = Dsu::new(tris.len().max(n));
    for tri in tris.iter().filter(|t| !index_degenerate(**t)) {
        parts.union(tri[0], tri[1]);
        parts.union(tri[1], tri[2]);
    }
    let non_orientable = non_orientable_roots(&tris, &edges.adjacency(tris.len()), &mut parts);
    let mut slot = vec![u32::MAX; n];
    let mut out: Vec<ComponentTopology> = Vec::new();
    let mut vertex_seen = vec![false; n];
    for (t, tri) in tris.iter().enumerate() {
        if index_degenerate(*tri) {
            continue;
        }
        let root = parts.find(tri[0]);
        let mut entry = slot.get(root as usize).copied().unwrap_or(u32::MAX);
        if entry == u32::MAX {
            entry = idx(out.len());
            if let Some(s) = slot.get_mut(root as usize) {
                *s = entry;
            }
            out.push(ComponentTopology {
                first_triangle: idx(t),
                triangles: 0,
                vertices: 0,
                edges: 0,
                boundary_loops: 0,
                euler: 0,
                orientable: !get(&non_orientable, root as usize),
                genus: None,
            });
        }
        if let Some(c) = out.get_mut(entry as usize) {
            c.triangles += 1;
            for &v in tri {
                if let Some(seen) = vertex_seen.get_mut(v as usize)
                    && !*seen
                {
                    *seen = true;
                    c.vertices += 1;
                }
            }
        }
    }
    let mut boundary = Dsu::new(n);
    let mut loop_counted = vec![false; n];
    for g in edges.groups() {
        let Some(e) = g.first() else { continue };
        let entry = get(&slot, parts.find(e.lo) as usize);
        if let Some(c) = out.get_mut(entry as usize) {
            c.edges += 1;
        }
        if g.len() == 1 {
            boundary.union(e.lo, e.hi);
        }
    }
    for g in edges.groups().filter(|g| g.len() == 1) {
        let Some(e) = g.first() else { continue };
        let root = boundary.find(e.lo);
        if let Some(counted) = loop_counted.get_mut(root as usize)
            && !*counted
        {
            *counted = true;
            let entry = get(&slot, parts.find(e.lo) as usize);
            if let Some(c) = out.get_mut(entry as usize) {
                c.boundary_loops += 1;
            }
        }
    }
    for c in &mut out {
        c.euler = i64::from(c.vertices) - i64::from(c.edges) + i64::from(c.triangles);
        let twice = 2 - c.euler - i64::from(c.boundary_loops);
        if c.orientable && twice >= 0 && twice % 2 == 0 {
            c.genus = u32::try_from(twice / 2).ok();
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::float_cmp,
    reason = "test values are small and exact enough"
)]
mod tests {
    use std::f64::consts::PI;

    use proptest::prelude::*;

    use super::*;
    use crate::testing::{cube, sphere};

    const LIMITS: Limits = Limits::DEFAULT;

    fn scale_mesh(mesh: &TriMesh, k: f64, shift: Vector3) -> TriMesh {
        let positions = mesh
            .positions()
            .iter()
            .map(|p| Point3::from(p.coords * k + shift))
            .collect();
        TriMesh::new(positions, mesh.triangles().to_vec(), &LIMITS).unwrap()
    }

    /// A torus with major radius 3, minor radius 1.
    fn torus(n: u32, m: u32) -> TriMesh {
        let mut positions = Vec::new();
        for i in 0..n {
            let u = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            for j in 0..m {
                let v = std::f64::consts::TAU * f64::from(j) / f64::from(m);
                let r = 3.0 + v.cos();
                positions.push(Point3::new(r * u.cos(), r * u.sin(), v.sin()));
            }
        }
        let at = |i: u32, j: u32| (i % n) * m + j % m;
        let mut triangles = Vec::new();
        for i in 0..n {
            for j in 0..m {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
            }
        }
        TriMesh::new(positions, triangles, &LIMITS).unwrap()
    }

    fn two_cubes() -> TriMesh {
        let a = cube();
        let b = scale_mesh(&a, 1.0, Vector3::new(5.0, 0.0, 0.0));
        TriMesh::merge([&a, &b], &LIMITS).unwrap()
    }

    #[test]
    fn box_closed_forms() {
        let m = scale_mesh(&cube(), 2.0, Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(surface_area(&m, &LIMITS).unwrap(), 24.0);
        let p = volume_and_centroid(&m, &LIMITS).unwrap();
        assert!((p.volume - 8.0).abs() < 1e-12);
        assert!((p.centroid - Point3::new(2.0, 3.0, 4.0)).norm() < 1e-12);
        assert_eq!(bounds(&m).max(), Point3::new(3.0, 4.0, 5.0));
        let t = component_topology(&m, &LIMITS).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(
            (t[0].euler, t[0].genus, t[0].boundary_loops),
            (2, Some(0), 0)
        );
    }

    #[test]
    fn tetrahedron_closed_forms() {
        let positions = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        let m = TriMesh::new(
            positions,
            vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
            &LIMITS,
        )
        .unwrap();
        let p = volume_and_centroid(&m, &LIMITS).unwrap();
        assert!((p.volume - 1.0 / 6.0).abs() < 1e-15);
        assert!((p.centroid - Point3::new(0.25, 0.25, 0.25)).norm() < 1e-15);
        assert!((surface_area(&m, &LIMITS).unwrap() - (1.5 + 3.0_f64.sqrt() / 2.0)).abs() < 1e-14);
    }

    #[test]
    fn sphere_converges_within_the_tessellation_bound() {
        let r = 2.0;
        for n in [8_u32, 16, 32] {
            let s = sphere(Point3::new(1.0, -1.0, 0.5), r, n, 2 * n);
            let area = surface_area(&s, &LIMITS).unwrap();
            let v = volume_and_centroid(&s, &LIMITS).unwrap();
            let bound = 4.0 / f64::from(n * n);
            assert!(area <= 4.0 * PI * r * r && area >= 4.0 * PI * r * r * (1.0 - bound * 4.0));
            let exact = 4.0 / 3.0 * PI * r * r * r;
            assert!(v.volume <= exact && v.volume >= exact * (1.0 - bound * 4.0));
            assert!((v.centroid - Point3::new(1.0, -1.0, 0.5)).norm() < 1e-9);
            let t = component_topology(&s, &LIMITS).unwrap();
            assert_eq!(t[0].genus, Some(0));
        }
    }

    #[test]
    fn torus_has_genus_one_and_known_volume() {
        let t = torus(48, 24);
        let c = component_topology(&t, &LIMITS).unwrap();
        assert_eq!((c.len(), c[0].euler, c[0].genus), (1, 0, Some(1)));
        let v = volume(&t, &LIMITS).unwrap();
        let exact = 2.0 * PI * PI * 3.0;
        assert!((v - exact).abs() < 0.02 * exact);
    }

    #[test]
    fn disjoint_boxes_are_two_components() {
        let m = two_cubes();
        let c = component_topology(&m, &LIMITS).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[1].first_triangle, 12);
        assert!(c.iter().all(|c| c.genus == Some(0) && c.vertices == 8));
        assert!((volume(&m, &LIMITS).unwrap() - 2.0).abs() < 1e-12);
    }

    #[test]
    fn open_and_non_manifold_meshes_have_specific_codes() {
        let mut open = cube().into_builder().build(&LIMITS).unwrap();
        open.triangles.pop();
        assert_eq!(
            volume(&open, &LIMITS).unwrap_err().code(),
            "mesh.measure.not_closed"
        );
        let open_topology = component_topology(&open, &LIMITS).unwrap();
        assert_eq!(open_topology[0].boundary_loops, 1);
        assert_eq!(open_topology[0].genus, Some(0));
        // Three triangles sharing one edge.
        let fin = TriMesh::new(
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(0.0, -1.0, 0.0),
                Point3::new(0.0, 0.0, 1.0),
            ],
            vec![[0, 1, 2], [0, 1, 3], [0, 1, 4]],
            &LIMITS,
        )
        .unwrap();
        assert_eq!(
            component_topology(&fin, &LIMITS).unwrap_err().code(),
            "mesh.topology.non_manifold"
        );
        // Two cubes sharing a corner vertex are pinched.
        let a = cube();
        let b = scale_mesh(&a, 1.0, Vector3::new(1.0, 1.0, 1.0));
        let pinched = TriMesh::merge([&a, &b], &LIMITS).unwrap();
        assert_eq!(
            component_topology(&pinched, &LIMITS).unwrap_err().code(),
            "mesh.topology.non_manifold"
        );
        assert_eq!(
            volume_and_centroid(&TriMesh::default(), &LIMITS)
                .unwrap_err()
                .code(),
            "mesh.measure.not_closed"
        );
    }

    #[test]
    fn flat_closed_mesh_has_no_centroid() {
        let m = TriMesh::new(
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 2, 1]],
            &LIMITS,
        )
        .unwrap();
        assert_eq!(
            volume_and_centroid(&m, &LIMITS).unwrap_err().code(),
            "mesh.measure.zero_volume"
        );
        assert_eq!(volume(&m, &LIMITS).unwrap(), 0.0);
    }

    #[test]
    fn split_vertices_measure_like_the_welded_mesh() {
        // The sphere helper carries UVs; weld nothing, split every corner instead.
        let s = sphere(Point3::origin(), 1.0, 6, 12);
        let positions: Vec<Point3> = s
            .triangles()
            .iter()
            .flatten()
            .map(|&i| s.positions()[i as usize])
            .collect();
        let triangles = (0..s.triangle_count() as u32)
            .map(|t| [3 * t, 3 * t + 1, 3 * t + 2])
            .collect();
        let split = TriMesh::new(positions, triangles, &LIMITS).unwrap();
        assert_eq!(
            volume(&split, &LIMITS).unwrap(),
            volume(&s, &LIMITS).unwrap()
        );
        assert_eq!(
            component_topology(&split, &LIMITS).unwrap()[0].genus,
            Some(0)
        );
    }

    #[test]
    fn limits_are_enforced() {
        let tight = Limits {
            array_length: 4,
            ..LIMITS
        };
        assert_eq!(
            surface_area(&cube(), &tight).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
    }

    #[test]
    fn extreme_magnitudes_work_or_fail_with_a_code() {
        for k in [1e-300, 1e-100, 1.0, 1e100, 1e150] {
            let m = scale_mesh(&cube(), k, Vector3::zeros());
            match surface_area(&m, &LIMITS) {
                Ok(a) => assert!((a / (6.0 * k * k) - 1.0).abs() < 1e-12, "{k}"),
                Err(e) => assert_eq!(e.code(), "mesh.precision.insufficient", "{k}"),
            }
            match volume_and_centroid(&m, &LIMITS) {
                Ok(p) => {
                    assert!((p.volume / (k * k * k) - 1.0).abs() < 1e-12, "{k}");
                    assert!((p.centroid.x / k - 0.5).abs() < 1e-12);
                }
                Err(e) => assert_eq!(e.code(), "mesh.precision.insufficient", "{k}"),
            }
        }
        let huge = scale_mesh(&cube(), 1e200, Vector3::zeros());
        assert_eq!(
            surface_area(&huge, &LIMITS).unwrap_err().code(),
            "mesh.precision.insufficient"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

        #[test]
        fn translated_and_scaled_boxes_measure_exactly(
            k in prop_oneof![0.001..1000.0_f64, 1e-60..1e60],
            dx in -1e3..1e3_f64, dy in -1e3..1e3_f64, dz in -1e3..1e3_f64,
        ) {
            let m = scale_mesh(&cube(), k, Vector3::new(dx, dy, dz) * k);
            let area = surface_area(&m, &LIMITS).unwrap();
            prop_assert!((area / (6.0 * k * k) - 1.0).abs() < 1e-9);
            let p = volume_and_centroid(&m, &LIMITS).unwrap();
            // Cancellation bounds the relative accuracy by the distance from the origin.
            let far = 1.0 + dx.abs().max(dy.abs()).max(dz.abs());
            prop_assert!((p.volume / (k * k * k) - 1.0).abs() < 1e-12 * far * far);
            prop_assert!((p.centroid.x / k - dx - 0.5).abs() < 1e-12 * far);
        }
    }
}
