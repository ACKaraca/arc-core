//! Boolean operations on closed triangle meshes (CORE-MESH-013, CORE-MESH-014): union, difference and
//! intersection of two solids given as closed, consistently oriented (outward counterclockwise)
//! triangle meshes.
//!
//! The result is the arrangement of the two surfaces, selected by classification:
//!
//! 1. **Welding.** Both operands are welded into one point pool: points closer than the linear
//!    tolerance are one point, triangles that collapse are dropped. An operand that is then not a
//!    closed oriented 2-manifold of positive volume is refused (`mesh.boolean.invalid_input`).
//! 2. **Intersection.** Every pair of triangles of the two operands whose boxes meet is
//!    intersected. A distance to the other plane within the tolerance counts as zero, so a vertex
//!    or an edge on a face is a contact, not a crossing. Triangles that lie in one plane
//!    (all three distances zero) are *coplanar partners*: each gets the edges of the other clipped
//!    to it. Every other pair gives one segment of the intersection curve, which both triangles
//!    receive. All endpoints go through the point pool, so both sides share them.
//! 3. **Conformity.** Every point of the intersection curve that lies within the tolerance of a
//!    mesh edge becomes a vertex of that edge for both triangles that share it, so no crack opens
//!    between a split triangle and its unsplit neighbor.
//! 4. **Splitting.** A triangle with constraints is retriangulated in its own plane with a
//!    constrained Delaunay triangulation of its outline and the segments of the curve.
//! 5. **Classification.** A piece that lies in the plane of a coplanar partner and inside it is
//!    *coplanar*, with the same or the opposite direction as the partner. The other pieces form
//!    components connected across edges that are not on the intersection curve; a component lies
//!    on one side of the other solid, which the generalized winding number at a point of its
//!    largest triangle decides.
//! 6. **Selection.** Union keeps the outside pieces of both and the coplanar pieces of the same
//!    direction once (from the first operand); intersection keeps the inside pieces and the same
//!    coplanar pieces once; difference keeps the outside pieces and the opposite coplanar pieces of
//!    the first operand and the inside pieces of the second, reversed. Other coplanar pieces are
//!    dropped.
//!
//! The result is checked: every directed edge must have its opposite. A result that is not closed
//! fails with `mesh.boolean.failed` instead of being returned, so a caller can keep its operand.
//! Edges where the operands touch along a line may be shared by four triangles.
//!
//! Work is bounded: the number of box tests and candidate triangle pairs and the size of the result
//! are limited (`mesh.limit.exceeded`). Normals within the angular tolerance count as parallel. Nothing in this module panics.

use std::collections::{HashMap, HashSet};

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Error, Limits, Result, Tolerance};
use arc_geom::triangulate_graph;

use crate::error::codes;
use crate::mesh::{TriMesh, TriMeshBuilder};

/// The operation of [`boolean`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BooleanOp {
    /// The volume of either operand.
    Union,
    /// The volume of the first operand outside the second.
    Difference,
    /// The volume of both operands.
    Intersection,
}

/// The most candidate pairs of triangles that are intersected.
const MAX_PAIRS: usize = 4_000_000;

/// The most box tests of the candidate sweep.
const MAX_TESTS: usize = 16 * MAX_PAIRS;

fn id(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

fn pair(a: u32, b: u32) -> (u32, u32) {
    if a <= b { (a, b) } else { (b, a) }
}

fn failed() -> Error {
    Error::new(codes::BOOLEAN_FAILED)
}

fn limit() -> Error {
    Error::new(codes::LIMIT_EXCEEDED)
}

/// The grid cell of a point for spatial hashing.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a coordinate over the cell size beyond i64 is not a hash key worth keeping distinct"
)]
fn cell_of(p: &Point3, size: f64) -> [i64; 3] {
    [
        (p.x / size).floor() as i64,
        (p.y / size).floor() as i64,
        (p.z / size).floor() as i64,
    ]
}

/// The welded points of both operands and of the intersection curve.
struct Pool {
    pts: Vec<Point3>,
    cells: HashMap<[i64; 3], Vec<u32>>,
    size: f64,
    tol: f64,
}

impl Pool {
    fn new(tol: f64) -> Self {
        Self {
            pts: Vec::new(),
            cells: HashMap::new(),
            size: (2.0 * tol).max(f64::MIN_POSITIVE),
            tol,
        }
    }

    /// The id of the pool point within the tolerance of `p`, or a new one.
    fn add(&mut self, p: Point3) -> u32 {
        let c = cell_of(&p, self.size);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1_i64 {
            for dy in -1..=1_i64 {
                for dz in -1..=1_i64 {
                    let key = [
                        c[0].saturating_add(dx),
                        c[1].saturating_add(dy),
                        c[2].saturating_add(dz),
                    ];
                    for &k in self.cells.get(&key).map_or(&[][..], Vec::as_slice) {
                        let d = (self.pts[k as usize] - p).norm();
                        if d <= self.tol && best.is_none_or(|(b, _)| d < b) {
                            best = Some((d, k));
                        }
                    }
                }
            }
        }
        if let Some((_, k)) = best {
            return k;
        }
        let k = id(self.pts.len());
        self.pts.push(p);
        self.cells.entry(c).or_default().push(k);
        k
    }

    fn p(&self, i: u32) -> Point3 {
        self.pts[i as usize]
    }
}

/// One welded operand: its triangles as point ids.
struct Side {
    tris: Vec<[u32; 3]>,
}

/// The unit normal, plane offset and doubled area of a triangle.
#[derive(Clone, Copy)]
struct Geo {
    n: Vector3,
    d: f64,
    area2: f64,
}

