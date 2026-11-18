//! Planar arrangements of exact curves: the engine shared by region construction, boundary
//! tracing, region booleans and region offsets (CORE-GEOM-016 to CORE-GEOM-019).
//!
//! The curves are split into smooth elements (segments, circular arcs, elliptical arcs; a full
//! circle or ellipse is a full arc), every pair whose boxes meet is intersected with
//! [`crate::intersect`] (candidate pairs from an R*-tree of the boxes), each element is cut at
//! its intersection parameters, and the cut points are snapped into vertices within twice the
//! linear tolerance. Sub-edges whose ends snap together are dropped when they are at most four
//! times the linear tolerance long and split in two otherwise, and sub-edges with the same ends
//! whose middles lie within twice the linear tolerance of each other are merged into one edge,
//! keeping the geometry of the first in input order and adding up their winding weights. The
//! edges around each vertex are ordered by the angle of their tangents and, where tangents agree
//! within the angular tolerance, by their signed curvature; walking each half-edge to the next
//! one clockwise around its end gives the boundary cycles of the faces, each with its face on
//! the left. In each connected component, the cycle of least signed area is the component's
//! outer boundary and every other cycle bounds one face.

use std::collections::{HashMap, VecDeque};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, Result, Tolerance};

use crate::bbox::Aabb2;
use crate::closest::CurveProject;
use crate::curve::Curve2;
use crate::error::codes;
use crate::eval::{CurveEval, Shape, Shaped, cross};
use crate::index::SpatialIndex2;
use crate::intersect::{Intersection, intersect_with_limits};
use crate::measure::piece_integrals;
use crate::polyline::PolylineSegment;
use crate::primitives::Segment2;
use crate::region::{Containment, Loop2, Region, RegionPart};

/// Cut points closer than this many linear tolerances are one vertex.
const SNAP_TOLERANCES: f64 = 2.0;
/// Sub-edges whose ends snap together are dropped up to this many linear tolerances long.
const COLLAPSE_TOLERANCES: f64 = 4.0;
/// The smallest angle within which two computed tangent directions count as equal: the
/// rounding of tangents computed from different curves.
const ANGLE_EPSILON: f64 = 1e-12;
/// Tangent angles are measured from this direction (1 radian), so that axis-aligned tangents,
/// which are common, do not sit on the cut of the angle range.
const REFERENCE_ANGLE: f64 = 1.0;

/// A detail value for an index.
pub(crate) fn detail(index: usize) -> i64 {
    i64::try_from(index).unwrap_or(i64::MAX)
}

