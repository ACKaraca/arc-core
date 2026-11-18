//! Mesh simplification by edge collapse with quadric error metrics (CORE-MESH-015).
//!
//! [`simplify`] repeatedly collapses the cheapest edge until a target triangle count is reached,
//! the cheapest remaining collapse costs more than an error bound, or no collapse is valid.
//!
//! - **Metric.** Every vertex carries the sum of the quadrics (squared distance to the plane)
//!   of the original triangles around it, unweighted; the cost of collapsing an edge is the
//!   value of the summed quadrics at the new position, and its square root is the *quadric
//!   error*, in the units of the model. The error bound applies to that quantity. It is an
//!   estimate, not a Hausdorff bound; the returned [`Simplified::max_error`] is measured.
//! - **Position.** The new vertex is the point of the collapsing edge that minimizes the
//!   quadric (a closed-form minimum on the segment), so it never leaves the edge. The
//!   unconstrained minimum of the quadric was rejected: in nearly flat neighborhoods it
//!   slides away from the surface without a bound.
//! - **Preserved features.** Boundary vertices, vertices on attribute seams (vertices
//!   that share a position with another vertex), vertices around edges used by more than two
//!   triangles or pinched, and vertices where face groups meet are *locked*: they never move
//!   and never merge with each other. An edge with one locked end collapses onto the locked
//!   end, so open boundaries, seams and group borders keep their vertices, positions and
//!   attributes exactly (locked vertices instead of constraint quadrics: the features are kept
//!   exactly, at the price that edges between two locked vertices are never collapsed).
//! - **Validity.** A collapse is rejected when the vertices adjacent to both ends differ from
//!   the apexes of the triangles on the edge (the link condition, which keeps the mesh
//!   manifold and creates no duplicate triangle or non-manifold edge), when an edge is shared
//!   by more than two triangles, when a surviving triangle would turn its normal by 60° or more
//!   (a flip), or when it would become degenerate at the linear tolerance.
//! - **Attributes.** The new vertex takes the attributes of the edge's ends linearly at the
//!   edge parameter of its position; normals and tangent directions are renormalized and the
//!   tangent handedness is taken from the nearer end.
//! - **Determinism.** Costs tie by vertex indices; the result depends only on the input.
//! - **Triangles.** Triangles with a repeated vertex are dropped. Triangles keep their order,
//!   so a face group still covers its surviving triangles; vertices that no triangle uses are
//!   removed, the others keep their order.
//! - **Measured error.** [`measure_deviation`]: the largest distance from the points of a
//!   barycentric grid with two divisions on each triangle (its vertices and edge midpoints) of one mesh to the other mesh, in
//!   both directions. It samples the surface, so it is a lower bound of the Hausdorff distance
//!   that is exact for the points sampled, not for the interior of triangles.
//! - **Work.** Candidate collapses and the work done for each are bounded by
//!   `256 · limits.array_length`; beyond that the call fails with `mesh.limit.exceeded`.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::bvh::MeshBvh;
use crate::error::codes;
use crate::measure::pinched_flags;
use crate::topo::{
    Budget, Edges, check_mesh, exact_canon, get, idx, index_degenerate, power_of_two_scale,
};
use crate::{AttributeKey, Channel, FaceGroup, TriMesh, TriMeshBuilder};

/// What to simplify to. At least one of the two bounds must be given.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SimplifyOptions {
    /// Stop once the mesh has at most this many triangles.
    pub target_triangles: Option<usize>,
    /// Stop before a collapse whose quadric error exceeds this distance.
    pub max_error: Option<f64>,
}

/// Why [`simplify`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The target triangle count was reached.
    TargetReached,
    /// The cheapest remaining collapse exceeds the error bound.
    ErrorBound,
    /// No valid collapse is left (locked features, the link condition or flip checks).
    NoValidCollapse,
}

/// The result of [`simplify`].
#[derive(Debug, Clone, PartialEq)]
pub struct Simplified {
    /// The simplified mesh.
    pub mesh: TriMesh,
    /// The triangle count of the input.
    pub triangles_before: usize,
    /// The triangle count of the result.
    pub triangles_after: usize,
    /// The number of edges collapsed.
    pub collapses: usize,
    /// Why the simplification stopped.
    pub stop: StopReason,
    /// The measured deviation of the result from the input ([`measure_deviation`]).
    pub max_error: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Quadric {
    a: [f64; 6],
    b: [f64; 3],
    c: f64,
}

impl Quadric {
    fn plane(n: &Vector3, d: f64) -> Self {
        Self {
            a: [
                n.x * n.x,
                n.x * n.y,
                n.x * n.z,
                n.y * n.y,
                n.y * n.z,
                n.z * n.z,
            ],
            b: [d * n.x, d * n.y, d * n.z],
            c: d * d,
        }
    }

    fn add(&self, o: &Self) -> Self {
        let mut r = *self;
        r.a.iter_mut().zip(o.a).for_each(|(x, y)| *x += y);
        r.b.iter_mut().zip(o.b).for_each(|(x, y)| *x += y);
        r.c += o.c;
        r
    }

    /// `A v`.
    fn apply(&self, v: &Vector3) -> Vector3 {
        let [a0, a1, a2, a3, a4, a5] = self.a;
        Vector3::new(
            a0 * v.x + a1 * v.y + a2 * v.z,
            a1 * v.x + a3 * v.y + a4 * v.z,
            a2 * v.x + a4 * v.y + a5 * v.z,
        )
    }

