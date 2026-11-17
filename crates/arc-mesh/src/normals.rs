//! Vertex normals and tangent frames (CORE-MESH-003).
//!
//! **Faces.** A triangle is degenerate when two of its indices are equal or its smallest height
//! (twice its area over its longest edge) is at most `tolerance.linear`. Degenerate triangles
//! contribute to no normal and no tangent. Face normals follow the winding: counterclockwise
//! seen from the side the normal points to. Every length and angle is computed from coordinates
//! scaled by their largest magnitude, so meshes with coordinates from 1e-300 to 1e300 work.
//!
//! **Normals.** [`TriMesh::with_normals`] gives every triangle corner a normal and stores it in
//! the [`AttributeKey::Normal`] channel:
//!
//! - Vertices at exactly equal positions are welded for smoothing: the faces around a corner are
//!   the non-degenerate triangles that have a vertex at the same position, so normals stay
//!   smooth across vertices split for texture seams.
//! - Around a corner of triangle `t`, a face `f` counts when the angle between the normals of
//!   `f` and `t` is not greater than the crease angle (`angle > crease` excludes it; `t` always
//!   counts). A crease angle of π smooths everything; 0 smooths only exactly parallel faces.
//! - [`NormalWeighting::Flat`] gives every corner its own face's normal and ignores the crease
//!   angle. [`NormalWeighting::Area`] sums the counted face normals weighted by face area;
//!   [`NormalWeighting::Angle`] weighted by the face's interior angle at that position. The sum
//!   is normalized.
//! - A corner of a degenerate triangle takes the weighted normal of all faces around it
//!   (area-weighted for `Flat`), with no crease test. A corner or vertex without any
//!   contribution, a sum that cancels to zero, and a vertex no triangle uses get
//!   [`FALLBACK_NORMAL`].
//! - Output order: vertex `i` of the input stays vertex `i` and gets the normal of its first
//!   corner in triangle order. Each further corner whose normal (as `f32`) differs is moved to
//!   a copy of the vertex, appended after the input vertices in the order of first use; corners
//!   with equal normals share a copy. Copies carry every channel of their vertex. The tangent
//!   channel is removed, since it depends on the normals.
//!
//! **Tangents.** [`TriMesh::with_tangents`] computes, for each non-degenerate triangle whose UV
//! derivative determinant `Δu₁Δv₂ − Δu₂Δv₁` is not zero, the unit directions of increasing `u`
//! and `v` across the triangle. Each vertex sums them weighted by the interior angle of each
//! triangle at the vertex; the tangent is the `u` sum made orthogonal to the vertex normal
//! (Gram–Schmidt) and normalized, and the handedness is −1 when `(normal × tangent)` points
//! against the `v` sum and 1 otherwise. Where the `u` sum is zero or parallel to the normal
//! (degenerate UVs), the tangent is the coordinate axis least aligned with the normal (the
//! first of x, y, z on ties) made orthogonal to the normal, with handedness 1. A zero normal is
//! replaced by [`FALLBACK_NORMAL`] for this purpose. No result is ever NaN.

use std::collections::HashMap;

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::error::codes;
use crate::mesh::{AttributeKey, Channel, TriMesh};
use crate::{angle_between, unit};

/// The normal of a vertex that no non-degenerate triangle contributes to: +Z.
pub const FALLBACK_NORMAL: [f32; 3] = [0.0, 0.0, 1.0];

/// How face normals are combined at a vertex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalWeighting {
    /// Each corner takes its face's normal; vertices split wherever face normals differ.
    Flat,
    /// Face normals weighted by face area.
    Area,
    /// Face normals weighted by the interior angle at the vertex.
    Angle,
}

/// The geometry of a non-degenerate triangle.
pub(crate) struct Face {
    /// The unit normal.
    pub(crate) normal: Vector3,
    /// The natural logarithm of the area, so areas of any magnitude compare without overflow.
    ln_area: f64,
    /// The interior angle at each corner.
    angles: [f64; 3],
    /// The scaled edge vectors `b − a` and `c − a`, sharing one positive scale factor.
    edges: [Vector3; 2],
}

