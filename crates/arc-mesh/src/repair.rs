//! Mesh repair with a change report (CORE-MESH-018).
//!
//! [`repair`] applies these steps in order and reports each:
//!
//! 1. **Weld.** Vertices are visited in index order. A vertex joins the lowest-numbered earlier
//!    representative that is within `tolerance.linear` of it and whose attributes agree;
//!    otherwise it becomes a representative. A merged vertex takes the position and attributes
//!    of its representative, so the result is deterministic and never drifts through chains of
//!    nearby vertices. Attributes agree when every component of every channel differs by at
//!    most `tolerance.linear`, unless [`RepairOptions::positions_only`] is set, which ignores
//!    attributes. A tolerance of 0 welds exactly equal positions.
//! 2. **Degenerate and duplicate triangles.** A triangle that has a repeated vertex after
//!    welding, or is degenerate at the linear tolerance (see [`TriMesh::degenerate_triangles`]),
//!    is removed. Of several triangles with the same three vertices, in any winding, the first
//!    is kept.
//! 3. **Orientation.** Triangles are connected across edges shared by exactly two triangles.
//!    In each such component the triangle with the lowest index keeps its winding and the others
//!    are reversed where needed to run shared edges in opposite directions. A component that
//!    cannot be oriented (a Möbius strip) is counted and left as the walk reached it.
//! 4. **Holes.** Each simple loop of boundary edges with at most `max_hole_edges` edges is
//!    closed by ear clipping in the loop's best-fit plane (the Newell normal), with no new
//!    vertices. A diagonal that already is an edge of the mesh is not used, so filling never
//!    makes an edge non-manifold. A loop that is pinched, too long or has no ear is counted as
//!    skipped and left open.
//! 5. **Outward.** A component whose edges are all shared by exactly two consistently wound
//!    triangles is closed; one with a negative signed volume is reversed.
//! 6. **Compaction.** Vertices no triangle uses are removed; the rest keep their order.
//!
//! Channels follow their vertices. Face groups follow their triangles: triangles keep their
//! order, a group covers its surviving triangles, a group left without triangles is dropped,
//! and triangles added by hole filling belong to no group. When any winding changed, the normal
//! and tangent channels no longer match the triangles and are dropped (recompute them with
//! `with_normals` and `with_tangents`); the report names them.

use std::collections::HashMap;
use std::collections::HashSet;

use arc_base::math::Vector3;
use arc_base::{Error, Limits, Result, Tolerance};

use crate::distance::scale_of;
use crate::error::codes;
use crate::topo::{Adj, Budget, Edges, Sum, check_mesh, flipped, get, idx, index_degenerate, pos};
use crate::{AttributeKey, FaceGroup, TriMesh, TriMeshBuilder, normals};

/// The longest hole loop, in edges, that [`RepairOptions::max_hole_edges`] may ask for.
pub const MAX_HOLE_EDGES: u32 = 1024;

/// Options of [`repair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairOptions {
    /// Weld vertices by position alone, ignoring their attributes.
    pub positions_only: bool,
    /// The most edges of a hole to fill; 0 fills none. At most [`MAX_HOLE_EDGES`].
    pub max_hole_edges: u32,
    /// The most indices listed in each [`Change`].
    pub max_reported: usize,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            positions_only: false,
            max_hole_edges: 16,
            max_reported: 1000,
        }
    }
}

/// One kind of change: how many elements it touched and which (indices of the input mesh).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// The number of elements changed.
    pub count: u64,
    /// The first `max_reported` indices, in increasing order.
    pub indices: Vec<u32>,
    /// Whether more elements changed than `indices` lists.
    pub truncated: bool,
}

impl Change {
    fn push(&mut self, index: u32, max: usize) {
        self.count += 1;
        if self.indices.len() < max {
            self.indices.push(index);
        } else {
            self.truncated = true;
        }
    }
}

/// What [`repair`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Input vertices merged into another vertex.
    pub welded_vertices: Change,
    /// Input triangles removed as degenerate.
    pub degenerate_triangles: Change,
    /// Input triangles removed as duplicates of an earlier triangle.
    pub duplicate_triangles: Change,
    /// Input vertices that no triangle uses after repair (and that were not merged).
    pub unused_vertices: Change,
    /// Input triangles whose winding is reversed in the output.
    pub flipped_triangles: Change,
    /// Components that could not be oriented consistently.
    pub non_orientable_components: u32,
    /// Closed components reversed to face outward.
    pub inverted_components: u32,
    /// Holes closed.
    pub holes_filled: u32,
    /// Triangles added to close holes, appended after the others.
    pub hole_triangles_added: u32,
    /// Boundary loops left open (too long, pinched or without an ear).
    pub holes_skipped: u32,
    /// Channels dropped because windings changed.
    pub dropped_channels: Vec<AttributeKey>,
}

