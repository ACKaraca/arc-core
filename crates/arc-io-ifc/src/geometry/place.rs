//! Points, directions, axis placements, local placement chains and cartesian transformation
//! operators (CORE-IO-015).
//!
//! Attribute positions follow the IFC2X3, IFC4 and IFC4X3 schemas:
//! - `IfcCartesianPoint` `Coordinates` 0; `IfcDirection` `DirectionRatios` 0.
//! - `IfcAxis2Placement3D` `Location` 0, `Axis` 1, `RefDirection` 2; an absent `Axis` is (0, 0, 1)
//!   and an absent `RefDirection` is the first of (1, 0, 0) and (0, 1, 0) not parallel to `Axis`.
//!   The reference direction is orthogonalized against the axis, and the third axis is their cross
//!   product.
//! - `IfcAxis2Placement2D` `Location` 0, `RefDirection` 1; an absent direction is (1, 0).
//! - `IfcLocalPlacement` `PlacementRelTo` 0 and `RelativePlacement` 1. A chain is walked iteratively
//!   with a visited set, bounded by the depth limit; a repeated placement is cut and reported as
//!   `io.ifc.cycle_cut`, and the cut placement is taken relative to the world.
//! - `IfcCartesianTransformationOperator3D` `Axis1` 0, `Axis2` 1, `LocalOrigin` 2, `Scale` 3, `Axis3`
//!   4, and for `...3DnonUniform` `Scale2` 5 and `Scale3` 6, which default to `Scale`. The third axis
//!   is `Axis3` (default (0, 0, 1)); the first is `Axis1` (default (1, 0, 0)) made orthogonal to the
//!   third; the second is `Axis2` (default: third crossed with first) made orthogonal to the third and
//!   the first. An axis parallel to one it must be orthogonal to is replaced by a perpendicular
//!   direction and reported as approximated. A scale that is not positive skips the item: a mirror is
//!   expressed through the axes.
//!
//! Every length is multiplied by the length scale of the file when it is read.

use std::collections::BTreeSet;

use arc_base::math::{Matrix4, Point3, Transform3, Vector3};

use crate::error::{limit, reasons};
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, skip};
use super::{Evaluator, Flow, Step};

/// The frame with the given origin and the axes `x` and `z`; `y` is `z` crossed with `x`.
fn frame(origin: Point3, x: Vector3, z: Vector3) -> Transform3 {
    let y = z.cross(&x);
    Transform3::from_matrix_unchecked(Matrix4::new(
        x.x, y.x, z.x, origin.x, //
        x.y, y.y, z.y, origin.y, //
        x.z, y.z, z.z, origin.z, //
        0.0, 0.0, 0.0, 1.0,
    ))
}

/// The unit vector of `v`; a zero or non-finite direction is a skip.
pub(crate) fn unit(v: Vector3, kind: &str) -> Step<Vector3> {
    let norm = v.norm();
    if norm > 0.0 && norm.is_finite() {
        Ok(v / norm)
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
    }
}

/// The unit vector of `v` made orthogonal to the unit vector `z`; `None` when `v` is parallel to `z`
/// within the tolerance.
fn orthogonal_to(v: Vector3, z: Vector3, tolerance: f64) -> Option<Vector3> {
    let x = v - z * v.dot(&z);
    let norm = x.norm();
    (norm > tolerance).then(|| x / norm)
}

/// A unit direction orthogonal to the unit vector `z`, chosen deterministically from the axes.
fn perpendicular(z: Vector3, tolerance: f64, kind: &str) -> Step<Vector3> {
    orthogonal_to(Vector3::x(), z, tolerance)
        .or_else(|| orthogonal_to(Vector3::y(), z, tolerance))
        .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))
}

/// The first of the candidate references that is not parallel to `z`, orthogonalized against it.
fn orthogonal_x(
    z: Vector3,
    reference: Option<Vector3>,
    tolerance: f64,
    kind: &str,
) -> Step<Vector3> {
    let candidates = match reference {
        Some(reference) => vec![reference],
        None => vec![Vector3::x(), Vector3::y()],
    };
    for candidate in candidates {
        let x = candidate - z * candidate.dot(&z);
        let norm = x.norm();
        if norm > tolerance {
            return Ok(x / norm);
        }
    }
    Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
}

