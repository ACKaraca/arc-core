//! Faces and shells (CORE-IO-015): `IfcTriangulatedFaceSet`, `IfcPolygonalFaceSet` (with
//! `...WithVoids` faces), `IfcFacetedBrep`, `IfcFaceBasedSurfaceModel` and
//! `IfcShellBasedSurfaceModel`, each evaluated into one triangle mesh in meters.
//!
//! Attribute positions: `IfcTriangulatedFaceSet` `Coordinates` 0, `Normals` 1, `Closed` 2,
//! `CoordIndex` 3, `PnIndex` 4; `IfcPolygonalFaceSet` `Coordinates` 0, `Closed` 1, `Faces` 2,
//! `PnIndex` 3; `IfcIndexedPolygonalFace` `CoordIndex` 0 and `...WithVoids` `InnerCoordIndices` 1;
//! `IfcFacetedBrep` `Outer` 0 (a closed shell); `IfcFace` `Bounds` 0; `IfcFaceBound` and
//! `IfcFaceOuterBound` `Bound` 0 and `Orientation` 1 (a `.F.` reverses the loop); `IfcPolyLoop`
//! `Polygon` 0.
//!
//! Index lists are one-based. With `PnIndex`, a `CoordIndex` value selects a `PnIndex` entry, which
//! is the one-based index into `Coordinates`. Every index is checked against its list before a
//! vertex is read. A face set whose declared list is longer than `array_length` is refused before
//! its points are read.
//!
//! The vertices of the mesh are welded by their coordinates, so faces that share a point share a
//! vertex. A planar face is triangulated in its own plane with the polygon triangulation of this
//! crate, with the outer loop's Newell normal as the plane normal: its triangles are
//! counterclockwise seen from that normal, which is the outward side for a closed shell whose
//! loops are given counterclockwise seen from outside. A face that is not planar within the linear
//! tolerance is triangulated in the projection onto its mean plane and reported as approximated.
//! An item with any face that cannot be evaluated is skipped as a whole, so no shell is emitted with
//! a face missing.

use std::collections::BTreeMap;

use arc_base::Limits;
use arc_base::math::{Point3, Vector3};
use arc_mesh::TriMeshBuilder;

use crate::error::reasons;
use crate::model::{Parameter, Record};

use super::attr::{self, skip};
use super::budget::Budget;
use super::polygon::{P2, prepare, triangulate};
use super::{
    Evaluator, Flow, MAX_LOOP_VERTICES, Piece, Shape, Step, TRIANGLE_BYTES, VERTEX_BYTES, index32,
};

/// Bytes charged for one entry of the vertex index.
const INDEX_BYTES: u64 = 48;

/// The vertices and triangles of one mesh under construction, welded by coordinates.
pub(crate) struct Welder {
    index: BTreeMap<[u64; 3], u32>,
    positions: Vec<Point3>,
    triangles: Vec<[u32; 3]>,
}

/// The key of a point: its bits, with the sign of zero normalized.
fn key(p: &Point3) -> [u64; 3] {
    [
        (p.x + 0.0).to_bits(),
        (p.y + 0.0).to_bits(),
        (p.z + 0.0).to_bits(),
    ]
}

impl Welder {
    fn new() -> Self {
        Self {
            index: BTreeMap::new(),
            positions: Vec::new(),
            triangles: Vec::new(),
        }
    }

    fn vertex(&mut self, budget: &mut Budget, p: Point3) -> Step<u32> {
        let k = key(&p);
        if let Some(&found) = self.index.get(&k) {
            return Ok(found);
        }
        budget.charge(VERTEX_BYTES + INDEX_BYTES)?;
        budget.array(self.positions.len() + 1)?;
        let found = index32(self.positions.len())?;
        self.positions.push(p);
        self.index.insert(k, found);
        Ok(found)
    }

