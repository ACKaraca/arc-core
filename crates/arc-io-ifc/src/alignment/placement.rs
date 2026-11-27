//! `IfcLinearPlacement`, `IfcAxis2PlacementLinear` and `IfcPointByDistanceExpression`: placements
//! measured along a curve, resolved to world transforms in meters.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_base::math::{Matrix4, Transform3, Vector3};
use arc_base::{Cancel, Limits, Result, Tolerance};

use super::curve::{Curve, Pose};
use super::read::{Reader, invalid, number};
use crate::geometry::Units;
use crate::model::IfcFile;

/// Resolves placements that are measured along curves. Curves are read once and shared, so a file
/// with thousands of placements along one alignment evaluates its curve definition once.
///
/// This is the hook for code that needs the world position of an element placed with an
/// `IfcLinearPlacement`: build one resolver per file, then call [`Self::placement`] with the
/// instance name of the object placement.
pub struct LinearResolver<'f> {
    reader: Reader<'f>,
    curves: BTreeMap<u64, Arc<Curve>>,
    tolerance: f64,
    depth_limit: u32,
}

impl IfcFile {
    /// A resolver for the linear placements of this file.
    ///
    /// # Errors
    ///
    /// `base.cancelled` when the token was cancelled.
    pub fn linear_resolver(
        &self,
        limits: &Limits,
        cancel: &Cancel,
        tolerance: &Tolerance,
    ) -> Result<LinearResolver<'_>> {
        let units = Units::of(self, limits, cancel)?;
        Ok(LinearResolver {
            reader: Reader::new(
                self,
                limits,
                cancel,
                units.meters_per_length_unit,
                units.radians_per_angle_unit,
            ),
            curves: BTreeMap::new(),
            tolerance: tolerance.linear,
            depth_limit: limits.nesting_depth.min(crate::step::MAX_NESTING),
        })
    }
}

impl LinearResolver<'_> {
    /// The curve an alignment, composite, gradient or segmented reference curve defines. An
    /// alignment gives its axis representation, or its layouts when it has none.
    ///
    /// # Errors
    ///
    /// `io.ifc.curve_invalid` or `io.ifc.curve_unsupported` when the curve cannot be built.
    pub fn curve(&mut self, name: u64) -> Result<Arc<Curve>> {
        if let Some(curve) = self.curves.get(&name) {
            return Ok(Arc::clone(curve));
        }
        let curve = Arc::new(self.reader.any_curve(name)?);
        self.curves.insert(name, Arc::clone(&curve));
        Ok(curve)
    }

    /// The pose of an `IfcPointByDistanceExpression`: the point at its distance along the basis
    /// curve moved by the offsets, with the frame of the curve there.
    ///
    /// # Errors
    ///
    /// `io.ifc.curve_*` for a curve that cannot be built, `io.ifc.distance_out_of_range` for a
    /// distance outside the curve.
    pub fn point(&mut self, name: u64) -> Result<Pose> {
        let record = self
            .reader
            .record(name, &["IFCPOINTBYDISTANCEEXPRESSION"])?;
        let along = self.reader.req(record, "DistanceAlong")?;
        let along = number(along).ok_or_else(invalid)?;
        let offset = |attr: &str| -> Result<f64> {
            Ok(self.reader.opt_real(record, attr)?.unwrap_or(0.0) * self.reader.scale)
        };
        let (lateral, vertical, longitudinal) = (
            offset("OffsetLateral")?,
            offset("OffsetVertical")?,
            offset("OffsetLongitudinal")?,
        );
        let basis = self.reader.reference(record, "BasisCurve")?;
        let curve = self.curve(basis)?;
        let tol = self.tolerance.max(1e-9);
        let mut pose = curve.evaluate_native(along, tol / self.reader.scale)?;
        let m = pose.transform.matrix();
        let column = |i: usize| Vector3::new(m[(0, i)], m[(1, i)], m[(2, i)]);
        let moved = pose.position.coords
            + column(1) * lateral
            + column(2) * vertical
            + column(0) * longitudinal;
        pose.position = moved.into();
        let mut matrix = *m;
        for row in 0..3 {
            matrix[(row, 3)] = moved[row];
        }
        pose.transform = Transform3::from_matrix_unchecked(matrix);
        Ok(pose)
    }

    /// The transform of an `IfcLinearPlacement` to world coordinates, including its
    /// `PlacementRelTo` chain.
    ///
    /// # Errors
    ///
    /// As [`Self::point`]; `io.ifc.curve_invalid` for a malformed or cyclic placement chain.
    pub fn placement(&mut self, name: u64) -> Result<Transform3> {
        self.object_placement(name, &mut BTreeSet::new())
    }

    /// As [`Self::placement`], but when the placement cannot be resolved along its curve and has
    /// a `CartesianPosition`, that fallback transform (relative to `PlacementRelTo`) is used.
    ///
    /// # Errors
    ///
    /// The error of [`Self::placement`] when there is no fallback.
    pub fn placement_with_fallback(&mut self, name: u64) -> Result<Transform3> {
        match self.placement(name) {
            Ok(t) => Ok(t),
            Err(error) if error.code() == arc_base::error::codes::CANCELLED.code => Err(error),
            Err(error) => {
                let record = self.reader.record(name, &["IFCLINEARPLACEMENT"])?;
                match self.reader.opt_reference(record, "CartesianPosition")? {
                    Some(position) => {
                        let local = self.reader.frame(position)?;
                        let parent = match self.reader.opt_reference(record, "PlacementRelTo")? {
                            Some(rel) => self.object_placement(rel, &mut BTreeSet::new())?,
                            None => Transform3::identity(),
                        };
                        Ok(Transform3::from_matrix_unchecked(
                            parent.matrix() * local.matrix(),
                        ))
                    }
                    None => Err(error),
                }
            }
        }
    }

    fn object_placement(&mut self, name: u64, seen: &mut BTreeSet<u64>) -> Result<Transform3> {
        self.reader.cancel.check()?;
        if !seen.insert(name) || u32::try_from(seen.len()).unwrap_or(u32::MAX) > self.depth_limit {
            return Err(invalid());
        }
        let record = self
            .reader
            .record(name, &["IFCLOCALPLACEMENT", "IFCLINEARPLACEMENT"])?;
        let parent = match self.reader.opt_reference(record, "PlacementRelTo")? {
            Some(rel) => self.object_placement(rel, seen)?,
            None => Transform3::identity(),
        };
        let relative = self.reader.reference(record, "RelativePlacement")?;
        let local = if record.type_name == "IFCLOCALPLACEMENT" {
            self.reader.frame(relative)?
        } else {
            self.axis2_linear(relative)?
        };
        Ok(Transform3::from_matrix_unchecked(
            parent.matrix() * local.matrix(),
        ))
    }

    /// The frame of an `IfcAxis2PlacementLinear` in the coordinates the curve is placed in.
    fn axis2_linear(&mut self, name: u64) -> Result<Transform3> {
        let record = self.reader.record(name, &["IFCAXIS2PLACEMENTLINEAR"])?;
        let pose = self.point(self.reader.reference(record, "Location")?)?;
        let m = pose.transform.matrix();
        let (x, y, z) = (
            Vector3::new(m[(0, 0)], m[(1, 0)], m[(2, 0)]),
            Vector3::new(m[(0, 1)], m[(1, 1)], m[(2, 1)]),
            Vector3::new(m[(0, 2)], m[(1, 2)], m[(2, 2)]),
        );
        let in_frame = |r: u64| -> Result<Vector3> {
            let d = self.reader.coordinates(r)?;
            Ok(x * d.first().copied().unwrap_or(0.0)
                + y * d.get(1).copied().unwrap_or(0.0)
                + z * d.get(2).copied().unwrap_or(0.0))
        };
        let axis = match self.reader.opt_reference(record, "Axis")? {
            Some(r) => in_frame(r)?,
            None => z,
        };
        let reference = match self.reader.opt_reference(record, "RefDirection")? {
            Some(r) => in_frame(r)?,
            None => x,
        };
        let axis = axis.try_normalize(1e-12).ok_or_else(invalid)?;
        let ex = (reference - axis * reference.dot(&axis))
            .try_normalize(1e-12)
            .ok_or_else(invalid)?;
        let ey = axis.cross(&ex);
        let origin = pose.position;
        Ok(Transform3::from_matrix_unchecked(Matrix4::new(
            ex.x, ey.x, axis.x, origin.x, //
            ex.y, ey.y, axis.y, origin.y, //
            ex.z, ey.z, axis.z, origin.z, //
            0.0, 0.0, 0.0, 1.0,
        )))
    }
}