/// A list of indices as a detail value, such as `"0,3"`.
pub(crate) fn index_list(indices: &[usize]) -> String {
    indices
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn too_large(what: &'static str) -> Error {
    Error::new(codes::ARRANGEMENT_TOO_LARGE).with_detail("count", what)
}

/// The largest count `limits` allows.
pub(crate) fn max_count(limits: &Limits) -> usize {
    usize::try_from(limits.array_length).unwrap_or(usize::MAX)
}

/// Fails unless the linear tolerance is finite and positive and the tolerance is valid.
pub(crate) fn check_tolerance(tolerance: &Tolerance) -> Result<()> {
    crate::ensure_tolerance(tolerance)?;
    crate::eval::ensure_tolerance("linear", tolerance.linear)
}

/// The smooth elements of a bounded curve, in order: segments, circular arcs and elliptical
/// arcs; a circle or ellipse becomes its full arc and a polyline its segments.
///
/// # Errors
///
/// `geom.curve.unbounded` for a ray or line and `geom.curve.kind_unsupported` for a NURBS
/// curve, each with the detail `curve` set to `source`.
pub(crate) fn explode(curve: &Curve2, source: usize) -> Result<Vec<Curve2>> {
    Ok(match curve {
        Curve2::Segment(_) | Curve2::Arc(_) | Curve2::EllipticalArc(_) => vec![curve.clone()],
        Curve2::Circle(c) => vec![Curve2::Arc(c.to_arc())],
        Curve2::Ellipse(e) => vec![Curve2::EllipticalArc(e.to_arc())],
        Curve2::Polyline(p) => p
            .segments()
            .map(|s| match s {
                PolylineSegment::Line(s) => Curve2::Segment(s),
                PolylineSegment::Arc(a) => Curve2::Arc(a),
            })
            .collect(),
        Curve2::Ray(_) | Curve2::Line(_) => {
            return Err(Error::new(codes::CURVE_UNBOUNDED).with_detail("curve", detail(source)));
        }
        Curve2::Nurbs(_) => {
            return Err(Error::new(codes::CURVE_KIND_UNSUPPORTED)
                .with_detail("operation", "region")
                .with_detail("curve", detail(source)));
        }
    })
}

/// Calls `visit(i, j, intersections)` for every pair `i < j` of `curves` that meet, finding
/// candidate pairs with an R*-tree of their boxes.
///
/// # Errors
///
/// `geom.arrangement.too_large` when there are more candidate pairs or intersections than
/// `limits.array_length`; as [`intersect_with_limits`]; whatever `visit` returns.
pub(crate) fn pairwise(
    curves: &[Curve2],
    tolerance: &Tolerance,
    limits: &Limits,
    mut visit: impl FnMut(usize, usize, Vec<Intersection>) -> Result<()>,
) -> Result<()> {
    let limit = max_count(limits);
    let boxes: Vec<Aabb2> = curves
        .iter()
        .enumerate()
        .map(|(i, c)| {
            c.bounding_box()
                .map(|b| b.expanded(tolerance.linear))
                .ok_or_else(|| Error::new(codes::CURVE_UNBOUNDED).with_detail("curve", detail(i)))
        })
        .collect::<Result<_>>()?;
    let index = SpatialIndex2::from_entries(boxes.iter().copied().enumerate())?;
    let (mut pairs, mut found) = (0_usize, 0_usize);
    for (i, boxed) in boxes.iter().enumerate() {
        let near = index.crossing(boxed)?;
        for j in near.into_iter().filter(|j| *j > i) {
            pairs += 1;
            if pairs > limit {
                return Err(too_large("pairs"));
            }
            let (Some(a), Some(b)) = (curves.get(i), curves.get(j)) else {
                continue;
            };
            let list = intersect_with_limits(a, b, tolerance, limits)?;
            found += list.len();
            if found > limit {
                return Err(too_large("intersections"));
            }
            if !list.is_empty() {
                visit(i, j, list)?;
            }
        }
    }
    Ok(())
}

/// One input element of an arrangement.
pub(crate) struct Input {
    /// A segment, circular arc or elliptical arc (from [`explode`]).
    pub(crate) curve: Curve2,
    /// The change of winding number from the right of the curve to its left, per operand.
    pub(crate) weight: [i32; 2],
}

/// An edge between two distinct vertices.
struct Edge {
    curve: Curve2,
    from: usize,
    to: usize,
    weight: [i32; 2],
}

/// The boundary cycles of an arrangement's faces.
pub(crate) struct Faces {
    /// The half-edges of each cycle in order.
    pub(crate) cycles: Vec<Vec<usize>>,
    /// The cycle of each half-edge.
    pub(crate) cycle_of: Vec<usize>,
    /// The signed area of each cycle.
    pub(crate) area: Vec<f64>,
    /// The component of each cycle.
    pub(crate) component: Vec<usize>,
    /// The outer cycle of each component.
    pub(crate) outer: Vec<usize>,
}

impl Faces {
    /// Whether the cycle bounds a face (it is not its component's outer boundary).
    pub(crate) fn is_bounded(&self, cycle: usize) -> bool {
        self.component
            .get(cycle)
            .and_then(|c| self.outer.get(*c))
            .is_some_and(|outer| *outer != cycle)
    }
}

/// A planar arrangement; see the [module documentation](self).
pub(crate) struct Arrangement {
    vertices: Vec<Point2>,
    edges: Vec<Edge>,
    /// The outgoing half-edges of each vertex, counterclockwise.
    outgoing: Vec<Vec<usize>>,
    /// The position of each half-edge in the list of its origin.
    slot: Vec<usize>,
    /// The first vertex of each input element, when any of it remains.
    element_vertex: Vec<Option<usize>>,
    /// The component of each vertex.
    component_of: Vec<usize>,
    components: usize,
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn find(&mut self, mut x: usize) -> usize {
        let mut root = x;
        while let Some(&parent) = self.0.get(root) {
            if parent == root {
                break;
            }
            root = parent;
        }
        while let Some(parent) = self.0.get_mut(x) {
            if *parent == root {
                break;
            }
            let next = *parent;
            *parent = root;
            x = next;
        }
        root
    }

    /// Joins the sets; the smaller root stays the root.
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        let (lo, hi) = (ra.min(rb), ra.max(rb));
        if let Some(slot) = self.0.get_mut(hi) {
            *slot = lo;
        }
    }
}