    fn eval(&self, p: &Vector3) -> f64 {
        let b = Vector3::from(self.b);
        (p.dot(&self.apply(p)) + 2.0 * b.dot(p) + self.c).max(0.0)
    }
}

struct Entry {
    cost: f64,
    a: u32,
    b: u32,
    versions: [u32; 2],
    t: f64,
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: the heap pops the cheapest, then the lowest indices.
        other
            .cost
            .total_cmp(&self.cost)
            .then(other.a.cmp(&self.a))
            .then(other.b.cmp(&self.b))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Entry {}

struct Chan {
    key: AttributeKey,
    k: usize,
    data: Vec<f32>,
}

impl Chan {
    /// The values at `a` and `b` mixed at parameter `t` (exactly one end for 0 and 1).
    fn mix(&self, a: u32, b: u32, t: f64) -> Vec<f32> {
        let slice = |v: u32| {
            self.data
                .get(v as usize * self.k..(v as usize + 1) * self.k)
                .unwrap_or(&[])
                .to_vec()
        };
        let (x, y) = (slice(a), slice(b));
        if t <= 0.0 {
            return x;
        }
        if t >= 1.0 {
            return y;
        }
        let mut out: Vec<f32> = x
            .iter()
            .zip(&y)
            .map(|(p, q)| {
                let (p, q) = (f64::from(*p), f64::from(*q));
                narrow(p + (q - p) * t)
            })
            .collect();
        match self.key {
            AttributeKey::Normal | AttributeKey::Tangent => {
                let n = out
                    .iter()
                    .take(3)
                    .map(|c| f64::from(*c))
                    .collect::<Vec<_>>();
                let v = Vector3::new(get(&n, 0), get(&n, 1), get(&n, 2));
                if let Some(u) = crate::unit(&v) {
                    for (o, c) in out.iter_mut().zip(u.iter()) {
                        *o = narrow(*c);
                    }
                } else {
                    // The ends cancel: keep the nearer end's direction.
                    out = if t < 0.5 { x.clone() } else { y.clone() };
                }
                if self.key == AttributeKey::Tangent
                    && let Some(w) = out.get_mut(3)
                {
                    *w = if t < 0.5 { get(&x, 3) } else { get(&y, 3) };
                }
            }
            _ => {}
        }
        out
    }
}

#[allow(clippy::cast_possible_truncation, reason = "channels are f32")]
fn narrow(x: f64) -> f32 {
    x as f32
}

struct State {
    tris: Vec<[u32; 3]>,
    tri_alive: Vec<bool>,
    vtris: Vec<Vec<u32>>,
    v_alive: Vec<bool>,
    locked: Vec<bool>,
    np: Vec<Vector3>,
    fp: Vec<Vector3>,
    quadric: Vec<Quadric>,
    version: Vec<u32>,
    chans: Vec<Chan>,
    tol: f64,
    alive: usize,
    heap: BinaryHeap<Entry>,
}

/// Linear interpolation that is exact at both ends and cannot overflow.
fn lerp(a: &Vector3, b: &Vector3, t: f64) -> Vector3 {
    if t <= 0.0 {
        *a
    } else if t >= 1.0 {
        *b
    } else {
        a * (1.0 - t) + b * t
    }
}

/// The largest angle a surviving triangle's normal may turn by in one collapse.
const MAX_TURN: f64 = std::f64::consts::FRAC_PI_3;

/// Whether replacing triangle `old` by `new` keeps its side and does not make it degenerate.
fn keeps_shape(old: &[Vector3; 3], new: &[Vector3; 3], tol: f64) -> bool {
    let cross = |t: &[Vector3; 3]| (t[1] - t[0]).cross(&(t[2] - t[0]));
    let (co, cn) = (cross(old), cross(new));
    let longest = (new[1] - new[0])
        .norm()
        .max((new[2] - new[0]).norm())
        .max((new[2] - new[1]).norm());
    let height = cn.norm() / longest;
    co.dot(&cn) > co.norm() * cn.norm() * MAX_TURN.cos() && height > tol
}

impl State {
    fn corners(&self, t: [u32; 3]) -> [Vector3; 3] {
        t.map(|v| get(&self.np, v as usize))
    }