/// The geometry of triangle `t`, or `None` when it is degenerate at the linear tolerance.
pub(crate) fn face(positions: &[Point3], t: [u32; 3], linear: f64) -> Option<Face> {
    let [i, j, k] = t;
    if i == j || j == k || k == i {
        return None;
    }
    let point = |i: u32| positions.get(i as usize).map(|p| p.coords);
    let (a, b, c) = (point(i)?, point(j)?, point(k)?);
    let scale = a.amax().max(b.amax()).max(c.amax());
    if scale.is_nan() || scale <= 0.0 {
        return None;
    }
    let (a, b, c) = (a / scale, b / scale, c / scale);
    let (ab, ac, bc) = (b - a, c - a, c - b);
    let cross = ab.cross(&ac);
    let double_area = cross.norm();
    let longest = ab.norm().max(ac.norm()).max(bc.norm());
    let height = double_area / longest * scale;
    // NaN is degenerate.
    if height.is_nan() || height <= linear {
        return None;
    }
    Some(Face {
        // +0.0 turns −0.0 into +0.0, so equal normals have equal bits.
        normal: (cross / double_area).map(|c| c + 0.0),
        ln_area: 2.0 * scale.ln() + (double_area / 2.0).ln(),
        angles: [
            angle_between(&ab, &ac),
            angle_between(&bc, &-ab),
            angle_between(&-ac, &-bc),
        ],
        edges: [ab, ac],
    })
}

fn to_f32(v: &Vector3) -> [f32; 3] {
    arc_base::math::vector3_to_gpu(v)
        .to_array()
        .map(|c| c + 0.0)
}

fn to_f64(v: &[f32; 3]) -> Vector3 {
    Vector3::new(f64::from(v[0]), f64::from(v[1]), f64::from(v[2]))
}

impl TriMesh {
    /// This mesh with normals computed by `weighting` and `crease_angle` (radians); see the
    /// module documentation for the exact rules and the vertex order of the result.
    ///
    /// # Errors
    ///
    /// `mesh.tolerance.invalid` for a negative or non-finite `tolerance.linear`;
    /// `mesh.crease.invalid` for a crease angle outside [0, π]; `mesh.limit.exceeded` when the
    /// split vertices exceed `limits`.
    pub fn with_normals(
        &self,
        weighting: NormalWeighting,
        crease_angle: f64,
        tolerance: &Tolerance,
        limits: &Limits,
    ) -> Result<TriMesh> {
        crate::ensure_tolerance(tolerance)?;
        if !(0.0..=std::f64::consts::PI).contains(&crease_angle) {
            return Err(Error::new(codes::CREASE_INVALID));
        }
        let faces: Vec<Option<Face>> = self
            .triangles
            .iter()
            .map(|t| face(&self.positions, *t, tolerance.linear))
            .collect();

        let (class, around) = self.faces_around(&faces);

        let all = crease_angle >= std::f64::consts::PI;
        // shortcut: crease filtering costs (distinct face normals × faces) per position, quadratic
        // at a high-valence cone apex; upgrade to an angular sweep when such meshes are measured.
        let mut memo: HashMap<(usize, Option<[u64; 3]>), [f32; 3]> = HashMap::new();
        let mut corner_normal = |own: Option<&Face>, class: usize| -> [f32; 3] {
            if let (NormalWeighting::Flat, Some(own)) = (weighting, own) {
                return to_f32(&own.normal);
            }
            let filter = own.filter(|_| !all).map(|f| f.normal);
            *memo
                .entry((class, filter.map(|n| n.map(f64::to_bits).into())))
                .or_insert_with(|| {
                    let counted: Vec<(&Face, usize)> = around
                        .get(class)
                        .into_iter()
                        .flatten()
                        .filter_map(|&(f, corner)| {
                            faces.get(f).and_then(Option::as_ref).map(|g| (g, corner))
                        })
                        .filter(|(g, _)| {
                            filter.is_none_or(|n| angle_between(&g.normal, &n) <= crease_angle)
                        })
                        .collect();
                    let max_ln_area = counted
                        .iter()
                        .map(|(g, _)| g.ln_area)
                        .fold(f64::NEG_INFINITY, f64::max);
                    let sum: Vector3 = counted
                        .iter()
                        .map(|(g, corner)| {
                            let weight = match weighting {
                                NormalWeighting::Angle => {
                                    g.angles.get(*corner).copied().unwrap_or(0.0)
                                }
                                NormalWeighting::Flat | NormalWeighting::Area => {
                                    (g.ln_area - max_ln_area).exp()
                                }
                            };
                            g.normal * weight
                        })
                        .sum();
                    unit(&sum).map_or(FALLBACK_NORMAL, |n| to_f32(&n))
                })
        };

        let mut normals: Vec<Option<[f32; 3]>> = vec![None; self.positions.len()];
        let mut remap: Vec<u32> = (0..).take(self.positions.len()).collect();
        let mut copies: HashMap<(u32, [u32; 3]), u32> = HashMap::new();
        let mut triangles = Vec::with_capacity(self.triangles.len());
        for (t, geometry) in self.triangles.iter().zip(&faces) {
            let mut out = *t;
            for v in &mut out {
                let Some(&c) = class.get(*v as usize) else {
                    continue;
                };
                let normal = corner_normal(geometry.as_ref(), c);
                let bits = normal.map(f32::to_bits);
                match normals.get_mut(*v as usize) {
                    Some(slot @ None) => *slot = Some(normal),
                    Some(Some(existing)) if existing.map(f32::to_bits) == bits => {}
                    _ => {
                        let next = u32::try_from(normals.len())
                            .map_err(|_| Error::new(codes::LIMIT_EXCEEDED))?;
                        let source = *v;
                        *v = *copies.entry((source, bits)).or_insert_with(|| {
                            normals.push(Some(normal));
                            remap.push(source);
                            next
                        });
                    }
                }
            }
            triangles.push(out);
        }

        let positions = remap
            .iter()
            .filter_map(|&i| self.positions.get(i as usize).copied())
            .collect();
        let normals = normals
            .into_iter()
            .map(|n| n.unwrap_or(FALLBACK_NORMAL))
            .collect();
        self.rebuilt(positions, triangles, &remap)
            .without_channel(&AttributeKey::Tangent)
            .channel(AttributeKey::Normal, Channel::Vec3(normals))
            .build(limits)
    }