/// Snaps points closer than `radius` into clusters; returns the cluster root of each point,
/// which is its smallest member.
fn snap(points: &[Point2], radius: f64) -> Result<Vec<usize>> {
    let mut sets = UnionFind((0..points.len()).collect());
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    // Cell indices beyond this are not exactly representable.
    let bound = 2.0_f64.powi(52);
    for (i, p) in points.iter().enumerate() {
        let (gx, gy) = ((p.x / radius).floor(), (p.y / radius).floor());
        if !(gx.abs() < bound && gy.abs() < bound) {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                .with_detail("coordinate", p.x.abs().max(p.y.abs())));
        }
        // Exact: both are integers below 2^52.
        #[allow(clippy::cast_possible_truncation)]
        let cell = (gx as i64, gy as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(members) = grid.get(&(cell.0 + dx, cell.1 + dy)) else {
                    continue;
                };
                for &j in members {
                    if let Some(q) = points.get(j)
                        && crate::length(&(p - q)) <= radius
                    {
                        sets.union(i, j);
                    }
                }
            }
        }
        grid.entry(cell).or_default().push(i);
    }
    Ok((0..points.len()).map(|i| sets.find(i)).collect())
}

/// `curve` (a piece of an element) rebuilt to run exactly from `p` to `q`: a segment between
/// them, a circular arc as the arc with the same sweep on that chord; an elliptical arc keeps its
/// own end points.
fn rebuilt(curve: Curve2, p: Point2, q: Point2) -> Result<Curve2> {
    Ok(match curve {
        Curve2::Segment(_) => Curve2::Segment(Segment2::new(p, q)?),
        Curve2::Arc(arc) => match crate::encoding::chord_arc(p, q, (0.25 * arc.sweep()).tan()) {
            Ok(Some(chord)) => Curve2::Arc(chord),
            _ => Curve2::Arc(arc),
        },
        other => other,
    })
}

/// The middle parameter of a curve.
fn middle(curve: &Curve2) -> f64 {
    let (lo, hi) = curve.parameter_range();
    0.5 * (lo + hi)
}

