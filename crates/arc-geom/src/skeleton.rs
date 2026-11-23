//! The weighted straight skeleton of a simple polygon: the planar surface that rises from every
//! edge at its own rate, as a roof rises from its eaves (CORE-GEOM-035).
//!
//! Each edge `i` of the polygon moves inward, parallel to itself, by `speeds[i]` per unit of
//! height. The surface is the set of points `(x, y, z)` where the moving edges pass over `(x, y)`
//! at height `z`: over each point it is the height at which the front of moving edges first
//! reaches it. Every part of the surface lies in the plane of one edge, the plane through the
//! edge that rises by `1 / speeds[i]` per unit of horizontal distance. An edge with speed zero
//! does not move: its plane is vertical (the gable of a roof).
//!
//! # Method
//!
//! The front is simulated event by event, from published descriptions of the straight skeleton
//! as a wavefront propagation. Every vertex of the front moves on a straight line, with the
//! velocity at which the lines of its two edges move it. Between events nothing changes but the
//! positions. The next event is the earliest of:
//!
//! - an *edge event*: an edge shrinks to no length as its two vertices meet;
//! - a *split event*: a reflex vertex reaches an edge that is not its own, within the edge.
//!
//! All vertices move to the event height, then the front is cleaned up within
//! `tolerance.linear`: vertices that meet merge (the edge between them ends), a vertex that
//! reaches an edge splits its polygon in two, and a part of the front that has folded flat (a
//! spike of two edges lying on each other) ends in a ridge. A polygon with fewer than three
//! vertices left ends; its last two vertices give a ridge. Several events at one height are
//! handled in one cleanup. The simulation ends when no polygon is left.
//!
//! The area each edge sweeps between two events is a convex cell (a trapezoid or triangle in the
//! plane of that edge); successive cells of an edge whose two vertices did not change are one
//! cell. Every vertex traces a straight arc between the event that made it and the event that
//! ends it: the arc of a convex vertex is a hip, of a reflex vertex a valley, and the line where
//! two edges meet flat is a ridge. Arcs next to an edge of speed zero lie on that edge's line
//! and are not listed.
//!
//! # Checks
//!
//! The input must be a simple polygon (no edge crosses, touches or folds back over another)
//! with at least three vertices, no edge shorter than `tolerance.linear`, at most
//! [`MAX_SKELETON_EDGES`] edges, finite coordinates and finite speeds that are not negative, at
//! least one of them positive. Adjacent edges on one line are merged when their speeds are equal
//! (the second edge then has no cells) and refused when they differ. After the simulation the
//! plan areas of the cells must add up to the area of the polygon within `tolerance.linear`
//! times the length of all cell boundaries, else the result is refused: a wrong surface is never
//! returned. The number of events is bounded by eight times the number of edges.
//!
//! | Case | Result |
//! |---|---|
//! | Fewer than three vertices, a crossing, touching or fold-back, an edge shorter than the tolerance, too many edges, a speed that is negative or not finite | `geom.skeleton.polygon_invalid` |
//! | Every speed zero | `geom.skeleton.no_slope` |
//! | Adjacent collinear edges with different speeds | `geom.skeleton.collinear_speeds`, with the vertex as `x` and `y` |
//! | A part of the polygon that no moving edge can reach (enclosed by edges of speed zero), or an edge of speed zero at a reflex corner, whose vertical plane would run into the polygon | `geom.skeleton.unclosed`, with a point of that part as `x` and `y` |
//! | Too many events, a velocity that is not finite, or cells that do not cover the polygon | `geom.skeleton.degenerate`, with a point near the failure as `x` and `y` |
//!
//! Cost: each event costs `O(n²)` in the number of vertices of the front (every reflex vertex
//! against every edge), and there are `O(n)` events.

use std::collections::HashMap;

use arc_base::math::{Point2, Point3, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::error::codes;
use crate::predicates::{Sign, orient2d};

/// The most edges a polygon may have.
pub const MAX_SKELETON_EDGES: usize = 256;

/// The kind of a line where two planes of the surface meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcKind {
    /// A level line where two planes meet at the top.
    Ridge,
    /// A sloping line from a convex corner, where the surface folds down.
    Hip,
    /// A sloping line from a reflex corner, where the surface folds up.
    Valley,
}

/// A line of the skeleton between two cells of different edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkeletonArc {
    /// The lower end (`z` is the height).
    pub start: Point3,
    /// The upper end.
    pub end: Point3,
    /// What the line is.
    pub kind: ArcKind,
    /// The two edges whose planes meet along it.
    pub edges: [usize; 2],
}

/// A convex planar piece of the surface in the plane of one edge.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonCell {
    /// The edge of the input polygon whose plane the cell lies in.
    pub edge: usize,
    /// The corners, counterclockwise seen from above for an edge that moves; vertical for an
    /// edge of speed zero.
    pub points: Vec<Point3>,
}

/// The weighted straight skeleton of a polygon.
#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    /// The cells, which together cover the polygon once (vertical cells cover nothing).
    pub cells: Vec<SkeletonCell>,
    /// The ridges, hips and valleys.
    pub arcs: Vec<SkeletonArc>,
    /// The height of the highest point.
    pub height: f64,
    lines: Vec<EdgeLine>,
    origin: Vector2,
}