/// The record of an instance of the given type. A missing instance is a missing target; an instance
/// of another type is not evaluated.
pub(crate) fn expect<'a>(
    file: &'a crate::model::IfcFile,
    name: u64,
    kind: &str,
) -> Step<&'a Record> {
    let Some(instance) = file.instance(name) else {
        return Err(skip(reasons::TARGET_MISSING, kind));
    };
    let Some(record) = instance.records.first() else {
        return Err(skip(reasons::TARGET_MISSING, kind));
    };
    if record.type_name != kind {
        return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
    }
    Ok(record)
}

impl Evaluator<'_> {
    /// The 3D point of an `IfcCartesianPoint`, in meters.
    pub(crate) fn point(&self, name: u64) -> Step<Point3> {
        let kind = "IFCCARTESIANPOINT";
        let record = expect(self.file, name, kind)?;
        attr::point3(
            attr::list(record.parameters.first(), kind)?,
            self.scale,
            kind,
        )
    }

    /// The direction of an `IfcDirection`, not normalized.
    pub(crate) fn direction(&self, name: u64) -> Step<Vector3> {
        let kind = "IFCDIRECTION";
        let record = expect(self.file, name, kind)?;
        attr::coordinates3(attr::list(record.parameters.first(), kind)?, 1.0, kind)
    }

    /// A unit direction of a reference, or the default when the reference is absent.
    fn unit_direction(
        &self,
        reference: Option<u64>,
        default: Vector3,
        kind: &str,
    ) -> Step<Vector3> {
        match reference {
            Some(name) => unit(self.direction(name)?, kind),
            None => Ok(default),
        }
    }

    /// The transform of an `IfcAxis2Placement3D`; an absent placement is the identity.
    pub(crate) fn axis3(&self, name: Option<u64>) -> Step<Transform3> {
        let Some(name) = name else {
            return Ok(Transform3::identity());
        };
        let kind = "IFCAXIS2PLACEMENT3D";
        let record = expect(self.file, name, kind)?;
        let origin = match attr::opt_reference(record.parameters.first(), kind)? {
            Some(point) => self.point(point)?,
            None => Point3::origin(),
        };
        let reference = attr::opt_reference(record.parameters.get(2), kind)?;
        let z = self.unit_direction(
            attr::opt_reference(record.parameters.get(1), kind)?,
            Vector3::z(),
            kind,
        )?;
        let reference = match reference {
            Some(name) => Some(unit(self.direction(name)?, kind)?),
            None => None,
        };
        let x = orthogonal_x(z, reference, self.tol.angular, kind)?;
        Ok(frame(origin, x, z))
    }

    /// The transform of an `IfcAxis2Placement2D`, placed in the plane `z = 0`; an absent placement is
    /// the identity.
    pub(crate) fn axis2(&self, name: Option<u64>) -> Step<Transform3> {
        let Some(name) = name else {
            return Ok(Transform3::identity());
        };
        let kind = "IFCAXIS2PLACEMENT2D";
        let record = expect(self.file, name, kind)?;
        let origin = match attr::opt_reference(record.parameters.first(), kind)? {
            Some(point) => self.point(point)?,
            None => Point3::origin(),
        };
        let x = match attr::opt_reference(record.parameters.get(1), kind)? {
            Some(direction) => {
                let record = expect(self.file, direction, "IFCDIRECTION")?;
                let planar =
                    attr::coordinates2(attr::list(record.parameters.first(), kind)?, 1.0, kind)?;
                unit(Vector3::new(planar.x, planar.y, 0.0), kind)?
            }
            None => Vector3::x(),
        };
        Ok(frame(origin, x, Vector3::z()))
    }

    /// The transform of an axis placement of either dimension.
    pub(crate) fn axis_any(&self, name: Option<u64>) -> Step<Transform3> {
        match name {
            Some(name)
                if self
                    .file
                    .instance(name)
                    .and_then(|i| i.records.first())
                    .is_some_and(|r| r.type_name == "IFCAXIS2PLACEMENT2D") =>
            {
                self.axis2(Some(name))
            }
            other => self.axis3(other),
        }
    }

    /// The world transform of a local placement, memoized: the chain is walked up to a placement
    /// already known, then composed from the top down.
    pub(crate) fn local_placement(&mut self, name: u64) -> Step<Transform3> {
        if let Some(known) = self.placement_memo(name) {
            return Ok(known);
        }
        let file = self.file;
        let kind = "IFCLOCALPLACEMENT";
        let limit_depth = usize::try_from(self.depth_limit).unwrap_or(usize::MAX);
        let mut chain: Vec<(u64, Transform3)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut base = Transform3::identity();
        let mut current = Some(name);
        while let Some(node) = current {
            if let Some(known) = self.placement_memo(node) {
                base = known;
                break;
            }
            if !seen.insert(node) {
                self.log
                    .add(reasons::CYCLE_CUT, kind, Some(Source::Decimal(node)), 1);
                break;
            }
            if chain.len() >= limit_depth {
                return Err(Flow::Hard(limit(
                    "nesting_depth",
                    u64::try_from(chain.len() + 1).unwrap_or(u64::MAX),
                )));
            }
            let record = expect(file, node, kind)?;
            let relative = self.axis_any(attr::opt_reference(record.parameters.get(1), kind)?)?;
            current = attr::opt_reference(record.parameters.first(), kind)?;
            self.charge_placement()?;
            chain.push((node, relative));
        }
        let mut world = base;
        for (node, relative) in chain.iter().rev() {
            world *= *relative;
            self.store_placement(*node, world);
        }
        Ok(world)
    }

    /// The transform of an `IfcCartesianTransformationOperator3D` or its non-uniform form, and whether
    /// an axis was replaced by a perpendicular direction (approximated).
    pub(crate) fn operator(&self, name: u64) -> Step<(Transform3, bool)> {
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Err(skip(
                reasons::TARGET_MISSING,
                "IFCCARTESIANTRANSFORMATIONOPERATOR3D",
            ));
        };
        let Some(record) = instance.records.first() else {
            return Err(skip(
                reasons::TARGET_MISSING,
                "IFCCARTESIANTRANSFORMATIONOPERATOR3D",
            ));
        };
        let kind = record.type_name.as_str();
        let non_uniform = match kind {
            "IFCCARTESIANTRANSFORMATIONOPERATOR3D" => false,
            "IFCCARTESIANTRANSFORMATIONOPERATOR3DNONUNIFORM" => true,
            _ => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind)),
        };
        let parameter = |i: usize| record.parameters.get(i);
        let tolerance = self.tol.angular;
        let axis3 =
            self.unit_direction(attr::opt_reference(parameter(4), kind)?, Vector3::z(), kind)?;
        let mut approximated = false;
        let axis1 = match attr::opt_reference(parameter(0), kind)? {
            Some(name) => unit(self.direction(name)?, kind)?,
            None => Vector3::x(),
        };
        let axis1 = match orthogonal_to(axis1, axis3, tolerance) {
            Some(v) => v,
            None => {
                approximated = true;
                perpendicular(axis3, tolerance, kind)?
            }
        };
        let axis2 = match attr::opt_reference(parameter(1), kind)? {
            Some(name) => {
                let given = unit(self.direction(name)?, kind)?;
                let v = given - axis3 * given.dot(&axis3) - axis1 * given.dot(&axis1);
                match v.norm() {
                    norm if norm > tolerance => v / norm,
                    _ => {
                        approximated = true;
                        axis3.cross(&axis1)
                    }
                }
            }
            None => axis3.cross(&axis1),
        };
        let origin = match attr::opt_reference(parameter(2), kind)? {
            Some(point) => self.point(point)?,
            None => Point3::origin(),
        };
        let scale = attr::opt_real(parameter(3), 1.0, kind)?.unwrap_or(1.0);
        let (scale2, scale3) = if non_uniform {
            (
                attr::opt_real(parameter(5), 1.0, kind)?.unwrap_or(scale),
                attr::opt_real(parameter(6), 1.0, kind)?.unwrap_or(scale),
            )
        } else {
            (scale, scale)
        };
        if !(scale > 0.0 && scale2 > 0.0 && scale3 > 0.0) {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let (a, b, c) = (axis1 * scale, axis2 * scale2, axis3 * scale3);
        let transform = Transform3::from_matrix_unchecked(Matrix4::new(
            a.x, b.x, c.x, origin.x, //
            a.y, b.y, c.y, origin.y, //
            a.z, b.z, c.z, origin.z, //
            0.0, 0.0, 0.0, 1.0,
        ));
        Ok((transform, approximated))
    }
}