fn geometry(pool: &Pool, t: [u32; 3]) -> Geo {
    let (a, b, c) = (pool.p(t[0]), pool.p(t[1]), pool.p(t[2]));
    let cross = (b - a).cross(&(c - a));
    let area2 = cross.norm();
    if area2 > 0.0 && area2.is_finite() {
        let n = cross / area2;
        Geo {
            n,
            d: n.dot(&a.coords),
            area2,
        }
    } else {
        Geo {
            n: Vector3::zeros(),
            d: 0.0,
            area2: 0.0,
        }
    }
}

/// Welds `mesh` into the pool and checks that it is a closed oriented 2-manifold of positive volume.
fn prepare(mesh: &TriMesh, pool: &mut Pool) -> Result<Side> {
    let ids: Vec<u32> = mesh.positions().iter().map(|p| pool.add(*p)).collect();
    let mut tris = Vec::with_capacity(mesh.triangles().len());
    for t in mesh.triangles() {
        let m = t.map(|i| ids.get(i as usize).copied().unwrap_or(0));
        if m[0] != m[1] && m[1] != m[2] && m[0] != m[2] {
            tris.push(m);
        }
    }
    let mut directed: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &tris {
        for k in 0..3 {
            *directed.entry((t[k], t[(k + 1) % 3])).or_insert(0) += 1;
        }
    }
    let unmatched = directed
        .iter()
        .filter(|&(&(a, b), &n)| n != 1 || directed.get(&(b, a)) != Some(&1))
        .count();
    let mut volume = 0.0;
    if let Some(first) = tris.first() {
        let o = pool.p(first[0]);
        for t in &tris {
            let (a, b, c) = (pool.p(t[0]) - o, pool.p(t[1]) - o, pool.p(t[2]) - o);
            volume += a.dot(&b.cross(&c));
        }
    }
    if tris.is_empty()
        || unmatched > 0
        || volume.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        return Err(Error::new(codes::BOOLEAN_INVALID_INPUT)
            .with_detail("edges", i64::try_from(unmatched).unwrap_or(i64::MAX)));
    }
    Ok(Side { tris })
}

/// The points of the intersection of a triangle with a plane, given the snapped signed distances
/// of its vertices: at most two. Crossings are computed from the lower vertex id, so the triangles
/// that share an edge compute the same point.
fn section(pool: &Pool, t: [u32; 3], d: [f64; 3]) -> Vec<Point3> {
    let mut out = Vec::with_capacity(2);
    for k in 0..3 {
        if d[k] == 0.0 {
            out.push(pool.p(t[k]));
        }
    }
    for i in 0..3 {
        let j = (i + 1) % 3;
        if d[i] * d[j] < 0.0 {
            let (lo, hi) = if t[i] < t[j] { (i, j) } else { (j, i) };
            let (p, q) = (pool.p(t[lo]), pool.p(t[hi]));
            out.push(p + (q - p) * (d[lo] / (d[lo] - d[hi])));
        }
    }
    out
}

/// The signed distances of the vertices of `t` to the plane of `g`, zero within the tolerance.
fn distances(pool: &Pool, t: [u32; 3], g: &Geo, tol: f64) -> [f64; 3] {
    t.map(|v| {
        let d = g.n.dot(&pool.p(v).coords) - g.d;
        if d.abs() <= tol { 0.0 } else { d }
    })
}

fn one_side(d: &[f64; 3]) -> bool {
    d.iter().all(|&x| x > 0.0) || d.iter().all(|&x| x < 0.0)
}

/// How two triangles meet.
enum Meet {
    Nothing,
    Coplanar,
    Segment(u32, u32),
}

fn meet(pool: &mut Pool, ta: [u32; 3], ga: &Geo, tb: [u32; 3], gb: &Geo, angular: f64) -> Meet {
    if ga.area2 == 0.0 || gb.area2 == 0.0 {
        return Meet::Nothing;
    }
    let tol = pool.tol;
    let da = distances(pool, tb, ga, tol);
    if one_side(&da) {
        return Meet::Nothing;
    }
    let db = distances(pool, ta, gb, tol);
    if one_side(&db) {
        return Meet::Nothing;
    }
    let dir = ga.n.cross(&gb.n);
    let all_zero = |d: &[f64; 3]| d.iter().all(|&x| x == 0.0);
    if all_zero(&da) || all_zero(&db) || dir.norm() <= angular {
        return Meet::Coplanar;
    }
    let dir = dir / dir.norm();
    let origin = pool.p(ta[0]);
    let along = |p: &Point3| (p - origin).dot(&dir);
    let sa = section(pool, ta, db);
    let sb = section(pool, tb, da);
    let (Some(a), Some(b)) = (ordered(&sa, &along), ordered(&sb, &along)) else {
        return Meet::Nothing;
    };
    let lo = if along(&a.0) >= along(&b.0) { a.0 } else { b.0 };
    let hi = if along(&a.1) <= along(&b.1) { a.1 } else { b.1 };
    if along(&hi) - along(&lo) <= tol {
        return Meet::Nothing;
    }
    let (p, q) = (pool.add(lo), pool.add(hi));
    if p == q {
        Meet::Nothing
    } else {
        Meet::Segment(p, q)
    }
}

/// The two points of a section ordered by `along`; `None` unless there are two.
fn ordered(points: &[Point3], along: &dyn Fn(&Point3) -> f64) -> Option<(Point3, Point3)> {
    let [p, q] = points else {
        return None;
    };
    if along(p) <= along(q) {
        Some((*p, *q))
    } else {
        Some((*q, *p))
    }
}