impl Skeleton {
    /// The height of the plane of `edge` over `point`; `None` for an edge of speed zero or an
    /// index out of range.
    #[must_use]
    pub fn edge_height(&self, edge: usize, point: &Point2) -> Option<f64> {
        let line = self.lines.get(edge)?;
        (line.speed > 0.0)
            .then(|| (line.normal.dot(&(point.coords - self.origin)) - line.offset) / line.speed)
    }

    /// The height of the surface over `point`: the plane of the first cell that contains it
    /// within `tolerance.linear`; `None` outside the polygon.
    #[must_use]
    pub fn height_at(&self, point: &Point2, tolerance: &Tolerance) -> Option<f64> {
        self.cells
            .iter()
            .filter(|c| self.lines.get(c.edge).is_some_and(|l| l.speed > 0.0))
            .find(|c| convex_contains(&c.points, point, tolerance.linear))
            .and_then(|c| self.edge_height(c.edge, point))
    }
}

/// Whether the convex counterclockwise polygon `points` (in plan) contains `p` within `margin`.
fn convex_contains(points: &[Point3], p: &Point2, margin: f64) -> bool {
    let n = points.len();
    n >= 3
        && (0..n).all(|k| {
            let (Some(a), Some(b)) = (points.get(k), points.get((k + 1) % n)) else {
                return false;
            };
            let e = Vector2::new(b.x - a.x, b.y - a.y);
            let length = e.norm();
            length <= margin || (e.x * (p.y - a.y) - e.y * (p.x - a.x)) / length >= -margin
        })
}

/// The line of an edge as it moves: the points `x` with `normal · x = offset + speed · z` at
/// height `z`; `normal` is the unit inward normal.
#[derive(Debug, Clone, Copy, PartialEq)]
struct EdgeLine {
    normal: Vector2,
    offset: f64,
    speed: f64,
}

impl EdgeLine {
    /// The direction of the edge (the inward normal turned clockwise).
    fn direction(&self) -> Vector2 {
        Vector2::new(self.normal.y, -self.normal.x)
    }
}

/// Where a vertex of the front was made.
#[derive(Debug, Clone, Copy)]
struct Born {
    at: Point2,
    z: f64,
    reflex: bool,
    edges: [usize; 2],
}

/// A vertex of the front: its position now, the edge that starts at it and its identity.
#[derive(Debug, Clone, Copy)]
struct Node {
    p: Point2,
    edge: usize,
    id: usize,
}

fn degenerate(at: Point2, origin: Vector2) -> Error {
    located(codes::SKELETON_DEGENERATE, at, origin)
}

fn located(code: arc_base::ErrorCode, at: Point2, origin: Vector2) -> Error {
    Error::new(code)
        .with_arg("x", format!("{:.6}", at.x + origin.x))
        .with_arg("y", format!("{:.6}", at.y + origin.y))
}

fn invalid() -> Error {
    Error::new(codes::SKELETON_POLYGON_INVALID)
}

