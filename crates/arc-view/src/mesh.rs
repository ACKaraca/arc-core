//! One object's geometry prepared for a view: welded triangles in view space, feature edges
//! (CORE-VIEW-008) and the plane section at the cut (CORE-VIEW-014).

use std::collections::HashMap;

use arc_base::math::{Point2, Point3};
use arc_base::{Error, Limits, Result, Tolerance};
use arc_mesh::TriMesh;
use arc_mesh::features::feature_edges;

use crate::def::ViewDirection;
use crate::error::codes;
use crate::source::Geometry;

/// A 3D edge in view space with its sub-object key.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Edge3 {
    pub a: Point3,
    pub b: Point3,
    pub key: Option<u32>,
}

/// A welded triangle mesh in view space.
#[derive(Debug, Default)]
pub(crate) struct Prepared {
    pub positions: Vec<Point3>,
    pub triangles: Vec<[u32; 3]>,
    /// The sub-object key of each triangle.
    pub keys: Vec<Option<u32>>,
    pub edges: Vec<Edge3>,
    pub z_min: f64,
    pub z_max: f64,
}

pub(crate) fn mesh_limits(array_length: usize) -> Limits {
    Limits {
        array_length: u64::try_from(array_length).unwrap_or(u64::MAX),
        ..Limits::DEFAULT
    }
}

/// Prepares `geometry`: meshes a solid, extracts boundary, sharp and non-manifold edges at
/// `crease`, welds exactly equal positions and maps everything to view space.
///
/// # Errors
///
/// `view.geometry.out_of_range` for a coordinate beyond `max_coordinate`;
/// `view.geometry.degenerate` when no triangle is higher than the linear tolerance; the errors of
/// meshing a solid and of [`feature_edges`].
pub(crate) fn prepare(
    geometry: &Geometry,
    direction: ViewDirection,
    crease: f64,
    max_coordinate: f64,
    array_length: usize,
    tolerance: &Tolerance,
) -> Result<Prepared> {
    let limits = mesh_limits(array_length);
    let owned;
    let mesh = match geometry {
        Geometry::Mesh(mesh) => mesh,
        Geometry::Solid(solid) => {
            owned = TriMesh::from_polyhedron(solid, tolerance, &limits)?;
            &owned
        }
    };
    let mut out = Prepared {
        z_min: f64::INFINITY,
        z_max: f64::NEG_INFINITY,
        ..Prepared::default()
    };
    let mut welded: HashMap<[u64; 3], u32> = HashMap::new();
    let mut map = Vec::with_capacity(mesh.vertex_count());
    for p in mesh.positions() {
        let v = direction.to_view(*p);
        if [v.x, v.y, v.z]
            .iter()
            .any(|c| c.is_nan() || c.abs() > max_coordinate)
        {
            return Err(Error::new(codes::GEOMETRY_OUT_OF_RANGE));
        }
        let next = u32::try_from(out.positions.len()).map_err(|_| too_many("triangles"))?;
        let id = *welded
            .entry([v.x, v.y, v.z].map(f64::to_bits))
            .or_insert(next);
        if id == next {
            out.positions.push(v);
            out.z_min = out.z_min.min(v.z);
            out.z_max = out.z_max.max(v.z);
        }
        map.push(id);
    }
    let at = |v: u32| usize::try_from(v).ok().and_then(|v| map.get(v)).copied();
    let position = |v: u32| {
        usize::try_from(v)
            .ok()
            .and_then(|v| out.positions.get(v))
            .copied()
    };
    let mut kept = Vec::new();
    for (index, t) in (0_u32..).zip(mesh.triangles()) {
        let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) else {
            continue;
        };
        let (Some(pa), Some(pb), Some(pc)) = (position(a), position(b), position(c)) else {
            continue;
        };
        // Triangles no higher than the linear tolerance (zero area or slivers) are dropped, as
        // `feature_edges` ignores them: they would open sections and draw spurious lines.
        let longest = (pb - pa).norm().max((pc - pb).norm()).max((pa - pc).norm());
        let doubled = (pb - pa).cross(&(pc - pa)).norm();
        if doubled.is_nan() || doubled <= tolerance.linear * longest {
            continue;
        }
        kept.push(([a, b, c], mesh.group_of(index).map(|g| g.key.local)));
    }
    if kept.is_empty() {
        return Err(Error::new(codes::GEOMETRY_DEGENERATE));
    }
    for (t, key) in kept {
        out.triangles.push(t);
        out.keys.push(key);
    }
    let features = feature_edges(mesh, crease, tolerance, &limits)?;
    let point = |v: u32| {
        at(v).and_then(|i| {
            usize::try_from(i)
                .ok()
                .and_then(|i| out.positions.get(i))
                .copied()
        })
    };
    let mut edges = Vec::new();
    for e in features
        .boundary
        .iter()
        .chain(&features.sharp)
        .chain(&features.non_manifold)
    {
        if let (Some(a), Some(b)) = (point(e.vertices[0]), point(e.vertices[1])) {
            let key = mesh.group_of(e.triangle).map(|g| g.key.local);
            edges.push(Edge3 { a, b, key });
        }
    }
    out.edges = edges;
    Ok(out)
}

/// A limit error naming `limit`.
pub(crate) fn too_many(limit: &'static str) -> Error {
    Error::new(codes::LIMIT_EXCEEDED).with_arg("limit", limit)
}