    /// The weld class of every vertex (vertices at exactly equal positions share a class) and
    /// the non-degenerate (face, corner) pairs around each class.
    fn faces_around(&self, faces: &[Option<Face>]) -> (Vec<usize>, Vec<Vec<(usize, usize)>>) {
        // Weld classes by exact position; −0.0 was stored as +0.0, so bits compare values.
        let mut class_of_position = HashMap::new();
        let class: Vec<usize> = self
            .positions
            .iter()
            .map(|p| {
                let next = class_of_position.len();
                *class_of_position
                    .entry(p.coords.map(f64::to_bits))
                    .or_insert(next)
            })
            .collect();
        let mut around: Vec<Vec<(usize, usize)>> = vec![Vec::new(); class_of_position.len()];
        for (f, (t, geometry)) in self.triangles.iter().zip(faces).enumerate() {
            if geometry.is_some() {
                for (corner, &v) in t.iter().enumerate() {
                    if let Some(list) = class.get(v as usize).and_then(|&c| around.get_mut(c)) {
                        list.push((f, corner));
                    }
                }
            }
        }
        (class, around)
    }

    /// This mesh with tangent frames computed from the normals and UV set `uv_set`, stored in
    /// the [`AttributeKey::Tangent`] channel; see the module documentation for the rules.
    ///
    /// # Errors
    ///
    /// `mesh.tolerance.invalid` for a negative or non-finite `tolerance.linear`;
    /// `mesh.channel.missing` when the mesh has no normals or no UV set `uv_set`.
    pub fn with_tangents(&self, uv_set: u32, tolerance: &Tolerance) -> Result<TriMesh> {
        crate::ensure_tolerance(tolerance)?;
        let (Some(normals), Some(uvs)) = (self.normals(), self.uv_set(uv_set)) else {
            return Err(Error::new(codes::CHANNEL_MISSING));
        };
        let mut u_sum = vec![Vector3::zeros(); self.positions.len()];
        let mut v_sum = vec![Vector3::zeros(); self.positions.len()];
        for t in &self.triangles {
            let Some(geometry) = face(&self.positions, *t, tolerance.linear) else {
                continue;
            };
            let uv = |i: u32| uvs.get(i as usize).map(|uv| uv.map(f64::from));
            let (Some(a), Some(b), Some(c)) = (uv(t[0]), uv(t[1]), uv(t[2])) else {
                continue;
            };
            let (du1, dv1, du2, dv2) = (b[0] - a[0], b[1] - a[1], c[0] - a[0], c[1] - a[1]);
            let determinant = du1 * dv2 - du2 * dv1;
            if determinant == 0.0 || !determinant.is_finite() {
                continue;
            }
            // Only directions are kept, so the division by the determinant reduces to its sign.
            let sign = determinant.signum();
            let [e1, e2] = geometry.edges;
            let (Some(du), Some(dv)) = (
                unit(&((e1 * dv2 - e2 * dv1) * sign)),
                unit(&((e2 * du1 - e1 * du2) * sign)),
            ) else {
                continue;
            };
            for (&v, angle) in t.iter().zip(geometry.angles) {
                if let (Some(u), Some(w)) = (u_sum.get_mut(v as usize), v_sum.get_mut(v as usize)) {
                    *u += du * angle;
                    *w += dv * angle;
                }
            }
        }
        let tangents = normals
            .iter()
            .zip(u_sum.iter().zip(&v_sum))
            .map(|(n, (u, v))| {
                let n = unit(&to_f64(n)).unwrap_or_else(|| to_f64(&FALLBACK_NORMAL));
                let tangent = unit(&(u - n * n.dot(u)));
                let (t, w) = match tangent {
                    Some(t) => (t, if n.cross(&t).dot(v) < 0.0 { -1.0 } else { 1.0 }),
                    None => (fallback_tangent(&n), 1.0),
                };
                let [x, y, z] = to_f32(&t);
                [x, y, z, w]
            })
            .collect();
        Ok(TriMesh {
            channels: {
                let mut channels = self.channels.clone();
                channels.insert(AttributeKey::Tangent, Channel::Vec4(tangents));
                channels
            },
            ..self.clone()
        })
    }
}

