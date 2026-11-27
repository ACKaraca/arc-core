//! Swept area solids built from rings of profile points (CORE-IO-015): `IfcExtrudedAreaSolid` and
//! its tapered form `IfcExtrudedAreaSolidTapered`.
//!
//! Attributes: `SweptArea` 0 (a profile), `Position` 1 (an `IfcAxis2Placement3D`, absent for the
//! identity), `ExtrudedDirection` 2 (an `IfcDirection`, normalized here), `Depth` 3 (positive) and, for
//! the tapered form, `EndSweptArea` 4. The profile lies in the plane `z = 0` of the position, and the
//! solid is the set of its points moved from zero to `depth` along the direction, which may be skew to
//! the profile's normal. The end profile of a tapered solid is placed in the plane `depth` along the
//! direction; it must have the loops of the start profile with the same point counts, in the same
//! order, and each side face joins corresponding edges (two triangles when the edges are not
//! parallel). A direction parallel to the profile plane is degenerate and skipped.
//!
//! The mesh has one ring of the profile's points at the base and one at the top, so the caps and the
//! side faces share vertices. The caps are the triangulation of the profile loops (with holes); the
//! sides are one quad per loop edge, split into two triangles. The winding is outward: the whole
//! mesh is wound consistently and reversed when its signed volume is negative. A profile with several
//! parts (a composite profile) gives one closed mesh per part in the same shape.

use crate::schema::access::known;
use arc_base::math::{Point3, Transform3, Vector3};
use arc_mesh::TriMeshBuilder;

use crate::error::reasons;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::polygon::{P2, charge_triangulation, prepare, ring_vertices, triangulate};
use super::profile::Parts;
use super::{
    Evaluator, Flow, MAX_LOOP_VERTICES, Piece, Shape, Step, TRIANGLE_BYTES, VERTEX_BYTES, index32,
};

/// The loops of a part as indices into one list of points, the prepared rings and the cap triangles
/// of the part.
pub(crate) struct Prepared {
    /// The points of the part, loops one after the other.
    pub(crate) points: Vec<P2>,
    /// The rings (outer counterclockwise, holes clockwise) as indices into `points`.
    pub(crate) rings: Vec<Vec<usize>>,
    /// The triangles of the cap, counterclockwise.
    pub(crate) caps: Vec<[usize; 3]>,
}

/// A mesh under construction: the vertices and triangles of closed parts.
#[derive(Default)]
pub(crate) struct MeshAcc {
    pub(crate) positions: Vec<Point3>,
    pub(crate) triangles: Vec<[u32; 3]>,
}

impl MeshAcc {
    /// Adds the closed part `positions` and `triangles` (indices into `positions`), reversing every
    /// triangle when the part is wound inward (negative signed volume).
    pub(crate) fn push_part(
        &mut self,
        positions: Vec<Point3>,
        triangles: &[[usize; 3]],
    ) -> Step<()> {
        let at = |k: usize| positions.get(k).copied().unwrap_or(Point3::origin());
        let reference = positions.first().copied().unwrap_or(Point3::origin());
        let volume: f64 = triangles
            .iter()
            .map(|t| {
                let (a, b, c) = (
                    at(t[0]) - reference,
                    at(t[1]) - reference,
                    at(t[2]) - reference,
                );
                a.dot(&b.cross(&c))
            })
            .sum();
        let offset = self.positions.len();
        for t in triangles {
            let [a, b, c] = [
                index32(t[0] + offset)?,
                index32(t[1] + offset)?,
                index32(t[2] + offset)?,
            ];
            self.triangles
                .push(if volume < 0.0 { [a, c, b] } else { [a, b, c] });
        }
        self.positions.extend(positions);
        Ok(())
    }
}