    /// Adds a triangle of three points, charged before its storage.
    pub(crate) fn triangle(&mut self, budget: &mut Budget, t: [Point3; 3]) -> Step<()> {
        budget.charge(TRIANGLE_BYTES)?;
        budget.array(self.triangles.len() + 1)?;
        let a = self.vertex(budget, t[0])?;
        let b = self.vertex(budget, t[1])?;
        let c = self.vertex(budget, t[2])?;
        self.triangles.push([a, b, c]);
        Ok(())
    }

    fn finish(self, limits: &Limits) -> Step<arc_mesh::TriMesh> {
        TriMeshBuilder::new(self.positions, self.triangles)
            .build(limits)
            .map_err(Flow::Hard)
    }
}

/// A one-based index checked against the count of its list.
fn one_based(value: i64, count: usize, kind: &str) -> Step<usize> {
    value
        .checked_sub(1)
        .and_then(|k| usize::try_from(k).ok())
        .filter(|&k| k < count)
        .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))
}

/// The point a `CoordIndex` value selects, through `PnIndex` when the face set has one.
fn corner(points: &[Point3], pn: Option<&[i64]>, value: i64, kind: &str) -> Step<Point3> {
    let coordinate = match pn {
        Some(pn) => {
            let slot = one_based(value, pn.len(), kind)?;
            pn.get(slot)
                .copied()
                .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))?
        }
        None => value,
    };
    let k = one_based(coordinate, points.len(), kind)?;
    points
        .get(k)
        .copied()
        .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))
}

fn corners(points: &[Point3], pn: Option<&[i64]>, values: &[i64], kind: &str) -> Step<Vec<Point3>> {
    values
        .iter()
        .map(|&v| corner(points, pn, v, kind))
        .collect()
}

