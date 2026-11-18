//! Vertex normals and tangent frames (CORE-MESH-003).
//!
//! **Faces.** A triangle is degenerate when two of its indices are equal or its smallest height
//! (twice its area over its longest edge) is at most `tolerance.linear`. Degenerate triangles
//! contribute to no normal and no tangent. Face normals follow the winding: counterclockwise
//! seen from the side the normal points to. Every length and angle is computed from the edge
//! vectors (vertex differences, exact for nearby vertices; from halved coordinates where a
//! difference would overflow) scaled by their largest magnitude, so a small triangle far from
//! the origin keeps its precision and coordinates from 1e-300 to 1e300 work.
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
//!   The faces at a position are searched through a tree over their normals: whole nodes inside
//!   or outside the crease angle are taken or skipped at once, and only faces near the boundary
//!   are tested one by one. The search takes at most 256 steps per element of
//!   `limits.array_length` and fails with `mesh.limit.exceeded` beyond that.
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
use crate::{angle_between, length, unit};

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
    /// The edge vectors `b − a` and `c − a`, sharing one positive scale factor.
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
    // Differences first, so a small triangle far from the origin keeps its precision (the
    // difference of close numbers is exact). Where a difference overflows, the halved
    // coordinates are subtracted instead: halving a number that large is exact.
    let mut edges = (b - a, c - a, c - b);
    let mut halved = false;
    if !edges
        .0
        .iter()
        .chain(&edges.1)
        .chain(&edges.2)
        .all(|c| c.is_finite())
    {
        let (a, b, c) = (a / 2.0, b / 2.0, c / 2.0);
        edges = (b - a, c - a, c - b);
        halved = true;
    }
    let scale = edges.0.amax().max(edges.1.amax()).max(edges.2.amax());
    if !(scale > 0.0 && scale.is_finite()) {
        return None;
    }
    let (ab, ac, bc) = (edges.0 / scale, edges.1 / scale, edges.2 / scale);
    let cross = ab.cross(&ac);
    let double_area = cross.norm();
    let longest = ab.norm().max(ac.norm()).max(bc.norm());
    let height = double_area / longest * scale * if halved { 2.0 } else { 1.0 };
    // NaN is degenerate.
    if height.is_nan() || height <= linear {
        return None;
    }
    let ln_scale = scale.ln() + if halved { std::f64::consts::LN_2 } else { 0.0 };
    Some(Face {
        // +0.0 turns −0.0 into +0.0, so equal normals have equal bits.
        normal: (cross / double_area).map(|c| c + 0.0),
        ln_area: 2.0 * ln_scale + (double_area / 2.0).ln(),
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

/// The crease search may take this many steps (tree nodes and face tests) per element of
/// `Limits::array_length` before `with_normals` fails with `mesh.limit.exceeded`.
const CREASE_WORK_PER_ELEMENT: u64 = 256;

/// The faces of a fan at most this many per leaf of its tree.
const LEAF_SIZE: usize = 8;

/// Rounding allowance on chord lengths between unit normals: a tree node is taken whole or
/// skipped whole only when every face in it is decided the same way by the exact angle test.
const CHORD_MARGIN: f64 = 64.0 * f64::EPSILON;

/// A weighted sum of normals, `vector · e^ln_scale`, kept in logarithmic scale so that weights
/// of any magnitude combine without overflow.
#[derive(Debug, Clone, Copy)]
struct Sum {
    ln_scale: f64,
    vector: Vector3,
}

impl Sum {
    const ZERO: Self = Self {
        ln_scale: f64::NEG_INFINITY,
        vector: Vector3::new(0.0, 0.0, 0.0),
    };

    fn add(self, other: Self) -> Self {
        if other.ln_scale == f64::NEG_INFINITY {
            return self;
        }
        if self.ln_scale == f64::NEG_INFINITY {
            return other;
        }
        let ln_scale = self.ln_scale.max(other.ln_scale);
        Self {
            ln_scale,
            vector: self.vector * (self.ln_scale - ln_scale).exp()
                + other.vector * (other.ln_scale - ln_scale).exp(),
        }
    }
}

/// One face corner around a position: its face normal (for the crease test) and its weighted
/// contribution `value · e^ln_weight`.
struct Entry {
    normal: Vector3,
    value: Vector3,
    ln_weight: f64,
}

impl Entry {
    fn sum(&self) -> Sum {
        Sum {
            ln_scale: self.ln_weight,
            vector: self.value,
        }
    }
}

/// A node of a fan's tree: the entries `start..end`, the box of their normals, their sum and
/// the two children of an inner node (0 for a leaf; the root is never a child).
struct Node {
    start: usize,
    end: usize,
    min: Vector3,
    max: Vector3,
    sum: Sum,
    children: [usize; 2],
}

/// The faces around one position in a tree over their normals (points on the unit sphere),
/// so the faces within the crease angle of a normal are summed without visiting all of them.
struct Fan {
    entries: Vec<Entry>,
    nodes: Vec<Node>,
}

impl Fan {
    fn new(mut entries: Vec<Entry>) -> Self {
        let mut nodes: Vec<Node> = Vec::new();
        // (start, end, parent, side); built in preorder without recursion.
        let mut pending = vec![(0, entries.len(), None::<(usize, usize)>)];
        while let Some((start, end, parent)) = pending.pop() {
            let index = nodes.len();
            if let Some((parent, side)) = parent
                && let Some(slot) = nodes.get_mut(parent).and_then(|p| p.children.get_mut(side))
            {
                *slot = index;
            }
            let slice = entries.get_mut(start..end).unwrap_or_default();
            let mut min = Vector3::repeat(f64::INFINITY);
            let mut max = Vector3::repeat(f64::NEG_INFINITY);
            for e in slice.iter() {
                min = min.inf(&e.normal);
                max = max.sup(&e.normal);
            }
            nodes.push(Node {
                start,
                end,
                min,
                max,
                sum: Sum::ZERO,
                children: [0, 0],
            });
            if slice.len() > LEAF_SIZE {
                let axis = (max - min).imax();
                let middle = slice.len() / 2;
                slice.select_nth_unstable_by(middle, |a, b| {
                    a.normal[axis].total_cmp(&b.normal[axis])
                });
                pending.push((start + middle, end, Some((index, 1))));
                pending.push((start, start + middle, Some((index, 0))));
            }
        }
        // Children come after their parent, so a reverse pass sums bottom-up.
        for index in (0..nodes.len()).rev() {
            let Some(node) = nodes.get(index) else {
                continue;
            };
            let sum = if node.children == [0, 0] {
                entries
                    .get(node.start..node.end)
                    .unwrap_or_default()
                    .iter()
                    .fold(Sum::ZERO, |s, e| s.add(e.sum()))
            } else {
                node.children
                    .iter()
                    .filter_map(|&c| nodes.get(c))
                    .fold(Sum::ZERO, |s, c| s.add(c.sum))
            };
            if let Some(node) = nodes.get_mut(index) {
                node.sum = sum;
            }
        }
        Self { entries, nodes }
    }

    /// The fan of the (face, corner) pairs `around` a position, weighted by `weighting`.
    fn around(
        around: &[(usize, usize)],
        faces: &[Option<Face>],
        weighting: NormalWeighting,
    ) -> Self {
        Self::new(
            around
                .iter()
                .filter_map(|&(f, corner)| {
                    let g = faces.get(f)?.as_ref()?;
                    Some(match weighting {
                        // Angle weights are applied exactly; area weights through their
                        // logarithms, so areas of any magnitude combine.
                        NormalWeighting::Angle => Entry {
                            normal: g.normal,
                            value: g.normal * g.angles.get(corner).copied().unwrap_or(0.0),
                            ln_weight: 0.0,
                        },
                        NormalWeighting::Flat | NormalWeighting::Area => Entry {
                            normal: g.normal,
                            value: g.normal,
                            ln_weight: g.ln_area,
                        },
                    })
                })
                .collect(),
        )
    }

    fn total(&self) -> Sum {
        self.nodes.first().map_or(Sum::ZERO, |root| root.sum)
    }
}

/// The crease test of one `with_normals` call, with its work budget.
struct Search {
    crease_angle: f64,
    /// A node whose farthest normal is closer than this chord is inside the crease angle.
    inside: f64,
    /// A node whose nearest normal is farther than this chord is outside it.
    outside: f64,
    work: u64,
    budget: u64,
}

impl Search {
    fn step(&mut self) -> Result<()> {
        self.work += 1;
        if self.work > self.budget {
            Err(Error::new(codes::LIMIT_EXCEEDED))
        } else {
            Ok(())
        }
    }

    /// The sum over the faces of `fan` whose normal is within the crease angle of `n`.
    fn sum(&mut self, fan: &Fan, n: &Vector3) -> Result<Sum> {
        let mut total = Sum::ZERO;
        let mut pending = vec![0_usize];
        while let Some(index) = pending.pop() {
            self.step()?;
            let Some(node) = fan.nodes.get(index) else {
                continue;
            };
            if node.start == node.end {
                continue;
            }
            let nearest = n.sup(&node.min).inf(&node.max) - n;
            let farthest = (n - node.min).abs().sup(&(node.max - n).abs());
            if nearest.norm() > self.outside {
                continue;
            }
            if farthest.norm() < self.inside {
                total = total.add(node.sum);
                continue;
            }
            if node.children == [0, 0] {
                for e in fan.entries.get(node.start..node.end).unwrap_or_default() {
                    self.step()?;
                    if angle_between(&e.normal, n) <= self.crease_angle {
                        total = total.add(e.sum());
                    }
                }
            } else {
                pending.extend(node.children);
            }
        }
        Ok(total)
    }
}

impl TriMesh {
    /// This mesh with normals computed by `weighting` and `crease_angle` (radians); see the
    /// module documentation for the exact rules and the vertex order of the result.
    ///
    /// # Errors
    ///
    /// `mesh.tolerance.invalid` for a negative or non-finite `tolerance.linear`;
    /// `mesh.crease.invalid` for a crease angle outside [0, π]; `mesh.limit.exceeded` when the
    /// split vertices exceed `limits`, or when the crease search exceeds its work budget of
    /// 256 steps per element of `limits.array_length`.
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
        let chord = 2.0 * (crease_angle / 2.0).sin();
        let mut search = Search {
            crease_angle,
            inside: chord - CHORD_MARGIN,
            outside: chord + CHORD_MARGIN,
            work: 0,
            budget: limits.array_length.saturating_mul(CREASE_WORK_PER_ELEMENT),
        };
        let mut fans: Vec<Option<Fan>> = Vec::new();
        fans.resize_with(around.len(), || None);
        let mut memo: HashMap<(usize, Option<[u64; 3]>), [f32; 3]> = HashMap::new();
        let mut corner_normal = |own: Option<&Face>, class: usize| -> Result<[f32; 3]> {
            if let (NormalWeighting::Flat, Some(own)) = (weighting, own) {
                return Ok(to_f32(&own.normal));
            }
            let filter = own.filter(|_| !all).map(|f| f.normal);
            let key = (class, filter.map(|n| n.map(f64::to_bits).into()));
            if let Some(normal) = memo.get(&key) {
                return Ok(*normal);
            }
            let Some(slot) = fans.get_mut(class) else {
                return Ok(FALLBACK_NORMAL);
            };
            let fan = slot.get_or_insert_with(|| {
                Fan::around(
                    around.get(class).map_or(&[][..], Vec::as_slice),
                    &faces,
                    weighting,
                )
            });
            let sum = match filter {
                None => fan.total(),
                Some(n) => search.sum(fan, &n)?,
            };
            let normal = unit(&sum.vector).map_or(FALLBACK_NORMAL, |n| to_f32(&n));
            memo.insert(key, normal);
            Ok(normal)
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
                let normal = corner_normal(geometry.as_ref(), c)?;
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
                let residual = u - n * n.dot(u);
                // Below this share of |u| the residual is rounding noise, not a direction.
                let tangent = unit(&residual).filter(|_| length(&residual) > PARALLEL * length(u));
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

/// Relative size below which a tangent sum counts as parallel to the normal (`u` is exact to
/// about 1e-16, so the orthogonalized remainder keeps at least eight digits above that).
const PARALLEL: f64 = 1e-8;

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

    /// arc-core #50: huge edges next to tiny ones with huge UV deltas once broke the frame.
    #[test]
    fn tangent_frames_survive_extreme_edges_and_uv_deltas() {
        let mut positions = vec![Point3::origin(); 17];
        positions[1] = Point3::new(0.0, -1.851_751_582_032_504e71, 0.0);
        positions[7] = Point3::new(0.0, 0.0, -59.232_837_913_994_37);
        positions[10] = Point3::new(5.886_688_892_696_375e64, 3.887_473_012_328_774e66, 0.0);
        positions[14] = Point3::new(-72.664_668_130_336_1, 0.0, 0.0);
        let mut uvs = vec![[0.0_f32; 2]; 17];
        uvs[1] = [-8.851_156, 8.765_436];
        uvs[2] = [1.937_496_8e25, -8.503_627];
        uvs[7] = [0.0, -66_355.15];
        uvs[10] = [5.953_736, -7.175_503_7];
        let mesh = TriMeshBuilder::new(positions, vec![[2, 10, 7], [7, 1, 2]])
            .channel(AttributeKey::Uv(3), Channel::Vec2(uvs))
            .build(&Limits::STRICT)
            .unwrap()
            .with_normals(
                NormalWeighting::Angle,
                FRAC_PI_4,
                &Tolerance::DEFAULT,
                &Limits::DEFAULT,
            )
            .unwrap()
            .with_tangents(3, &Tolerance::DEFAULT)
            .unwrap();
        for (n, t) in mesh.normals().unwrap().iter().zip(mesh.tangents().unwrap()) {
            let tangent = Vector3::new(f64::from(t[0]), f64::from(t[1]), f64::from(t[2]));
            assert!((tangent.norm() - 1.0).abs() < 1e-6, "{t:?}");
            assert!(v(n).dot(&tangent).abs() < 1e-6, "{n:?} {t:?}");
            assert!(t[3] == 1.0 || t[3] == -1.0);
        }
    }

    /// A cone of `k` faces around an apex at (0, 0, 1) over the unit circle.
    fn cone(k: u32) -> TriMesh {
        let mut positions = vec![Point3::new(0.0, 0.0, 1.0)];
        positions.extend((0..k).map(|i| {
            let (sin, cos) = (std::f64::consts::TAU * f64::from(i) / f64::from(k)).sin_cos();
            Point3::new(cos, sin, 0.0)
        }));
        let triangles = (0..k).map(|i| [0, 1 + i, 1 + (i + 1) % k]).collect();
        TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap()
    }

    /// The normal of corner `corner` of triangle `t` by the definition: the counted faces at its
    /// position summed directly, one by one.
    fn reference_normal(
        mesh: &TriMesh,
        weighting: NormalWeighting,
        crease_angle: f64,
        t: usize,
        corner: usize,
    ) -> Vector3 {
        let positions = mesh.positions();
        let triangles = mesh.triangles();
        let own = face(positions, triangles[t], 0.0).unwrap();
        let at = positions[triangles[t][corner] as usize];
        let mut sum = Vector3::zeros();
        for tri in triangles {
            let Some(g) = face(positions, *tri, 0.0) else {
                continue;
            };
            for (k, &v) in tri.iter().enumerate() {
                if positions[v as usize] == at
                    && angle_between(&g.normal, &own.normal) <= crease_angle
                {
                    let w = match weighting {
                        NormalWeighting::Angle => g.angles[k],
                        _ => (g.ln_area - own.ln_area).exp(),
                    };
                    sum += g.normal * w;
                }
            }
        }
        sum.normalize()
    }

    #[test]
    #[ignore = "takes about 20 s in a debug build; run with --run-ignored only"]
    fn a_cone_apex_of_valence_100000_is_creased_in_near_linear_time() {
        let k = 100_000;
        let mesh = cone(k);
        let start = std::time::Instant::now();
        let creased = mesh
            .with_normals(
                NormalWeighting::Angle,
                0.5,
                &Tolerance::DEFAULT,
                &Limits::DEFAULT,
            )
            .unwrap();
        // Quadratic filtering took over four minutes in an optimized build; the tree takes well
        // under a second there. The bound leaves room for unoptimized builds and slow machines.
        assert!(start.elapsed().as_secs() < 60, "{:?}", start.elapsed());
        // By symmetry every apex corner normal makes the same angle with the axis and lies in
        // the plane through the axis and its face normal.
        let axis = Vector3::z();
        let first = corner(&creased, 0, 0);
        for t in (0..k as usize).step_by(997) {
            let n = corner(&creased, t, 0);
            let own = face(creased.positions(), creased.triangles()[t], 0.0)
                .unwrap()
                .normal;
            assert!((angle_between(&n, &axis) - angle_between(&first, &axis)).abs() < 1e-6);
            assert!(n.dot(&axis.cross(&own)).abs() < 1e-6);
        }
        // The apex is a copy per face; the base vertices are smooth around the circle.
        assert_eq!(creased.vertex_count(), mesh.vertex_count() + k as usize - 1);
    }

    #[test]
    #[ignore = "takes about 30 s in a debug build; run with --run-ignored only"]
    fn a_fan_of_scattered_normals_fails_when_the_crease_search_exceeds_its_budget() {
        // 20,000 faces around one apex with normals scattered over the sphere: the faces within
        // the crease angle of a normal are bounded by a circle that crosses many tree nodes.
        let k = 20_000_u32;
        let mut positions = vec![Point3::origin()];
        let mut triangles = Vec::new();
        for i in 0..k {
            let a = f64::from(i) * 2.399_963;
            let b = f64::from(i) * 0.618_034 * std::f64::consts::TAU;
            positions.push(Point3::new(a.cos(), a.sin(), (a * 0.37).sin()));
            positions.push(Point3::new(b.sin(), (b * 1.3).cos(), b.cos()));
            triangles.push([0, 1 + 2 * i, 2 + 2 * i]);
        }
        let mesh = TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap();
        // Room for every split vertex (flat normals split the apex once per face), but a budget
        // of 256 steps per element is below what this fan needs.
        let tight = Limits {
            array_length: u64::from(3 * k),
            ..Limits::DEFAULT
        };
        let tolerance = Tolerance::DEFAULT;
        assert!(
            mesh.with_normals(NormalWeighting::Flat, 1.0, &tolerance, &tight)
                .is_ok()
        );
        let result = mesh.with_normals(NormalWeighting::Area, 1.0, &tolerance, &tight);
        assert_eq!(result.unwrap_err().code(), "mesh.limit.exceeded");
    }

    #[test]
    fn the_crease_search_stops_at_its_budget() {
        let entries = || {
            (0..2_000_u32)
                .map(|i| {
                    let a = f64::from(i) * 2.399_963;
                    let n = Vector3::new(a.cos(), a.sin(), (a * 0.37).sin()).normalize();
                    Entry {
                        normal: n,
                        value: n,
                        ln_weight: 0.0,
                    }
                })
                .collect()
        };
        let fan = Fan::new(entries());
        let search = |budget| Search {
            crease_angle: 1.0,
            inside: 2.0 * 0.5_f64.sin() - CHORD_MARGIN,
            outside: 2.0 * 0.5_f64.sin() + CHORD_MARGIN,
            work: 0,
            budget,
        };
        let mut unlimited = search(u64::MAX);
        let n = Vector3::new(0.6, 0.0, 0.8);
        let sum = unlimited.sum(&fan, &n).unwrap();
        // The tree visits far fewer than all 2,000 faces, and sums the same faces as a scan.
        assert!(unlimited.work < 1_000, "{}", unlimited.work);
        let direct: Vector3 = fan
            .entries
            .iter()
            .filter(|e| angle_between(&e.normal, &n) <= 1.0)
            .map(|e| e.value)
            .sum();
        assert!((sum.vector * sum.ln_scale.exp() - direct).norm() < 1e-9);
        let mut tight = search(unlimited.work - 1);
        assert_eq!(
            tight.sum(&fan, &n).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// The tree search sums exactly the faces the crease rule counts: corner normals agree
        /// with a direct sum over every face at the position, on fans large enough to need it.
        #[test]
        fn crease_search_matches_the_direct_sum(
            directions in prop::collection::vec(
                (prop::array::uniform3(-1.0..1.0_f64), prop::array::uniform3(-1.0..1.0_f64)),
                20..200,
            ),
            crease_angle in 0.0..PI,
            angle_weighted in any::<bool>(),
        ) {
            let mut positions = vec![Point3::origin()];
            let mut triangles = Vec::new();
            for (i, (a, b)) in (0_u32..).zip(&directions) {
                positions.push(Point3::from(*a));
                positions.push(Point3::from(*b));
                triangles.push([0, 1 + 2 * i, 2 + 2 * i]);
            }
            let mesh = TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap();
            let weighting = if angle_weighted { NormalWeighting::Angle } else { NormalWeighting::Area };
            let exact = Tolerance { linear: 0.0, ..Tolerance::DEFAULT };
            let smooth = mesh.with_normals(weighting, crease_angle, &exact, &Limits::DEFAULT).unwrap();
            for t in 0..mesh.triangle_count() {
                if face(mesh.positions(), mesh.triangles()[t], 0.0).is_none() {
                    continue;
                }
                let expected = reference_normal(&mesh, weighting, crease_angle, t, 0);
                prop_assert!(angle_between(&corner(&smooth, t, 0), &expected) < 1e-6);
            }
        }

        /// A small triangle far from the origin gets the same normals, tangents and degeneracy
        /// as the same triangle moved to the origin: its edges are exact differences.
        #[test]
        fn small_triangles_far_from_the_origin_keep_their_precision(
            base in prop::array::uniform3(1e6..1e15_f64),
            offsets in prop::array::uniform9(-1.0..1.0_f64),
            size_exponent in -6.0..1.0_f64,
            uvs in prop::array::uniform6(-1.0..1.0_f32),
        ) {
            let size = 10.0_f64.powf(size_exponent);
            let base = Point3::from(base);
            let far: Vec<Point3> = offsets
                .chunks(3)
                .map(|o| base + Vector3::new(o[0], o[1], o[2]) * size)
                .collect();
            // The differences of close numbers are exact, so `near` has the same edges.
            let near: Vec<Point3> = far.iter().map(|p| Point3::from(p - far[0])).collect();
            let build = |positions: Vec<Point3>| {
                TriMeshBuilder::new(positions, vec![[0, 1, 2]])
                    .channel(
                        AttributeKey::Uv(0),
                        Channel::Vec2(uvs.chunks(2).map(|c| [c[0], c[1]]).collect()),
                    )
                    .build(&Limits::STRICT)
                    .unwrap()
            };
            let (far, near) = (build(far), build(near));
            let tolerance = Tolerance { linear: size * 1e-3, ..Tolerance::DEFAULT };
            prop_assert_eq!(
                far.degenerate_triangles(&tolerance).unwrap(),
                near.degenerate_triangles(&tolerance).unwrap()
            );
            let frame = |mesh: &TriMesh| {
                mesh.with_normals(NormalWeighting::Angle, 1.0, &tolerance, &Limits::STRICT)
                    .unwrap()
                    .with_tangents(0, &tolerance)
                    .unwrap()
            };
            let (far, near) = (frame(&far), frame(&near));
            prop_assert_eq!(far.normals(), near.normals());
            prop_assert_eq!(far.tangents(), near.tangents());
            // And the normal is the exact one, up to the f32 rounding of the output.
            let p = near.positions();
            if let Some(exact) = unit(&(p[1] - p[0]).cross(&(p[2] - p[0])))
                && face(p, [0, 1, 2], tolerance.linear).is_some()
            {
                prop_assert!(angle_between(&v(&far.normals().unwrap()[0]), &exact) < 1e-6);
            }
        }

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