/// The coordinate axis least aligned with the unit normal `n`, made orthogonal to it.
fn fallback_tangent(n: &Vector3) -> Vector3 {
    let axis = if n.x.abs() <= n.y.abs() && n.x.abs() <= n.z.abs() {
        Vector3::x()
    } else if n.y.abs() <= n.z.abs() {
        Vector3::y()
    } else {
        Vector3::z()
    };
    // The least aligned axis makes an angle of at least about 54.7° with n, so this is nonzero.
    unit(&(axis - n * n.dot(&axis))).unwrap_or(axis)
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test values are small and exact enough"
)]
mod tests {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    use proptest::prelude::*;

    use super::*;
    use crate::TriMeshBuilder;
    use crate::testing::{cube, sphere};

    fn v(n: &[f32; 3]) -> Vector3 {
        to_f64(n)
    }

    fn crease(mesh: &TriMesh, weighting: NormalWeighting, angle: f64) -> TriMesh {
        mesh.with_normals(weighting, angle, &Tolerance::DEFAULT, &Limits::STRICT)
            .unwrap()
    }

    /// The corner normal of corner `k` of triangle `t`.
    fn corner(mesh: &TriMesh, t: usize, k: usize) -> Vector3 {
        v(&mesh.normals().unwrap()[mesh.triangles()[t][k] as usize])
    }

    #[test]
    fn a_smooth_cube_has_diagonal_corner_normals() {
        for weighting in [NormalWeighting::Area, NormalWeighting::Angle] {
            let smooth = crease(&cube(), weighting, PI);
            assert_eq!(smooth.vertex_count(), 8);
            for (p, n) in smooth.positions().iter().zip(smooth.normals().unwrap()) {
                let expected = (p.coords - Vector3::repeat(0.5)).normalize();
                // Area weighting: every corner has 3 faces' worth of equal total area per axis,
                // except the two triangles of a face split unevenly; angle weighting is exact.
                let error = angle_between(&v(n), &expected);
                if weighting == NormalWeighting::Angle {
                    assert!(error < 1e-6, "{error}");
                } else {
                    assert!(error < 0.4, "{error}");
                }
            }
        }
    }

    #[test]
    fn a_creased_cube_splits_into_flat_faces() {
        for weighting in [
            NormalWeighting::Flat,
            NormalWeighting::Area,
            NormalWeighting::Angle,
        ] {
            let creased = crease(&cube(), weighting, FRAC_PI_4);
            assert_eq!(creased.vertex_count(), 24, "{weighting:?}");
            assert_eq!(creased.triangle_count(), 12);
            for (t, triangle) in creased.triangles().iter().enumerate() {
                let geometry = face(creased.positions(), *triangle, 0.0).unwrap();
                for k in 0..3 {
                    assert_eq!(
                        corner(&creased, t, k),
                        geometry.normal.map(|c| f64::from(c as f32))
                    );
                }
            }
            // The first 8 vertices keep their positions.
            assert_eq!(&creased.positions()[..8], cube().positions());
        }
        // Exactly 90° between neighbours: a crease of exactly π/2 still smooths (`>` excludes).
        assert_eq!(
            crease(&cube(), NormalWeighting::Angle, FRAC_PI_2).vertex_count(),
            8
        );
        assert_eq!(
            crease(&cube(), NormalWeighting::Angle, FRAC_PI_2 - 1e-9).vertex_count(),
            24
        );
    }