impl Evaluator<'_> {
    /// The pieces of a face-based item: one mesh of all its faces.
    pub(crate) fn faces(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let mut welder = Welder::new();
        let mut deviation = 0.0_f64;
        let first = record.parameters.first();
        match kind {
            "IFCTRIANGULATEDFACESET" => self.triangulated(record, &mut welder)?,
            "IFCPOLYGONALFACESET" => self.polygonal(record, &mut welder, &mut deviation)?,
            "IFCFACETEDBREP" => {
                let shell = attr::reference(first, kind)?;
                for face in self.shell_faces(shell)? {
                    self.add_face(face, &mut welder, &mut deviation)?;
                }
            }
            "IFCFACEBASEDSURFACEMODEL" | "IFCSHELLBASEDSURFACEMODEL" => {
                let (members, _) = attr::references(first, kind)?;
                for member in members {
                    for face in self.shell_faces(member)? {
                        self.add_face(face, &mut welder, &mut deviation)?;
                    }
                }
            }
            other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
        if welder.triangles.is_empty() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        if deviation > self.tol.linear {
            self.log
                .add(reasons::GEOMETRY_APPROXIMATED, kind, Some(name), 1);
        }
        self.charge_shape()?;
        let mesh = welder.finish(&self.limits)?;
        Ok(Self::own(Shape::Mesh(mesh)))
    }

    /// The `PnIndex` list of a face set, or `None` when it has none.
    fn pn_index(&self, parameter: Option<&Parameter>) -> Step<Option<Vec<i64>>> {
        match parameter {
            None | Some(Parameter::Omitted | Parameter::Derived) => Ok(None),
            Some(_) => attr::integers(parameter, "IFCPNINDEX").map(Some),
        }
    }

    fn triangulated(&mut self, record: &Record, welder: &mut Welder) -> Step<()> {
        let kind = "IFCTRIANGULATEDFACESET";
        let coordinates = attr::reference(record.parameters.first(), kind)?;
        let points = self.point_list(coordinates)?;
        let pn = self.pn_index(record.parameters.get(4))?;
        let rows = attr::list(record.parameters.get(3), kind)?;
        self.budget.array(rows.len())?;
        for row in rows {
            let values = attr::integers(Some(row), kind)?;
            let [a, b, c] = values.as_slice() else {
                return Err(skip(reasons::INDEX_INVALID, kind));
            };
            let t = corners(&points, pn.as_deref(), &[*a, *b, *c], kind)?;
            let [p, q, r] = t.as_slice() else {
                return Err(skip(reasons::INDEX_INVALID, kind));
            };
            welder.triangle(&mut self.budget, [*p, *q, *r])?;
        }
        Ok(())
    }

    fn polygonal(&mut self, record: &Record, welder: &mut Welder, deviation: &mut f64) -> Step<()> {
        let kind = "IFCPOLYGONALFACESET";
        let file = self.file;
        let coordinates = attr::reference(record.parameters.first(), kind)?;
        let points = self.point_list(coordinates)?;
        let pn = self.pn_index(record.parameters.get(3))?;
        let faces = attr::references(record.parameters.get(2), kind)?.0;
        self.budget.array(faces.len())?;
        for face in faces {
            let face_record =
                attr::typed_first(file, face).ok_or_else(|| skip(reasons::TARGET_MISSING, kind))?;
            let mut loops = vec![corners(
                &points,
                pn.as_deref(),
                &attr::integers(face_record.parameters.first(), kind)?,
                kind,
            )?];
            match face_record.type_name.as_str() {
                "IFCINDEXEDPOLYGONALFACE" => {}
                "IFCINDEXEDPOLYGONALFACEWITHVOIDS" => {
                    for row in attr::list(face_record.parameters.get(1), kind)? {
                        let values = attr::integers(Some(row), kind)?;
                        loops.push(corners(&points, pn.as_deref(), &values, kind)?);
                    }
                }
                other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
            }
            self.emit_planar(&loops, welder, deviation)?;
        }
        Ok(())
    }

    /// Triangulates a planar face given by its loops and adds the triangles to the welder.
    fn emit_planar(
        &mut self,
        loops: &[Vec<Point3>],
        welder: &mut Welder,
        deviation: &mut f64,
    ) -> Step<()> {
        let Some((triangles, dev)) = self.planar(loops)? else {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, "IFCFACE"));
        };
        *deviation = deviation.max(dev);
        for t in triangles {
            welder.triangle(&mut self.budget, t)?;
        }
        Ok(())
    }

    /// The faces of a shell, connected face set or open shell, in order.
    fn shell_faces(&self, name: u64) -> Step<Vec<u64>> {
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCCLOSEDSHELL"));
        };
        let Some(record) = instance.records.first() else {
            return Err(skip(reasons::TARGET_MISSING, "IFCCLOSEDSHELL"));
        };
        match record.type_name.as_str() {
            "IFCCLOSEDSHELL" | "IFCOPENSHELL" | "IFCCONNECTEDFACESET" => {
                Ok(attr::references(record.parameters.first(), &record.type_name)?.0)
            }
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    /// Adds one `IfcFace`: its outer bound and holes, each loop with its orientation applied.
    fn add_face(&mut self, face: u64, welder: &mut Welder, deviation: &mut f64) -> Step<()> {
        let kind = "IFCFACE";
        let file = self.file;
        let record =
            attr::typed_first(file, face).ok_or_else(|| skip(reasons::TARGET_MISSING, kind))?;
        if record.type_name != kind {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        let bounds = attr::references(record.parameters.first(), kind)?.0;
        let mut loops: Vec<(bool, Vec<Point3>)> = Vec::with_capacity(bounds.len());
        for bound in bounds {
            let bound_record = attr::typed_first(file, bound)
                .ok_or_else(|| skip(reasons::TARGET_MISSING, "IFCFACEBOUND"))?;
            let outer = match bound_record.type_name.as_str() {
                "IFCFACEOUTERBOUND" => true,
                "IFCFACEBOUND" => false,
                other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
            };
            let loop_name =
                attr::reference(bound_record.parameters.first(), &bound_record.type_name)?;
            let mut points = self.polygon_loop(loop_name)?;
            if attr::enumeration(bound_record.parameters.get(1)) == Some("F") {
                points.reverse();
            }
            loops.push((outer, points));
        }
        let outer_position = loops.iter().position(|(outer, _)| *outer).unwrap_or(0);
        let mut outer_loop = None;
        let mut ordered = Vec::with_capacity(loops.len());
        for (position, (_, points)) in loops.into_iter().enumerate() {
            if position == outer_position {
                outer_loop = Some(points);
            } else {
                ordered.push(points);
            }
        }
        let Some(outer_loop) = outer_loop else {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        };
        let mut all = vec![outer_loop];
        all.extend(ordered);
        self.emit_planar(&all, welder, deviation)
    }

    /// The points of an `IfcPolyLoop`, with three or more and at most [`MAX_LOOP_VERTICES`].
    fn polygon_loop(&mut self, name: u64) -> Step<Vec<Point3>> {
        let kind = "IFCPOLYLOOP";
        let file = self.file;
        let record =
            attr::typed_first(file, name).ok_or_else(|| skip(reasons::TARGET_MISSING, kind))?;
        if record.type_name != kind {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        let (refs, _) = attr::references(record.parameters.first(), kind)?;
        if refs.len() > MAX_LOOP_VERTICES {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        if refs.len() < 3 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        refs.into_iter().map(|p| self.point(p)).collect()
    }

    /// The triangles of a planar face given by its loops, outer loop first. The triangles are
    /// counterclockwise seen from the Newell normal of the outer loop, and the deviation is the
    /// largest distance of a point from the plane through the outer loop's first point. `None` for a
    /// loop with zero area or a region that is not valid.
    fn planar(&mut self, loops: &[Vec<Point3>]) -> Step<Option<(Vec<[Point3; 3]>, f64)>> {
        let Some(outer) = loops.first() else {
            return Ok(None);
        };
        let n = outer.len();
        let mut normal = Vector3::zeros();
        for k in 0..n {
            let (Some(a), Some(b)) = (outer.get(k), outer.get((k + 1) % n)) else {
                continue;
            };
            normal.x += (a.y - b.y) * (a.z + b.z);
            normal.y += (a.z - b.z) * (a.x + b.x);
            normal.z += (a.x - b.x) * (a.y + b.y);
        }
        let length = normal.norm();
        let Some(origin) = outer.first().copied() else {
            return Ok(None);
        };
        if !length.is_finite() || length <= 0.0 {
            return Ok(None);
        }
        let unit_normal = normal / length;
        let helper = if unit_normal.x.abs() <= unit_normal.y.abs()
            && unit_normal.x.abs() <= unit_normal.z.abs()
        {
            Vector3::x()
        } else if unit_normal.y.abs() <= unit_normal.z.abs() {
            Vector3::y()
        } else {
            Vector3::z()
        };
        let u_raw = helper - unit_normal * helper.dot(&unit_normal);
        let u_norm = u_raw.norm();
        if !u_norm.is_finite() || u_norm <= 0.0 {
            return Ok(None);
        }
        let u = u_raw / u_norm;
        let v = unit_normal.cross(&u);

        let mut all3: Vec<Point3> = Vec::new();
        let mut all2: Vec<P2> = Vec::new();
        let mut index_loops: Vec<Vec<usize>> = Vec::with_capacity(loops.len());
        for loop_ in loops {
            let start = all3.len();
            for p in loop_ {
                let d = *p - origin;
                all2.push([d.dot(&u), d.dot(&v)]);
                all3.push(*p);
            }
            index_loops.push((start..all3.len()).collect());
        }
        let deviation = all3
            .iter()
            .map(|p| (*p - origin).dot(&unit_normal).abs())
            .fold(0.0_f64, f64::max);
        let Some(prepared) = prepare(&all2, index_loops) else {
            return Ok(None);
        };
        let Some(triangles) = triangulate(&all2, &prepared, &mut self.budget)? else {
            return Ok(None);
        };
        let at = |k: usize| all3.get(k).copied().unwrap_or(origin);
        let out = triangles
            .iter()
            .map(|t| [at(t[0]), at(t[1]), at(t[2])])
            .collect();
        Ok(Some((out, deviation)))
    }
}