    fn live_tris(&self, v: u32) -> Vec<u32> {
        self.vtris
            .get(v as usize)
            .map(|l| {
                l.iter()
                    .copied()
                    .filter(|&t| get(&self.tri_alive, t as usize))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The vertices adjacent to `v`, sorted.
    fn neighbors(&self, v: u32) -> Vec<u32> {
        let mut n: Vec<u32> = self
            .live_tris(v)
            .iter()
            .flat_map(|&t| get(&self.tris, t as usize))
            .filter(|&w| w != v)
            .collect();
        n.sort_unstable();
        n.dedup();
        n
    }

    /// The cost and edge parameter of collapsing `a < b`, or `None` when both ends are locked.
    fn evaluate(&self, a: u32, b: u32) -> Option<(f64, f64)> {
        let (la, lb) = (get(&self.locked, a as usize), get(&self.locked, b as usize));
        if la && lb {
            return None;
        }
        let q = get(&self.quadric, a as usize).add(&get(&self.quadric, b as usize));
        let (pa, pb) = (get(&self.np, a as usize), get(&self.np, b as usize));
        let t = if la {
            0.0
        } else if lb {
            1.0
        } else {
            let d = pb - pa;
            let h = d.dot(&q.apply(&d));
            let g = d.dot(&(q.apply(&pa) + Vector3::from(q.b)));
            if h > 0.0 {
                (-g / h).clamp(0.0, 1.0)
            } else if q.eval(&pa) <= q.eval(&pb) {
                0.0
            } else {
                1.0
            }
        };
        let cost = q.eval(&lerp(&pa, &pb, t));
        cost.is_finite().then_some((cost, t))
    }

    fn push_edge(&mut self, a: u32, b: u32) {
        let (a, b) = (a.min(b), a.max(b));
        if let Some((cost, t)) = self.evaluate(a, b) {
            self.heap.push(Entry {
                cost,
                a,
                b,
                versions: [
                    get(&self.version, a as usize),
                    get(&self.version, b as usize),
                ],
                t,
            });
        }
    }

    fn valid(&self, e: &Entry) -> bool {
        get(&self.v_alive, e.a as usize)
            && get(&self.v_alive, e.b as usize)
            && [e.a, e.b]
                .iter()
                .zip(e.versions)
                .all(|(&v, ver)| get(&self.version, v as usize) == ver)
    }

    /// Collapses the edge when the result is valid; `Some(work)` tells it did.
    fn collapse(&mut self, e: &Entry) -> (bool, usize) {
        let (s, r) = if get(&self.locked, e.b as usize) {
            (e.b, e.a)
        } else {
            (e.a, e.b)
        };
        let (tris_r, tris_s) = (self.live_tris(r), self.live_tris(s));
        let work = tris_r.len() + tris_s.len();
        let shared: Vec<u32> = tris_r
            .iter()
            .copied()
            .filter(|&t| get(&self.tris, t as usize).contains(&s))
            .collect();
        if shared.is_empty() || shared.len() > 2 {
            return (false, work);
        }
        let mut apexes: Vec<u32> = shared
            .iter()
            .flat_map(|&t| get(&self.tris, t as usize))
            .filter(|&v| v != r && v != s)
            .collect();
        apexes.sort_unstable();
        let common: Vec<u32> = {
            let ns = self.neighbors(s);
            self.neighbors(r)
                .into_iter()
                .filter(|v| *v != s && ns.binary_search(v).is_ok())
                .collect()
        };
        if common != apexes {
            return (false, work);
        }
        let target = lerp(
            &get(&self.np, e.a as usize),
            &get(&self.np, e.b as usize),
            e.t,
        );
        let ok = |this: &Self, tris: &[u32], moving: u32| {
            tris.iter().filter(|t| !shared.contains(t)).all(|&t| {
                let tri = get(&this.tris, t as usize);
                let old = this.corners(tri);
                let mut new = old;
                for (k, v) in tri.iter().enumerate() {
                    if (*v == moving || *v == s)
                        && let Some(c) = new.get_mut(k)
                    {
                        *c = target;
                    }
                }
                keeps_shape(&old, &new, this.tol)
            })
        };
        if !ok(self, &tris_r, r) || !ok(self, &tris_s, s) {
            return (false, work);
        }
        self.apply(e, s, r, &shared, &tris_r);
        (true, work)
    }

    fn apply(&mut self, e: &Entry, s: u32, r: u32, shared: &[u32], tris_r: &[u32]) {
        for chan in &mut self.chans {
            let mixed = chan.mix(e.a, e.b, e.t);
            let at = s as usize * chan.k;
            if let Some(slot) = chan.data.get_mut(at..at + chan.k) {
                slot.copy_from_slice(&mixed);
            }
        }
        let (na, nb) = (get(&self.np, e.a as usize), get(&self.np, e.b as usize));
        let (fa, fb) = (get(&self.fp, e.a as usize), get(&self.fp, e.b as usize));
        let (nq, version) = (
            get(&self.quadric, e.a as usize).add(&get(&self.quadric, e.b as usize)),
            get(&self.version, s as usize) + 1,
        );
        for (list, value) in [
            (&mut self.np, lerp(&na, &nb, e.t)),
            (&mut self.fp, lerp(&fa, &fb, e.t)),
        ] {
            if let Some(slot) = list.get_mut(s as usize) {
                *slot = value;
            }
        }
        if let Some(q) = self.quadric.get_mut(s as usize) {
            *q = nq;
        }
        if let Some(v) = self.version.get_mut(s as usize) {
            *v = version;
        }
        if let Some(a) = self.v_alive.get_mut(r as usize) {
            *a = false;
        }
        for &t in shared {
            if let Some(a) = self.tri_alive.get_mut(t as usize) {
                *a = false;
            }
        }
        self.alive -= shared.len();
        for &t in tris_r.iter().filter(|t| !shared.contains(t)) {
            if let Some(tri) = self.tris.get_mut(t as usize) {
                for v in tri.iter_mut().filter(|v| **v == r) {
                    *v = s;
                }
            }
            if let Some(list) = self.vtris.get_mut(s as usize) {
                list.push(t);
            }
        }
        let alive = &self.tri_alive;
        if let Some(list) = self.vtris.get_mut(s as usize) {
            list.retain(|&t| get(alive, t as usize));
        }
        for w in self.neighbors(s) {
            self.push_edge(s, w);
        }
    }
}

/// Which vertices must not move or merge: see the module documentation.
fn locked_vertices(mesh: &TriMesh, tris: &[[u32; 3]], edges: &Edges) -> Vec<bool> {
    let n = mesh.positions.len();
    let mut locked = pinched_flags(tris, edges, n);
    for g in edges.groups().filter(|g| g.len() != 2) {
        for e in g {
            for v in [e.lo, e.hi] {
                if let Some(l) = locked.get_mut(v as usize) {
                    *l = true;
                }
            }
        }
    }
    let canon = exact_canon(&mesh.positions);
    let mut count = vec![0_u32; n];
    for &c in &canon {
        if let Some(k) = count.get_mut(c as usize) {
            *k += 1;
        }
    }
    let mut group_of = vec![u32::MAX; mesh.triangles.len()];
    for (gi, g) in mesh.groups.iter().enumerate() {
        for t in g.start..g.end {
            if let Some(slot) = group_of.get_mut(t as usize) {
                *slot = idx(gi);
            }
        }
    }
    let mut first = vec![None::<u32>; n];
    for (t, tri) in tris.iter().enumerate() {
        if index_degenerate(*tri) {
            continue;
        }
        for &v in tri {
            let group = get(&group_of, t);
            match first.get_mut(v as usize) {
                Some(f @ None) => *f = Some(group),
                Some(Some(f)) if *f != group => {
                    if let Some(l) = locked.get_mut(v as usize) {
                        *l = true;
                    }
                }
                _ => {}
            }
        }
    }
    for (v, c) in canon.iter().enumerate() {
        if get(&count, *c as usize) > 1
            && let Some(l) = locked.get_mut(v)
        {
            *l = true;
        }
    }
    locked
}

/// The positions scaled into [-1, 1] around the center of their box.
fn normalize(mesh: &TriMesh) -> (Vec<Vector3>, f64) {
    let (mut lo, mut hi) = (
        Vector3::repeat(f64::INFINITY),
        Vector3::repeat(f64::NEG_INFINITY),
    );
    for p in &mesh.positions {
        lo = lo.inf(&p.coords);
        hi = hi.sup(&p.coords);
    }
    let (center, half) = if mesh.positions.is_empty() {
        (Vector3::zeros(), Vector3::zeros())
    } else {
        (lo / 2.0 + hi / 2.0, hi / 2.0 - lo / 2.0)
    };
    let reach = power_of_two_scale(half.amax());
    let reach = if reach > 0.0 { reach } else { 1.0 };
    (
        mesh.positions
            .iter()
            .map(|p| (p.coords / 2.0 - center / 2.0) / (reach / 2.0))
            .collect(),
        reach,
    )
}

fn plane_quadrics(np: &[Vector3], tris: &[[u32; 3]]) -> Vec<Quadric> {
    let mut q = vec![Quadric::default(); np.len()];
    for t in tris {
        let [a, b, c] = t.map(|v| get(np, v as usize));
        let normal = (b - a).cross(&(c - a));
        if let Some(n) = crate::unit(&normal) {
            let plane = Quadric::plane(&n, -n.dot(&a));
            for v in t {
                if let Some(slot) = q.get_mut(*v as usize) {
                    *slot = slot.add(&plane);
                }
            }
        }
    }
    q
}

fn validate(options: &SimplifyOptions) -> Result<()> {
    let bound_ok = options.max_error.is_none_or(|e| e.is_finite() && e >= 0.0);
    if (options.target_triangles.is_none() && options.max_error.is_none()) || !bound_ok {
        return Err(Error::new(codes::SIMPLIFY_INVALID));
    }
    Ok(())
}

/// Simplifies without measuring the result: the mesh, the stop reason and the collapse count.
pub(crate) fn simplify_core(
    mesh: &TriMesh,
    options: &SimplifyOptions,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<(TriMesh, StopReason, usize)> {
    validate(options)?;
    crate::ensure_tolerance(tolerance)?;
    check_mesh(mesh, limits, 512)?;
    let tri_alive: Vec<bool> = mesh
        .triangles
        .iter()
        .map(|t| !index_degenerate(*t))
        .collect();
    let live: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .map(|t| if index_degenerate(*t) { [0, 0, 0] } else { *t })
        .collect();
    let edges = Edges::build((0..).zip(mesh.triangles.iter().copied()));
    let locked = locked_vertices(mesh, &live, &edges);
    let (np, reach) = normalize(mesh);
    let mut vtris = vec![Vec::new(); np.len()];
    for (t, tri) in mesh.triangles.iter().enumerate() {
        if get(&tri_alive, t) {
            for v in tri {
                if let Some(l) = vtris.get_mut(*v as usize) {
                    l.push(idx(t));
                }
            }
        }
    }
    let alive_tris: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .zip(&tri_alive)
        .filter(|(_, a)| **a)
        .map(|(t, _)| *t)
        .collect();
    let mut st = State {
        quadric: plane_quadrics(&np, &alive_tris),
        fp: mesh.positions.iter().map(|p| p.coords).collect(),
        tris: mesh.triangles.clone(),
        alive: alive_tris.len(),
        tri_alive,
        vtris,
        v_alive: vec![true; np.len()],
        locked,
        version: vec![0; np.len()],
        chans: mesh
            .channels
            .iter()
            .map(|(key, c)| Chan {
                key: key.clone(),
                k: c.components(),
                data: c.floats().to_vec(),
            })
            .collect(),
        tol: tolerance.linear / reach,
        np,
        heap: BinaryHeap::new(),
    };
    for g in edges.groups().filter(|g| g.len() == 2) {
        if let Some(e) = g.first() {
            st.push_edge(e.lo, e.hi);
        }
    }
    let mut budget = Budget::new(limits, 256);
    let mut collapses = 0;
    let mut stop = StopReason::NoValidCollapse;
    loop {
        if options.target_triangles.is_some_and(|t| st.alive <= t) {
            stop = StopReason::TargetReached;
            break;
        }
        let Some(entry) = st.heap.pop() else { break };
        if !st.valid(&entry) {
            continue;
        }
        if options
            .max_error
            .is_some_and(|m| entry.cost.sqrt() > m / reach)
        {
            stop = StopReason::ErrorBound;
            break;
        }
        budget.spend(1)?;
        let (done, work) = st.collapse(&entry);
        budget.spend(work)?;
        collapses += usize::from(done);
    }
    Ok((assemble(mesh, &st)?, stop, collapses))
}

fn assemble(mesh: &TriMesh, st: &State) -> Result<TriMesh> {
    let mut new_index = vec![u32::MAX; mesh.positions.len()];
    let mut remap = Vec::new();
    for t in st.tris.iter().zip(&st.tri_alive).filter(|(_, a)| **a) {
        for &v in t.0 {
            if get(&new_index, v as usize) == u32::MAX
                && let Some(n) = new_index.get_mut(v as usize)
            {
                *n = 0;
            }
        }
    }
    for (v, n) in new_index.iter_mut().enumerate() {
        if *n != u32::MAX {
            *n = idx(remap.len());
            remap.push(idx(v));
        }
    }
    let mut triangles = Vec::new();
    let mut survivor = vec![u32::MAX; st.tris.len()];
    for (t, (tri, alive)) in st.tris.iter().zip(&st.tri_alive).enumerate() {
        if *alive {
            if let Some(s) = survivor.get_mut(t) {
                *s = idx(triangles.len());
            }
            triangles.push(tri.map(|v| get(&new_index, v as usize)));
        }
    }
    let positions = remap
        .iter()
        .map(|&v| Point3::from(get(&st.fp, v as usize)))
        .collect();
    let mut builder = TriMeshBuilder::new(positions, triangles);
    for (chan, original) in st.chans.iter().zip(mesh.channels.values()) {
        let flat: Vec<f32> = remap
            .iter()
            .flat_map(|&v| {
                chan.data
                    .get(v as usize * chan.k..(v as usize + 1) * chan.k)
                    .unwrap_or(&[])
                    .to_vec()
            })
            .collect();
        let channel = match original {
            Channel::Scalar(_) => Channel::Scalar(flat),
            Channel::Vec2(_) => Channel::Vec2(flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect()),
            Channel::Vec3(_) => {
                Channel::Vec3(flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
            }
            Channel::Vec4(_) => Channel::Vec4(
                flat.chunks_exact(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect(),
            ),
        };
        builder = builder.channel(chan.key.clone(), channel);
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
    builder.build(&Limits {
        array_length: u64::MAX,
        ..Limits::DEFAULT
    })
}

/// The largest distance from the sample points of `from` to `to` (see the module
/// documentation for the samples).
fn one_sided(from: &TriMesh, to: &MeshBvh<'_>) -> Result<f64> {
    const DIVISIONS: u32 = 2;
    let mut worst = 0.0_f64;
    for t in &from.triangles {
        if index_degenerate(*t) {
            continue;
        }
        let c = crate::topo::corners(&from.positions, *t);
        for i in 0..=DIVISIONS {
            for j in 0..=(DIVISIONS - i) {
                let (x, y) = (
                    f64::from(i) / f64::from(DIVISIONS),
                    f64::from(j) / f64::from(DIVISIONS),
                );
                let p = c[0] * (1.0 - x - y) + c[1] * x + c[2] * y;
                worst = worst.max(to.closest_point(&Point3::from(p))?.distance);
            }
        }
    }
    Ok(worst)
}

/// The deviation between two meshes: the larger of the two one-sided sampled distances.
pub(crate) fn deviation(a: &MeshBvh<'_>, b: &MeshBvh<'_>) -> Result<f64> {
    Ok(one_sided(a.mesh(), b)?.max(one_sided(b.mesh(), a)?))
}

/// The sampled deviation of two meshes: the largest distance from the points of a barycentric
/// grid with two divisions on each triangle (its vertices and edge midpoints) of one mesh to the other mesh, in both directions.
///
/// # Errors
///
/// `mesh.query.empty` when a mesh has no triangles; `mesh.limit.exceeded` beyond `limits`;
/// `mesh.precision.insufficient` when a distance is not representable.
pub fn measure_deviation(a: &TriMesh, b: &TriMesh, limits: &Limits) -> Result<f64> {
    deviation(&MeshBvh::build(a, limits)?, &MeshBvh::build(b, limits)?)
}

/// Simplifies `mesh` by edge collapse; see the module documentation for the rules.
///
/// A mesh already at or below the target is returned unchanged with
/// [`StopReason::TargetReached`].
///
/// # Errors
///
/// `mesh.simplify.invalid` when neither bound is given or the error bound is negative or not
/// finite; `mesh.tolerance.invalid` for an invalid tolerance; `mesh.limit.exceeded` beyond
/// `limits`; `mesh.query.empty` when the result has no triangles to measure.
pub fn simplify(
    mesh: &TriMesh,
    options: &SimplifyOptions,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Simplified> {
    let (out, stop, collapses) = simplify_core(mesh, options, tolerance, limits)?;
    let max_error = if collapses == 0 {
        0.0
    } else {
        measure_deviation(mesh, &out, limits)?
    };
    Ok(Simplified {
        triangles_before: mesh.triangle_count(),
        triangles_after: out.triangle_count(),
        mesh: out,
        collapses,
        stop,
        max_error,
    })
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "test values are small"
)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;
    use crate::measure::{component_topology, volume};
    use crate::testing::sphere;
    use crate::{NormalWeighting, SubObjectKey};

    const TOL: Tolerance = Tolerance::DEFAULT;
    const LIMITS: Limits = Limits::DEFAULT;

    fn target(n: usize) -> SimplifyOptions {
        SimplifyOptions {
            target_triangles: Some(n),
            max_error: None,
        }
    }

    fn bits(p: &Point3) -> [u64; 3] {
        [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
    }

    /// An `n` by `n` grid of cells in the plane, heights from `z`, cells in the order given by
    /// `order` (cells with `x < split` first when `split` is set), optionally with the
    /// vertices of column `seam` duplicated for the cells right of it.
    fn grid(n: u32, z: &dyn Fn(u32, u32) -> f64, split: Option<u32>, seam: Option<u32>) -> TriMesh {
        let columns = n + 1;
        let mut positions: Vec<Point3> = (0..columns * columns)
            .map(|i| {
                Point3::new(
                    f64::from(i % columns),
                    f64::from(i / columns),
                    z(i % columns, i / columns),
                )
            })
            .collect();
        let mut uv: Vec<[f32; 2]> = positions
            .iter()
            .map(|p| [p.x as f32 / n as f32, p.y as f32 / n as f32])
            .collect();
        let mut copy = std::collections::HashMap::new();
        if let Some(x) = seam {
            for y in 0..=n {
                copy.insert(y, idx(positions.len()));
                positions.push(Point3::new(f64::from(x), f64::from(y), z(x, y)));
                uv.push([9.0, y as f32]);
            }
        }
        let mut cells: Vec<(u32, u32)> = (0..n * n).map(|i| (i % n, i / n)).collect();
        if let Some(s) = split {
            cells.sort_by_key(|&(x, y)| (x >= s, y, x));
        }
        let mut triangles = Vec::new();
        let mut first_right = None;
        for (x, y) in cells {
            if split.is_some_and(|s| x >= s) && first_right.is_none() {
                first_right = Some(triangles.len());
            }
            let v = |cx: u32, cy: u32| {
                let base = cy * columns + cx;
                match seam {
                    Some(s) if cx == s && x >= s => copy[&cy],
                    _ => base,
                }
            };
            let (a, b, c, d) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        let mut builder = TriMeshBuilder::new(positions, triangles.clone())
            .channel(AttributeKey::Uv(0), Channel::Vec2(uv));
        if let Some(f) = first_right {
            let g = |start, end, local| FaceGroup {
                start,
                end,
                material: local,
                key: SubObjectKey {
                    object: None,
                    local,
                },
            };
            builder = builder
                .group(g(0, idx(f), 1))
                .group(g(idx(f), idx(triangles.len()), 2));
        }
        builder.build(&LIMITS).unwrap()
    }

    fn flat(_: u32, _: u32) -> f64 {
        0.0
    }

    fn boundary_positions(m: &TriMesh) -> BTreeSet<[u64; 3]> {
        let canon = exact_canon(&m.positions);
        let edges =
            Edges::build((0..).zip(m.triangles.iter().map(|t| t.map(|v| canon[v as usize]))));
        edges
            .groups()
            .filter(|g| g.len() == 1)
            .flat_map(|g| [g[0].lo, g[0].hi])
            .map(|v| bits(&m.positions[v as usize]))
            .collect()
    }

    /// Distances from sample points of `a` (vertices and edge midpoints) to `b`, by brute
    /// force over all triangles.
    fn brute_one_sided(a: &TriMesh, b: &TriMesh) -> f64 {
        let mut worst = 0.0_f64;
        for t in a.triangles() {
            let c = crate::topo::corners(&a.positions, *t);
            let samples = [
                c[0],
                c[1],
                c[2],
                (c[0] + c[1]) / 2.0,
                (c[1] + c[2]) / 2.0,
                (c[2] + c[0]) / 2.0,
            ];
            for p in samples {
                let d = b
                    .triangles()
                    .iter()
                    .map(|u| {
                        crate::distance::closest_on_triangle(
                            &p,
                            crate::topo::corners(&b.positions, *u),
                        )
                        .unwrap()
                        .distance
                    })
                    .fold(f64::INFINITY, f64::min);
                worst = worst.max(d);
            }
        }
        worst
    }

    #[test]
    fn a_sphere_reaches_its_target_and_stays_a_sound_solid() {
        let s = sphere(Point3::new(1.0, 2.0, 3.0), 2.0, 12, 24);
        let r = simplify(&s, &target(120), &TOL, &LIMITS).unwrap();
        assert_eq!(r.stop, StopReason::TargetReached);
        assert!(
            r.triangles_after <= 120 && r.triangles_after >= 90,
            "{}",
            r.triangles_after
        );
        assert_eq!(r.triangles_before, s.triangle_count());
        let c = component_topology(&r.mesh, &LIMITS).unwrap();
        assert_eq!((c.len(), c[0].genus, c[0].boundary_loops), (1, Some(0), 0));
        let v = volume(&r.mesh, &LIMITS).unwrap();
        assert!(v > 0.0 && v < volume(&s, &LIMITS).unwrap());
        // No triangle faces inward.
        let center = Vector3::new(1.0, 2.0, 3.0);
        for t in r.mesh.triangles() {
            let [a, b, c] = crate::topo::corners(&r.mesh.positions, *t);
            let n = (b - a).cross(&(c - a));
            assert!(n.dot(&((a + b + c) / 3.0 - center)) > 0.0);
        }
        // The measured error is small against the radius and equals the stated measure.
        assert!(r.max_error > 0.0 && r.max_error < 0.2, "{}", r.max_error);
        assert_eq!(
            r.max_error,
            measure_deviation(&s, &r.mesh, &LIMITS).unwrap()
        );
        assert_eq!(simplify(&s, &target(120), &TOL, &LIMITS).unwrap(), r);
    }

    #[test]
    fn an_error_bound_limits_the_collapses() {
        let s = sphere(Point3::origin(), 2.0, 10, 20);
        let loose = simplify(
            &s,
            &SimplifyOptions {
                target_triangles: None,
                max_error: Some(0.5),
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        let tight = simplify(
            &s,
            &SimplifyOptions {
                target_triangles: None,
                max_error: Some(0.01),
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(loose.stop, StopReason::ErrorBound);
        assert!(loose.triangles_after < tight.triangles_after);
        assert!(tight.max_error <= loose.max_error);
        let none = simplify(
            &s,
            &SimplifyOptions {
                target_triangles: None,
                max_error: Some(0.0),
            },
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(none.mesh, s);
        assert_eq!(none.max_error, 0.0);
        let already = simplify(&s, &target(10_000), &TOL, &LIMITS).unwrap();
        assert_eq!(
            (already.stop, already.collapses),
            (StopReason::TargetReached, 0)
        );
        assert_eq!(already.mesh, s);
    }

    #[test]
    fn open_boundaries_stay_and_an_unreachable_target_says_why() {
        let g = grid(12, &flat, None, None);
        let r = simplify(&g, &target(8), &TOL, &LIMITS).unwrap();
        assert_eq!(r.stop, StopReason::NoValidCollapse);
        // Every boundary vertex survives; the interior is gone: n boundary vertices make n − 2
        // triangles.
        assert_eq!(boundary_positions(&r.mesh), boundary_positions(&g));
        assert_eq!(r.triangles_after, boundary_positions(&g).len() - 2);
        assert!(r.max_error < 1e-9);
        for t in r.mesh.triangles() {
            let [a, b, c] = crate::topo::corners(&r.mesh.positions, *t);
            assert!((b - a).cross(&(c - a)).z > 0.0);
        }
    }

    #[test]
    fn seams_and_group_borders_keep_their_vertices_and_attributes() {
        let bump = |x: u32, y: u32| ((f64::from(x) * 0.7).sin() + (f64::from(y) * 0.4).cos()) * 0.3;
        let g = grid(10, &bump, Some(5), Some(5));
        assert_eq!(g.groups().len(), 2);
        let r = simplify(&g, &target(20), &TOL, &LIMITS).unwrap();
        assert!(r.triangles_after < g.triangle_count());
        // Both copies of every seam vertex survive with their UVs.
        let seam: Vec<(Point3, [f32; 2])> = g
            .positions()
            .iter()
            .zip(g.uv_set(0).unwrap())
            .filter(|(p, _)| p.x == 5.0)
            .map(|(p, uv)| (*p, *uv))
            .collect();
        let kept: BTreeSet<([u64; 3], [u32; 2])> = r
            .mesh
            .positions()
            .iter()
            .zip(r.mesh.uv_set(0).unwrap())
            .map(|(p, uv)| (bits(p), uv.map(f32::to_bits)))
            .collect();
        for (p, uv) in seam {
            assert!(
                kept.contains(&(bits(&p), uv.map(f32::to_bits))),
                "{p} {uv:?}"
            );
        }
        assert_eq!(boundary_positions(&r.mesh), boundary_positions(&g));
        // Groups still partition the triangles and each covers its side.
        let groups = r.mesh.groups();
        assert_eq!(groups.len(), 2);
        assert_eq!((groups[0].start, groups[0].end), (0, groups[1].start));
        assert_eq!(groups[1].end as usize, r.mesh.triangle_count());
        for (i, t) in r.mesh.triangles().iter().enumerate() {
            let cx: f64 = t
                .iter()
                .map(|&v| r.mesh.positions[v as usize].x)
                .sum::<f64>()
                / 3.0;
            let side = r.mesh.group_of(idx(i)).unwrap().key.local;
            assert_eq!(side == 1, cx < 5.0 + 1e-9, "triangle {i}");
        }
        assert_eq!((groups[0].material, groups[1].material), (1, 2));
    }

    #[test]
    fn attributes_are_interpolated_and_directions_stay_unit() {
        let s = sphere(Point3::origin(), 1.0, 12, 24)
            .with_normals(NormalWeighting::Angle, std::f64::consts::PI, &TOL, &LIMITS)
            .unwrap();
        let r = simplify(&s, &target(80), &TOL, &LIMITS).unwrap();
        assert!(r.triangles_after <= 80);
        for n in r.mesh.normals().unwrap() {
            let l = f64::from(n[0])
                .hypot(f64::from(n[1]))
                .hypot(f64::from(n[2]));
            assert!((l - 1.0).abs() < 1e-5);
        }
        assert_eq!(r.mesh.uv_set(0).unwrap().len(), r.mesh.vertex_count());
        // Normals still point away from the center.
        for (p, n) in r.mesh.positions().iter().zip(r.mesh.normals().unwrap()) {
            let d = p
                .coords
                .dot(&Vector3::new(n[0].into(), n[1].into(), n[2].into()));
            assert!(d > 0.5);
        }
    }

    #[test]
    fn invalid_options_and_hostile_input_are_rejected_with_codes() {
        let s = sphere(Point3::origin(), 1.0, 6, 12);
        for bad in [
            SimplifyOptions::default(),
            SimplifyOptions {
                target_triangles: None,
                max_error: Some(-1.0),
            },
            SimplifyOptions {
                target_triangles: Some(5),
                max_error: Some(f64::NAN),
            },
            SimplifyOptions {
                target_triangles: None,
                max_error: Some(f64::INFINITY),
            },
        ] {
            assert_eq!(
                simplify(&s, &bad, &TOL, &LIMITS).unwrap_err().code(),
                "mesh.simplify.invalid"
            );
        }
        let nan = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(
            simplify(&s, &target(5), &nan, &LIMITS).unwrap_err().code(),
            "mesh.tolerance.invalid"
        );
        let tight = Limits {
            array_length: 10,
            ..LIMITS
        };
        assert_eq!(
            simplify(&s, &target(5), &TOL, &tight).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
        let work = Limits {
            array_length: 100,
            ..LIMITS
        };
        assert_eq!(
            simplify(&s, &target(5), &TOL, &work).unwrap_err().code(),
            "mesh.limit.exceeded"
        );
        // Meshes cannot hold NaN positions or indices out of range.
        let nan_mesh = TriMesh::new(vec![Point3::new(f64::NAN, 0.0, 0.0)], vec![], &LIMITS);
        assert_eq!(nan_mesh.unwrap_err().code(), "mesh.value.not_finite");
        let bad_index = TriMesh::new(vec![Point3::origin()], vec![[0, 0, 7]], &LIMITS);
        assert_eq!(bad_index.unwrap_err().code(), "mesh.index.out_of_range");
        // Degenerate triangles are dropped; a mesh of only those has nothing to measure.
        let degenerate = TriMesh::new(
            vec![Point3::origin(), Point3::new(1.0, 0.0, 0.0)],
            vec![[0, 0, 1]],
            &LIMITS,
        )
        .unwrap();
        assert!(simplify(&degenerate, &target(0), &TOL, &LIMITS).is_ok());
    }

    #[test]
    fn extreme_magnitudes_simplify_or_fail_with_a_code() {
        for k in [1e-200, 1e-6, 1.0, 1e100, 1e300] {
            let s = sphere(Point3::origin(), 1.0, 8, 16);
            let big = TriMesh::new(
                s.positions().iter().map(|p| p * k).collect(),
                s.triangles().to_vec(),
                &LIMITS,
            )
            .unwrap();
            let tol = Tolerance {
                linear: k * 1e-9,
                ..TOL
            };
            match simplify(&big, &target(40), &tol, &LIMITS) {
                Ok(r) => assert!(r.triangles_after <= 40, "{k}"),
                Err(e) => assert!(e.code().starts_with("mesh."), "{k}: {}", e.code()),
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

        #[test]
        fn heightfields_keep_boundary_orientation_and_measured_error(
            n in 4_u32..9,
            heights in prop::collection::vec(-0.15..0.15_f64, 144),
            keep in 2_usize..40,
        ) {
            let z = |x: u32, y: u32| heights[(y * 12 + x) as usize];
            let g = grid(n, &z, None, None);
            let r = simplify(&g, &target(keep), &TOL, &LIMITS).unwrap();
            prop_assert_eq!(r.stop == StopReason::TargetReached, r.triangles_after <= keep);
            prop_assert!(r.triangles_after <= g.triangle_count());
            prop_assert_eq!(boundary_positions(&r.mesh), boundary_positions(&g));
            // A height field has upward triangles; a flipped face would not.
            for t in r.mesh.triangles() {
                let [a, b, c] = crate::topo::corners(&r.mesh.positions, *t);
                prop_assert!((b - a).cross(&(c - a)).z > 0.0);
            }
            // The returned error covers the error measured independently on a subset of the
            // samples, in both directions.
            let brute = brute_one_sided(&g, &r.mesh).max(brute_one_sided(&r.mesh, &g));
            prop_assert!(brute <= r.max_error * (1.0 + 1e-9) + 1e-12, "{} > {}", brute, r.max_error);
            // Deterministic.
            prop_assert_eq!(simplify(&g, &target(keep), &TOL, &LIMITS).unwrap().mesh, r.mesh);
        }
    }
}