/// The part of every edge of `source` inside the triangle `clip` (both in one plane), as pool ids.
fn clip_edges(pool: &mut Pool, clip: [u32; 3], gc: &Geo, source: [u32; 3]) -> Vec<(u32, u32)> {
    let tol = pool.tol;
    let sides: Vec<(Point3, Vector3)> = (0..3)
        .filter_map(|k| {
            let (v, w) = (pool.p(clip[k]), pool.p(clip[(k + 1) % 3]));
            let inward = gc.n.cross(&(w - v));
            let norm = inward.norm();
            (norm > 0.0).then(|| (v, inward / norm))
        })
        .collect();
    let mut out = Vec::new();
    for k in 0..3 {
        let (p, q) = (pool.p(source[k]), pool.p(source[(k + 1) % 3]));
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        for (v, m) in &sides {
            let (sp, sq) = ((p - v).dot(m), (q - v).dot(m));
            if sp < -tol && sq < -tol {
                t1 = -1.0;
                break;
            }
            if sp < -tol || sq < -tol {
                let t = (sp / (sp - sq)).clamp(0.0, 1.0);
                if sp < -tol {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
        if t1 < t0 || (t1 - t0) * (q - p).norm() <= tol {
            continue;
        }
        let at = |t: f64| {
            if t <= 0.0 {
                p
            } else if t >= 1.0 {
                q
            } else {
                p + (q - p) * t
            }
        };
        let (a, b) = (pool.add(at(t0)), pool.add(at(t1)));
        if a != b {
            out.push((a, b));
        }
    }
    out
}

/// The bounding box of a triangle widened by `tol`.
fn tri_box(pool: &Pool, t: [u32; 3], tol: f64) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for v in t {
        let p = pool.p(v);
        for k in 0..3 {
            lo[k] = lo[k].min(p[k] - tol);
            hi[k] = hi[k].max(p[k] + tol);
        }
    }
    (lo, hi)
}

/// The pairs of triangles of the two sides whose boxes meet, by a sweep along the longest axis.
fn candidates(pool: &Pool, a: &Side, b: &Side) -> Result<Vec<(u32, u32)>> {
    let tol = pool.tol;
    let boxes = |s: &Side| -> Vec<([f64; 3], [f64; 3])> {
        s.tris.iter().map(|&t| tri_box(pool, t, tol)).collect()
    };
    let (ba, bb) = (boxes(a), boxes(b));
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for (l, h) in ba.iter().chain(bb.iter()) {
        for k in 0..3 {
            lo[k] = lo[k].min(l[k]);
            hi[k] = hi[k].max(h[k]);
        }
    }
    let axis = (0..3)
        .max_by(|&i, &j| (hi[i] - lo[i]).total_cmp(&(hi[j] - lo[j])))
        .unwrap_or(0);
    // Sweep along the axis: every box is tested against the boxes of the other side that started
    // before it and have not ended, so separated boxes are never compared.
    let start = |&(in_b, k): &(bool, usize)| if in_b { bb[k].0[axis] } else { ba[k].0[axis] };
    let mut events: Vec<(bool, usize)> = (0..ba.len())
        .map(|i| (false, i))
        .chain((0..bb.len()).map(|j| (true, j)))
        .collect();
    events.sort_by(|x, y| start(x).total_cmp(&start(y)));
    let (mut active_a, mut active_b): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let (mut out, mut tests) = (Vec::new(), 0_usize);
    for &(in_b, k) in &events {
        let ((l, h), active, boxes) = if in_b {
            (&bb[k], &mut active_a, &ba)
        } else {
            (&ba[k], &mut active_b, &bb)
        };
        active.retain(|&j| boxes[j].1[axis] >= l[axis]);
        tests = tests.saturating_add(active.len());
        if tests > MAX_TESTS {
            return Err(limit());
        }
        for &j in active.iter() {
            let (bl, bh) = &boxes[j];
            if (0..3).all(|m| bl[m] <= h[m] && bh[m] >= l[m]) {
                out.push(if in_b { (id(j), id(k)) } else { (id(k), id(j)) });
                if out.len() > MAX_PAIRS {
                    return Err(limit());
                }
            }
        }
        if in_b {
            active_b.push(k);
        } else {
            active_a.push(k);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// The curve points (endpoints of intersection segments), hashed for queries along segments.
struct Curve {
    ids: Vec<u32>,
    cells: HashMap<[i64; 3], Vec<u32>>,
    size: f64,
    tol: f64,
}

impl Curve {
    fn new(pool: &Pool, ids: Vec<u32>) -> Self {
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for &i in &ids {
            let p = pool.p(i);
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
        let size = (8.0 * pool.tol).max(extent / 128.0).max(f64::MIN_POSITIVE);
        let mut cells: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
        for &i in &ids {
            cells.entry(cell_of(&pool.p(i), size)).or_default().push(i);
        }
        Self {
            ids,
            cells,
            size,
            tol: pool.tol,
        }
    }

    /// The curve points strictly between `a` and `b` and within the tolerance of the segment,
    /// ordered from `a` to `b`.
    fn along(&self, pool: &Pool, a: u32, b: u32) -> Vec<u32> {
        let (p, q) = (pool.p(a), pool.p(b));
        let line = q - p;
        let len2 = line.norm_squared();
        if len2 <= 0.0 || self.ids.is_empty() {
            return Vec::new();
        }
        let lo = cell_of(
            &Point3::new(
                p.x.min(q.x) - self.tol,
                p.y.min(q.y) - self.tol,
                p.z.min(q.z) - self.tol,
            ),
            self.size,
        );
        let hi = cell_of(
            &Point3::new(
                p.x.max(q.x) + self.tol,
                p.y.max(q.y) + self.tol,
                p.z.max(q.z) + self.tol,
            ),
            self.size,
        );
        let span = (0..3)
            .map(|k| u128::try_from(hi[k].saturating_sub(lo[k]).max(0)).unwrap_or(0) + 1)
            .product::<u128>();
        let mut found: Vec<(f64, u32)> = Vec::new();
        let mut consider = |k: u32| {
            if k == a || k == b {
                return;
            }
            let r = pool.p(k);
            let t = (r - p).dot(&line) / len2;
            if t <= 0.0 || t >= 1.0 {
                return;
            }
            if (r - (p + line * t)).norm() <= self.tol {
                found.push((t, k));
            }
        };
        if span > 4096 {
            self.ids.iter().for_each(|&k| consider(k));
        } else {
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        for &k in self.cells.get(&[x, y, z]).map_or(&[][..], Vec::as_slice) {
                            consider(k);
                        }
                    }
                }
            }
        }
        found.sort_by(|u, v| u.0.total_cmp(&v.0));
        found.into_iter().map(|(_, k)| k).collect()
    }
}

/// A piece of a split triangle.
#[derive(Clone, Copy)]
struct Piece {
    ids: [u32; 3],
    parent: usize,
}

/// The planar coordinates of `p` that keep a triangle with normal `n` counterclockwise.
fn flat(p: &Point3, n: &Vector3) -> Point2 {
    let axis = (0..3)
        .max_by(|&i, &j| n[i].abs().total_cmp(&n[j].abs()))
        .unwrap_or(2);
    let (u, v) = match axis {
        0 => (p.y, p.z),
        1 => (p.z, p.x),
        _ => (p.x, p.y),
    };
    if n[axis] < 0.0 {
        Point2::new(-u, v)
    } else {
        Point2::new(u, v)
    }
}

/// Retriangulates one triangle along its edge chains and constraint segments.
fn retriangulate(
    pool: &Pool,
    t: [u32; 3],
    n: &Vector3,
    chains: [&[u32]; 3],
    units: &[(u32, u32)],
) -> Result<Vec<[u32; 3]>> {
    let mut outline: Vec<u32> = Vec::new();
    for k in 0..3 {
        outline.push(t[k]);
        outline.extend_from_slice(chains[k]);
    }
    let mut local: HashMap<u32, u32> = outline
        .iter()
        .enumerate()
        .map(|(k, &v)| (v, id(k)))
        .collect();
    let mut order = outline.clone();
    for &(a, b) in units {
        for v in [a, b] {
            if let std::collections::hash_map::Entry::Vacant(e) = local.entry(v) {
                e.insert(id(order.len()));
                order.push(v);
            }
        }
    }
    let points: Vec<Point2> = order.iter().map(|&v| flat(&pool.p(v), n)).collect();
    let ring: Vec<u32> = (0..outline.len()).map(id).collect();
    let edges: Vec<[u32; 2]> = units
        .iter()
        .filter_map(|(a, b)| Some([*local.get(a)?, *local.get(b)?]))
        .collect();
    let triangles = triangulate_graph(&points, &[ring], &edges).map_err(|_| failed())?;
    Ok(triangles
        .into_iter()
        .map(|t| t.map(|k| order.get(k as usize).copied().unwrap_or(0)))
        .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        .collect())
}

/// The relation of a piece of one operand to the other solid.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Outside,
    Inside,
    Same,
    Opposite,
}

/// The state shared by the two sides while they are split and classified.
struct Context<'a> {
    pool: &'a Pool,
    curve: &'a Curve,
    chains: HashMap<(u32, u32), Vec<u32>>,
    barrier: &'a HashSet<(u32, u32)>,
}

impl Context<'_> {
    /// The chain of curve points along the edge `a`→`b`, in that direction.
    fn chain(&mut self, a: u32, b: u32) -> Vec<u32> {
        let key = pair(a, b);
        let pool = self.pool;
        let curve = self.curve;
        let stored = self
            .chains
            .entry(key)
            .or_insert_with(|| curve.along(pool, key.0, key.1));
        if a <= b {
            stored.clone()
        } else {
            stored.iter().rev().copied().collect()
        }
    }

    /// The pieces of every triangle of a side, split along `segments`.
    fn split(
        &mut self,
        side: &Side,
        geos: &[Geo],
        segments: &[Vec<(u32, u32)>],
    ) -> Result<Vec<Piece>> {
        let mut pieces = Vec::with_capacity(side.tris.len());
        for (k, &t) in side.tris.iter().enumerate() {
            let chains = [
                self.chain(t[0], t[1]),
                self.chain(t[1], t[2]),
                self.chain(t[2], t[0]),
            ];
            let segs = segments.get(k).map_or(&[][..], Vec::as_slice);
            let split = chains.iter().any(|c| !c.is_empty()) || !segs.is_empty();
            let n = geos.get(k).map_or(Vector3::z(), |g| g.n);
            if !split || geos.get(k).is_none_or(|g| g.area2 == 0.0) {
                pieces.push(Piece { ids: t, parent: k });
                continue;
            }
            let mut units: Vec<(u32, u32)> = Vec::new();
            for &(a, b) in segs {
                let mut chain = vec![a];
                chain.extend(self.curve.along(self.pool, a, b));
                chain.push(b);
                units.extend(chain.windows(2).map(|w| (w[0], w[1])));
            }
            let parts = retriangulate(
                self.pool,
                t,
                &n,
                [&chains[0], &chains[1], &chains[2]],
                &units,
            )?;
            pieces.extend(parts.into_iter().map(|ids| Piece { ids, parent: k }));
            if pieces.len() > MAX_PAIRS {
                return Err(limit());
            }
        }
        Ok(pieces)
    }

    /// Classifies the pieces of one side against the other solid.
    fn classify(
        &self,
        pieces: &[Piece],
        geos: &[Geo],
        partners: &[Vec<u32>],
        other: &Side,
        other_geos: &[Geo],
    ) -> Vec<Class> {
        let tol = self.pool.tol;
        let mut class = vec![Class::Outside; pieces.len()];
        let mut free = vec![true; pieces.len()];
        for (k, piece) in pieces.iter().enumerate() {
            let c = centroid(self.pool, piece.ids);
            let n = geos.get(piece.parent).map_or(Vector3::zeros(), |g| g.n);
            for &q in partners.get(piece.parent).map_or(&[][..], Vec::as_slice) {
                let (Some(&tq), Some(gq)) =
                    (other.tris.get(q as usize), other_geos.get(q as usize))
                else {
                    continue;
                };
                if inside_triangle(self.pool, tq, gq, &c, tol) {
                    class[k] = if n.dot(&gq.n) > 0.0 {
                        Class::Same
                    } else {
                        Class::Opposite
                    };
                    free[k] = false;
                    break;
                }
            }
        }
        // Components of free pieces, connected across edges that are not on the curve.
        let mut parent: Vec<usize> = (0..pieces.len()).collect();
        let mut by_edge: HashMap<(u32, u32), usize> = HashMap::new();
        for (k, piece) in pieces.iter().enumerate() {
            if !free[k] {
                continue;
            }
            for e in 0..3 {
                let key = pair(piece.ids[e], piece.ids[(e + 1) % 3]);
                if self.barrier.contains(&key) {
                    continue;
                }
                if let Some(&j) = by_edge.get(&key) {
                    union(&mut parent, j, k);
                } else {
                    by_edge.insert(key, k);
                }
            }
        }
        let mut best: HashMap<usize, (f64, usize)> = HashMap::new();
        for (k, piece) in pieces.iter().enumerate() {
            if free[k] {
                let root = find(&mut parent, k);
                let area = area2(self.pool, piece.ids);
                let entry = best.entry(root).or_insert((-1.0, k));
                if area > entry.0 {
                    *entry = (area, k);
                }
            }
        }
        let mut verdict: HashMap<usize, Class> = HashMap::new();
        for (&root, &(_, k)) in &best {
            let c = centroid(self.pool, pieces[k].ids);
            let w = winding(self.pool, other, &c);
            verdict.insert(
                root,
                if w > 0.5 {
                    Class::Inside
                } else {
                    Class::Outside
                },
            );
        }
        for (k, c) in class.iter_mut().enumerate() {
            if free[k] {
                let root = find(&mut parent, k);
                *c = verdict.get(&root).copied().unwrap_or(Class::Outside);
            }
        }
        class
    }
}

