//! Constructive solid geometry (CORE-IO-015): the primitives `IfcBlock`, `IfcRectangularPyramid`,
//! `IfcRightCircularCone`, `IfcRightCircularCylinder` and `IfcSphere`, and `IfcCsgSolid`.
//!
//! Every primitive has a `Position` (an `IfcAxis2Placement3D`) and is built in its local frame:
//!
//! - a block spans `[0, XLength] x [0, YLength] x [0, ZLength]`;
//! - a rectangular pyramid has the base `[0, XLength] x [0, YLength]` at `z = 0` and its apex above
//!   the center of the base at `Height`;
//! - a right circular cone has its base circle (`BottomRadius`) centered on the origin in `z = 0` and
//!   its apex at `Height` on the axis;
//! - a right circular cylinder runs from `z = 0` to `Height` with `Radius`;
//! - a sphere of `Radius` is centered on the origin.
//!
//! Circles are regular polygons inscribed in the circle with the chord tolerance (at most
//! [`MAX_CIRCLE_SEGMENTS`] per turn, reported as approximated when the limit applies). A sphere is
//! rings of latitude and meridians in the same way, with a vertex at each pole. A circle of
//! tessellation has less volume than the circle: the deficit of a sphere is below three times the
//! chord tolerance over the radius.
//!
//! An `IfcCsgSolid` is its `TreeRootExpression`, a primitive or a boolean result, evaluated as an item
//! of its own; the boolean results of a tree use the mesh boolean of `arc-mesh`.

use std::f64::consts::{PI, TAU};

use arc_base::math::Point3;

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::curves::arc_segments;
use super::profile::MAX_CIRCLE_SEGMENTS;
use super::solid::MeshAcc;
use super::{Evaluator, Piece, Step};

impl Evaluator<'_> {
    /// The pieces of a CSG primitive.
    pub(crate) fn csg_primitive(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let position = self.axis3(attr::opt_reference(
            named(file, record, "Position", 0),
            kind,
        )?)?;
        let length = |this: &Self, attribute: &str, index: usize| -> Step<f64> {
            let value = attr::real(named(this.file, record, attribute, index), this.scale, kind)?;
            if value > 0.0 {
                Ok(value)
            } else {
                Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
            }
        };
        let (positions, triangles) = match kind {
            "IFCBLOCK" => {
                let (x, y, z) = (
                    length(self, "XLength", 1)?,
                    length(self, "YLength", 2)?,
                    length(self, "ZLength", 3)?,
                );
                block(x, y, z)
            }
            "IFCRECTANGULARPYRAMID" => {
                let (x, y, h) = (
                    length(self, "XLength", 1)?,
                    length(self, "YLength", 2)?,
                    length(self, "Height", 3)?,
                );
                pyramid(x, y, h)
            }
            "IFCRIGHTCIRCULARCONE" => {
                let (h, r) = (length(self, "Height", 1)?, length(self, "BottomRadius", 2)?);
                let n = self.circle_count(name, kind, r)?;
                cone(h, r, n)
            }
            "IFCRIGHTCIRCULARCYLINDER" => {
                let (h, r) = (length(self, "Height", 1)?, length(self, "Radius", 2)?);
                let n = self.circle_count(name, kind, r)?;
                cylinder(h, r, n)
            }
            _ => {
                let r = length(self, "Radius", 1)?;
                let n = self.circle_count(name, kind, r)?;
                self.budget.array(n.saturating_mul(n))?;
                sphere(r, n)
            }
        };
        self.budget.array(positions.len())?;
        self.budget.array(triangles.len())?;
        self.budget.charge(
            u64::try_from(positions.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(super::VERTEX_BYTES)
                .saturating_add(
                    u64::try_from(triangles.len())
                        .unwrap_or(u64::MAX)
                        .saturating_mul(super::TRIANGLE_BYTES),
                ),
        )?;
        let moved: Vec<Point3> = positions
            .iter()
            .map(|p| position.transform_point(p))
            .collect();
        let mut acc = MeshAcc::default();
        acc.push_part(moved, &triangles)?;
        self.finish_mesh(acc)
    }

    /// The pieces of an `IfcCsgSolid`: those of its root expression.
    pub(crate) fn csg_solid(&mut self, name: u64, record: &Record, depth: u32) -> Step<Vec<Piece>> {
        let kind = "IFCCSGSOLID";
        let root = attr::reference(named(self.file, record, "TreeRootExpression", 0), kind)?;
        let _ = name;
        match self.item(root, depth.saturating_add(1))? {
            Some(pieces) => Ok(pieces.iter().cloned().collect()),
            None => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind)),
        }
    }

    /// The segments of a full circle of `radius`, reported when the limit applies.
    fn circle_count(&mut self, name: u64, kind: &str, radius: f64) -> Step<usize> {
        let (count, limited) = arc_segments(radius, TAU, self.tol.tessellation_chord);
        if limited {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        Ok(count.clamp(3, MAX_CIRCLE_SEGMENTS))
    }
}

