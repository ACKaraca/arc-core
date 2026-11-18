//! Shared helpers: checked accessors, exact vertex welding, edge incidence and work budgets.
//!
//! The accessors return a neutral value for an index outside the slice. Meshes are validated on
//! construction, so that value is never used; it only keeps this crate free of panics.

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, Result};

use crate::TriMesh;
use crate::error::codes;

/// The value at `i`, or the default for an index outside the slice.
pub(crate) fn get<T: Copy + Default>(s: &[T], i: usize) -> T {
    s.get(i).copied().unwrap_or_default()
}

/// `n` as a `u32`, saturating; counts of a validated mesh fit.
pub(crate) fn idx(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `n` as an `i64` for error details.
pub(crate) fn detail(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The position of vertex `i` as a vector.
pub(crate) fn pos(positions: &[Point3], i: u32) -> Vector3 {
    positions
        .get(i as usize)
        .map_or_else(Vector3::zeros, |p| p.coords)
}

/// The three positions of a triangle.
pub(crate) fn corners(positions: &[Point3], t: [u32; 3]) -> [Vector3; 3] {
    t.map(|i| pos(positions, i))
}

/// Fails with `mesh.limit.exceeded` when the mesh has more vertices or triangles than
/// `limits.array_length`, or when `bytes_per_triangle` times the triangle count exceeds the
/// memory budget.
pub(crate) fn check_mesh(mesh: &TriMesh, limits: &Limits, bytes_per_triangle: u64) -> Result<()> {
    let fits = |n: usize| u64::try_from(n).is_ok_and(|n| n <= limits.array_length);
    if !fits(mesh.positions.len()) || !fits(mesh.triangles.len()) {
        return Err(Error::new(codes::LIMIT_EXCEEDED));
    }
    let bytes = u64::try_from(mesh.triangles.len())
        .unwrap_or(u64::MAX)
        .saturating_mul(bytes_per_triangle);
    check_memory(limits, bytes)
}

/// Fails with `mesh.limit.exceeded` when `bytes` exceeds the memory budget.
pub(crate) fn check_memory(limits: &Limits, bytes: u64) -> Result<()> {
    match limits.memory_bytes {
        Some(budget) if bytes > budget => Err(Error::new(codes::LIMIT_EXCEEDED)),
        _ => Ok(()),
    }
}

/// A bound on the steps of a loop: `per_element` steps for each element `limits.array_length`
/// allows.
pub(crate) struct Budget(u64);

impl Budget {
    pub(crate) fn new(limits: &Limits, per_element: u64) -> Self {
        Self(limits.array_length.saturating_mul(per_element))
    }

    /// Spends `n` steps; fails with `mesh.limit.exceeded` when none are left.
    pub(crate) fn spend(&mut self, n: usize) -> Result<()> {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        match self.0.checked_sub(n) {
            Some(left) => {
                self.0 = left;
                Ok(())
            }
            None => Err(Error::new(codes::LIMIT_EXCEEDED)),
        }
    }
}

/// A compensated (Neumaier) sum, so long sums of measured quantities keep their precision.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Sum {
    total: f64,
    carry: f64,
}

impl Sum {
    pub(crate) fn add(&mut self, x: f64) {
        let t = self.total + x;
        if self.total.abs() >= x.abs() {
            self.carry += (self.total - t) + x;
        } else {
            self.carry += (x - t) + self.total;
        }
        self.total = t;
    }

    pub(crate) fn value(&self) -> f64 {
        self.total + self.carry
    }
}

/// The largest power of two not above `m` (exact scaling by it loses no bits), or `m` itself
/// for zero and subnormal values.
pub(crate) fn power_of_two_scale(m: f64) -> f64 {
    let bits = m.to_bits() & 0x7ff0_0000_0000_0000;
    if bits == 0 || !m.is_finite() {
        m
    } else {
        f64::from_bits(bits)
    }
}

/// For every vertex the lowest index of the vertices at bit-for-bit the same position.
pub(crate) fn exact_canon(positions: &[Point3]) -> Vec<u32> {
    let key = |i: u32| {
        let p = pos(positions, i);
        [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
    };
    let mut order: Vec<u32> = (0..idx(positions.len())).collect();
    order.sort_unstable_by_key(|&i| (key(i), i));
    let mut canon = vec![0_u32; positions.len()];
    let mut first = 0_u32;
    let mut previous = None;
    for &i in &order {
        if previous != Some(key(i)) {
            first = i;
            previous = Some(key(i));
        }
        if let Some(slot) = canon.get_mut(i as usize) {
            *slot = first;
        }
    }
    canon
}

/// A union-find over indices whose roots are the lowest index of their set.
pub(crate) struct Dsu(Vec<u32>);

impl Dsu {
    pub(crate) fn new(n: usize) -> Self {
        Self((0..idx(n)).collect())
    }

    pub(crate) fn find(&mut self, mut i: u32) -> u32 {
        loop {
            let parent = get(&self.0, i as usize);
            if parent == i {
                return i;
            }
            let grand = get(&self.0, parent as usize);
            if let Some(slot) = self.0.get_mut(i as usize) {
                *slot = grand;
            }
            i = grand;
        }
    }

    pub(crate) fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        let (low, high) = (a.min(b), a.max(b));
        if let Some(slot) = self.0.get_mut(high as usize) {
            *slot = low;
        }
    }
}

/// One use of an undirected edge by a triangle.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EdgeUse {
    pub(crate) lo: u32,
    pub(crate) hi: u32,
    pub(crate) tri: u32,
    /// The corner of the triangle the edge starts at.
    pub(crate) slot: u8,
    /// Whether the triangle runs the edge from `lo` to `hi`.
    pub(crate) forward: bool,
}