impl Evaluator<'_> {
    /// Prepares the loops of one part: the rings, the cap triangulation and the budget for both.
    pub(crate) fn prepare_part(&mut self, loops: Vec<Vec<P2>>, kind: &str) -> Step<Prepared> {
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
        let rings = prepare(&points, index_loops, true)
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        // The caps and the sides use these rings. Their total, holes bridged twice, is capped and
        // charged before they are triangulated.
        let lengths: Vec<usize> = rings.iter().map(Vec::len).collect();
        let ring = ring_vertices(&lengths)
            .filter(|&total| total <= MAX_LOOP_VERTICES)
            .ok_or_else(|| skip(reasons::GEOMETRY_NOT_EVALUATED, kind))?;
        charge_triangulation(&mut self.budget, ring)?;
        let caps = triangulate(&points, &rings, &mut self.budget)?
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        Ok(Prepared {
            points,
            rings,
            caps,
        })
    }

    /// Builds the closed part whose `ring_count` rings of `part.points.len()` points each are the
    /// consecutive blocks of `positions`: the caps at the first and last ring (unless `wrap`, where
    /// the last ring joins the first) and a quad per loop edge between consecutive rings. A point
    /// flagged in `fixed` is the same vertex in every ring (a point on an axis of revolution). The
    /// part is added to `acc`.
    pub(crate) fn ring_part(
        &mut self,
        acc: &mut MeshAcc,
        part: &Prepared,
        positions: Vec<Point3>,
        ring_count: usize,
        wrap: bool,
        fixed: &[bool],
    ) -> Step<()> {
        let n = part.points.len();
        let steps = if wrap {
            ring_count
        } else {
            ring_count.saturating_sub(1)
        };
        let sides: usize = part.rings.iter().map(Vec::len).sum();
        let triangle_count = sides
            .saturating_mul(steps)
            .saturating_mul(2)
            .saturating_add(if wrap { 0 } else { part.caps.len() * 2 });
        self.budget.array(positions.len())?;
        self.budget.array(triangle_count)?;
        self.budget.charge(
            u64::try_from(positions.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES)
                .saturating_add(
                    u64::try_from(triangle_count)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(TRIANGLE_BYTES),
                ),
        )?;
        if !positions
            .iter()
            .all(|p| p.coords.iter().all(|c| c.is_finite()))
        {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, "IFCSWEPTAREASOLID"));
        }
        let vertex = |ring: usize, k: usize| -> usize {
            if fixed.get(k).copied().unwrap_or(false) {
                k
            } else {
                (ring % ring_count) * n + k
            }
        };
        let mut triangles: Vec<[usize; 3]> = Vec::with_capacity(triangle_count);
        let mut push = |t: [usize; 3]| {
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                triangles.push(t);
            }
        };
        for step in 0..steps {
            for ring in &part.rings {
                for (k, &a) in ring.iter().enumerate() {
                    let b = ring.get((k + 1) % ring.len()).copied().unwrap_or(a);
                    let (a0, b0) = (vertex(step, a), vertex(step, b));
                    let (a1, b1) = (vertex(step + 1, a), vertex(step + 1, b));
                    push([a0, b0, b1]);
                    push([a0, b1, a1]);
                }
            }
        }
        if !wrap {
            for t in &part.caps {
                // The first cap faces against the sweep, the last along it.
                push([vertex(0, t[0]), vertex(0, t[2]), vertex(0, t[1])]);
                let last = ring_count - 1;
                push([vertex(last, t[0]), vertex(last, t[1]), vertex(last, t[2])]);
            }
        }
        acc.push_part(positions, &triangles)
    }

    /// The shape of the accumulated parts.
    pub(crate) fn finish_mesh(&mut self, acc: MeshAcc) -> Step<Vec<Piece>> {
        self.charge_shape()?;
        let mesh = TriMeshBuilder::new(acc.positions, acc.triangles)
            .build(&self.limits)
            .map_err(Flow::Hard)?;
        Ok(Self::own(Shape::Mesh(mesh)))
    }

    /// The pieces of an extruded area solid, tapered or not.
    pub(crate) fn extrusion(&mut self, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let parameter = |a| file.attr(record, a);
        let profile = attr::reference(parameter(known::EXTRUSION_AREA), kind)?;
        let position = self.axis3(attr::opt_reference(
            parameter(known::EXTRUSION_POSITION),
            kind,
        )?)?;
        let direction = attr::reference(parameter(known::EXTRUSION_DIRECTION), kind)?;
        let direction = super::place::unit(self.direction(direction)?, kind)?;
        let depth = attr::real(parameter(known::EXTRUSION_DEPTH), self.scale, kind)?;
        if depth <= 0.0 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let parts = self.profile(profile)?;
        let end_parts = if kind == "IFCEXTRUDEDAREASOLIDTAPERED" {
            let end = attr::reference(named(file, record, "EndSweptArea", 4), kind)?;
            let end_parts = self.profile(end)?;
            let same = parts.len() == end_parts.len()
                && parts.iter().zip(&end_parts).all(|(a, b)| {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.len() == y.len())
                });
            if !same {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            Some(end_parts)
        } else {
            None
        };
        let normal = position.transform_vector(&Vector3::z());
        let along = position.transform_vector(&direction);
        if along.dot(&normal).abs() <= self.tol.angular {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let mut acc = MeshAcc::default();
        for (k, part) in parts.into_iter().enumerate() {
            let end = end_parts.as_ref().and_then(|e| e.get(k));
            self.extrude_part(&mut acc, part, end, &position, along * depth, kind)?;
        }
        self.finish_mesh(acc)
    }

    /// One closed part of an extrusion: the profile loops at the base, the end loops (or the same
    /// loops) moved by `offset` at the top.
    fn extrude_part(
        &mut self,
        acc: &mut MeshAcc,
        loops: Vec<Vec<P2>>,
        end: Option<&Vec<Vec<P2>>>,
        position: &Transform3,
        offset: Vector3,
        kind: &str,
    ) -> Step<()> {
        let part = self.prepare_part(loops, kind)?;
        let base: Vec<Point3> = part
            .points
            .iter()
            .map(|p| position.transform_point(&Point3::new(p[0], p[1], 0.0)))
            .collect();
        let top: Vec<Point3> = match end {
            Some(end) => end
                .iter()
                .flatten()
                .map(|p| position.transform_point(&Point3::new(p[0], p[1], 0.0)) + offset)
                .collect(),
            None => base.iter().map(|p| p + offset).collect(),
        };
        if top.len() != base.len() {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        let mut positions = base;
        positions.extend(top);
        self.ring_part(acc, &part, positions, 2, false, &[])
    }

    /// The profile parts of a sweep as loops of 2D points with their prepared rings.
    pub(crate) fn prepared_parts(&mut self, parts: Parts, kind: &str) -> Step<Vec<Prepared>> {
        parts
            .into_iter()
            .map(|loops| self.prepare_part(loops, kind))
            .collect()
    }
}