/// The sorted parameters at which each element is cut: its ends, its intersections with the
/// others, and thirds of a closed element (so that no edge starts and ends at one vertex).
fn cut_parameters(
    curves: &[Curve2],
    radius: f64,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Vec<f64>>> {
    let mut params: Vec<Vec<f64>> = curves
        .iter()
        .map(|c| {
            let (lo, hi) = c.parameter_range();
            let closed = match (c.start_point(), c.end_point()) {
                (Some(s), Some(e)) => crate::length(&(e - s)) <= radius,
                _ => false,
            };
            // A closed element is cut in three, so that no edge starts and ends at one
            // vertex.
            if closed {
                vec![lo, lo + (hi - lo) / 3.0, lo + 2.0 * (hi - lo) / 3.0, hi]
            } else {
                vec![lo, hi]
            }
        })
        .collect();
    pairwise(curves, tolerance, limits, |i, j, list| {
        for x in list {
            let (on_a, on_b) = match x {
                Intersection::Point { a, b, .. } => (vec![a], vec![b]),
                Intersection::Overlap { a, b, .. } => (a.to_vec(), b.to_vec()),
            };
            if let Some(p) = params.get_mut(i) {
                p.extend(on_a);
            }
            if let Some(p) = params.get_mut(j) {
                p.extend(on_b);
            }
        }
        Ok(())
    })?;
    for p in &mut params {
        p.sort_by(f64::total_cmp);
        p.dedup();
    }
    Ok(params)
}

/// The cut points snapped into vertices: the vertex positions, the vertex of each cut point,
/// and for each element its cuts as (parameter, cut point).
#[allow(clippy::type_complexity)]
fn cut_vertices(
    curves: &[Curve2],
    params: Vec<Vec<f64>>,
    radius: f64,
    limit: usize,
) -> Result<(Vec<Point2>, Vec<usize>, Vec<Vec<(f64, usize)>>)> {
    // The cut points, end points of elements first so that they represent their clusters.
    let mut points = Vec::new();
    let mut cut_ids: Vec<Vec<(f64, usize)>> = Vec::with_capacity(curves.len());
    for (c, p) in curves.iter().zip(&params) {
        let ends = [p.first().copied(), p.last().copied()];
        let mut ids = Vec::with_capacity(p.len());
        for t in ends.into_iter().flatten() {
            ids.push((t, points.len()));
            points.push(c.point_at(t));
        }
        cut_ids.push(ids);
    }
    for ((c, p), ids) in curves.iter().zip(&params).zip(cut_ids.iter_mut()) {
        let last = ids.pop();
        for t in p.iter().skip(1).take(p.len().saturating_sub(2)) {
            ids.push((*t, points.len()));
            points.push(c.point_at(*t));
        }
        ids.extend(last);
        if points.len() > limit {
            return Err(too_large("vertices"));
        }
    }
    let roots = snap(&points, radius)?;
    // Compact vertex numbers in order of the roots.
    let mut number: HashMap<usize, usize> = HashMap::new();
    let mut vertices = Vec::new();
    let mut vertex_of = Vec::with_capacity(points.len());
    for root in &roots {
        let id = *number.entry(*root).or_insert_with(|| {
            vertices.push(points.get(*root).copied().unwrap_or_else(Point2::origin));
            vertices.len() - 1
        });
        vertex_of.push(id);
    }
    Ok((vertices, vertex_of, cut_ids))
}

/// Adds `edge`, or merges it into an edge with the same ends whose curve passes within `radius`
/// of its middle, adding its weights.
fn merge(
    edges: &mut Vec<Edge>,
    by_ends: &mut HashMap<(usize, usize), Vec<usize>>,
    edge: Edge,
    radius: f64,
    tolerance: &Tolerance,
) -> Result<()> {
    let key = (edge.from.min(edge.to), edge.from.max(edge.to));
    let mid = edge.curve.point_at(middle(&edge.curve));
    let list = by_ends.entry(key).or_default();
    for &k in list.iter() {
        let Some(other) = edges.get_mut(k) else {
            continue;
        };
        if other.curve.closest_point(mid, tolerance)?.distance <= radius {
            let sign = if other.from == edge.from { 1 } else { -1 };
            for (w, add) in other.weight.iter_mut().zip(edge.weight) {
                *w += sign * add;
            }
            return Ok(());
        }
    }
    list.push(edges.len());
    edges.push(edge);
    Ok(())
}

impl Arrangement {
    /// Builds the arrangement of `inputs`.
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for an invalid tolerance or a zero linear tolerance;
    /// `geom.arrangement.too_large` when the elements, candidate pairs, intersections or edges
    /// exceed `limits.array_length`; `geom.precision.insufficient` when coordinates are too
    /// large for the linear tolerance; as [`crate::intersect`].
    pub(crate) fn build(inputs: &[Input], tolerance: &Tolerance, limits: &Limits) -> Result<Self> {
        check_tolerance(tolerance)?;
        let limit = max_count(limits);
        if inputs.len() > limit {
            return Err(too_large("elements"));
        }
        let radius = SNAP_TOLERANCES * tolerance.linear;
        let curves: Vec<Curve2> = inputs.iter().map(|i| i.curve.clone()).collect();
        let params = cut_parameters(&curves, radius, tolerance, limits)?;
        let (mut vertices, vertex_of, cut_ids) = cut_vertices(&curves, params, radius, limit)?;
        let collapse = COLLAPSE_TOLERANCES * tolerance.linear;
        let mut edges: Vec<Edge> = Vec::new();
        let mut by_ends: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        let mut element_vertex = Vec::with_capacity(curves.len());
        for ((c, ids), input) in curves.iter().zip(&cut_ids).zip(inputs) {
            element_vertex.push(ids.first().and_then(|(_, id)| vertex_of.get(*id).copied()));
            for pair in ids.windows(2) {
                let [(t0, i0), (t1, i1)] = pair else { continue };
                let (Some(&u), Some(&v)) = (vertex_of.get(*i0), vertex_of.get(*i1)) else {
                    continue;
                };
                if t1 <= t0 {
                    continue;
                }
                let piece = match crate::edit::subcurve(c, *t0, *t1) {
                    Ok(piece) => piece,
                    // Cuts so close that the piece has no length are one vertex.
                    Err(_) if u == v => continue,
                    Err(error) => return Err(error),
                };
                let mut parts = Vec::new();
                if u == v {
                    if piece.length(tolerance)? <= collapse {
                        continue;
                    }
                    // A long piece whose ends snapped together: split it at its middle.
                    let w = vertices.len();
                    vertices.push(piece.point_at(middle(&piece)));
                    let tm = 0.5 * (t0 + t1);
                    parts.push((crate::edit::subcurve(c, *t0, tm)?, u, w));
                    parts.push((crate::edit::subcurve(c, tm, *t1)?, w, v));
                } else {
                    parts.push((piece, u, v));
                }
                for (piece, from, to) in parts {
                    let (Some(&p), Some(&q)) = (vertices.get(from), vertices.get(to)) else {
                        continue;
                    };
                    let edge = Edge {
                        curve: rebuilt(piece, p, q)?,
                        from,
                        to,
                        weight: input.weight,
                    };
                    merge(&mut edges, &mut by_ends, edge, radius, tolerance)?;
                    if edges.len() > limit {
                        return Err(too_large("edges"));
                    }
                }
            }
        }
        let mut arrangement = Self {
            vertices,
            edges,
            outgoing: Vec::new(),
            slot: Vec::new(),
            element_vertex,
            component_of: Vec::new(),
            components: 0,
        };
        arrangement.order(tolerance);
        arrangement.connect();
        Ok(arrangement)
    }

    /// Orders the outgoing half-edges of every vertex counterclockwise.
    fn order(&mut self, tolerance: &Tolerance) {
        let reference = Vector2::new(REFERENCE_ANGLE.cos(), REFERENCE_ANGLE.sin());
        let epsilon = tolerance.angular.max(ANGLE_EPSILON);
        let mut lists: Vec<Vec<(f64, f64, usize)>> = vec![Vec::new(); self.vertices.len()];
        for h in 0..2 * self.edges.len() {
            let Some(edge) = self.edges.get(h / 2) else {
                continue;
            };
            let (lo, hi) = edge.curve.parameter_range();
            let (origin, tangent, curvature) = if h.is_multiple_of(2) {
                (edge.from, edge.curve.tangent(lo), edge.curve.curvature(lo))
            } else {
                (edge.to, -edge.curve.tangent(hi), -edge.curve.curvature(hi))
            };
            let angle = cross(&reference, &tangent).atan2(reference.dot(&tangent));
            if let Some(list) = lists.get_mut(origin) {
                list.push((angle, curvature, h));
            }
        }
        self.slot = vec![0; 2 * self.edges.len()];
        self.outgoing = Vec::with_capacity(lists.len());
        for mut list in lists {
            list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
            // Runs of equal tangents are ordered by curvature: the curve turning more to the
            // left lies counterclockwise of the others.
            let mut start = 0;
            while start < list.len() {
                let mut end = start + 1;
                while end < list.len()
                    && list
                        .get(end)
                        .zip(list.get(end - 1))
                        .is_some_and(|(b, a)| b.0 - a.0 <= epsilon)
                {
                    end += 1;
                }
                if let Some(run) = list.get_mut(start..end) {
                    run.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
                }
                start = end;
            }
            for (k, (_, _, h)) in list.iter().enumerate() {
                if let Some(s) = self.slot.get_mut(*h) {
                    *s = k;
                }
            }
            self.outgoing
                .push(list.into_iter().map(|(_, _, h)| h).collect());
        }
    }

    /// Finds the connected components.
    fn connect(&mut self) {
        let mut sets = UnionFind((0..self.vertices.len()).collect());
        for e in &self.edges {
            sets.union(e.from, e.to);
        }
        let mut number: HashMap<usize, usize> = HashMap::new();
        self.component_of = (0..self.vertices.len())
            .map(|v| {
                let root = sets.find(v);
                let next = number.len();
                *number.entry(root).or_insert(next)
            })
            .collect();
        self.components = number.len();
    }

    fn origin(&self, h: usize) -> usize {
        self.edges
            .get(h / 2)
            .map_or(0, |e| if h.is_multiple_of(2) { e.from } else { e.to })
    }

    /// The change of winding number from the right of half-edge `h` to its left.
    fn delta(&self, h: usize) -> [i32; 2] {
        let w = self.edges.get(h / 2).map_or([0, 0], |e| e.weight);
        if h.is_multiple_of(2) {
            w
        } else {
            [-w[0], -w[1]]
        }
    }

    /// The curve of half-edge `h` in its direction.
    fn curve(&self, h: usize) -> Option<Curve2> {
        let curve = &self.edges.get(h / 2)?.curve;
        if h.is_multiple_of(2) {
            Some(curve.clone())
        } else {
            curve.reversed()
        }
    }

    /// The vertex positions at the ends of half-edge `h`.
    fn ends(&self, h: usize) -> Option<(Point2, Point2)> {
        let e = self.edges.get(h / 2)?;
        let (a, b) = (*self.vertices.get(e.from)?, *self.vertices.get(e.to)?);
        Some(if h.is_multiple_of(2) { (a, b) } else { (b, a) })
    }

    /// The half-edge that follows `h` around the face on its left: the first active half-edge
    /// clockwise from the reverse of `h` around the end of `h`.
    fn next(&self, h: usize, active: &[bool]) -> Option<usize> {
        let twin = h ^ 1;
        let list = self.outgoing.get(self.origin(twin))?;
        let position = *self.slot.get(twin)?;
        let count = list.len();
        (1..=count)
            .filter_map(|k| list.get((position + count - k) % count))
            .copied()
            .find(|c| active.get(*c).copied().unwrap_or(false))
    }

    /// The cycles of the active half-edges.
    ///
    /// # Errors
    ///
    /// `geom.precision.insufficient` when the active half-edges do not form closed cycles.
    pub(crate) fn walk(&self, active: &[bool]) -> Result<Vec<Vec<usize>>> {
        let mut seen = vec![false; active.len()];
        let mut cycles = Vec::new();
        for start in 0..active.len() {
            if !active.get(start).copied().unwrap_or(false)
                || seen.get(start).copied().unwrap_or(true)
            {
                continue;
            }
            let mut cycle = Vec::new();
            let mut h = start;
            loop {
                if let Some(s) = seen.get_mut(h) {
                    *s = true;
                }
                cycle.push(h);
                match self.next(h, active) {
                    Some(n) if n == start => break,
                    Some(n) if !seen.get(n).copied().unwrap_or(true) => h = n,
                    _ => {
                        return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                            .with_detail("operation", "arrangement"));
                    }
                }
            }
            cycles.push(cycle);
        }
        Ok(cycles)
    }

    /// The signed area enclosed by a cycle.
    pub(crate) fn cycle_area(&self, cycle: &[usize]) -> f64 {
        let Some(origin) = cycle.first().and_then(|h| self.ends(*h)).map(|(a, _)| a) else {
            return 0.0;
        };
        cycle
            .iter()
            .filter_map(|h| {
                let (a, b) = self.ends(*h)?;
                match self.curve(*h)?.shape() {
                    Shape::Piece(piece) => Some(piece_integrals(&piece, a, b, origin).area),
                    _ => None,
                }
            })
            .sum()
    }

    /// The bounding box of a cycle.
    pub(crate) fn cycle_box(&self, cycle: &[usize]) -> Aabb2 {
        cycle
            .iter()
            .filter_map(|h| self.edges.get(h / 2)?.curve.bounding_box())
            .fold(Aabb2::EMPTY, |a, b| a.union(&b))
    }

    /// A cycle as a loop.
    ///
    /// # Errors
    ///
    /// As [`Loop2::new`].
    pub(crate) fn cycle_loop(&self, cycle: &[usize], tolerance: &Tolerance) -> Result<Loop2> {
        let mut curves: Vec<Curve2> = Vec::with_capacity(cycle.len());
        let mut previous: Option<Point2> = None;
        for curve in cycle.iter().filter_map(|h| self.curve(*h)) {
            if let (Some(end), Some(start)) = (previous, curve.start_point())
                && end != start
            {
                curves.push(Curve2::Segment(Segment2::new(end, start)?));
            }
            previous = curve.end_point();
            curves.push(curve);
        }
        if let (Some(end), Some(start)) = (previous, curves.first().and_then(Curve2::start_point))
            && end != start
        {
            curves.push(Curve2::Segment(Segment2::new(end, start)?));
        }
        Loop2::new(curves, tolerance)
    }

    /// The boundary cycles of all faces.
    ///
    /// # Errors
    ///
    /// As [`Self::walk`].
    pub(crate) fn faces(&self) -> Result<Faces> {
        let active = vec![true; 2 * self.edges.len()];
        let cycles = self.walk(&active)?;
        let mut cycle_of = vec![0; active.len()];
        for (c, cycle) in cycles.iter().enumerate() {
            for h in cycle {
                if let Some(slot) = cycle_of.get_mut(*h) {
                    *slot = c;
                }
            }
        }
        let area: Vec<f64> = cycles.iter().map(|c| self.cycle_area(c)).collect();
        let component: Vec<usize> = cycles
            .iter()
            .map(|c| {
                c.first()
                    .and_then(|h| self.component_of.get(self.origin(*h)))
                    .copied()
                    .unwrap_or(0)
            })
            .collect();
        let mut outer: Vec<Option<usize>> = vec![None; self.components];
        for (c, comp) in component.iter().enumerate() {
            if let Some(slot) = outer.get_mut(*comp) {
                let better = match slot {
                    None => true,
                    Some(o) => area.get(c) < area.get(*o),
                };
                if better {
                    *slot = Some(c);
                }
            }
        }
        Ok(Faces {
            cycles,
            cycle_of,
            area,
            component,
            outer: outer.into_iter().map(|o| o.unwrap_or(0)).collect(),
        })
    }

    /// The vertices of each component, in order.
    pub(crate) fn component_vertices(&self) -> Vec<Vec<Point2>> {
        let mut out = vec![Vec::new(); self.components];
        for (p, c) in self.vertices.iter().zip(&self.component_of) {
            if let Some(list) = out.get_mut(*c) {
                list.push(*p);
            }
        }
        out
    }

    /// The number of components.
    pub(crate) fn component_count(&self) -> usize {
        self.components
    }

    /// The component of input element `element`, when any of it remains.
    pub(crate) fn element_component(&self, element: usize) -> Option<usize> {
        let v = (*self.element_vertex.get(element)?)?;
        self.component_of.get(v).copied()
    }

    /// The winding numbers of every cycle's face, given those outside each component.
    ///
    /// # Errors
    ///
    /// Whatever `outside` returns.
    pub(crate) fn windings(
        &self,
        faces: &Faces,
        mut outside: impl FnMut(usize) -> Result<[i32; 2]>,
    ) -> Result<Vec<[i32; 2]>> {
        let mut winding: Vec<Option<[i32; 2]>> = vec![None; faces.cycles.len()];
        for (comp, outer) in faces.outer.iter().enumerate() {
            if faces.component.get(*outer) != Some(&comp) {
                continue;
            }
            if let Some(slot) = winding.get_mut(*outer) {
                *slot = Some(outside(comp)?);
            }
            let mut queue = VecDeque::from([*outer]);
            while let Some(c) = queue.pop_front() {
                let Some(w) = winding.get(c).copied().flatten() else {
                    continue;
                };
                for h in faces.cycles.get(c).map_or(&[][..], Vec::as_slice) {
                    let Some(&other) = faces.cycle_of.get(h ^ 1) else {
                        continue;
                    };
                    if let Some(slot @ None) = winding.get_mut(other) {
                        let d = self.delta(*h);
                        *slot = Some([w[0] - d[0], w[1] - d[1]]);
                        queue.push_back(other);
                    }
                }
            }
        }
        Ok(winding.into_iter().map(|w| w.unwrap_or([0, 0])).collect())
    }

    /// The region bounded by the half-edges with a filled face on their left and an unfilled
    /// one on their right, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// As [`Self::walk`], [`Self::cycle_loop`] and [`nest`].
    pub(crate) fn assemble(
        &self,
        faces: &Faces,
        filled: &[bool],
        tolerance: &Tolerance,
    ) -> Result<Option<Region>> {
        let is_filled =
            |c: Option<&usize>| c.and_then(|c| filled.get(*c)).copied().unwrap_or(false);
        let active: Vec<bool> = (0..faces.cycle_of.len())
            .map(|h| is_filled(faces.cycle_of.get(h)) && !is_filled(faces.cycle_of.get(h ^ 1)))
            .collect();
        let mut outers = Vec::new();
        let mut holes = Vec::new();
        for cycle in self.walk(&active)? {
            if self.cycle_area(&cycle) > 0.0 {
                outers.push(self.cycle_loop(&cycle, tolerance)?);
            } else {
                holes.push(self.cycle_loop(&cycle, tolerance)?);
            }
        }
        nest(outers, holes, tolerance)
    }
}