impl<'f> Reader<'f> {
    /// The transform of an `IfcAxis2Placement2D` or `IfcAxis2Placement3D`, lengths in meters.
    pub(crate) fn frame(&self, name: u64) -> Result<Transform3> {
        let record = self.record(name, &["IFCAXIS2PLACEMENT2D", "IFCAXIS2PLACEMENT3D"])?;
        let direction = |attr: &str| -> Result<Option<Vector3>> {
            match self.opt_reference(record, attr)? {
                Some(r) => {
                    let d = self.coordinates(r)?;
                    Ok(Some(Vector3::new(
                        d.first().copied().unwrap_or(0.0),
                        d.get(1).copied().unwrap_or(0.0),
                        d.get(2).copied().unwrap_or(0.0),
                    )))
                }
                None => Ok(None),
            }
        };
        let (location, _, _) = self.placement(name)?;
        let z = direction("Axis")?
            .and_then(|v| v.try_normalize(1e-12))
            .unwrap_or_else(Vector3::z);
        let x = match direction("RefDirection")? {
            Some(r) => (r - z * r.dot(&z))
                .try_normalize(1e-12)
                .ok_or_else(invalid)?,
            None => {
                let candidate = if z.x.abs() < 0.9 {
                    Vector3::x()
                } else {
                    Vector3::y()
                };
                (candidate - z * candidate.dot(&z))
                    .try_normalize(1e-12)
                    .ok_or_else(invalid)?
            }
        };
        let y = z.cross(&x);
        let s = self.scale;
        Ok(Transform3::from_matrix_unchecked(Matrix4::new(
            x.x,
            y.x,
            z.x,
            location[0] * s, //
            x.y,
            y.y,
            z.y,
            location[1] * s, //
            x.z,
            y.z,
            z.z,
            location[2] * s, //
            0.0,
            0.0,
            0.0,
            1.0,
        )))
    }
}