/// A repaired mesh and the report of what changed.
#[derive(Debug, Clone, PartialEq)]
pub struct Repaired {
    /// The repaired mesh.
    pub mesh: TriMesh,
    /// What changed.
    pub report: RepairReport,
}

/// Cells of the weld grid and the vertices that represent clusters.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is clamped into range first"
)]
fn cell(p: &Vector3, size: f64) -> [i64; 3] {
    p.map(|c| {
        if size > 0.0 {
            (c / size).floor().clamp(-4.0e18, 4.0e18) as i64
        } else {
            i64::from_ne_bytes(c.to_bits().to_ne_bytes())
        }
    })
    .into()
}

fn attributes_agree(mesh: &TriMesh, a: usize, b: usize, linear: f64) -> bool {
    mesh.channels.values().all(|ch| {
        let k = ch.components();
        let floats = ch.floats();
        let (x, y) = (
            floats.get(a * k..(a + 1) * k),
            floats.get(b * k..(b + 1) * k),
        );
        match (x, y) {
            (Some(x), Some(y)) => x
                .iter()
                .zip(y)
                .all(|(p, q)| (f64::from(*p) - f64::from(*q)).abs() <= linear),
            _ => false,
        }
    })
}

/// For every vertex its representative (the lowest-numbered vertex of its cluster).
fn weld(
    mesh: &TriMesh,
    options: &RepairOptions,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<u32>> {
    let size = tolerance.linear;
    let mut budget = Budget::new(limits, 256);
    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut rep: Vec<u32> = (0..idx(mesh.positions.len())).collect();
    let reach: &[i64] = if size > 0.0 { &[-1, 0, 1] } else { &[0] };
    for i in 0..idx(mesh.positions.len()) {
        let p = pos(&mesh.positions, i);
        let home = cell(&p, size);
        let mut found: Option<u32> = None;
        for dx in reach {
            for dy in reach {
                for dz in reach {
                    let key = [
                        home[0].saturating_add(*dx),
                        home[1].saturating_add(*dy),
                        home[2].saturating_add(*dz),
                    ];
                    for &r in grid.get(&key).map_or(&[][..], Vec::as_slice) {
                        budget.spend(1)?;
                        let near = crate::length(&(pos(&mesh.positions, r) - p)) <= size;
                        if near
                            && found.is_none_or(|f| r < f)
                            && (options.positions_only
                                || attributes_agree(mesh, r as usize, i as usize, size))
                        {
                            found = Some(r);
                        }
                    }
                }
            }
        }
        match found {
            Some(r) => {
                if let Some(slot) = rep.get_mut(i as usize) {
                    *slot = r;
                }
            }
            None => grid.entry(home).or_default().push(i),
        }
    }
    Ok(rep)
}

/// The working triangle list: vertices (of the input numbering), where each came from, and
/// whether its winding differs from the input.
struct Work {
    tris: Vec<[u32; 3]>,
    origin: Vec<Option<u32>>,
    reversed: Vec<bool>,
}

impl Work {
    fn reverse(&mut self, t: usize) {
        if let Some(tri) = self.tris.get_mut(t) {
            *tri = flipped(*tri);
        }
        if let Some(r) = self.reversed.get_mut(t) {
            *r = !*r;
        }
    }
}

/// Components of triangles joined across two-triangle edges: the component of every
/// triangle, the number of components that cannot be oriented, and for each triangle whether
/// it must be reversed to agree with the lowest-numbered triangle of its component.
fn orientation(tris: &[[u32; 3]], adj: &[[Adj; 3]]) -> (Vec<u32>, u32, Vec<bool>) {
    let mut component = vec![u32::MAX; tris.len()];
    let mut reverse = vec![false; tris.len()];
    let mut conflicts = 0;
    for start in 0..tris.len() {
        if get(&component, start) != u32::MAX {
            continue;
        }
        let id = idx(start);
        let mut conflict = false;
        if let Some(c) = component.get_mut(start) {
            *c = id;
        }
        let mut stack = vec![idx(start)];
        while let Some(t) = stack.pop() {
            let mine = get(&reverse, t as usize);
            for a in get(adj, t as usize) {
                let Adj::Pair { tri, same } = a else { continue };
                let want = mine ^ same;
                if get(&component, tri as usize) == u32::MAX {
                    if let Some(c) = component.get_mut(tri as usize) {
                        *c = id;
                    }
                    if let Some(r) = reverse.get_mut(tri as usize) {
                        *r = want;
                    }
                    stack.push(tri);
                } else if get(&reverse, tri as usize) != want {
                    conflict = true;
                }
            }
        }
        conflicts += u32::from(conflict);
    }
    (component, conflicts, reverse)
}

/// The simple loops of boundary edges as vertex sequences following the edge directions, and
/// the number of boundary chains that are not simple loops.
fn boundary_loops(tris: &[[u32; 3]], vertices: usize) -> (Vec<Vec<u32>>, u32) {
    let edges = Edges::build((0..).zip(tris.iter().copied()));
    let mut next = vec![u32::MAX; vertices];
    let mut out_count = vec![0_u32; vertices];
    let mut starts = Vec::new();
    for g in edges.groups().filter(|g| g.len() == 1) {
        let Some(e) = g.first() else { continue };
        let tri = get(tris, e.tri as usize);
        let (from, to) = (
            get(&tri, usize::from(e.slot)),
            get(&tri, (usize::from(e.slot) + 1) % 3),
        );
        if let (Some(n), Some(c)) = (
            next.get_mut(from as usize),
            out_count.get_mut(from as usize),
        ) {
            *n = to;
            *c += 1;
        }
        starts.push(from);
    }
    starts.sort_unstable();
    let mut done = vec![false; vertices];
    let (mut loops, mut bad) = (Vec::new(), 0);
    for &s in &starts {
        if get(&done, s as usize) {
            continue;
        }
        let mut path = vec![s];
        let mut simple = true;
        let mut v = s;
        loop {
            if let Some(d) = done.get_mut(v as usize) {
                *d = true;
            }
            if get(&out_count, v as usize) != 1 {
                simple = false;
                break;
            }
            v = get(&next, v as usize);
            if v == s {
                break;
            }
            if get(&done, v as usize) {
                simple = false;
                break;
            }
            path.push(v);
        }
        if simple {
            loops.push(path);
        } else {
            bad += 1;
        }
    }
    (loops, bad)
}

fn cross2(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Triangulates the polygon `poly` (vertices in order) by ear clipping in its best-fit plane,
/// or `None` when there is no valid triangulation.
fn ear_clip(
    mesh: &TriMesh,
    poly: &[u32],
    existing: &HashSet<(u32, u32)>,
    budget: &mut Budget,
) -> Result<Option<Vec<[u32; 3]>>> {
    let Some(&first) = poly.first() else {
        return Ok(None);
    };
    let origin = pos(&mesh.positions, first);
    let rel: Vec<Vector3> = poly
        .iter()
        .map(|&i| pos(&mesh.positions, i) - origin)
        .collect();
    let scale = scale_of(&rel);
    if !scale.is_finite() {
        return Ok(None);
    }
    let q: Vec<Vector3> = rel.iter().map(|v| v / scale).collect();
    let mut normal = Vector3::zeros();
    for (i, p) in q.iter().enumerate() {
        normal += p.cross(&get(&q, (i + 1) % q.len()));
    }
    let Some(n) = crate::unit(&normal) else {
        return Ok(None);
    };
    let axis = if n.x.abs() <= n.y.abs() && n.x.abs() <= n.z.abs() {
        Vector3::x()
    } else if n.y.abs() <= n.z.abs() {
        Vector3::y()
    } else {
        Vector3::z()
    };
    let Some(u) = crate::unit(&n.cross(&axis)) else {
        return Ok(None);
    };
    let v = n.cross(&u);
    let flat: Vec<[f64; 2]> = q.iter().map(|p| [p.dot(&u), p.dot(&v)]).collect();
    let key = |a: u32, b: u32| (a.min(b), a.max(b));
    let mut added: HashSet<(u32, u32)> = HashSet::new();
    let mut ring: Vec<usize> = (0..poly.len()).collect();
    let mut triangles = Vec::new();
    let (mut at, mut misses) = (0_usize, 0_usize);
    while ring.len() > 3 {
        budget.spend(ring.len())?;
        let len = ring.len();
        at %= len;
        let (ia, ib, ic) = (
            get(&ring, (at + len - 1) % len),
            get(&ring, at),
            get(&ring, (at + 1) % len),
        );
        let (pa, pb, pc) = (get(&flat, ia), get(&flat, ib), get(&flat, ic));
        let (va, vb, vc) = (get(poly, ia), get(poly, ib), get(poly, ic));
        let ear = cross2(pa, pb, pc) > 0.0
            && !existing.contains(&key(va, vc))
            && !added.contains(&key(va, vc))
            && ring.iter().all(|&k| {
                k == ia || k == ib || k == ic || {
                    let p = get(&flat, k);
                    !(cross2(pa, pb, p) >= 0.0
                        && cross2(pb, pc, p) >= 0.0
                        && cross2(pc, pa, p) >= 0.0)
                }
            });
        if ear {
            triangles.push([va, vb, vc]);
            added.insert(key(va, vc));
            ring.remove(at);
            misses = 0;
        } else {
            at += 1;
            misses += 1;
            if misses > len {
                return Ok(None);
            }
        }
    }
    let [a, b, c] = [0, 1, 2].map(|k| get(&ring, k));
    if cross2(get(&flat, a), get(&flat, b), get(&flat, c)) <= 0.0 {
        return Ok(None);
    }
    triangles.push([get(poly, a), get(poly, b), get(poly, c)]);
    Ok(Some(triangles))
}

/// Closes the boundary loops of at most `max_edges` edges; returns the holes filled and the
/// loops left open.
fn fill_holes(
    mesh: &TriMesh,
    work: &mut Work,
    max_edges: u32,
    report: &mut RepairReport,
    limits: &Limits,
) -> Result<()> {
    let (loops, mut skipped) = boundary_loops(&work.tris, mesh.positions.len());
    let mut existing: HashSet<(u32, u32)> = Edges::build((0..).zip(work.tris.iter().copied()))
        .uses
        .iter()
        .map(|e| (e.lo, e.hi))
        .collect();
    let mut budget = Budget::new(limits, 256);
    for path in loops {
        if path.len() > max_edges as usize {
            skipped += 1;
            continue;
        }
        // The new triangles run each boundary edge backwards.
        let polygon: Vec<u32> = path.iter().rev().copied().collect();
        match ear_clip(mesh, &polygon, &existing, &mut budget)? {
            Some(new) => {
                report.holes_filled += 1;
                for t in new {
                    for k in 0..3 {
                        let (a, b) = (get(&t, k), get(&t, (k + 1) % 3));
                        existing.insert((a.min(b), a.max(b)));
                    }
                    work.tris.push(t);
                    work.origin.push(None);
                    work.reversed.push(false);
                    report.hole_triangles_added += 1;
                }
            }
            None => skipped += 1,
        }
    }
    report.holes_skipped = skipped;
    Ok(())
}

/// Reverses every closed, consistently wound component with a negative signed volume.
fn make_outward(mesh: &TriMesh, work: &mut Work, report: &mut RepairReport) {
    let edges = Edges::build((0..).zip(work.tris.iter().copied()));
    let adj = edges.adjacency(work.tris.len());
    let (component, _, _) = orientation(&work.tris, &adj);
    let (q, _) = crate::measure::scaled(mesh);
    let mut closed = std::collections::BTreeMap::<u32, bool>::new();
    let mut volume = std::collections::BTreeMap::<u32, Sum>::new();
    for (t, tri) in work.tris.iter().enumerate() {
        let id = get(&component, t);
        let ok = get(&adj, t)
            .iter()
            .all(|a| matches!(a, Adj::Pair { same: false, .. }))
            && !index_degenerate(*tri);
        let entry = closed.entry(id).or_insert(true);
        *entry &= ok;
        let [a, b, c] = tri.map(|i| get(&q, i as usize));
        volume.entry(id).or_default().add(a.dot(&b.cross(&c)));
    }
    let inverted: Vec<u32> = closed
        .iter()
        .filter(|(id, ok)| **ok && volume.get(id).is_some_and(|v| v.value() < 0.0))
        .map(|(id, _)| *id)
        .collect();
    for t in 0..work.tris.len() {
        if inverted.contains(&get(&component, t)) {
            work.reverse(t);
        }
    }
    report.inverted_components = idx(inverted.len());
}

/// Repairs `mesh`; see the module documentation for the steps and their rules.
///
/// # Errors
///
/// `mesh.tolerance.invalid` for an invalid tolerance; `mesh.repair.invalid` when
/// `max_hole_edges` exceeds [`MAX_HOLE_EDGES`]; `mesh.limit.exceeded` when the mesh or the work
/// exceeds `limits`.
pub fn repair(
    mesh: &TriMesh,
    options: &RepairOptions,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Repaired> {
    crate::ensure_tolerance(tolerance)?;
    if options.max_hole_edges > MAX_HOLE_EDGES {
        return Err(Error::new(codes::REPAIR_INVALID).with_detail("option", "max_hole_edges"));
    }
    check_mesh(mesh, limits, 256)?;
    let max = options.max_reported;
    let mut report = RepairReport::default();
    let rep = weld(mesh, options, tolerance, limits)?;
    for (i, &r) in rep.iter().enumerate() {
        if r as usize != i {
            report.welded_vertices.push(idx(i), max);
        }
    }
    let mut work = Work {
        tris: Vec::new(),
        origin: Vec::new(),
        reversed: Vec::new(),
    };
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    for (i, t) in (0..).zip(&mesh.triangles) {
        let t = t.map(|v| get(&rep, v as usize));
        if normals::face(&mesh.positions, t, tolerance.linear).is_none() {
            report.degenerate_triangles.push(i, max);
            continue;
        }
        let mut key = t;
        key.sort_unstable();
        if !seen.insert(key) {
            report.duplicate_triangles.push(i, max);
            continue;
        }
        work.tris.push(t);
        work.origin.push(Some(i));
        work.reversed.push(false);
    }
    let adj = Edges::build((0..).zip(work.tris.iter().copied())).adjacency(work.tris.len());
    let (_, conflicts, reverse) = orientation(&work.tris, &adj);
    report.non_orientable_components = conflicts;
    for (t, r) in reverse.iter().enumerate() {
        if *r {
            work.reverse(t);
        }
    }
    fill_holes(mesh, &mut work, options.max_hole_edges, &mut report, limits)?;
    make_outward(mesh, &mut work, &mut report);
    assemble(mesh, work, rep, report, max, limits)
}

/// Compacts the vertices and builds the output mesh with its groups and channels.
fn assemble(
    mesh: &TriMesh,
    work: Work,
    rep: Vec<u32>,
    mut report: RepairReport,
    max: usize,
    limits: &Limits,
) -> Result<Repaired> {
    let mut used = vec![false; mesh.positions.len()];
    for v in work.tris.iter().flatten() {
        if let Some(u) = used.get_mut(*v as usize) {
            *u = true;
        }
    }
    let mut remap = Vec::new();
    let mut new_index = vec![u32::MAX; mesh.positions.len()];
    for (i, &u) in used.iter().enumerate() {
        if u {
            if let Some(n) = new_index.get_mut(i) {
                *n = idx(remap.len());
            }
            remap.push(idx(i));
        } else if get(&rep, i) as usize == i {
            report.unused_vertices.push(idx(i), max);
        }
    }
    let positions = remap
        .iter()
        .map(|&i| mesh.positions.get(i as usize).copied().unwrap_or_default())
        .collect();
    let triangles: Vec<[u32; 3]> = work
        .tris
        .iter()
        .map(|t| t.map(|v| get(&new_index, v as usize)))
        .collect();
    let mut survivor = vec![u32::MAX; mesh.triangles.len()];
    for (t, origin) in work.origin.iter().enumerate() {
        if let Some(o) = origin {
            if let Some(s) = survivor.get_mut(*o as usize) {
                *s = idx(t);
            }
            if get(&work.reversed, t) {
                report.flipped_triangles.push(*o, max);
            }
        }
    }
    let windings_changed = work.reversed.iter().any(|r| *r);
    let mut builder = TriMeshBuilder::new(positions, triangles);
    for (key, channel) in &mesh.channels {
        if windings_changed && matches!(key, AttributeKey::Normal | AttributeKey::Tangent) {
            report.dropped_channels.push(key.clone());
        } else {
            builder = builder.channel(key.clone(), channel.gather(&remap));
        }
    }
    for g in &mesh.groups {
        let kept: Vec<u32> = (g.start..g.end)
            .map(|t| get(&survivor, t as usize))
            .filter(|&s| s != u32::MAX)
            .collect();
        if let (Some(&start), Some(&last)) = (kept.first(), kept.last()) {
            builder = builder.group(FaceGroup {
                start,
                end: last + 1,
                ..*g
            });
        }
    }
    Ok(Repaired {
        mesh: builder.build(limits)?,
        report,
    })
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "test values are small"
)]
mod tests {
    use arc_base::math::Point3;
    use proptest::prelude::*;

    use super::*;
    use crate::measure::{component_topology, volume};
    use crate::testing::{cube, sphere};
    use crate::{Channel, SubObjectKey};

    const TOL: Tolerance = Tolerance::DEFAULT;
    const LIMITS: Limits = Limits::DEFAULT;

    fn run(mesh: &TriMesh) -> Repaired {
        repair(mesh, &RepairOptions::default(), &TOL, &LIMITS).unwrap()
    }

    /// A small deterministic generator for choosing what to inject.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    #[test]
    fn a_clean_mesh_is_unchanged() {
        let r = run(&sphere(Point3::origin(), 1.0, 6, 10));
        assert_eq!(r.mesh, sphere(Point3::origin(), 1.0, 6, 10));
        assert_eq!(r.report, RepairReport::default());
    }

    #[test]
    fn welding_follows_position_and_attribute_rules() {
        let positions = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(TOL.linear * 0.4, 0.0, 0.0),
            Point3::new(TOL.linear * 3.0, 0.0, 0.0),
        ];
        let uv = vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 0.0], [0.5, 0.5]];
        let mesh = TriMeshBuilder::new(positions, vec![[0, 1, 2], [3, 2, 1], [4, 1, 2]])
            .channel(AttributeKey::Uv(0), Channel::Vec2(uv))
            .build(&LIMITS)
            .unwrap();
        let r = run(&mesh);
        // Vertex 3 joins 0; vertex 4 is too far from 0 and has different attributes.
        assert_eq!(r.report.welded_vertices.indices, vec![3]);
        // [3,2,1] becomes a duplicate of [0,1,2] (same vertices, opposite winding).
        assert_eq!(r.report.duplicate_triangles.indices, vec![1]);
        let positions_only = RepairOptions {
            positions_only: true,
            ..RepairOptions::default()
        };
        let near = TriMeshBuilder::new(
            vec![
                Point3::origin(),
                Point3::new(0.0, 0.0, TOL.linear * 0.5),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(0.0, 1.0, 0.0),
            ],
            vec![[0, 2, 3], [1, 3, 2]],
        )
        .channel(
            AttributeKey::Uv(0),
            Channel::Vec2(vec![[0.0, 0.0], [0.7, 0.7], [1.0, 0.0], [0.0, 1.0]]),
        )
        .build(&LIMITS)
        .unwrap();
        assert_eq!(run(&near).report.welded_vertices.count, 0);
        let merged = repair(&near, &positions_only, &TOL, &LIMITS).unwrap();
        assert_eq!(merged.report.welded_vertices.indices, vec![1]);
        assert_eq!(merged.mesh.uv_set(0).unwrap()[0], [0.0, 0.0]);
        // Tolerance 0 welds only identical positions.
        let exact = Tolerance { linear: 0.0, ..TOL };
        let r = repair(&near, &positions_only, &exact, &LIMITS).unwrap();
        assert_eq!(r.report.welded_vertices.count, 0);
    }

    #[test]
    fn groups_and_channels_follow_their_triangles() {
        let mut m = cube().into_builder();
        let g = |start, end, local| FaceGroup {
            start,
            end,
            material: local,
            key: SubObjectKey {
                object: None,
                local,
            },
        };
        m = m.group(g(0, 4, 1)).group(g(4, 6, 2)).group(g(6, 12, 3));
        let mut mesh = m.build(&LIMITS).unwrap();
        // Make triangles 4 and 5 (the whole second group) duplicates of triangle 0 and 1.
        mesh.triangles[4] = mesh.triangles[0];
        mesh.triangles[5] = mesh.triangles[1];
        let r = run(&mesh);
        assert_eq!(r.report.duplicate_triangles.indices, vec![4, 5]);
        let groups = r.mesh.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            (groups[0].start, groups[0].end, groups[0].material),
            (0, 4, 1)
        );
        assert_eq!(
            (groups[1].start, groups[1].end, groups[1].material),
            (4, 10, 3)
        );
        // The cube lost two faces, leaving a hole of 4 edges that is closed after the group.
        assert_eq!(r.mesh.triangle_count(), 12);
        assert!(r.mesh.group_of(11).is_none());
    }

    #[test]
    fn holes_respect_the_edge_limit_and_never_create_non_manifold_edges() {
        let mut mesh = cube();
        mesh.triangles.truncate(10);
        let skip = repair(
            &mesh,
            &RepairOptions {
                max_hole_edges: 3,
                ..RepairOptions::default()
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(
            (skip.report.holes_filled, skip.report.holes_skipped),
            (0, 1)
        );
        assert_eq!(skip.mesh.triangle_count(), 10);
        let filled = run(&mesh);
        assert_eq!(
            (
                filled.report.holes_filled,
                filled.report.hole_triangles_added
            ),
            (1, 2)
        );
        assert!((volume(&filled.mesh, &LIMITS).unwrap() - 1.0).abs() < 1e-12);
        let none = repair(
            &mesh,
            &RepairOptions {
                max_hole_edges: 0,
                ..RepairOptions::default()
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(none.mesh.triangle_count(), 10);
    }

    #[test]
    fn inverted_and_flipped_closed_meshes_face_outward() {
        let mut mesh = cube();
        for t in &mut mesh.triangles {
            t.swap(1, 2);
        }
        let r = run(&mesh);
        assert_eq!(r.report.inverted_components, 1);
        assert_eq!(r.report.flipped_triangles.count, 12);
        assert!((volume(&r.mesh, &LIMITS).unwrap() - 1.0).abs() < 1e-12);
        // Normals follow the old winding, so they are dropped when windings change.
        let with_normals = mesh
            .with_normals(
                crate::NormalWeighting::Angle,
                std::f64::consts::PI,
                &TOL,
                &LIMITS,
            )
            .unwrap();
        let r = run(&with_normals);
        assert_eq!(r.report.dropped_channels, vec![AttributeKey::Normal]);
        assert!(r.mesh.normals().is_none());
    }

    #[test]
    fn a_mobius_strip_is_reported_not_oriented() {
        let n = 8_u32;
        let mut positions = Vec::new();
        for i in 0..n {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
            for w in [-0.3, 0.3] {
                let r = 2.0 + w * (a / 2.0).cos();
                positions.push(Point3::new(r * a.cos(), r * a.sin(), w * (a / 2.0).sin()));
            }
        }
        let mut tris = Vec::new();
        for i in 0..n {
            let (a, b) = (2 * i, 2 * ((i + 1) % n));
            // The last quad joins with a twist.
            let (c, d) = if i + 1 == n { (b + 1, b) } else { (b, b + 1) };
            tris.push([a, c, a + 1]);
            tris.push([a + 1, c, d]);
        }
        let mesh = TriMesh::new(positions, tris, &LIMITS).unwrap();
        let r = repair(
            &mesh,
            &RepairOptions {
                max_hole_edges: 0,
                ..RepairOptions::default()
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(r.report.non_orientable_components, 1);
    }

    #[test]
    fn options_and_limits_are_validated() {
        let bad = RepairOptions {
            max_hole_edges: MAX_HOLE_EDGES + 1,
            ..RepairOptions::default()
        };
        assert_eq!(
            repair(&cube(), &bad, &TOL, &LIMITS).unwrap_err().code(),
            "mesh.repair.invalid"
        );
        let nan = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(
            repair(&cube(), &RepairOptions::default(), &nan, &LIMITS)
                .unwrap_err()
                .code(),
            "mesh.tolerance.invalid"
        );
        let tight = Limits {
            array_length: 4,
            ..LIMITS
        };
        assert_eq!(
            repair(&cube(), &RepairOptions::default(), &TOL, &tight)
                .unwrap_err()
                .code(),
            "mesh.limit.exceeded"
        );
        let empty = repair(
            &TriMesh::default(),
            &RepairOptions::default(),
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(empty.mesh, TriMesh::default());
        // Reports are bounded.
        let few = RepairOptions {
            max_reported: 1,
            ..RepairOptions::default()
        };
        let mut m = cube();
        m.triangles
            .extend_from_slice(&[[0, 0, 1], [1, 1, 2], [2, 2, 3]]);
        let r = repair(&m, &few, &TOL, &LIMITS).unwrap();
        assert_eq!(r.report.degenerate_triangles.count, 3);
        assert_eq!(r.report.degenerate_triangles.indices, vec![12]);
        assert!(r.report.degenerate_triangles.truncated);
    }

    #[test]
    fn extreme_magnitudes_are_repaired_or_rejected_with_a_code() {
        for k in [1e-150, 1e-6, 1.0, 1e150] {
            let positions = cube().positions().iter().map(|p| p * k).collect();
            let mut triangles = cube().triangles().to_vec();
            triangles.pop();
            let m = TriMesh::new(positions, triangles, &LIMITS).unwrap();
            let tol = Tolerance {
                linear: k * 1e-9,
                ..TOL
            };
            match repair(&m, &RepairOptions::default(), &tol, &LIMITS) {
                Ok(r) => assert_eq!(r.mesh.triangle_count(), 12, "{k}"),
                Err(e) => assert!(e.code().starts_with("mesh."), "{k}"),
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 40, ..ProptestConfig::default() })]

        #[test]
        fn injected_defects_are_repaired_and_reported(
            rings in 5_u32..9, segments in 6_u32..13, seed in any::<u64>(),
            duplicates in 0_usize..6, flips in 0_usize..8, degenerates in 0_usize..4,
            copies in 0_usize..4,
        ) {
            let base = sphere(Point3::origin(), 3.0, rings, segments);
            prop_assume!(volume(&base, &LIMITS).unwrap() > 0.0);
            let mut rng = Lcg(seed);
            // Remove the fan of one interior vertex.
            let victim = idx_between(&mut rng, 1 + segments as usize, (rings as usize - 2) * segments as usize);
            let mut triangles: Vec<[u32; 3]> =
                base.triangles().iter().copied().filter(|t| !t.contains(&victim)).collect();
            let valence = base.triangle_count() - triangles.len();
            let mut positions = base.positions().to_vec();
            let mut uvs = base.uv_set(0).unwrap().to_vec();
            // Flip some triangles.
            let mut flipped_set = std::collections::BTreeSet::new();
            for _ in 0..flips { flipped_set.insert(rng.next(triangles.len())); }
            for &t in &flipped_set { triangles[t].swap(1, 2); }
            // Duplicate some vertices (never the victim) and re-point some of their triangles.
            let mut copied = std::collections::BTreeSet::new();
            for _ in 0..copies {
                let v = idx_between(&mut rng, 0, positions.len() - 1);
                if v != victim { copied.insert(v); }
            }
            let mut welded_expected = Vec::new();
            for &v in &copied {
                let copy = positions.len() as u32;
                positions.push(positions[v as usize]);
                uvs.push(uvs[v as usize]);
                welded_expected.push(copy);
                for t in &mut triangles {
                    if rng.next(2) == 0 { for i in t.iter_mut() { if *i == v { *i = copy; } } }
                }
            }
            // Degenerate triangles and duplicate triangles, appended.
            let mut degenerate_expected = Vec::new();
            for _ in 0..degenerates {
                degenerate_expected.push(triangles.len() as u32);
                let v = rng.next(positions.len()) as u32;
                triangles.push([v, v, rng.next(positions.len()) as u32]);
            }
            let proper = triangles.len() - degenerate_expected.len();
            let mut duplicate_expected = Vec::new();
            for _ in 0..duplicates {
                let t = triangles[rng.next(proper)];
                duplicate_expected.push(triangles.len() as u32);
                triangles.push(t);
            }
            let broken = TriMeshBuilder::new(positions, triangles.clone())
                .channel(AttributeKey::Uv(0), Channel::Vec2(uvs))
                .build(&LIMITS).unwrap();
            let r = run(&broken);
            let rep = &r.report;
            prop_assert_eq!(&rep.welded_vertices.indices, &welded_expected);
            prop_assert_eq!(&rep.degenerate_triangles.indices, &degenerate_expected);
            prop_assert_eq!(&rep.duplicate_triangles.indices, &duplicate_expected);
            let expected_flips: Vec<u32> = flipped_set.iter().map(|&t| t as u32).collect();
            // A duplicated triangle's copy may precede nothing; flips refer to input indices.
            prop_assert_eq!(&rep.flipped_triangles.indices, &expected_flips);
            prop_assert_eq!((rep.holes_filled, rep.holes_skipped), (1, 0));
            prop_assert_eq!(rep.hole_triangles_added as usize, valence - 2);
            prop_assert_eq!(&rep.unused_vertices.indices, &vec![victim]);
            let out = &r.mesh;
            prop_assert!(volume(out, &LIMITS).unwrap() > 0.0);
            let topology = component_topology(out, &LIMITS).unwrap();
            prop_assert_eq!(topology.len(), 1);
            prop_assert_eq!((topology[0].genus, topology[0].boundary_loops), (Some(0), 0));
            prop_assert_eq!(out.triangle_count(), base.triangle_count() - 2);
            prop_assert_eq!(out.vertex_count(), base.vertex_count() - 1);
            // Repairing again changes nothing.
            prop_assert_eq!(run(out).mesh, out.clone());
        }
    }

    fn idx_between(rng: &mut Lcg, low: usize, high: usize) -> u32 {
        (low + rng.next(high - low + 1)) as u32
    }
}