/// What lies across an edge of a triangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Adj {
    /// No other triangle (or the triangle is not part of the table).
    #[default]
    Boundary,
    /// Exactly one other triangle; `same` when it runs the edge in the same direction.
    Pair { tri: u32, same: bool },
    /// More than one other triangle.
    NonManifold,
}

/// Every edge use of a set of triangles, sorted by edge and then by triangle.
pub(crate) struct Edges {
    pub(crate) uses: Vec<EdgeUse>,
    /// The start of each group of uses of one edge, and the end of the last.
    pub(crate) starts: Vec<usize>,
}

impl Edges {
    /// The edges of the given `(index, vertices)` triangles. Triangles with a repeated vertex
    /// have no edges and are skipped.
    pub(crate) fn build(triangles: impl IntoIterator<Item = (u32, [u32; 3])>) -> Self {
        let mut uses = Vec::new();
        for (tri, v) in triangles {
            if v[0] == v[1] || v[1] == v[2] || v[2] == v[0] {
                continue;
            }
            for slot in 0..3_usize {
                let (a, b) = (get(&v, slot), get(&v, (slot + 1) % 3));
                uses.push(EdgeUse {
                    lo: a.min(b),
                    hi: a.max(b),
                    tri,
                    slot: u8::try_from(slot).unwrap_or(0),
                    forward: a < b,
                });
            }
        }
        uses.sort_unstable_by_key(|e| (e.lo, e.hi, e.tri, e.slot));
        let mut starts = Vec::new();
        for (i, e) in uses.iter().enumerate() {
            let new = i == 0 || uses.get(i - 1).is_none_or(|p| (p.lo, p.hi) != (e.lo, e.hi));
            if new {
                starts.push(i);
            }
        }
        starts.push(uses.len());
        Self { uses, starts }
    }

    /// The groups of uses, one per distinct edge, in edge order.
    pub(crate) fn groups(&self) -> impl Iterator<Item = &[EdgeUse]> {
        self.starts
            .windows(2)
            .filter_map(|w| self.uses.get(w[0]..w[1]))
    }

    /// For every triangle and edge slot what lies across it.
    pub(crate) fn adjacency(&self, triangle_count: usize) -> Vec<[Adj; 3]> {
        let mut adj = vec![[Adj::Boundary; 3]; triangle_count];
        let mut set = |tri: u32, slot: u8, value: Adj| {
            if let Some(cell) = adj
                .get_mut(tri as usize)
                .and_then(|a| a.get_mut(usize::from(slot)))
            {
                *cell = value;
            }
        };
        for g in self.groups() {
            match g {
                [a, b] => {
                    let same = a.forward == b.forward;
                    set(a.tri, a.slot, Adj::Pair { tri: b.tri, same });
                    set(b.tri, b.slot, Adj::Pair { tri: a.tri, same });
                }
                [_] | [] => {}
                many => {
                    for e in many {
                        set(e.tri, e.slot, Adj::NonManifold);
                    }
                }
            }
        }
        adj
    }
}

/// Whether the triangle has two equal vertices.
pub(crate) fn index_degenerate(t: [u32; 3]) -> bool {
    t[0] == t[1] || t[1] == t[2] || t[2] == t[0]
}

/// The triangle with the winding reversed.
pub(crate) fn flipped(t: [u32; 3]) -> [u32; 3] {
    [t[0], t[2], t[1]]
}
