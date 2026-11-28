//! Boolean results with half spaces (CORE-IO-015): `IfcBooleanResult` and its subtype
//! `IfcBooleanClippingResult`, whose `Operator` is `.DIFFERENCE.` or `.INTERSECTION.`, whose first
//! operand is any solid or boolean result and whose second operand is an `IfcHalfSpaceSolid` on an
//! `IfcPlane` or an `IfcPolygonalBoundedHalfSpace` on one.
//!
//! The half space is the side of the plane opposite to its normal when `AgreementFlag` is `.T.` and
//! the side of the normal when it is `.F.`. The mesh of the first operand is cut by the plane exactly:
//! each triangle is split along the plane, the vertices within the linear tolerance of the plane count
//! as on it, and the section is closed by a cap triangulated in the plane (holes of the section
//! included). The cap and the cut triangles share their vertices, so the result is closed whenever the
//! operand was. A result that keeps nothing emits nothing.
//!
//! A polygonal bounded half space removes only the part of the half space inside the prism of its
//! boundary. It is evaluated as the whole half space when the boundary is convex and every vertex of
//! the removed part lies inside it; otherwise the boundary clips the solid itself, which is not
//! evaluated.
//!
//! A second operand that is a solid (any evaluated solid, CSG primitive or boolean result) is
//! combined with the first by [`arc_mesh::boolean`] in the coordinates of the representation; when
//! that is refused (an open or inverted operand, or a result that cannot be closed at the tolerance)
//! the first operand is kept and the result is reported as approximated.
//!
//! Anything else (a half space that is not on a plane, a base surface that is not a plane, a
//! polygonal boundary that clips the solid) leaves the first operand unchanged and is reported as
//! approximated, so the element is never dropped. Clipping a mesh that is not closed leaves the cut
//! open and is reported as approximated.

use std::collections::{BTreeMap, HashMap};