/// Where a section point lies: on a vertex exactly at the cut height or on an edge crossing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Vertex(u32),
    Edge(u32, u32),
}

/// A chain of section points; `keys[i]` is the sub-object key of the segment from point `i`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Chain {
    pub points: Vec<Point2>,
    pub keys: Vec<Option<u32>>,
}

/// The plane section of a prepared mesh.
#[derive(Debug, Default)]
pub(crate) struct Section {
    /// Closed loops: the segment from the last point returns to the first.
    pub loops: Vec<Chain>,
    /// Open chains.
    pub open: Vec<Chain>,
}

struct Piece {
    from: Key,
    to: Key,
    a: Point2,
    b: Point2,
    key: Option<u32>,
}

/// The section of `mesh` at height `h`.
///
/// A vertex at exactly `h` counts as above, so a face lying in the plane is cut just below it.
/// Each triangle with vertices on both sides gives one segment, oriented from its edge running
/// from above to below toward its edge running from below to above, so for a closed outward
/// mesh outer loops run counterclockwise and holes clockwise. Points on an edge are computed
/// from its lower vertex, so both triangles of the edge give the same point and chains join by
/// key, without tolerance.
pub(crate) fn section(mesh: &Prepared, h: f64, tol: &Tolerance) -> Section {
    let z = |v: u32| {
        usize::try_from(v)
            .ok()
            .and_then(|i| mesh.positions.get(i))
            .copied()
    };
    let crossing = |below: u32, above: u32| -> Option<(Key, Point2)> {
        let (pb, pa) = (z(below)?, z(above)?);
        if pa.z == h {
            return Some((Key::Vertex(above), Point2::new(pa.x, pa.y)));
        }
        let t = (h - pb.z) / (pa.z - pb.z);
        let key = Key::Edge(below.min(above), below.max(above));
        Some((
            key,
            Point2::new(pb.x + t * (pa.x - pb.x), pb.y + t * (pa.y - pb.y)),
        ))
    };
    let mut pieces = Vec::new();
    for (index, tri) in mesh.triangles.iter().enumerate() {
        let above = tri.map(|v| z(v).is_some_and(|p| p.z >= h));
        let (mut start, mut end) = (None, None);
        for i in 0..3 {
            let (Some(&u), Some(&w)) = (tri.get(i), tri.get((i + 1) % 3)) else {
                continue;
            };
            let (Some(&au), Some(&aw)) = (above.get(i), above.get((i + 1) % 3)) else {
                continue;
            };
            if au && !aw {
                start = crossing(w, u);
            } else if !au && aw {
                end = crossing(u, w);
            }
        }
        if let (Some((from, a)), Some((to, b))) = (start, end)
            && from != to
        {
            let key = mesh.keys.get(index).copied().flatten();
            pieces.push(Piece {
                from,
                to,
                a,
                b,
                key,
            });
        }
    }
    chain(&pieces, tol)
}

fn chain(pieces: &[Piece], tol: &Tolerance) -> Section {
    let mut starts: HashMap<Key, Vec<usize>> = HashMap::new();
    let mut ends: HashMap<Key, usize> = HashMap::new();
    for (i, p) in pieces.iter().enumerate() {
        starts.entry(p.from).or_default().push(i);
        *ends.entry(p.to).or_default() += 1;
    }
    let mut used = vec![false; pieces.len()];
    // Open chains start where nothing ends; then the rest are loops.
    let heads = (0..pieces.len())
        .filter(|&i| pieces.get(i).is_some_and(|p| !ends.contains_key(&p.from)))
        .chain(0..pieces.len());
    let mut out = Section::default();
    for first in heads {
        if used.get(first).copied().unwrap_or(true) {
            continue;
        }
        let Some(head) = pieces.get(first) else {
            continue;
        };
        let mut chain = Chain::default();
        chain.points.push(head.a);
        let mut current = first;
        let closed = loop {
            let Some(piece) = pieces.get(current) else {
                break false;
            };
            if let Some(flag) = used.get_mut(current) {
                *flag = true;
            }
            chain.keys.push(piece.key);
            if piece.to == head.from {
                break true;
            }
            chain.points.push(piece.b);
            let next = starts.get(&piece.to).and_then(|list| {
                list.iter()
                    .copied()
                    .find(|&i| !used.get(i).copied().unwrap_or(true))
            });
            match next {
                Some(n) => current = n,
                None => break false,
            }
        };
        let chain = simplify(chain, closed, tol);
        if closed {
            if chain.points.len() >= 3 {
                out.loops.push(chain);
            }
        } else if chain.points.len() >= 2 {
            out.open.push(chain);
        }
    }
    out
}

/// Drops points within the linear tolerance of the previous kept point.
fn simplify(chain: Chain, closed: bool, tol: &Tolerance) -> Chain {
    let mut out = Chain::default();
    for (i, p) in chain.points.iter().enumerate() {
        let key = chain.keys.get(i).copied().flatten();
        match out.points.last() {
            Some(last) if (p - last).norm() <= tol.linear => {}
            _ => {
                out.points.push(*p);
                out.keys.push(key);
            }
        }
    }
    if closed
        && out.points.len() > 1
        && let (Some(first), Some(last)) = (out.points.first(), out.points.last())
        && (first - last).norm() <= tol.linear
    {
        out.points.pop();
        out.keys.pop();
    }
    out
}