type Mesh = (Vec<Point3>, Vec<[usize; 3]>);

fn block(x: f64, y: f64, z: f64) -> Mesh {
    let mut positions = Vec::with_capacity(8);
    for k in 0..8 {
        positions.push(Point3::new(
            if k & 1 == 0 { 0.0 } else { x },
            if k & 2 == 0 { 0.0 } else { y },
            if k & 4 == 0 { 0.0 } else { z },
        ));
    }
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [1, 3, 7, 5],
        [0, 4, 6, 2],
    ];
    let triangles = quads
        .iter()
        .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
        .collect();
    (positions, triangles)
}

fn pyramid(x: f64, y: f64, h: f64) -> Mesh {
    let positions = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(x, 0.0, 0.0),
        Point3::new(x, y, 0.0),
        Point3::new(0.0, y, 0.0),
        Point3::new(x / 2.0, y / 2.0, h),
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [0, 1, 4],
        [1, 2, 4],
        [2, 3, 4],
        [3, 0, 4],
    ];
    (positions, triangles)
}

fn ring(r: f64, z: f64, n: usize) -> Vec<Point3> {
    (0..n)
        .map(|k| {
            let t = TAU * (k as f64) / (n as f64);
            Point3::new(r * t.cos(), r * t.sin(), z)
        })
        .collect()
}

fn cone(h: f64, r: f64, n: usize) -> Mesh {
    let mut positions = ring(r, 0.0, n);
    positions.push(Point3::new(0.0, 0.0, 0.0));
    positions.push(Point3::new(0.0, 0.0, h));
    let (center, apex) = (n, n + 1);
    let mut triangles = Vec::with_capacity(2 * n);
    for k in 0..n {
        let next = (k + 1) % n;
        triangles.push([center, next, k]);
        triangles.push([k, next, apex]);
    }
    (positions, triangles)
}

fn cylinder(h: f64, r: f64, n: usize) -> Mesh {
    let mut positions = ring(r, 0.0, n);
    positions.extend(ring(r, h, n));
    positions.push(Point3::new(0.0, 0.0, 0.0));
    positions.push(Point3::new(0.0, 0.0, h));
    let (bottom, top) = (2 * n, 2 * n + 1);
    let mut triangles = Vec::with_capacity(4 * n);
    for k in 0..n {
        let next = (k + 1) % n;
        triangles.push([bottom, next, k]);
        triangles.push([top, n + k, n + next]);
        triangles.push([k, next, n + next]);
        triangles.push([k, n + next, n + k]);
    }
    (positions, triangles)
}

/// A sphere of `n` meridians and `n / 2` (at least 2) bands of latitude.
fn sphere(r: f64, n: usize) -> Mesh {
    let bands = (n / 2).max(2);
    let mut positions = vec![Point3::new(0.0, 0.0, r), Point3::new(0.0, 0.0, -r)];
    for j in 1..bands {
        let phi = PI * (j as f64) / (bands as f64);
        positions.extend(ring(r * phi.sin(), r * phi.cos(), n));
    }
    let at = |band: usize, k: usize| 2 + (band - 1) * n + k % n;
    let mut triangles = Vec::new();
    for k in 0..n {
        triangles.push([0, at(1, k), at(1, k + 1)]);
        triangles.push([1, at(bands - 1, k + 1), at(bands - 1, k)]);
    }
    for band in 1..bands - 1 {
        for k in 0..n {
            let (a, b) = (at(band, k), at(band, k + 1));
            let (c, d) = (at(band + 1, k), at(band + 1, k + 1));
            triangles.push([a, c, d]);
            triangles.push([a, d, b]);
        }
    }
    (positions, triangles)
}