use arc_base::math::{Point3, Transform3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::polygon::{P2, charge_triangulation, prepare, ring_vertices, triangulate};
use super::{
    Evaluator, Flow, MAX_LOOP_VERTICES, Piece, Shape, Step, TRIANGLE_BYTES, VERTEX_BYTES, index32,
    shape_mesh,
};
use arc_mesh::boolean::BooleanOp;
use arc_mesh::{TriMesh, TriMeshBuilder};

/// The most triangles of two operands of a mesh boolean.
const MAX_BOOLEAN_TRIANGLES: usize = 100_000;

/// A half space the solid is cut by: a plane point, the unit normal of the kept side and the
/// optional boundary of a polygonal bounded half space (in the frame of its position).
struct Cut {
    origin: Point3,
    keep: Vector3,
    bound: Option<Bound>,
}

/// The boundary of a polygonal bounded half space: its position and convex polygon in it.
struct Bound {
    to_local: Transform3,
    polygon: Vec<P2>,
}

/// The result of cutting a mesh.
enum Clipped {
    /// Nothing is on the removed side.
    Unchanged,
    /// Nothing is kept.
    Empty,
    /// The kept part, whether its cap is closed, and the vertices of the removed part.
    Part(Vec<Point3>, Vec<[u32; 3]>, bool, Vec<Point3>),
}

impl Evaluator<'_> {
    /// The pieces of a boolean result.
    pub(crate) fn boolean(&mut self, name: u64, record: &Record, depth: u32) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let operator = attr::enumeration(named(file, record, "Operator", 0));
        let first = attr::reference(named(file, record, "FirstOperand", 1), kind)?;
        let second = attr::reference(named(file, record, "SecondOperand", 2), kind)?;
        let Some(pieces) = self.item(first, depth.saturating_add(1))? else {
            return Ok(Vec::new());
        };
        let pieces: Vec<Piece> = pieces.iter().cloned().collect();
        let cut = match (operator, self.half_space(second)) {
            (Some("DIFFERENCE"), Ok(Some(cut))) => Some(cut),
            (Some("INTERSECTION"), Ok(Some(mut cut))) => {
                cut.keep = -cut.keep;
                Some(cut)
            }
            (_, Err(Flow::Hard(error))) => return Err(Flow::Hard(error)),
            _ => None,
        };
        let Some(cut) = cut else {
            let half = self
                .file
                .instance(second)
                .and_then(|i| i.records.first())
                .is_none_or(|r| r.type_name.contains("HALFSPACE"));
            if !half
                && let Some(done) = self.solid_boolean(name, operator, &pieces, second, depth)?
            {
                return Ok(done);
            }
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
            return Ok(pieces);
        };
        let mut out = Vec::with_capacity(pieces.len());
        for piece in pieces {
            self.budget.step()?;
            let Shape::Mesh(mesh) = piece.shape.as_ref() else {
                out.push(piece);
                continue;
            };
            let reflected = piece
                .transform
                .matrix()
                .fixed_view::<3, 3>(0, 0)
                .determinant()
                < 0.0;
            let positions: Vec<Point3> = mesh
                .positions()
                .iter()
                .map(|p| piece.transform.transform_point(p))
                .collect();
            let triangles: Vec<[u32; 3]> = mesh
                .triangles()
                .iter()
                .map(|t| if reflected { [t[0], t[2], t[1]] } else { *t })
                .collect();
            match self.clip(&positions, &triangles, &cut)? {
                Clipped::Unchanged => {
                    out.push(piece);
                }
                Clipped::Empty => {}
                Clipped::Part(positions, triangles, closed, removed) => {
                    if let Some(bound) = &cut.bound
                        && !within_bound(bound, &removed, self.tol.linear)
                    {
                        // The boundary clips the solid: the whole half space is not the cut.
                        self.log.add(
                            reasons::GEOMETRY_APPROXIMATED,
                            kind,
                            Some(Source::Decimal(name)),
                            1,
                        );
                        out.push(piece);
                        continue;
                    }
                    if !closed {
                        self.log.add(
                            reasons::GEOMETRY_APPROXIMATED,
                            kind,
                            Some(Source::Decimal(name)),
                            1,
                        );
                    }
                    self.charge_shape()?;
                    let mesh = TriMeshBuilder::new(positions, triangles)
                        .build(&self.limits)
                        .map_err(Flow::Hard)?;
                    out.push(Piece {
                        shape: std::sync::Arc::new(Shape::Mesh(mesh)),
                        transform: Transform3::identity(),
                    });
                }
            }
        }
        Ok(out)
    }

    /// The first operand combined with the solid `second` by a mesh boolean; `None` when the second
    /// operand is not evaluated, the operator is unknown or an operand is not a closed solid (the
    /// caller reports the approximation and keeps the first operand).
    fn solid_boolean(
        &mut self,
        name: u64,
        operator: Option<&str>,
        firsts: &[Piece],
        second: u64,
        depth: u32,
    ) -> Step<Option<Vec<Piece>>> {
        let op = match operator {
            Some("DIFFERENCE") => BooleanOp::Difference,
            Some("UNION") => BooleanOp::Union,
            Some("INTERSECTION") => BooleanOp::Intersection,
            _ => return Ok(None),
        };
        let Some(seconds) = self.item(second, depth.saturating_add(1))? else {
            return Ok(None);
        };
        let mut tools: Vec<TriMesh> = Vec::new();
        for piece in seconds.iter() {
            if let Some(mesh) = shape_mesh(&piece.shape, &piece.transform, &self.limits)? {
                tools.push(mesh);
            }
        }
        if tools.is_empty() {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(firsts.len());
        for piece in firsts {
            self.budget.step()?;
            let Some(mut current) = shape_mesh(&piece.shape, &piece.transform, &self.limits)?
            else {
                out.push(piece.clone());
                continue;
            };
            for tool in &tools {
                self.budget.step()?;
                let size = current
                    .triangle_count()
                    .saturating_add(tool.triangle_count());
                if size > MAX_BOOLEAN_TRIANGLES {
                    return Ok(None);
                }
                self.budget.array(size.saturating_mul(4))?;
                self.budget.charge(
                    u64::try_from(size)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(8 * (VERTEX_BYTES + TRIANGLE_BYTES)),
                )?;
                match arc_mesh::boolean::boolean(&current, tool, op, &self.tol, &self.limits) {
                    Ok(next) => current = next,
                    Err(_) => return Ok(None),
                }
            }
            if current.triangle_count() == 0 {
                continue;
            }
            self.charge_shape()?;
            out.push(Piece {
                shape: std::sync::Arc::new(Shape::Mesh(current)),
                transform: Transform3::identity(),
            });
        }
        let _ = name;
        Ok(Some(out))
    }

    /// The cut of a half space operand; `Ok(None)` for an operand that is not a half space on a
    /// plane (the caller reports it).
    fn half_space(&mut self, name: u64) -> Step<Option<Cut>> {
        let file = self.file;
        let Some(record) = file.instance(name).and_then(|i| i.records.first()) else {
            return Ok(None);
        };
        let kind = record.type_name.as_str();
        if kind != "IFCHALFSPACESOLID" && kind != "IFCPOLYGONALBOUNDEDHALFSPACE" {
            return Ok(None);
        }
        let Ok(surface) = attr::reference(named(file, record, "BaseSurface", 0), kind) else {
            return Ok(None);
        };
        let Some(plane) = file.instance(surface).and_then(|i| i.records.first()) else {
            return Ok(None);
        };
        if plane.type_name != "IFCPLANE" {
            return Ok(None);
        }
        let position = self.axis3(attr::opt_reference(
            named(file, plane, "Position", 0),
            "IFCPLANE",
        )?)?;
        let origin = position.transform_point(&Point3::origin());
        let normal = position.transform_vector(&Vector3::z());
        let agreement = attr::boolean(named(file, record, "AgreementFlag", 1)).unwrap_or(true);
        let keep = if agreement { normal } else { -normal };
        let bound = if kind == "IFCPOLYGONALBOUNDEDHALFSPACE" {
            let local = self.axis3(attr::opt_reference(
                named(file, record, "Position", 2),
                kind,
            )?)?;
            let curve = attr::reference(named(file, record, "PolygonalBoundary", 3), kind)?;
            let mut points = self.curve_points(curve, MAX_LOOP_VERTICES)?;
            if points.len() > 1 && points.first() == points.last() {
                points.pop();
            }
            let polygon: Vec<P2> = points.iter().map(|p| [p.x, p.y]).collect();
            if polygon.len() < 3 || !convex(&polygon) {
                return Ok(None);
            }
            Some(Bound {
                to_local: local.inverse(),
                polygon,
            })
        } else {
            None
        };
        Ok(Some(Cut {
            origin,
            keep,
            bound,
        }))
    }

    /// Cuts the triangles by the plane of `cut`, keeping the side of its normal.
    #[allow(
        clippy::too_many_lines,
        reason = "one pass over the triangles and the cap"
    )]
    fn clip(&mut self, positions: &[Point3], triangles: &[[u32; 3]], cut: &Cut) -> Step<Clipped> {
        let eps = self.tol.linear;
        let n = cut.keep;
        let d: Vec<f64> = positions
            .iter()
            .map(|p| {
                let v = (p - cut.origin).dot(&n);
                if v.abs() <= eps { 0.0 } else { v }
            })
            .collect();
        if !d.iter().any(|&v| v < 0.0) {
            return Ok(Clipped::Unchanged);
        }
        if !d.iter().any(|&v| v > 0.0) {
            return Ok(Clipped::Empty);
        }
        self.budget.array(positions.len().saturating_mul(2))?;
        self.budget.charge(
            u64::try_from(positions.len().saturating_add(triangles.len()))
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES + TRIANGLE_BYTES)
                .saturating_mul(2),
        )?;
        let mut out_positions: Vec<Point3> = Vec::new();
        let mut out_planar: Vec<bool> = Vec::new();
        let mut kept_map: HashMap<u32, u32> = HashMap::new();
        let mut cut_map: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        let mut out_triangles: Vec<[u32; 3]> = Vec::new();
        let mut cut_edges: Vec<(u32, u32)> = Vec::new();
        let at = |k: u32| positions.get(k as usize).copied();
        for t in triangles {
            self.budget.step()?;
            let mut polygon: Vec<(u32, bool)> = Vec::with_capacity(4);
            for e in 0..3 {
                let (a, b) = (t[e], t[(e + 1) % 3]);
                let (da, db) = (
                    d.get(a as usize).copied().unwrap_or(0.0),
                    d.get(b as usize).copied().unwrap_or(0.0),
                );
                if da >= 0.0 {
                    let id =
                        match kept_map.get(&a) {
                            Some(&id) => id,
                            None => {
                                let id = index32(out_positions.len())?;
                                out_positions.push(at(a).ok_or_else(|| {
                                    skip(reasons::INDEX_INVALID, "IFCBOOLEANRESULT")
                                })?);
                                out_planar.push(da == 0.0);
                                kept_map.insert(a, id);
                                id
                            }
                        };
                    polygon.push((id, da == 0.0));
                }
                if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
                    let key = (a.min(b), a.max(b));
                    let id = match cut_map.get(&key) {
                        Some(&id) => id,
                        None => {
                            let (pa, pb) = (
                                at(a).ok_or_else(|| {
                                    skip(reasons::INDEX_INVALID, "IFCBOOLEANRESULT")
                                })?,
                                at(b).ok_or_else(|| {
                                    skip(reasons::INDEX_INVALID, "IFCBOOLEANRESULT")
                                })?,
                            );
                            // The point is computed from the endpoints in a fixed order, so both
                            // triangles of an edge get the same coordinates.
                            let (lo, hi, dl, dh) = if a < b {
                                (pa, pb, da, db)
                            } else {
                                (pb, pa, db, da)
                            };
                            let s = dl / (dl - dh);
                            let id = index32(out_positions.len())?;
                            out_positions.push(lo + (hi - lo) * s);
                            out_planar.push(true);
                            cut_map.insert(key, id);
                            id
                        }
                    };
                    polygon.push((id, true));
                }
            }
            if polygon.len() < 3 {
                continue;
            }
            for k in 0..polygon.len() {
                let (u, v) = (polygon[k], polygon[(k + 1) % polygon.len()]);
                if u.1 && v.1 && u.0 != v.0 {
                    cut_edges.push((u.0, v.0));
                }
            }
            for k in 1..polygon.len() - 1 {
                out_triangles.push([polygon[0].0, polygon[k].0, polygon[k + 1].0]);
            }
        }
        // The removed part: the vertices on the removed side and the section's points.
        let mut removed: Vec<Point3> = positions
            .iter()
            .filter(|p| (*p - cut.origin).dot(&n) < -eps)
            .copied()
            .collect();
        removed.extend(
            out_positions
                .iter()
                .zip(&out_planar)
                .filter(|(_, planar)| **planar)
                .map(|(p, _)| *p),
        );
        let closed = self.cap(&out_positions, &mut out_triangles, cut_edges, &n)?;
        if out_triangles.is_empty() {
            return Ok(Clipped::Empty);
        }
        Ok(Clipped::Part(out_positions, out_triangles, closed, removed))
    }

    /// Closes the section: the loops of the cut edges (reversed) triangulated in the plane. Returns
    /// whether every loop was closed and triangulated.
    #[allow(
        clippy::too_many_lines,
        reason = "loops, holes and triangulation of one section"
    )]
    fn cap(
        &mut self,
        positions: &[Point3],
        triangles: &mut Vec<[u32; 3]>,
        edges: Vec<(u32, u32)>,
        normal: &Vector3,
    ) -> Step<bool> {
        // Edges met by their reverse cancel: they are interior to the plane (a face of the solid in it).
        let mut count: HashMap<(u32, u32), i64> = HashMap::new();
        for &(a, b) in &edges {
            *count.entry((a, b)).or_insert(0) += 1;
        }
        let mut remaining: Vec<(u32, u32)> = Vec::new();
        for &(a, b) in &edges {
            let reverse = count.get(&(b, a)).copied().unwrap_or(0);
            let forward = count.get(&(a, b)).copied().unwrap_or(0);
            if forward > 0 && reverse > 0 {
                if let Some(c) = count.get_mut(&(a, b)) {
                    *c -= 1;
                }
                if let Some(c) = count.get_mut(&(b, a)) {
                    *c -= 1;
                }
            } else if forward > 0 {
                if let Some(c) = count.get_mut(&(a, b)) {
                    *c -= 1;
                }
                remaining.push((a, b));
            }
        }
        if remaining.is_empty() {
            return Ok(true);
        }
        // The cap runs against the cut edges.
        let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(a, b) in &remaining {
            next.entry(b).or_default().push(a);
        }
        let mut loops: Vec<Vec<u32>> = Vec::new();
        let mut complete = true;
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        for start in starts {
            loop {
                self.budget.step()?;
                let Some(first) = next.get_mut(&start).and_then(|v| v.pop()) else {
                    break;
                };
                let mut ring = vec![start];
                let mut current = first;
                let mut closed = false;
                while ring.len() <= remaining.len() {
                    if current == start {
                        closed = true;
                        break;
                    }
                    ring.push(current);
                    match next.get_mut(&current).and_then(|v| v.pop()) {
                        Some(n) => current = n,
                        None => break,
                    }
                }
                if closed && ring.len() >= 3 {
                    loops.push(ring);
                } else {
                    complete = false;
                }
            }
        }
        // The basis of the plane seen from the cap's outward side, which is against `normal`.
        let helper = if normal.x.abs() < 0.9 {
            Vector3::x()
        } else {
            Vector3::y()
        };
        let e1 = (helper - normal * helper.dot(normal)).normalize();
        let e2 = (-normal).cross(&e1);
        let flat = |positions: &[Point3], id: u32| -> P2 {
            let p = positions
                .get(id as usize)
                .copied()
                .unwrap_or(Point3::origin());
            [p.coords.dot(&e1), p.coords.dot(&e2)]
        };
        let area = |ring: &[u32], positions: &[Point3]| -> f64 {
            let pts: Vec<P2> = ring.iter().map(|&i| flat(positions, i)).collect();
            let mut sum = 0.0;
            for k in 0..pts.len() {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                sum += a[0] * b[1] - b[0] * a[1];
            }
            sum / 2.0
        };
        let mut outers: Vec<(Vec<u32>, f64)> = Vec::new();
        let mut holes: Vec<Vec<u32>> = Vec::new();
        for ring in loops {
            let a = area(&ring, positions);
            if a > 0.0 {
                outers.push((ring, a));
            } else {
                holes.push(ring);
            }
        }
        let mut groups: Vec<Vec<Vec<u32>>> = outers.iter().map(|(r, _)| vec![r.clone()]).collect();
        for hole in holes {
            let probe = hole
                .first()
                .map(|&i| flat(positions, i))
                .unwrap_or([0.0, 0.0]);
            let owner = outers
                .iter()
                .enumerate()
                .filter(|(_, (ring, _))| {
                    let pts: Vec<P2> = ring.iter().map(|&i| flat(positions, i)).collect();
                    point_in(&pts, probe)
                })
                .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
                .map(|(k, _)| k);
            match owner.and_then(|k| groups.get_mut(k)) {
                Some(group) => group.push(hole),
                None => complete = false,
            }
        }
        for group in groups {
            let mut points: Vec<P2> = Vec::new();
            let mut ids: Vec<u32> = Vec::new();
            let mut index_loops: Vec<Vec<usize>> = Vec::new();
            for ring in &group {
                let start = points.len();
                for &id in ring {
                    points.push(flat(positions, id));
                    ids.push(id);
                }
                index_loops.push((start..points.len()).collect());
            }
            let Some(prepared) = prepare(&points, index_loops, false) else {
                complete = false;
                continue;
            };
            let lengths: Vec<usize> = prepared.iter().map(Vec::len).collect();
            let Some(ring_total) = ring_vertices(&lengths).filter(|&t| t <= MAX_LOOP_VERTICES)
            else {
                complete = false;
                continue;
            };
            charge_triangulation(&mut self.budget, ring_total)?;
            match triangulate(&points, &prepared, &mut self.budget)? {
                Some(tris) => {
                    for t in tris {
                        triangles.push([
                            ids.get(t[0]).copied().unwrap_or(0),
                            ids.get(t[1]).copied().unwrap_or(0),
                            ids.get(t[2]).copied().unwrap_or(0),
                        ]);
                    }
                }
                None => complete = false,
            }
        }
        Ok(complete)
    }
}

