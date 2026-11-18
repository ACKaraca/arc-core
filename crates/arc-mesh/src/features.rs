//! Feature edges: boundary edges and sharp edges (CORE-MESH-024).
//!
//! Vertices at exactly equal positions are welded first, so a mesh whose vertices are split
//! for normals or texture seams gives the same edges as the welded mesh. Triangles that are
//! degenerate at the linear tolerance have no normal and are ignored.

use arc_base::{Error, Limits, Result, Tolerance};

use crate::TriMesh;
use crate::error::codes;
use crate::topo::{check_mesh, exact_canon, get};
use crate::{angle_between, ensure_tolerance, normals};

/// An edge of the welded mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeatureEdge {
    /// The end vertices as indices of the input mesh: the lowest index of the vertices at each
    /// end position, in increasing order.
    pub vertices: [u32; 2],
    /// The lowest-numbered triangle using the edge.
    pub triangle: u32,
    /// The second-lowest triangle using the edge, if any.
    pub other: Option<u32>,
    /// The number of triangles using the edge.
    pub triangle_count: u32,
    /// The angle between the outward normals of the two triangles in [0, π] (zero for a flat
    /// edge, π/2 for the edge of a box); zero for boundary and non-manifold edges.
    pub angle: f64,
}

/// The classified edges of a mesh, each list ordered by vertex indices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeatureEdges {
    /// Edges used by exactly one triangle.
    pub boundary: Vec<FeatureEdge>,
    /// Edges used by exactly two triangles whose normals differ by more than the threshold.
    pub sharp: Vec<FeatureEdge>,
    /// Edges used by more than two triangles.
    pub non_manifold: Vec<FeatureEdge>,
}