/// Points spread over a loop for containment tests: the middle of each curve, then its start.
pub(crate) fn samples(lp: &Loop2) -> impl Iterator<Item = Point2> + '_ {
    lp.curves()
        .iter()
        .map(|c| c.point_at(middle(c)))
        .chain(lp.curves().iter().filter_map(Curve2::start_point))
}

/// Whether `inner`, which neither crosses nor touches `outer`, lies inside it: the first sample
/// of `inner` that is not on the boundary of `outer` decides. `None` when every sample is.
pub(crate) fn loop_inside(
    inner: &Loop2,
    outer: &Loop2,
    tolerance: &Tolerance,
) -> Result<Option<bool>> {
    if !outer.bounding_box().contains(&inner.bounding_box()) {
        return Ok(Some(false));
    }
    for p in samples(inner) {
        match outer.classify(p, tolerance)? {
            Containment::Inside => return Ok(Some(true)),
            Containment::Outside => return Ok(Some(false)),
            Containment::OnBoundary => {}
        }
    }
    Ok(None)
}

/// Groups loops into a region: each hole goes to the outer loop of least area that contains
/// it. `None` when there is no outer loop.
///
/// # Errors
///
/// `geom.precision.insufficient` when no outer loop contains a hole; as [`Region::new`].
pub(crate) fn nest(
    outers: Vec<Loop2>,
    holes: Vec<Loop2>,
    tolerance: &Tolerance,
) -> Result<Option<Region>> {
    if outers.is_empty() {
        return Ok(None);
    }
    let areas: Vec<f64> = outers.iter().map(|o| o.signed_area().abs()).collect();
    let mut groups: Vec<Vec<Loop2>> = vec![Vec::new(); outers.len()];
    for hole in holes {
        let mut best: Option<usize> = None;
        for (k, outer) in outers.iter().enumerate() {
            if loop_inside(&hole, outer, tolerance)? == Some(true)
                && best.is_none_or(|b| areas.get(k) < areas.get(b))
            {
                best = Some(k);
            }
        }
        let group = best.and_then(|b| groups.get_mut(b)).ok_or_else(|| {
            Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "nest")
        })?;
        group.push(hole);
    }
    let parts = outers
        .into_iter()
        .zip(groups)
        .map(|(outer, holes)| RegionPart::new(outer, holes))
        .collect();
    Region::new(parts).map(Some)
}