    #[test]
    fn welded_positions_smooth_across_split_vertices() {
        // Two triangles folded at 90° along the x axis, with the hinge vertices duplicated.
        let positions = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        let mesh = TriMesh::new(positions, vec![[0, 1, 2], [3, 5, 4]], &Limits::STRICT).unwrap();
        let smooth = crease(&mesh, NormalWeighting::Angle, PI);
        assert_eq!(smooth.vertex_count(), 6);
        let hinge = v(&smooth.normals().unwrap()[0]);
        assert!(angle_between(&hinge, &Vector3::new(0.0, 1.0, 1.0)) < 1e-6);
        assert_eq!(smooth.normals().unwrap()[0], smooth.normals().unwrap()[3]);
    }

    #[test]
    fn degenerate_and_unused_vertices_get_defined_normals() {
        let positions = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(9.0, 9.0, 9.0),
        ];
        // A sliver along the x axis next to a regular triangle, and an unused vertex.
        let mesh = TriMesh::new(positions, vec![[0, 1, 2], [0, 3, 1]], &Limits::STRICT).unwrap();
        let normals = crease(&mesh, NormalWeighting::Flat, 0.0);
        let n = normals.normals().unwrap();
        assert_eq!(n[0], [0.0, 0.0, 1.0]);
        // Vertex 3 belongs only to the degenerate triangle: no contribution.
        assert_eq!(n[3], FALLBACK_NORMAL);
        assert_eq!(n[4], FALLBACK_NORMAL);
        assert!(n.iter().flatten().all(|c| c.is_finite()));
    }

    #[test]
    fn parameters_are_checked() {
        let mesh = cube();
        let tolerance = Tolerance::DEFAULT;
        for angle in [-0.1, PI + 0.1, f64::NAN] {
            assert_eq!(
                mesh.with_normals(NormalWeighting::Area, angle, &tolerance, &Limits::STRICT)
                    .unwrap_err()
                    .code(),
                "mesh.crease.invalid"
            );
        }
        let bad = Tolerance {
            linear: f64::NAN,
            ..tolerance
        };
        assert_eq!(
            mesh.with_normals(NormalWeighting::Area, 1.0, &bad, &Limits::STRICT)
                .unwrap_err()
                .code(),
            "mesh.tolerance.invalid"
        );
        let tight = Limits {
            array_length: 12,
            ..Limits::STRICT
        };
        assert_eq!(
            mesh.with_normals(NormalWeighting::Flat, 0.0, &tolerance, &tight)
                .unwrap_err()
                .code(),
            "mesh.limit.exceeded"
        );
        assert_eq!(
            mesh.with_tangents(0, &tolerance).unwrap_err().code(),
            "mesh.channel.missing"
        );
    }

    #[test]
    fn a_rotated_uv_plane_has_tangents_along_increasing_u() {
        for (angle, mirrored) in [(0.0, false), (0.7, false), (2.5, true), (-1.2, true)] {
            let (sin, cos) = f64::sin_cos(angle);
            let positions = vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
                Point3::new(2.0, 1.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ];
            let mirror = if mirrored { -1.0 } else { 1.0 };
            let uvs = positions
                .iter()
                .map(|p| {
                    let u = p.x * cos + p.y * sin;
                    let v = (-p.x * sin + p.y * cos) * mirror;
                    [u as f32, v as f32]
                })
                .collect();
            let mesh = TriMeshBuilder::new(positions, vec![[0, 1, 2], [0, 2, 3]])
                .channel(AttributeKey::Uv(1), Channel::Vec2(uvs))
                .build(&Limits::STRICT)
                .unwrap()
                .with_normals(
                    NormalWeighting::Area,
                    PI,
                    &Tolerance::DEFAULT,
                    &Limits::STRICT,
                )
                .unwrap()
                .with_tangents(1, &Tolerance::DEFAULT)
                .unwrap();
            for t in mesh.tangents().unwrap() {
                let tangent = Vector3::new(f64::from(t[0]), f64::from(t[1]), f64::from(t[2]));
                assert!(angle_between(&tangent, &Vector3::new(cos, sin, 0.0)) < 1e-6);
                assert_eq!(t[3], if mirrored { -1.0 } else { 1.0 });
            }
        }
    }

    #[test]
    fn degenerate_uvs_fall_back_to_a_perpendicular_axis() {
        let mesh = TriMeshBuilder::new(
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 1, 2]],
        )
        .channel(
            AttributeKey::Normal,
            Channel::Vec3(vec![[0.0, 0.0, 1.0], [0.0; 3], [1.0, 0.0, 0.0]]),
        )
        .channel(AttributeKey::Uv(0), Channel::Vec2(vec![[0.5, 0.5]; 3]))
        .build(&Limits::STRICT)
        .unwrap()
        .with_tangents(0, &Tolerance::DEFAULT)
        .unwrap();
        let tangents = mesh.tangents().unwrap();
        assert_eq!(tangents[0], [1.0, 0.0, 0.0, 1.0]);
        // A zero normal is treated as +Z.
        assert_eq!(tangents[1], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(tangents[2], [0.0, 1.0, 0.0, 1.0]);
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Normals of a UV sphere with `n` rings and `2n` segments are radial within 1.6 / n²
        /// radians with angle weighting (the error of the diagonally split bands is second
        /// order) and within 0.65 / n with area weighting (first order), at any center and
        /// radius.
        #[test]
        fn sphere_normals_are_radial(
            x in -1e3..1e3_f64,
            y in -1e3..1e3_f64,
            exponent in -100.0..100.0_f64,
            rings in 6_u32..24,
        ) {
            let radius = 10.0_f64.powf(exponent);
            let center = Point3::new(x, y, 0.0) * radius;
            let mesh = sphere(center, radius, rings, 2 * rings);
            let exact = Tolerance { linear: 0.0, ..Tolerance::DEFAULT };
            let n = f64::from(rings);
            for (weighting, bound) in [
                (NormalWeighting::Area, 0.65 / n),
                (NormalWeighting::Angle, 1.6 / (n * n)),
            ] {
                let smooth = mesh.with_normals(weighting, PI, &exact, &Limits::DEFAULT).unwrap();
                prop_assert_eq!(smooth.vertex_count(), mesh.vertex_count());
                for (p, n) in smooth.positions().iter().zip(smooth.normals().unwrap()) {
                    let radial = p - center;
                    prop_assert!(angle_between(&v(n), &radial) <= bound);
                }
            }
        }

        /// Tangent frames are unit, orthogonal to unit normals and never NaN, whatever the UVs.
        #[test]
        fn tangents_are_orthonormal_frames(mesh in crate::testing::mesh()) {
            let smooth = mesh
                .with_normals(NormalWeighting::Angle, FRAC_PI_4, &Tolerance::DEFAULT, &Limits::DEFAULT)
                .unwrap();
            let framed = smooth.with_tangents(3, &Tolerance::DEFAULT).unwrap();
            for (n, t) in framed.normals().unwrap().iter().zip(framed.tangents().unwrap()) {
                let n = v(n);
                let tangent = Vector3::new(f64::from(t[0]), f64::from(t[1]), f64::from(t[2]));
                prop_assert!((n.norm() - 1.0).abs() < 1e-6);
                prop_assert!((tangent.norm() - 1.0).abs() < 1e-6);
                prop_assert!(n.dot(&tangent).abs() < 1e-6);
                prop_assert!(t[3] == 1.0 || t[3] == -1.0);
            }
        }

        /// Every corner normal is unit and the result is deterministic; creasing never changes
        /// the triangles' positions.
        #[test]
        fn normals_are_unit_deterministic_and_keep_geometry(
            mesh in crate::testing::mesh(),
            crease_angle in 0.0..=PI,
            weighting in prop_oneof![
                Just(NormalWeighting::Flat),
                Just(NormalWeighting::Area),
                Just(NormalWeighting::Angle),
            ],
        ) {
            let once = mesh.with_normals(weighting, crease_angle, &Tolerance::DEFAULT, &Limits::DEFAULT).unwrap();
            let twice = mesh.with_normals(weighting, crease_angle, &Tolerance::DEFAULT, &Limits::DEFAULT).unwrap();
            prop_assert_eq!(&once, &twice);
            prop_assert!(once.tangents().is_none());
            for n in once.normals().unwrap() {
                prop_assert!((v(n).norm() - 1.0).abs() < 1e-6);
            }
            for (a, b) in once.triangles().iter().zip(mesh.triangles()) {
                for (i, j) in a.iter().zip(b) {
                    prop_assert_eq!(once.positions()[*i as usize], mesh.positions()[*j as usize]);
                }
            }
            prop_assert_eq!(&once.positions()[..mesh.vertex_count()], mesh.positions());
        }
    }
}