/// Extracts boundary, sharp and non-manifold edges.
///
/// An edge of two triangles is sharp when the angle between their normals, taken as if the
/// triangles were wound consistently across the edge (so the result does not depend on the
/// winding of either triangle), is greater than `threshold` radians. An angle equal to the
/// threshold is not sharp.
///
/// # Errors
///
/// `mesh.crease.invalid` when `threshold` is not in [0, π]; `mesh.tolerance.invalid` for an
/// invalid tolerance; `mesh.limit.exceeded` beyond `limits`.
pub fn feature_edges(
    mesh: &TriMesh,
    threshold: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<FeatureEdges> {
    ensure_tolerance(tolerance)?;
    if !(0.0..=std::f64::consts::PI).contains(&threshold) {
        return Err(Error::new(codes::CREASE_INVALID));
    }
    check_mesh(mesh, limits, 160)?;
    let canon = exact_canon(&mesh.positions);
    let normal_of: Vec<Option<arc_base::math::Vector3>> = mesh
        .triangles
        .iter()
        .map(|&t| normals::face(&mesh.positions, t, tolerance.linear).map(|f| f.normal))
        .collect();
    let edges = crate::topo::Edges::build((0..).zip(&mesh.triangles).filter_map(|(i, t)| {
        get(&normal_of, i as usize).map(|_| (i, t.map(|v| get(&canon, v as usize))))
    }));
    let mut out = FeatureEdges::default();
    for g in edges.groups() {
        let Some(first) = g.first() else { continue };
        let mut edge = FeatureEdge {
            vertices: [first.lo, first.hi],
            triangle: first.tri,
            other: g.get(1).map(|e| e.tri),
            triangle_count: u32::try_from(g.len()).unwrap_or(u32::MAX),
            angle: 0.0,
        };
        match g {
            [_] => out.boundary.push(edge),
            [a, b] => {
                let (Some(na), Some(nb)) = (
                    get(&normal_of, a.tri as usize),
                    get(&normal_of, b.tri as usize),
                ) else {
                    continue;
                };
                // Triangles running the edge the same way disagree on which side is outside.
                let nb = if a.forward == b.forward { -nb } else { nb };
                edge.angle = angle_between(&na, &nb);
                if edge.angle > threshold {
                    out.sharp.push(edge);
                }
            }
            _ => out.non_manifold.push(edge),
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, reason = "test values are small")]
mod tests {
    use arc_base::math::Point3;
    use std::f64::consts::{FRAC_PI_2, PI};

    use super::*;
    use crate::NormalWeighting;
    use crate::testing::{cube, sphere};

    const TOL: Tolerance = Tolerance::DEFAULT;
    const LIMITS: Limits = Limits::DEFAULT;

    #[test]
    fn box_has_twelve_sharp_edges_below_a_right_angle_and_none_above() {
        for degrees in [0.0_f64, 1.0, 45.0, 89.0, 89.999] {
            let f = feature_edges(&cube(), degrees.to_radians(), &TOL, &LIMITS).unwrap();
            assert_eq!(f.sharp.len(), 12, "{degrees}");
            assert!(f.boundary.is_empty() && f.non_manifold.is_empty());
            assert!(f.sharp.iter().all(|e| (e.angle - FRAC_PI_2).abs() < 1e-12));
        }
        for threshold in [FRAC_PI_2, 91.0_f64.to_radians(), PI] {
            let f = feature_edges(&cube(), threshold, &TOL, &LIMITS).unwrap();
            assert!(f.sharp.is_empty(), "{threshold}");
        }
    }

    #[test]
    fn winding_of_a_triangle_does_not_change_the_edges() {
        let reference = feature_edges(&cube(), 0.5, &TOL, &LIMITS).unwrap();
        let mut triangles = cube().triangles().to_vec();
        for t in triangles.iter_mut().step_by(3) {
            t.swap(1, 2);
        }
        let flipped = TriMesh::new(cube().positions().to_vec(), triangles, &LIMITS).unwrap();
        let f = feature_edges(&flipped, 0.5, &TOL, &LIMITS).unwrap();
        assert_eq!(f.sharp.len(), reference.sharp.len());
    }

    #[test]
    fn open_grid_has_only_boundary() {
        let n = 5_u32;
        let positions = (0..=n * (n + 1) + n)
            .map(|i| Point3::new(f64::from(i % (n + 1)), f64::from(i / (n + 1)), 0.0))
            .collect();
        let mut triangles = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * (n + 1) + x;
                triangles.push([a, a + 1, a + n + 2]);
                triangles.push([a, a + n + 2, a + n + 1]);
            }
        }
        let grid = TriMesh::new(positions, triangles, &LIMITS).unwrap();
        for threshold in [0.0, 0.3, PI] {
            let f = feature_edges(&grid, threshold, &TOL, &LIMITS).unwrap();
            assert_eq!(f.boundary.len(), 4 * n as usize);
            assert!(f.sharp.is_empty() && f.non_manifold.is_empty());
            assert!(f.boundary.iter().all(|e| e.other.is_none()));
        }
    }

    #[test]
    fn split_normals_give_the_same_edges_as_the_welded_mesh() {
        let welded = sphere(Point3::origin(), 1.0, 8, 16);
        let split = welded
            .with_normals(NormalWeighting::Flat, 0.0, &TOL, &LIMITS)
            .unwrap();
        assert!(split.vertex_count() > welded.vertex_count());
        let a = feature_edges(&welded, 0.3, &TOL, &LIMITS).unwrap();
        let b = feature_edges(&split, 0.3, &TOL, &LIMITS).unwrap();
        assert_eq!(a.sharp.len(), b.sharp.len());
        assert_eq!(a.boundary.len(), b.boundary.len());
        assert!(b.boundary.is_empty());
        // Same positions at the end points.
        let pos = |m: &TriMesh, e: &FeatureEdge| e.vertices.map(|v| m.positions()[v as usize]);
        let mut pa: Vec<_> = a.sharp.iter().map(|e| pos(&welded, e)).collect();
        let mut pb: Vec<_> = b.sharp.iter().map(|e| pos(&split, e)).collect();
        let key = |p: &[Point3; 2]| p.map(|q| [q.x.to_bits(), q.y.to_bits(), q.z.to_bits()]);
        pa.sort_by_key(key);
        pb.sort_by_key(key);
        assert_eq!(pa, pb);
    }

    #[test]
    fn non_manifold_edges_are_reported_separately_and_input_is_validated() {
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
        let f = feature_edges(&fin, 0.1, &TOL, &LIMITS).unwrap();
        assert_eq!(f.non_manifold.len(), 1);
        assert_eq!(f.non_manifold[0].triangle_count, 3);
        assert_eq!(f.boundary.len(), 6);
        for bad in [-0.1, PI + 0.1, f64::NAN] {
            assert_eq!(
                feature_edges(&cube(), bad, &TOL, &LIMITS)
                    .unwrap_err()
                    .code(),
                "mesh.crease.invalid"
            );
        }
        let bad_tol = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(
            feature_edges(&cube(), 0.1, &bad_tol, &LIMITS)
                .unwrap_err()
                .code(),
            "mesh.tolerance.invalid"
        );
    }
}