/// The winding number of a loop at `point` (+1 inside a counterclockwise loop, −1 inside a
/// clockwise one), or `None` when the point is on its boundary.
pub(crate) fn winding(lp: &Loop2, point: Point2, tolerance: &Tolerance) -> Result<Option<i32>> {
    if !lp
        .bounding_box()
        .expanded(tolerance.linear)
        .contains_point(&point)
    {
        return Ok(Some(0));
    }
    Ok(match lp.classify(point, tolerance)? {
        Containment::Inside => Some(if lp.is_counterclockwise() { 1 } else { -1 }),
        Containment::Outside => Some(0),
        Containment::OnBoundary => None,
    })
}

/// Combines loops, each with its operand, by their winding numbers: the result holds the
/// points where `keep` holds for the winding numbers of the two operands. `None` when it is
/// empty.
///
/// # Errors
///
/// As [`Arrangement::build`]; `geom.precision.insufficient` when the winding outside a
/// component cannot be decided.
pub(crate) fn combine(
    loops: &[(Loop2, usize)],
    keep: impl Fn([i32; 2]) -> bool,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Option<Region>> {
    let mut inputs = Vec::new();
    // The first element of each loop.
    let mut first = Vec::with_capacity(loops.len());
    for (l, (lp, operand)) in loops.iter().enumerate() {
        first.push(inputs.len());
        let mut weight = [0, 0];
        if let Some(w) = weight.get_mut(*operand) {
            *w = 1;
        }
        for curve in lp.curves() {
            for curve in explode(curve, l)? {
                inputs.push(Input { curve, weight });
            }
        }
        if inputs.len() > max_count(limits) {
            return Err(too_large("elements"));
        }
    }
    let arrangement = Arrangement::build(&inputs, tolerance, limits)?;
    let faces = arrangement.faces()?;
    // The component of each loop: of the first of its elements that remains.
    let ends: Vec<usize> = first
        .iter()
        .skip(1)
        .copied()
        .chain([inputs.len()])
        .collect();
    let loop_component: Vec<Option<usize>> = first
        .iter()
        .zip(&ends)
        .map(|(a, b)| (*a..*b).find_map(|e| arrangement.element_component(e)))
        .collect();
    let members = arrangement.component_vertices();
    let windings = arrangement.windings(&faces, |comp| {
        let points = members.get(comp).map_or(&[][..], Vec::as_slice);
        let mut total = [0, 0];
        for ((lp, operand), c) in loops.iter().zip(&loop_component) {
            if *c == Some(comp) {
                continue;
            }
            let mut decided = None;
            for p in points {
                if let Some(w) = winding(lp, *p, tolerance)? {
                    decided = Some(w);
                    break;
                }
            }
            let w = decided.ok_or_else(|| {
                Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "winding")
            })?;
            if let Some(t) = total.get_mut(*operand) {
                *t += w;
            }
        }
        Ok(total)
    })?;
    let filled: Vec<bool> = windings.into_iter().map(keep).collect();
    arrangement.assemble(&faces, &filled, tolerance)
}

#[cfg(test)]
pub(crate) mod tests {
    /// `default` property cases, or `PROPTEST_CASES` when it is set.
    pub(crate) fn cases(default: u32) -> u32 {
        std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}
