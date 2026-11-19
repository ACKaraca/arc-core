//! `IfcExtrudedAreaSolid` (CORE-IO-015): a profile swept along a direction for a depth.
//!
//! Attributes: `SweptArea` 0 (a profile), `Position` 1 (an `IfcAxis2Placement3D`, absent for the
//! identity), `ExtrudedDirection` 2 (an `IfcDirection`, normalized here) and `Depth` 3 (positive).
//! The profile lies in the plane `z = 0` of the position, and the solid is the set of its points
//! moved from zero to `depth` along the direction, which may be skew to the profile's normal.
//!
//! The mesh has two copies of the profile's points, at the base and at the top, so the caps and the
//! side faces share vertices. The caps are the triangulation of the profile loops (with holes); the
//! sides are one quad per loop edge, split into two triangles. The winding is outward: the side of
//! the profile with the material on its left faces outward when the direction points to the same
//! side of the profile plane as its normal (`d . n > 0`), and the other orientation is used when it
//! does not. A direction parallel to the profile plane is degenerate and skipped.

use arc_base::math::{Point3, Vector3};
use arc_mesh::TriMeshBuilder;

use crate::error::reasons;
use crate::model::Record;

use super::attr::{self, skip};
use super::polygon::{P2, charge_triangulation, prepare, ring_vertices, triangulate};
use super::{
    Evaluator, Flow, MAX_LOOP_VERTICES, Piece, Shape, Step, TRIANGLE_BYTES, VERTEX_BYTES, index32,
};

impl Evaluator<'_> {
    /// The pieces of an extruded area solid.
    pub(crate) fn extrusion(&mut self, record: &Record) -> Step<Vec<Piece>> {
        let kind = "IFCEXTRUDEDAREASOLID";
        let parameter = |i: usize| record.parameters.get(i);
        let profile = attr::reference(parameter(0), kind)?;
        let position = self.axis3(attr::opt_reference(parameter(1), kind)?)?;
        let direction = attr::reference(parameter(2), kind)?;
        let direction = super::place::unit(self.direction(direction)?, kind)?;
        let depth = attr::real(parameter(3), self.scale, kind)?;
        if depth <= 0.0 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let loops = self.profile(profile)?;
        let mut points: Vec<P2> = Vec::new();
        let mut index_loops: Vec<Vec<usize>> = Vec::with_capacity(loops.len());
        for loop_ in loops {
            if loop_.len() > MAX_LOOP_VERTICES {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            let start = points.len();
            index_loops.push((start..start + loop_.len()).collect());
            points.extend(loop_);
        }
        let prepared = prepare(&points, index_loops, true)
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        // The caps and the sides use this one prepared ring. Its total, holes bridged twice, is capped
        // and charged before it is triangulated.
        let lengths: Vec<usize> = prepared.iter().map(Vec::len).collect();
        let ring = ring_vertices(&lengths)
            .filter(|&total| total <= MAX_LOOP_VERTICES)
            .ok_or_else(|| skip(reasons::GEOMETRY_NOT_EVALUATED, kind))?;
        charge_triangulation(&mut self.budget, ring)?;
        let triangles = triangulate(&points, &prepared, &mut self.budget)?
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;

        // The profile plane's normal and the direction, in the coordinates of the representation.
        let normal = position.transform_vector(&Vector3::z());
        let along = position.transform_vector(&direction);
        let facing = along.dot(&normal);
        if facing.abs() <= self.tol.angular {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let outward = facing > 0.0;

        let n = points.len();
        let sides: usize = prepared.iter().map(Vec::len).sum();
        let vertex_count = n
            .checked_mul(2)
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        let triangle_count = triangles
            .len()
            .checked_mul(2)
            .and_then(|c| c.checked_add(sides.checked_mul(2)?))
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        self.budget.array(vertex_count)?;
        self.budget.charge(
            u64::try_from(vertex_count)
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES)
                + u64::try_from(triangle_count)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(TRIANGLE_BYTES),
        )?;

        let mut positions: Vec<Point3> = Vec::with_capacity(vertex_count);
        for p in &points {
            positions.push(position.transform_point(&Point3::new(p[0], p[1], 0.0)));
        }
        for k in 0..n {
            let base = positions.get(k).copied().unwrap_or(Point3::origin());
            positions.push(base + along * depth);
        }
        if !positions
            .iter()
            .all(|p| p.coords.iter().all(|c| c.is_finite()))
        {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }

        let out = closed_faces(&triangles, &prepared, n, outward, triangle_count);
        let indices: Vec<[u32; 3]> = out
            .iter()
            .map(|t| Ok([index32(t[0])?, index32(t[1])?, index32(t[2])?]))
            .collect::<Step<_>>()?;
        self.charge_shape()?;
        let mesh = TriMeshBuilder::new(positions, indices)
            .build(&self.limits)
            .map_err(Flow::Hard)?;
        Ok(Self::own(Shape::Mesh(mesh)))
    }
}

/// The triangles of a closed extrusion over the cap triangles `triangles` of `n` profile points, each
/// ring of `prepared` giving its side quads. The points of the top are the base points plus `n`.
fn closed_faces(
    triangles: &[[usize; 3]],
    prepared: &[Vec<usize>],
    n: usize,
    outward: bool,
    capacity: usize,
) -> Vec<[usize; 3]> {
    let top = |k: usize| k + n;
    let mut out: Vec<[usize; 3]> = Vec::with_capacity(capacity);
    for t in triangles {
        // The caps: the base faces down, the top faces up, for outward winding.
        if outward {
            out.push([t[0], t[2], t[1]]);
            out.push([top(t[0]), top(t[1]), top(t[2])]);
        } else {
            out.push([t[0], t[1], t[2]]);
            out.push([top(t[0]), top(t[2]), top(t[1])]);
        }
    }
    for ring in prepared {
        for (k, &a) in ring.iter().enumerate() {
            let b = ring.get((k + 1) % ring.len()).copied().unwrap_or(a);
            if outward {
                out.push([a, b, top(b)]);
                out.push([a, top(b), top(a)]);
            } else {
                out.push([a, top(b), b]);
                out.push([a, top(a), top(b)]);
            }
        }
    }
    out
}