fn find(parent: &mut [usize], mut k: usize) -> usize {
    while parent[k] != k {
        parent[k] = parent[parent[k]];
        k = parent[k];
    }
    k
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

fn centroid(pool: &Pool, t: [u32; 3]) -> Point3 {
    Point3::from((pool.p(t[0]).coords + pool.p(t[1]).coords + pool.p(t[2]).coords) / 3.0)
}

fn area2(pool: &Pool, t: [u32; 3]) -> f64 {
    (pool.p(t[1]) - pool.p(t[0]))
        .cross(&(pool.p(t[2]) - pool.p(t[0])))
        .norm()
}

/// Whether `c`, in the plane of the triangle, lies inside it (within the tolerance).
fn inside_triangle(pool: &Pool, t: [u32; 3], g: &Geo, c: &Point3, tol: f64) -> bool {
    if (g.n.dot(&c.coords) - g.d).abs() > 10.0 * tol {
        return false;
    }
    (0..3).all(|k| {
        let (v, w) = (pool.p(t[k]), pool.p(t[(k + 1) % 3]));
        let inward = g.n.cross(&(w - v));
        let norm = inward.norm();
        norm > 0.0 && (c - v).dot(&inward) / norm >= -tol
    })
}

/// The generalized winding number of the closed mesh `side` at `p`: 1 inside, 0 outside.
fn winding(pool: &Pool, side: &Side, p: &Point3) -> f64 {
    let mut total = 0.0;
    for t in &side.tris {
        let a = pool.p(t[0]) - p;
        let b = pool.p(t[1]) - p;
        let c = pool.p(t[2]) - p;
        let (la, lb, lc) = (a.norm(), b.norm(), c.norm());
        let denominator = la * lb * lc + a.dot(&b) * lc + a.dot(&c) * lb + b.dot(&c) * la;
        total += 2.0 * a.dot(&b.cross(&c)).atan2(denominator);
    }
    total / (4.0 * std::f64::consts::PI)
}

/// The union, difference or intersection of two closed meshes; see the [module
/// documentation](self).
///
/// Both meshes must be closed, consistently oriented with counterclockwise triangles seen from
/// outside, and have positive volume. The result has the same orientation and is closed; it is
/// empty (no triangles) when the operation leaves no volume.
///
/// # Errors
///
/// `mesh.tolerance.invalid` for a tolerance that is not finite and positive;
/// `mesh.boolean.invalid_input` for an operand that is not a closed oriented 2-manifold of positive
/// volume; `mesh.boolean.failed` when the arrangement cannot be computed at the tolerance or the
/// result is not closed; `mesh.limit.exceeded` beyond `limits` or the bound on triangle pairs.
pub fn boolean(
    a: &TriMesh,
    b: &TriMesh,
    op: BooleanOp,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<TriMesh> {
    crate::ensure_tolerance(tolerance)?;
    if tolerance.linear <= 0.0 {
        return Err(Error::new(codes::TOLERANCE_INVALID));
    }
    let mut pool = Pool::new(tolerance.linear);
    let sa = prepare(a, &mut pool)?;
    let sb = prepare(b, &mut pool)?;
    let ga: Vec<Geo> = sa.tris.iter().map(|&t| geometry(&pool, t)).collect();
    let gb: Vec<Geo> = sb.tris.iter().map(|&t| geometry(&pool, t)).collect();
    let mut segs_a: Vec<Vec<(u32, u32)>> = vec![Vec::new(); sa.tris.len()];
    let mut segs_b: Vec<Vec<(u32, u32)>> = vec![Vec::new(); sb.tris.len()];
    let mut part_a: Vec<Vec<u32>> = vec![Vec::new(); sa.tris.len()];
    let mut part_b: Vec<Vec<u32>> = vec![Vec::new(); sb.tris.len()];
    for (i, j) in candidates(&pool, &sa, &sb)? {
        let (ia, ib) = (i as usize, j as usize);
        let (ta, tb) = (sa.tris[ia], sb.tris[ib]);
        match meet(&mut pool, ta, &ga[ia], tb, &gb[ib], tolerance.angular) {
            Meet::Nothing => {}
            Meet::Segment(p, q) => {
                segs_a[ia].push((p, q));
                segs_b[ib].push((p, q));
            }
            Meet::Coplanar => {
                part_a[ia].push(j);
                part_b[ib].push(i);
                let on_a = clip_edges(&mut pool, ta, &ga[ia], tb);
                let on_b = clip_edges(&mut pool, tb, &gb[ib], ta);
                segs_a[ia].extend(on_a);
                segs_b[ib].extend(on_b);
            }
        }
    }
    let mut curve_ids: Vec<u32> = segs_a
        .iter()
        .chain(segs_b.iter())
        .flatten()
        .flat_map(|&(p, q)| [p, q])
        .collect();
    curve_ids.sort_unstable();
    curve_ids.dedup();
    let curve = Curve::new(&pool, curve_ids);
    let mut barrier: HashSet<(u32, u32)> = HashSet::new();
    for &(p, q) in segs_a.iter().chain(segs_b.iter()).flatten() {
        let mut chain = vec![p];
        chain.extend(curve.along(&pool, p, q));
        chain.push(q);
        barrier.extend(chain.windows(2).map(|w| pair(w[0], w[1])));
    }
    let mut context = Context {
        pool: &pool,
        curve: &curve,
        chains: HashMap::new(),
        barrier: &barrier,
    };
    let pieces_a = context.split(&sa, &ga, &segs_a)?;
    let pieces_b = context.split(&sb, &gb, &segs_b)?;
    let class_a = context.classify(&pieces_a, &ga, &part_a, &sb, &gb);
    let class_b = context.classify(&pieces_b, &gb, &part_b, &sa, &ga);
    let (keep_a, keep_b, flip_b): (&[Class], &[Class], bool) = match op {
        BooleanOp::Union => (&[Class::Outside, Class::Same], &[Class::Outside], false),
        BooleanOp::Intersection => (&[Class::Inside, Class::Same], &[Class::Inside], false),
        BooleanOp::Difference => (&[Class::Outside, Class::Opposite], &[Class::Inside], true),
    };
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for (piece, class) in pieces_a.iter().zip(&class_a) {
        if keep_a.contains(class) {
            triangles.push(piece.ids);
        }
    }
    for (piece, class) in pieces_b.iter().zip(&class_b) {
        if keep_b.contains(class) {
            let [x, y, z] = piece.ids;
            triangles.push(if flip_b { [x, z, y] } else { [x, y, z] });
        }
    }
    assemble(&pool, triangles, limits)
}

/// Builds the result mesh from pool triangles and checks that it is closed.
fn assemble(pool: &Pool, triangles: Vec<[u32; 3]>, limits: &Limits) -> Result<TriMesh> {
    let mut directed: HashMap<(u32, u32), i64> = HashMap::new();
    for t in &triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            *directed.entry(key).or_insert(0) += sign;
        }
    }
    if directed.values().any(|&n| n != 0) {
        return Err(failed());
    }
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut positions: Vec<Point3> = Vec::new();
    let mut out: Vec<[u32; 3]> = Vec::with_capacity(triangles.len());
    for t in &triangles {
        out.push(t.map(|v| {
            *map.entry(v).or_insert_with(|| {
                positions.push(pool.p(v));
                id(positions.len() - 1)
            })
        }));
    }
    TriMeshBuilder::new(positions, out).build(limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::volume;

    const TOL: Tolerance = Tolerance::DEFAULT;

    /// A closed box with outward counterclockwise triangles.
    pub(crate) fn cuboid(lo: [f64; 3], hi: [f64; 3]) -> TriMesh {
        let p = |x: usize, y: usize, z: usize| {
            Point3::new(
                if x == 0 { lo[0] } else { hi[0] },
                if y == 0 { lo[1] } else { hi[1] },
                if z == 0 { lo[2] } else { hi[2] },
            )
        };
        let positions = vec![
            p(0, 0, 0),
            p(1, 0, 0),
            p(1, 1, 0),
            p(0, 1, 0),
            p(0, 0, 1),
            p(1, 0, 1),
            p(1, 1, 1),
            p(0, 1, 1),
        ];
        let quads = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [1, 2, 6, 5],
            [3, 0, 4, 7],
        ];
        let triangles = quads
            .iter()
            .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
            .collect();
        TriMeshBuilder::new(positions, triangles)
            .build(&Limits::DEFAULT)
            .unwrap()
    }

    fn vol(mesh: &TriMesh) -> f64 {
        if mesh.triangle_count() == 0 {
            0.0
        } else {
            volume(mesh, &Limits::DEFAULT).unwrap()
        }
    }

    fn run(a: &TriMesh, b: &TriMesh, op: BooleanOp) -> TriMesh {
        boolean(a, b, op, &TOL, &Limits::DEFAULT).unwrap()
    }

    #[test]
    fn overlapping_boxes() {
        let a = cuboid([0.0; 3], [2.0; 3]);
        let b = cuboid([1.0; 3], [3.0; 3]);
        assert!((vol(&run(&a, &b, BooleanOp::Union)) - 15.0).abs() < 1e-9);
        assert!((vol(&run(&a, &b, BooleanOp::Difference)) - 7.0).abs() < 1e-9);
        assert!((vol(&run(&a, &b, BooleanOp::Intersection)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn coplanar_and_touching_boxes() {
        let a = cuboid([0.0; 3], [2.0; 3]);
        // Shares the face x = 2: the union is one box, the intersection is empty.
        let touching = cuboid([2.0, 0.0, 0.0], [4.0, 2.0, 2.0]);
        assert!((vol(&run(&a, &touching, BooleanOp::Union)) - 16.0).abs() < 1e-9);
        assert_eq!(
            run(&a, &touching, BooleanOp::Intersection).triangle_count(),
            0
        );
        assert!((vol(&run(&a, &touching, BooleanOp::Difference)) - 8.0).abs() < 1e-9);
        // A notch with three coplanar faces.
        let notch = cuboid([1.0, 0.0, 0.0], [3.0, 2.0, 2.0]);
        assert!((vol(&run(&a, &notch, BooleanOp::Difference)) - 4.0).abs() < 1e-9);
        assert!((vol(&run(&a, &notch, BooleanOp::Union)) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn a_box_inside_a_box_leaves_a_void_or_nothing() {
        let a = cuboid([0.0; 3], [4.0; 3]);
        let b = cuboid([1.0; 3], [2.0; 3]);
        assert!((vol(&run(&a, &b, BooleanOp::Union)) - 64.0).abs() < 1e-9);
        assert!((vol(&run(&a, &b, BooleanOp::Intersection)) - 1.0).abs() < 1e-9);
        // The difference is a hollow solid: its volume counts the inner shell as negative.
        assert!((vol(&run(&a, &b, BooleanOp::Difference)) - 63.0).abs() < 1e-9);
        assert_eq!(run(&b, &a, BooleanOp::Difference).triangle_count(), 0);
    }

    #[test]
    fn identical_operands_and_a_cylinder_prism_with_a_slot() {
        let a = cuboid([0.0; 3], [2.0; 3]);
        assert!((vol(&run(&a, &a, BooleanOp::Union)) - 8.0).abs() < 1e-9);
        assert!((vol(&run(&a, &a, BooleanOp::Intersection)) - 8.0).abs() < 1e-9);
        assert_eq!(run(&a, &a, BooleanOp::Difference).triangle_count(), 0);
        // A 64-sided prism (radius 1, height 2) minus a slab |x| <= 0.25 through it: the polygon area
        // inside the slab is computed by clipping the polygon, independently of the boolean.
        let n: u32 = 64;
        let ring = |z: f64| -> Vec<Point3> {
            (0..n)
                .map(|k| {
                    let t = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                    Point3::new(t.cos(), t.sin(), z)
                })
                .collect()
        };
        let mut positions = ring(0.0);
        positions.extend(ring(2.0));
        positions.push(Point3::new(0.0, 0.0, 0.0));
        positions.push(Point3::new(0.0, 0.0, 2.0));
        let (bottom, top) = (2 * n, 2 * n + 1);
        let mut triangles = Vec::new();
        for k in 0..n {
            let m = (k + 1) % n;
            triangles.extend([
                [bottom, m, k],
                [top, n + k, n + m],
                [k, m, n + m],
                [k, n + m, n + k],
            ]);
        }
        let prism = TriMeshBuilder::new(positions, triangles)
            .build(&Limits::DEFAULT)
            .unwrap();
        let slab = cuboid([-0.25, -2.0, -1.0], [0.25, 2.0, 3.0]);
        let result = run(&prism, &slab, BooleanOp::Difference);
        let ring_area = |lo: f64, hi: f64| -> f64 {
            // The area of the polygon with the vertices above between x = lo and x = hi.
            let pts: Vec<(f64, f64)> = (0..n)
                .map(|k| {
                    let t = std::f64::consts::TAU * f64::from(k) / f64::from(n);
                    (t.cos(), t.sin())
                })
                .collect();
            let clip =
                |poly: Vec<(f64, f64)>, keep: &dyn Fn(f64) -> bool, edge: f64| -> Vec<(f64, f64)> {
                    let mut out = Vec::new();
                    for i in 0..poly.len() {
                        let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                        if keep(p.0) {
                            out.push(p);
                        }
                        if keep(p.0) != keep(q.0) {
                            let t = (edge - p.0) / (q.0 - p.0);
                            out.push((edge, p.1 + t * (q.1 - p.1)));
                        }
                    }
                    out
                };
            let poly = clip(pts, &|x| x >= lo, lo);
            let poly = clip(poly, &|x| x <= hi, hi);
            (0..poly.len())
                .map(|i| {
                    let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                    p.0 * q.1 - q.0 * p.1
                })
                .sum::<f64>()
                / 2.0
        };
        let polygon = ring_area(-1.0, 1.0);
        let expected = (polygon - ring_area(-0.25, 0.25)) * 2.0;
        assert!(
            (vol(&result) - expected).abs() < 1e-9,
            "{} vs {expected}",
            vol(&result)
        );
        assert!(manifold(&result));
    }

    #[test]
    fn open_operand_is_refused() {
        let a = cuboid([0.0; 3], [1.0; 3]);
        let open = TriMeshBuilder::new(
            a.positions().to_vec(),
            a.triangles().iter().take(11).copied().collect(),
        )
        .build(&Limits::DEFAULT)
        .unwrap();
        let error = boolean(&a, &open, BooleanOp::Union, &TOL, &Limits::DEFAULT).unwrap_err();
        assert_eq!(error.code(), codes::BOOLEAN_INVALID_INPUT.code);
        // One missing triangle leaves its three edges without their opposites.
        assert_eq!(
            error.details().get("edges"),
            Some(&arc_base::error::Value::from(3_i64))
        );
    }

    /// Cells of the 6x6x6 grid inside a box with integer corners.
    fn cells(lo: [i32; 3], hi: [i32; 3]) -> HashSet<[i32; 3]> {
        let mut out = HashSet::new();
        for x in lo[0]..hi[0] {
            for y in lo[1]..hi[1] {
                for z in lo[2]..hi[2] {
                    out.insert([x, y, z]);
                }
            }
        }
        out
    }

    /// Every undirected edge is used by exactly two triangles, one in each direction.
    fn manifold(mesh: &TriMesh) -> bool {
        let mut count: HashMap<(u32, u32), u32> = HashMap::new();
        for t in mesh.triangles() {
            for k in 0..3 {
                *count.entry(pair(t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        count.values().all(|&n| n == 2)
    }

    fn rotated(mesh: &TriMesh, angle: f64) -> TriMesh {
        let k = Vector3::new(1.0, 2.0, 3.0).normalize();
        let (sin, cos) = angle.sin_cos();
        let positions = mesh
            .positions()
            .iter()
            .map(|p| {
                let v = p.coords;
                let r = v * cos + k.cross(&v) * sin + k * (k.dot(&v) * (1.0 - cos));
                Point3::from(r + Vector3::new(0.3, -0.2, 0.1))
            })
            .collect();
        TriMeshBuilder::new(positions, mesh.triangles().to_vec())
            .build(&Limits::DEFAULT)
            .unwrap()
    }

    fn corner() -> impl Strategy<Value = ([i32; 3], [i32; 3])> {
        proptest::array::uniform3(0..5_i32).prop_flat_map(|lo| {
            proptest::array::uniform3(1..4_i32)
                .prop_map(move |e| (lo, [lo[0] + e[0], lo[1] + e[1], lo[2] + e[2]]))
        })
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        #[test]
        fn grid_boxes_match_the_cell_count(a in corner(), b in corner(), c in corner(), turn in prop::bool::ANY) {
            let angle = if turn { 0.7 } else { 0.0 };
            let make = |(lo, hi): ([i32; 3], [i32; 3])| {
                let m = cuboid(lo.map(f64::from), hi.map(f64::from));
                if turn { rotated(&m, angle) } else { m }
            };
            let (ma, mb, mc) = (make(a), make(b), make(c));
            let (ca, cb, cc) = (cells(a.0, a.1), cells(b.0, b.1), cells(c.0, c.1));
            for op in [BooleanOp::Union, BooleanOp::Difference, BooleanOp::Intersection] {
                let result = boolean(&ma, &mb, op, &TOL, &Limits::DEFAULT).unwrap();
                let expected = match op {
                    BooleanOp::Union => ca.union(&cb).count(),
                    BooleanOp::Difference => ca.difference(&cb).count(),
                    BooleanOp::Intersection => ca.intersection(&cb).count(),
                } as f64;
                prop_assert!((vol(&result) - expected).abs() < 1e-6, "{op:?} {} vs {expected}", vol(&result));
                let base: HashSet<[i32; 3]> = match op {
                    BooleanOp::Union => ca.union(&cb).copied().collect(),
                    BooleanOp::Difference => ca.difference(&cb).copied().collect(),
                    BooleanOp::Intersection => ca.intersection(&cb).copied().collect(),
                };
                if expected > 0.0 && !pinched(&base) {
                    prop_assert!(manifold(&result));
                }
                // A second operation on the result (an L or U shaped solid).
                if result.triangle_count() > 0 && expected > 0.0 {
                    let second = boolean(&result, &mc, BooleanOp::Difference, &TOL, &Limits::DEFAULT);
                    if pinched(&base) {
                        continue;
                    }
                    let second = second.unwrap();
                    prop_assert!((vol(&second) - base.difference(&cc).count() as f64).abs() < 1e-6);
                }
            }
        }
    }

    /// Whether the cell set has an edge shared by two diagonal cells only (a non-manifold edge).
    fn pinched(set: &HashSet<[i32; 3]>) -> bool {
        set.iter().any(|c| {
            (0..3).any(|axis| {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let at = |du: i32, dv: i32| {
                    let mut q = *c;
                    q[u] += du;
                    q[v] += dv;
                    set.contains(&q)
                };
                (at(1, 1) && !at(1, 0) && !at(0, 1)) || (at(1, -1) && !at(1, 0) && !at(0, -1))
            })
        })
    }
}