fn cross(a: Vector2, b: Vector2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// Twice the signed area of a polygon.
fn double_area(points: &[Point2]) -> f64 {
    let n = points.len();
    (0..n)
        .filter_map(|k| Some((points.get(k)?, points.get((k + 1) % n)?)))
        .map(|(a, b)| a.x * b.y - a.y * b.x)
        .sum()
}

/// Whether segments `ab` and `cd` cross or touch, decided by the exact orientation tests.
fn segments_meet(a: &Point2, b: &Point2, c: &Point2, d: &Point2) -> Result<bool> {
    let (d1, d2) = (orient2d(a, b, c)?, orient2d(a, b, d)?);
    let (d3, d4) = (orient2d(c, d, a)?, orient2d(c, d, b)?);
    let on = |p: &Point2, q: &Point2, r: &Point2| {
        r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
    };
    let opposite = |s: Sign, t: Sign| {
        matches!(
            (s, t),
            (Sign::Positive, Sign::Negative) | (Sign::Negative, Sign::Positive)
        )
    };
    Ok((opposite(d1, d2) && opposite(d3, d4))
        || (d1 == Sign::Zero && on(a, b, c))
        || (d2 == Sign::Zero && on(a, b, d))
        || (d3 == Sign::Zero && on(c, d, a))
        || (d4 == Sign::Zero && on(c, d, b)))
}

/// Refuses a polygon that is not simple: two edges that are not neighbors meet, or two
/// neighbors fold back over each other.
fn check_simple(points: &[Point2]) -> Result<()> {
    let n = points.len();
    let at = |k: usize| points.get(k % n).ok_or_else(invalid);
    for i in 0..n {
        let (a, b) = (at(i)?, at(i + 1)?);
        // Neighbors fold back when the next edge turns back along this one.
        let c = at(i + 2)?;
        if orient2d(a, b, c)? == Sign::Zero && (b - a).dot(&(c - b)) < 0.0 {
            return Err(invalid());
        }
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if segments_meet(a, b, at(j)?, at(j + 1)?)? {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

/// The weighted straight skeleton of the simple polygon `polygon` (either orientation; edge `i`
/// runs from vertex `i` to the next) with the edge speeds `speeds`, as described in the [module
/// documentation](self). Heights are measured from the plane of the polygon.
///
/// # Errors
/// See the table in the module documentation.
#[allow(
    clippy::too_many_lines,
    reason = "the checks and set-up of the input, step by step"
)]
pub fn weighted_skeleton(
    polygon: &[Point2],
    speeds: &[f64],
    tolerance: &Tolerance,
) -> Result<Skeleton> {
    crate::ensure_tolerance(tolerance)?;
    let n = polygon.len();
    if !(3..=MAX_SKELETON_EDGES).contains(&n) || speeds.len() != n {
        return Err(invalid());
    }
    let finite = polygon.iter().all(|p| p.x.is_finite() && p.y.is_finite());
    if !finite || speeds.iter().any(|s| !s.is_finite() || *s < 0.0) {
        return Err(invalid());
    }
    if speeds.iter().all(|s| *s == 0.0) {
        return Err(Error::new(codes::SKELETON_NO_SLOPE));
    }
    // Work about the middle of the bounding box, and counterclockwise.
    let (mut low, mut high) = (
        Vector2::repeat(f64::INFINITY),
        Vector2::repeat(f64::NEG_INFINITY),
    );
    for p in polygon {
        low = low.inf(&p.coords);
        high = high.sup(&p.coords);
    }
    let origin = (low + high) / 2.0;
    let mut points: Vec<Point2> = polygon.iter().map(|p| p - origin).collect();
    let mut ids: Vec<usize> = (0..n).collect();
    let mut rates: Vec<f64> = speeds.to_vec();
    let area = double_area(&points) / 2.0;
    if area.abs() <= tolerance.linear * tolerance.linear {
        return Err(invalid());
    }
    if area < 0.0 {
        // Reversed, edge `k` runs from old vertex n-1-k to n-2-k: the old edge n-2-k.
        points.reverse();
        ids = (0..n).map(|k| (2 * n - 2 - k) % n).collect();
        rates = ids
            .iter()
            .map(|&i| speeds.get(i).copied().unwrap_or(0.0))
            .collect();
    }
    check_simple(&points)?;
    let mut lines = vec![
        EdgeLine {
            normal: Vector2::zeros(),
            offset: 0.0,
            speed: 0.0,
        };
        n
    ];
    let mut front: Vec<Node> = Vec::with_capacity(n);
    for k in 0..n {
        let (Some(a), Some(b)) = (points.get(k), points.get((k + 1) % n)) else {
            return Err(invalid());
        };
        let d = b - a;
        let length = d.norm();
        if length <= tolerance.linear {
            return Err(invalid());
        }
        let normal = Vector2::new(-d.y, d.x) / length;
        let (Some(&id), Some(&speed)) = (ids.get(k), rates.get(k)) else {
            return Err(invalid());
        };
        if let Some(line) = lines.get_mut(id) {
            *line = EdgeLine {
                normal,
                offset: normal.dot(&a.coords),
                speed,
            };
        }
        front.push(Node {
            p: *a,
            edge: id,
            id: 0,
        });
    }
    // Merge collinear neighbors of one speed; refuse them with different speeds.
    let mut k = 0;
    while k < front.len() && front.len() > 3 {
        let m = front.len();
        let (Some(prev), Some(here)) = (front.get((k + m - 1) % m), front.get(k)) else {
            return Err(invalid());
        };
        let (Some(a), Some(b)) = (lines.get(prev.edge), lines.get(here.edge)) else {
            return Err(invalid());
        };
        if cross(a.normal, b.normal).abs() <= tolerance.angular && a.normal.dot(&b.normal) > 0.0 {
            if (a.speed - b.speed).abs() > tolerance.angular {
                return Err(located(codes::SKELETON_COLLINEAR_SPEEDS, here.p, origin));
            }
            front.remove(k);
            continue;
        }
        k += 1;
    }
    let total = double_area(&front.iter().map(|v| v.p).collect::<Vec<_>>()) / 2.0;
    let mut sim = Simulation {
        lines,
        tolerance: *tolerance,
        origin,
        z: 0.0,
        born: Vec::new(),
        cells: Vec::new(),
        open: HashMap::new(),
        arcs: Vec::new(),
    };
    for v in &mut front {
        v.id = usize::MAX;
    }
    let count = front.len();
    let mut polygon_front = front;
    for k in 0..count {
        sim.rebirth(&mut polygon_front, k)?;
    }
    let mut polygons = vec![polygon_front];
    let limit = 8 * n + 16;
    let mut steps = 0;
    while !polygons.is_empty() {
        steps += 1;
        if steps > limit {
            let at = polygons
                .first()
                .and_then(|p| p.first())
                .map_or(Point2::origin(), |v| v.p);
            return Err(degenerate(at, origin));
        }
        polygons = sim.cleanup(polygons)?;
        if polygons.is_empty() {
            break;
        }
        let dt = sim.next_event(&polygons)?;
        sim.advance(&mut polygons, dt)?;
    }
    sim.finish(total)
}

/// The state of the simulation.
struct Simulation {
    lines: Vec<EdgeLine>,
    tolerance: Tolerance,
    origin: Vector2,
    /// The height of the front now.
    z: f64,
    born: Vec<Born>,
    cells: Vec<SkeletonCell>,
    /// The cell that each pair of vertex identities is sweeping now.
    open: HashMap<(usize, usize), usize>,
    arcs: Vec<SkeletonArc>,
}

impl Simulation {
    fn line(&self, edge: usize) -> Result<EdgeLine> {
        self.lines
            .get(edge)
            .copied()
            .ok_or_else(|| degenerate(Point2::origin(), self.origin))
    }

    /// The velocity of a vertex between the edges `incoming` and `outgoing`.
    fn velocity(&self, incoming: usize, outgoing: usize, at: Point2) -> Result<Vector2> {
        let (a, b) = (self.line(incoming)?, self.line(outgoing)?);
        // A vertex between an edge that moves and one that does not slides along the still
        // edge's line; at a reflex corner that line runs into the polygon and the surface would
        // need a vertical face inside it.
        if (a.speed == 0.0) != (b.speed == 0.0)
            && cross(a.direction(), b.direction()) < -self.tolerance.angular
        {
            return Err(located(codes::SKELETON_UNCLOSED, at, self.origin));
        }
        let det = cross(a.normal, b.normal);
        if det.abs() <= self.tolerance.angular {
            if a.normal.dot(&b.normal) > 0.0 && (a.speed - b.speed).abs() <= self.tolerance.angular
            {
                return Ok(a.normal * a.speed);
            }
            return Err(located(codes::SKELETON_COLLINEAR_SPEEDS, at, self.origin));
        }
        // Solve [a.n; b.n] v = [a.s; b.s].
        let v = Vector2::new(
            (a.speed * b.normal.y - b.speed * a.normal.y) / det,
            (a.normal.x * b.speed - b.normal.x * a.speed) / det,
        );
        if v.x.is_finite() && v.y.is_finite() {
            Ok(v)
        } else {
            Err(degenerate(at, self.origin))
        }
    }

    /// Gives vertex `k` of `poly` a new identity, born here and now.
    fn rebirth(&mut self, poly: &mut [Node], k: usize) -> Result<()> {
        let m = poly.len();
        let incoming = poly
            .get((k + m - 1) % m)
            .map(|v| v.edge)
            .ok_or_else(|| degenerate(Point2::origin(), self.origin))?;
        let id = self.born.len();
        let Some(v) = poly.get_mut(k) else {
            return Err(degenerate(Point2::origin(), self.origin));
        };
        let (a, b) = (
            self.lines.get(incoming).copied(),
            self.lines.get(v.edge).copied(),
        );
        let reflex = match (a, b) {
            (Some(a), Some(b)) => cross(a.direction(), b.direction()) < 0.0,
            _ => false,
        };
        self.born.push(Born {
            at: v.p,
            z: self.z,
            reflex,
            edges: [incoming, v.edge],
        });
        v.id = id;
        Ok(())
    }

    /// Ends the life of a vertex: its arc from where it was made to where it is.
    fn retire(&mut self, v: &Node) {
        let Some(born) = self.born.get(v.id).copied() else {
            return;
        };
        let moved = (v.p - born.at).norm() > self.tolerance.linear;
        let sloped = born
            .edges
            .iter()
            .all(|&e| self.lines.get(e).is_some_and(|l| l.speed > 0.0));
        if moved && sloped && self.z > born.z {
            self.arcs.push(SkeletonArc {
                start: Point3::new(born.at.x, born.at.y, born.z),
                end: Point3::new(v.p.x, v.p.y, self.z),
                kind: if born.reflex {
                    ArcKind::Valley
                } else {
                    ArcKind::Hip
                },
                edges: born.edges,
            });
        }
    }

    /// A level ridge between `a` and `b` where the planes of `edges` meet.
    fn ridge(&mut self, a: Point2, b: Point2, edges: [usize; 2]) {
        let sloped = edges
            .iter()
            .all(|&e| self.lines.get(e).is_some_and(|l| l.speed > 0.0));
        if sloped && (a - b).norm() > self.tolerance.linear {
            self.arcs.push(SkeletonArc {
                start: Point3::new(a.x, a.y, self.z),
                end: Point3::new(b.x, b.y, self.z),
                kind: ArcKind::Ridge,
                edges,
            });
        }
    }

    /// Cleans up every polygon at the current height until nothing changes; returns the
    /// polygons that remain.
    fn cleanup(&mut self, polygons: Vec<Vec<Node>>) -> Result<Vec<Vec<Node>>> {
        let mut work = polygons;
        let mut done = Vec::new();
        let mut rounds = 0_usize;
        let budget = 16 * MAX_SKELETON_EDGES;
        while let Some(mut poly) = work.pop() {
            rounds += 1;
            if rounds > budget {
                let at = poly.first().map_or(Point2::origin(), |v| v.p);
                return Err(degenerate(at, self.origin));
            }
            if poly.len() < 3 {
                self.collapse(&poly);
                continue;
            }
            if self.merge_close(&mut poly)? || self.remove_spike(&mut poly)? {
                work.push(poly);
                continue;
            }
            if let Some((a, b)) = self.split(&poly)? {
                work.push(a);
                work.push(b);
                continue;
            }
            let points: Vec<Point2> = poly.iter().map(|v| v.p).collect();
            let perimeter: f64 = (0..points.len())
                .filter_map(|k| Some((points.get(k)?, points.get((k + 1) % points.len())?)))
                .map(|(a, b)| (b - a).norm())
                .sum();
            if (double_area(&points) / 2.0).abs() <= self.tolerance.linear * perimeter {
                self.collapse(&poly);
                continue;
            }
            done.push(poly);
        }
        Ok(done)
    }

    /// Ends a polygon that has folded flat: ridges along its edges between the planes that meet.
    fn collapse(&mut self, poly: &[Node]) {
        let m = poly.len();
        if m == 2 {
            if let (Some(a), Some(b)) = (poly.first(), poly.get(1)) {
                self.ridge(a.p, b.p, [a.edge, b.edge]);
            }
        } else if m > 2 {
            for k in 0..m {
                if let (Some(a), Some(b), Some(c)) = (
                    poly.get(k),
                    poly.get((k + 1) % m),
                    poly.get((k + m - 1) % m),
                ) {
                    self.ridge(a.p, b.p, [a.edge, c.edge]);
                }
            }
        }
        for v in poly {
            self.retire(v);
        }
    }

    /// Merges the first two neighbors that meet; true when it did.
    fn merge_close(&mut self, poly: &mut Vec<Node>) -> Result<bool> {
        let m = poly.len();
        for k in 0..m {
            let (Some(a), Some(b)) = (poly.get(k).copied(), poly.get((k + 1) % m).copied()) else {
                continue;
            };
            if (b.p - a.p).norm() > self.tolerance.linear {
                continue;
            }
            self.retire(&a);
            self.retire(&b);
            let next = (k + 1) % m;
            if let Some(v) = poly.get_mut(k) {
                v.edge = b.edge;
            }
            poly.remove(next);
            let at = if next < k { k - 1 } else { k };
            self.rebirth(poly, at)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Removes the tip of the first spike (two edges folded onto each other); true when it did.
    fn remove_spike(&mut self, poly: &mut Vec<Node>) -> Result<bool> {
        let m = poly.len();
        for k in 0..m {
            let (Some(a), Some(b), Some(c)) = (
                poly.get((k + m - 1) % m).copied(),
                poly.get(k).copied(),
                poly.get((k + 1) % m).copied(),
            ) else {
                continue;
            };
            let (ab, bc) = (b.p - a.p, c.p - b.p);
            let (la, lc) = (ab.norm(), bc.norm());
            if cross(ab, bc).abs() > self.tolerance.linear * (la + lc) || ab.dot(&bc) >= 0.0 {
                continue;
            }
            self.retire(&b);
            let prev = (k + m - 1) % m;
            if la <= lc {
                // `a` lies on `bc`: the edge from `b` continues from `a`.
                self.ridge(b.p, a.p, [a.edge, b.edge]);
                self.retire(&a);
                if let Some(v) = poly.get_mut(prev) {
                    v.edge = b.edge;
                }
                poly.remove(k);
                let at = if prev > k { prev - 1 } else { prev };
                self.rebirth(poly, at)?;
            } else {
                // `c` lies on `ab`: the edge into `b` continues to `c`.
                self.ridge(b.p, c.p, [a.edge, b.edge]);
                self.retire(&c);
                poly.remove(k);
                let at = if k < poly.len() { k } else { 0 };
                self.rebirth(poly, at)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Splits the polygon where a reflex vertex lies on an edge that is not its own.
    fn split(&mut self, poly: &[Node]) -> Result<Option<(Vec<Node>, Vec<Node>)>> {
        let m = poly.len();
        for k in 0..m {
            let (Some(a), Some(b), Some(c)) = (
                poly.get((k + m - 1) % m),
                poly.get(k),
                poly.get((k + 1) % m),
            ) else {
                continue;
            };
            if cross(b.p - a.p, c.p - b.p) >= 0.0 {
                continue;
            }
            for j in 0..m {
                // Not the vertex's own edges (j = k - 1, j = k).
                if j == k || (j + 1) % m == k {
                    continue;
                }
                let (Some(s), Some(e)) = (poly.get(j), poly.get((j + 1) % m)) else {
                    continue;
                };
                let d = e.p - s.p;
                let length2 = d.norm_squared();
                if length2 <= 0.0 {
                    continue;
                }
                let u = ((b.p - s.p).dot(&d) / length2).clamp(0.0, 1.0);
                let foot = s.p + d * u;
                if (b.p - foot).norm() > self.tolerance.linear {
                    continue;
                }
                let hit = *b;
                self.retire(&hit);
                // A: the hit vertex with the edge j, then j+1 .. k-1.
                let mut first = vec![Node {
                    p: hit.p,
                    edge: s.edge,
                    id: 0,
                }];
                let mut i = (j + 1) % m;
                while i != k {
                    first.extend(poly.get(i).copied());
                    i = (i + 1) % m;
                }
                // B: the hit vertex with its own outgoing edge, then k+1 .. j.
                let mut second = vec![Node {
                    p: hit.p,
                    edge: hit.edge,
                    id: 0,
                }];
                let mut i = (k + 1) % m;
                loop {
                    second.extend(poly.get(i).copied());
                    if i == j {
                        break;
                    }
                    i = (i + 1) % m;
                }
                self.rebirth(&mut first, 0)?;
                self.rebirth(&mut second, 0)?;
                return Ok(Some((first, second)));
            }
        }
        Ok(None)
    }

    /// The height step to the next event of any polygon.
    fn next_event(&self, polygons: &[Vec<Node>]) -> Result<f64> {
        let mut best = f64::INFINITY;
        for poly in polygons {
            let m = poly.len();
            let velocities = self.velocities(poly)?;
            for k in 0..m {
                let (Some(u), Some(w), Some(vu), Some(vw)) = (
                    poly.get(k),
                    poly.get((k + 1) % m),
                    velocities.get(k),
                    velocities.get((k + 1) % m),
                ) else {
                    continue;
                };
                let d = w.p - u.p;
                let length = d.norm();
                if length > 0.0 {
                    let rate = (vw - vu).dot(&(d / length));
                    if rate < 0.0 {
                        best = best.min(length / -rate);
                    }
                }
            }
            for k in 0..m {
                let (Some(a), Some(b), Some(c), Some(vb)) = (
                    poly.get((k + m - 1) % m),
                    poly.get(k),
                    poly.get((k + 1) % m),
                    velocities.get(k),
                ) else {
                    continue;
                };
                if cross(b.p - a.p, c.p - b.p) >= 0.0 {
                    continue;
                }
                for j in 0..m {
                    if j == k || (j + 1) % m == k {
                        continue;
                    }
                    let (Some(s), Some(e), Some(vs), Some(ve)) = (
                        poly.get(j),
                        poly.get((j + 1) % m),
                        velocities.get(j),
                        velocities.get((j + 1) % m),
                    ) else {
                        continue;
                    };
                    let line = self.line(s.edge)?;
                    let gap = line.normal.dot(&b.p.coords) - line.offset - line.speed * self.z;
                    let rate = line.normal.dot(vb) - line.speed;
                    if rate >= 0.0 || gap < -self.tolerance.linear {
                        continue;
                    }
                    let dt = gap.max(0.0) / -rate;
                    if dt >= best {
                        continue;
                    }
                    let (s1, e1, hit) = (s.p + vs * dt, e.p + ve * dt, b.p + vb * dt);
                    let d = e1 - s1;
                    let length2 = d.norm_squared();
                    if length2 <= 0.0 {
                        continue;
                    }
                    let u = (hit - s1).dot(&d) / length2;
                    let margin = self.tolerance.linear / length2.sqrt();
                    if (-margin..=1.0 + margin).contains(&u) {
                        best = dt;
                    }
                }
            }
        }
        if best.is_finite() {
            return Ok(best);
        }
        // Nothing moves toward anything: a part that no moving edge reaches.
        let at = polygons
            .first()
            .map(|p| {
                let sum = p.iter().fold(Vector2::zeros(), |s, v| s + v.p.coords);
                Point2::from(sum / p.len().max(1) as f64)
            })
            .unwrap_or_else(Point2::origin);
        Err(located(codes::SKELETON_UNCLOSED, at, self.origin))
    }

    fn velocities(&self, poly: &[Node]) -> Result<Vec<Vector2>> {
        let m = poly.len();
        (0..m)
            .map(|k| {
                let (Some(prev), Some(v)) = (poly.get((k + m - 1) % m), poly.get(k)) else {
                    return Err(degenerate(Point2::origin(), self.origin));
                };
                self.velocity(prev.edge, v.edge, v.p)
            })
            .collect()
    }

    /// Moves every vertex up by `dt` and extends the cells the edges sweep.
    fn advance(&mut self, polygons: &mut [Vec<Node>], dt: f64) -> Result<()> {
        let z0 = self.z;
        let z1 = z0 + dt;
        if !z1.is_finite() {
            return Err(degenerate(Point2::origin(), self.origin));
        }
        for poly in polygons.iter_mut() {
            let velocities = self.velocities(poly)?;
            let before: Vec<Node> = poly.clone();
            for (v, velocity) in poly.iter_mut().zip(&velocities) {
                v.p += velocity * dt;
            }
            if dt <= 0.0 {
                continue;
            }
            let m = poly.len();
            for k in 0..m {
                let (Some(u0), Some(w0), Some(u1), Some(w1)) = (
                    before.get(k),
                    before.get((k + 1) % m),
                    poly.get(k),
                    poly.get((k + 1) % m),
                ) else {
                    continue;
                };
                let top = [
                    Point3::new(w1.p.x, w1.p.y, z1),
                    Point3::new(u1.p.x, u1.p.y, z1),
                ];
                match self.open.get(&(u0.id, w0.id)) {
                    Some(&at) => {
                        if let Some(cell) = self.cells.get_mut(at) {
                            cell.points.truncate(2);
                            cell.points.extend(top);
                        }
                    }
                    None => {
                        self.open.insert((u0.id, w0.id), self.cells.len());
                        let mut points = vec![
                            Point3::new(u0.p.x, u0.p.y, z0),
                            Point3::new(w0.p.x, w0.p.y, z0),
                        ];
                        points.extend(top);
                        self.cells.push(SkeletonCell {
                            edge: u0.edge,
                            points,
                        });
                    }
                }
            }
        }
        self.z = z1;
        Ok(())
    }

    /// The result: cells without repeated corners, moved back to the input's coordinates, after
    /// the check that they cover the polygon.
    fn finish(self, total: f64) -> Result<Skeleton> {
        let tolerance = self.tolerance.linear;
        let mut cells = Vec::with_capacity(self.cells.len());
        let mut covered = 0.0;
        let mut boundary = 0.0;
        for mut cell in self.cells {
            cell.points.dedup_by(|a, b| (*a - *b).norm() <= tolerance);
            while cell.points.len() > 1
                && match (cell.points.first(), cell.points.last()) {
                    (Some(a), Some(b)) => (a - b).norm() <= tolerance,
                    _ => false,
                }
            {
                cell.points.pop();
            }
            if cell.points.len() < 3 {
                continue;
            }
            let plan: Vec<Point2> = cell.points.iter().map(|p| Point2::new(p.x, p.y)).collect();
            covered += double_area(&plan) / 2.0;
            let m = plan.len();
            boundary += (0..m)
                .filter_map(|k| Some((plan.get(k)?, plan.get((k + 1) % m)?)))
                .map(|(a, b)| (b - a).norm())
                .sum::<f64>();
            for p in &mut cell.points {
                p.x += self.origin.x;
                p.y += self.origin.y;
            }
            cells.push(cell);
        }
        if (covered - total).abs() > tolerance * boundary.max(1.0) {
            return Err(degenerate(Point2::origin(), self.origin));
        }
        let mut arcs = self.arcs;
        for arc in &mut arcs {
            for p in [&mut arc.start, &mut arc.end] {
                p.x += self.origin.x;
                p.y += self.origin.y;
            }
        }
        Ok(Skeleton {
            cells,
            arcs,
            height: self.z,
            lines: self.lines,
            origin: self.origin,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(list: &[(f64, f64)]) -> Vec<Point2> {
        list.iter().map(|&(x, y)| Point2::new(x, y)).collect()
    }

    fn check(skeleton: &Skeleton, polygon: &[Point2]) {
        let t = Tolerance::DEFAULT;
        let area: f64 = double_area(polygon).abs() / 2.0;
        let covered: f64 = skeleton
            .cells
            .iter()
            .map(|c| {
                double_area(
                    &c.points
                        .iter()
                        .map(|p| Point2::new(p.x, p.y))
                        .collect::<Vec<_>>(),
                ) / 2.0
            })
            .sum();
        assert!(
            (area - covered).abs() < 1e-6 * area.max(1.0),
            "{area} {covered}"
        );
        for cell in &skeleton.cells {
            for p in &cell.points {
                if let Some(z) = skeleton.edge_height(cell.edge, &Point2::new(p.x, p.y)) {
                    assert!((z - p.z).abs() < 1e-6, "off plane {z} {}", p.z);
                } else {
                    // A vertical cell lies on its edge's line.
                    let line = skeleton.lines[cell.edge];
                    let d = line
                        .normal
                        .dot(&(Point2::new(p.x, p.y).coords - skeleton.origin))
                        - line.offset;
                    assert!(d.abs() < 1e-6, "vertical cell off its line {d}");
                }
            }
        }
        // Heights agree at shared corners of sloped cells.
        for a in &skeleton.cells {
            for b in &skeleton.cells {
                if skeleton.edge_height(a.edge, &Point2::origin()).is_none()
                    || skeleton.edge_height(b.edge, &Point2::origin()).is_none()
                {
                    continue;
                }
                for p in &a.points {
                    for q in &b.points {
                        if (p.x - q.x).hypot(p.y - q.y) < 1e-9 {
                            assert!((p.z - q.z).abs() < 1e-6);
                        }
                    }
                }
            }
        }
        let _ = t;
    }

    #[test]
    fn hip_roof_on_a_rectangle() {
        let p = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]);
        let s = weighted_skeleton(&p, &[1.0; 4], &Tolerance::DEFAULT).unwrap();
        check(&s, &p);
        assert!((s.height - 3.0).abs() < 1e-9);
        let ridges: Vec<_> = s.arcs.iter().filter(|a| a.kind == ArcKind::Ridge).collect();
        assert_eq!(ridges.len(), 1);
        let r = ridges[0];
        assert!(((r.end - r.start).norm() - 4.0).abs() < 1e-9);
        assert_eq!(s.arcs.iter().filter(|a| a.kind == ArcKind::Hip).count(), 4);
        assert!(
            (s.height_at(&Point2::new(5.0, 1.0), &Tolerance::DEFAULT)
                .unwrap()
                - 1.0)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn gable_roof_has_vertical_gables() {
        let p = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]);
        let s = weighted_skeleton(&p, &[1.0, 0.0, 1.0, 0.0], &Tolerance::DEFAULT).unwrap();
        check(&s, &p);
        let ridge: Vec<_> = s.arcs.iter().filter(|a| a.kind == ArcKind::Ridge).collect();
        assert_eq!(ridge.len(), 1);
        assert!(((ridge[0].end - ridge[0].start).norm() - 10.0).abs() < 1e-9);
        let gables: Vec<_> = s.cells.iter().filter(|c| c.edge == 1).collect();
        assert!(!gables.is_empty());
        for c in gables {
            assert!(c.points.iter().all(|p| (p.x - 10.0).abs() < 1e-9));
        }
        assert_eq!(s.arcs.iter().filter(|a| a.kind == ArcKind::Hip).count(), 0);
    }

    #[test]
    fn l_shape_has_a_valley() {
        let p = pts(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 4.0),
            (4.0, 4.0),
            (4.0, 10.0),
            (0.0, 10.0),
        ]);
        let s = weighted_skeleton(&p, &[1.0; 6], &Tolerance::DEFAULT).unwrap();
        check(&s, &p);
        let valleys: Vec<_> = s
            .arcs
            .iter()
            .filter(|a| a.kind == ArcKind::Valley)
            .collect();
        assert_eq!(valleys.len(), 1);
        assert!((valleys[0].start - Point3::new(4.0, 4.0, 0.0)).norm() < 1e-9);
        // Clockwise input gives the same surface.
        let mut r = p.clone();
        r.reverse();
        let s2 = weighted_skeleton(&r, &[1.0; 6], &Tolerance::DEFAULT).unwrap();
        check(&s2, &r);
        assert!((s.height - s2.height).abs() < 1e-9);
    }

    #[test]
    fn shed_roof_and_refusals() {
        let p = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]);
        let s = weighted_skeleton(&p, &[1.0, 0.0, 0.0, 0.0], &Tolerance::DEFAULT).unwrap();
        check(&s, &p);
        assert!((s.height - 6.0).abs() < 1e-9);
        let t = Tolerance::DEFAULT;
        let code = |r: Result<Skeleton>| r.unwrap_err().code();
        assert_eq!(
            code(weighted_skeleton(&p, &[0.0; 4], &t)),
            "geom.skeleton.no_slope"
        );
        let bow = pts(&[(0.0, 0.0), (10.0, 6.0), (10.0, 0.0), (0.0, 6.0)]);
        assert_eq!(
            code(weighted_skeleton(&bow, &[1.0; 4], &t)),
            "geom.skeleton.polygon_invalid"
        );
        let line = pts(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]);
        assert_eq!(
            code(weighted_skeleton(&line, &[1.0, 0.0, 1.0, 1.0, 1.0], &t)),
            "geom.skeleton.collinear_speeds"
        );
        assert!(weighted_skeleton(&line, &[1.0; 5], &t).is_ok());
        // An L with only one moving edge leaves a corner nothing reaches.
        let l = pts(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 4.0),
            (4.0, 4.0),
            (4.0, 10.0),
            (0.0, 10.0),
        ]);
        assert_eq!(
            code(weighted_skeleton(&l, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0], &t)),
            "geom.skeleton.unclosed"
        );
    }

    #[test]
    fn unequal_slopes() {
        let p = pts(&[(0.0, 0.0), (10.0, 0.0), (10.0, 6.0), (0.0, 6.0)]);
        let s = weighted_skeleton(&p, &[1.0, 2.0, 0.5, 1.5], &Tolerance::DEFAULT).unwrap();
        check(&s, &p);
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn star(convex: bool) -> impl Strategy<Value = (Vec<Point2>, Vec<f64>)> {
            (5_usize..14)
                .prop_flat_map(move |n| {
                    (
                        proptest::collection::vec(0.1..1.0_f64, n),
                        proptest::collection::vec(
                            if convex { 5.0..5.0001 } else { 2.0..8.0_f64 },
                            n,
                        ),
                        proptest::collection::vec(
                            prop_oneof![3 => 0.3..3.0_f64, 1 => Just(0.0)],
                            n,
                        ),
                    )
                })
                .prop_map(|(gaps, radii, speeds)| {
                    let total: f64 = gaps.iter().sum();
                    let mut angle = 0.0;
                    let points = gaps
                        .iter()
                        .zip(&radii)
                        .map(|(g, r)| {
                            angle += g / total * std::f64::consts::TAU;
                            Point2::new(r * angle.cos(), r * angle.sin())
                        })
                        .collect();
                    (points, speeds)
                })
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(256))]

            #[test]
            fn convex_polygons((points, speeds) in star(true)) {
                match weighted_skeleton(&points, &speeds, &Tolerance::DEFAULT) {
                    Ok(s) => check(&s, &points),
                    Err(e) => prop_assert!(
                        ["geom.skeleton.no_slope", "geom.skeleton.polygon_invalid"].contains(&e.code()),
                        "{e:?}"
                    ),
                }
            }

            #[test]
            fn star_polygons((points, speeds) in star(false)) {
                match weighted_skeleton(&points, &speeds, &Tolerance::DEFAULT) {
                    Ok(s) => check(&s, &points),
                    Err(e) => prop_assert!(
                        ["geom.skeleton.no_slope", "geom.skeleton.polygon_invalid", "geom.skeleton.unclosed"]
                            .contains(&e.code()),
                        "{e:?}"
                    ),
                }
            }
        }
    }
}
