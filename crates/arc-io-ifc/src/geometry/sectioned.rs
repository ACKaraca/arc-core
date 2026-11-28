//! Sectioned solids (CORE-IO-015): `IfcSectionedSpine` (IFC4) and `IfcSectionedSolidHorizontal`
//! (IFC4X3), lofts of cross sections.
//!
//! - `IfcSectionedSpine`: each `CrossSections` profile placed by the `IfcAxis2Placement3D` of the same
//!   index in `CrossSectionPositions`. The `SpineCurve` only relates the sections and is not used.
//! - `IfcSectionedSolidHorizontal`: each profile placed at the `Location` (an
//!   `IfcPointByDistanceExpression`) of its `IfcAxis2PlacementLinear`: the point of the directrix
//!   polyline at `DistanceAlong` (the ends for a distance outside it), moved by `OffsetLateral` to the
//!   left in the plane `z = 0` and by `OffsetVertical` upward. The local z axis is the `Axis` of the
//!   placement, or else the tangent of the directrix at that point; the local x axis is `RefDirection`
//!   (made perpendicular to z), or else the horizontal direction to the left of the tangent, and the
//!   local y axis is z cross x. A non-zero `OffsetLongitudinal` is not applied. The result is reported
//!   as approximated because the default orientation of the profile is an interpretation.
//!
//! Consecutive sections are joined by one quad per edge of their loops, so every section must have the
//! same number of loops and of points per loop as the first; the first and last sections are the caps.
//! Sections with different counts are not evaluated.

use arc_base::math::{Point3, Transform3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::place::{expect, unit};
use super::solid::MeshAcc;
use super::{Evaluator, MAX_LOOP_VERTICES, Piece, Step};

/// The most cross sections of a solid.
const MAX_SECTIONS: usize = 4096;

impl Evaluator<'_> {
    /// The pieces of an `IfcSectionedSpine` or `IfcSectionedSolidHorizontal`.
    pub(crate) fn sectioned(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let sections = attr::ordered_references(
            &mut self.budget,
            named(file, record, "CrossSections", 1),
            kind,
            MAX_SECTIONS,
        )?;
        let positions = attr::ordered_references(
            &mut self.budget,
            named(file, record, "CrossSectionPositions", 2),
            kind,
            MAX_SECTIONS,
        )?;
        if sections.len() < 2 || sections.len() != positions.len() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let mut frames: Vec<Transform3> = Vec::with_capacity(positions.len());
        if kind == "IFCSECTIONEDSPINE" {
            for p in positions {
                self.budget.step()?;
                frames.push(self.axis3(Some(p))?);
            }
        } else {
            let directrix = attr::reference(named(file, record, "Directrix", 0), kind)?;
            let path = self.curve_points(directrix, MAX_LOOP_VERTICES)?;
            for p in positions {
                self.budget.step()?;
                frames.push(self.linear_frame(&path, p)?);
            }
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        let mut loops: Vec<Vec<Vec<[f64; 2]>>> = Vec::with_capacity(sections.len());
        for profile in sections {
            self.budget.step()?;
            let mut parts = self.profile(profile)?;
            let (Some(first), true) = (parts.pop(), parts.is_empty()) else {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            };
            loops.push(first);
        }
        let shape = |l: &Vec<Vec<[f64; 2]>>| l.iter().map(Vec::len).collect::<Vec<_>>();
        let first_shape = loops.first().map(shape).unwrap_or_default();
        if loops.iter().any(|l| shape(l) != first_shape) {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        let first = loops.remove(0);
        let part = self.prepare_part(first.clone(), kind)?;
        let count = loops.len() + 1;
        self.budget.array(count.saturating_mul(part.points.len()))?;
        let mut points: Vec<Point3> = Vec::with_capacity(count * part.points.len());
        for (frame, l) in frames
            .iter()
            .zip(std::iter::once(&first).chain(loops.iter()))
        {
            for p in l.iter().flatten() {
                points.push(frame.transform_point(&Point3::new(p[0], p[1], 0.0)));
            }
        }
        let mut acc = MeshAcc::default();
        self.ring_part(&mut acc, &part, points, count, false, &[])?;
        self.finish_mesh(acc)
    }

    /// The placement of a cross section of a horizontal sectioned solid.
    fn linear_frame(&mut self, path: &[Point3], name: u64) -> Step<Transform3> {
        let kind = "IFCAXIS2PLACEMENTLINEAR";
        let file = self.file;
        let placement = expect(file, name, kind)?;
        let location = attr::reference(named(file, placement, "Location", 0), kind)?;
        let location = expect(file, location, "IFCPOINTBYDISTANCEEXPRESSION")?;
        let value = |this: &Self, attribute: &str, index: usize| -> Step<f64> {
            Ok(
                attr::opt_real(named(file, location, attribute, index), this.scale, kind)?
                    .unwrap_or(0.0),
            )
        };
        let (along, lateral, vertical) = (
            value(self, "DistanceAlong", 0)?,
            value(self, "OffsetLateral", 1)?,
            value(self, "OffsetVertical", 2)?,
        );
        let (point, tangent) = at_distance(path, along)
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, "IFCSECTIONEDSOLIDHORIZONTAL"))?;
        let left = unit(Vector3::z().cross(&tangent), kind).unwrap_or_else(|_| Vector3::y());
        let origin = point + left * lateral + Vector3::z() * vertical;
        let z = match attr::opt_reference(named(file, placement, "Axis", 1), kind)? {
            Some(d) => unit(self.direction(d)?, kind)?,
            None => tangent,
        };
        let x_hint = match attr::opt_reference(named(file, placement, "RefDirection", 2), kind)? {
            Some(d) => self.direction(d)?,
            None => left,
        };
        let x = unit(x_hint - z * x_hint.dot(&z), kind)?;
        let y = z.cross(&x);
        let matrix = arc_base::math::Matrix4::new(
            x.x, y.x, z.x, origin.x, //
            x.y, y.y, z.y, origin.y, //
            x.z, y.z, z.z, origin.z, //
            0.0, 0.0, 0.0, 1.0,
        );
        Ok(Transform3::from_matrix_unchecked(matrix))
    }
}

/// The point and the unit tangent of the polyline at the distance `along` from its start (clamped to
/// its ends).
fn at_distance(path: &[Point3], along: f64) -> Option<(Point3, Vector3)> {
    let mut walked = 0.0;
    for w in path.windows(2) {
        let length = (w[1] - w[0]).norm();
        if length <= 0.0 {
            continue;
        }
        if along <= walked + length {
            let t = ((along - walked) / length).clamp(0.0, 1.0);
            return Some((w[0] + (w[1] - w[0]) * t, (w[1] - w[0]) / length));
        }
        walked += length;
    }
    let (a, b) = (path.get(path.len().checked_sub(2)?)?, path.last()?);
    let d = b - a;
    (d.norm() > 0.0).then(|| (*b, d / d.norm()))
}