/// Whether `polygon` (in either orientation) is convex.
fn convex(polygon: &[P2]) -> bool {
    let n = polygon.len();
    let mut sign = 0.0_f64;
    for k in 0..n {
        let (a, b, c) = (polygon[k], polygon[(k + 1) % n], polygon[(k + 2) % n]);
        let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
        if cross.abs() < 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

/// Whether `p` is inside the polygon by the crossing rule.
fn point_in(polygon: &[P2], p: P2) -> bool {
    let mut inside = false;
    let n = polygon.len();
    for k in 0..n {
        let (a, b) = (polygon[k], polygon[(k + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

/// Whether every point of the removed part lies inside the boundary prism (with its edges).
fn within_bound(bound: &Bound, removed: &[Point3], tolerance: f64) -> bool {
    removed.iter().all(|p| {
        let q = bound.to_local.transform_point(p);
        let flat = [q.x, q.y];
        point_in(&bound.polygon, flat) || on_boundary(&bound.polygon, flat, tolerance)
    })
}

fn on_boundary(polygon: &[P2], p: P2, tolerance: f64) -> bool {
    let n = polygon.len();
    (0..n).any(|k| {
        let (a, b) = (polygon[k], polygon[(k + 1) % n]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        if len2 == 0.0 {
            return false;
        }
        let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0);
        let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
        (p[0] - q[0]).hypot(p[1] - q[1]) <= tolerance
    })
}
