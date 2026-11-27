//! Offset curves and geometric sets (CORE-IO-015).
//!
//! - `IfcOffsetCurve2D`: the basis curve moved by `Distance` to the left of its direction, in the
//!   plane `z = 0`. `IfcOffsetCurve3D`: the same along `RefDirection x tangent` (the left of the
//!   curve when the reference direction is up; the default is the z axis). A tangent parallel to the
//!   reference direction is not evaluated. Both offset the vertices of the basis polyline along the
//!   mitered normal of the two segments that meet there (the miter is limited to the average normal
//!   of a turn above 154 degrees); `SelfIntersect` is not applied.
//! - `IfcOffsetCurveByDistances`: each point of the basis polyline moves laterally (to the left in
//!   the plane `z = 0`) by `OffsetLateral` and vertically by `OffsetVertical`, interpolated linearly
//!   over the `DistanceAlong` of the point-by-distance expressions (the nearest value before the first and after
//!   the last), with the distance measured along the basis polyline; a non-zero `OffsetLongitudinal` is not applied
//!   (reported as approximated). The entity is `IfcPointByDistanceExpression` of IFC4X3.
//! - `IfcGeometricSet` and `IfcGeometricCurveSet`: the curves and points of `Elements` as one shape of
//!   polylines, never a body. A point (here and as a representation item of its own) is a polyline of two
//!   equal points. A surface element is not
//!   evaluated and is reported with its type.

use arc_base::math::{Point3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::{Evaluator, Piece, Shape, Step};

/// The least `1 + cos` of a turn that is mitered.
const MIN_MITER: f64 = 0.1;

fn unit(v: Vector3) -> Option<Vector3> {
    let n = v.norm();
    (n > 1e-12 && n.is_finite()).then(|| v / n)
}

/// The tangents of a polyline at its vertices: those of the segments, averaged at joins (a closed
/// curve joins its last segment to its first).
fn normals(points: &[Point3], reference: Vector3) -> Option<Vec<Vector3>> {
    let closed = points.len() > 2 && points.first() == points.last();
    let segments: Vec<Vector3> = points
        .windows(2)
        .filter_map(|w| unit(w[1] - w[0]))
        .collect();
    if segments.len() + 1 != points.len() {
        return None;
    }
    let left = |t: Vector3| unit(reference.cross(&t));
    let mut out = Vec::with_capacity(points.len());
    for i in 0..points.len() {
        let before = if i > 0 {
            segments.get(i - 1)
        } else if closed {
            segments.last()
        } else {
            None
        };
        let after = segments
            .get(i)
            .or(if closed { segments.first() } else { None });
        let normal = match (before, after) {
            (Some(a), Some(b)) => {
                let (na, nb) = (left(*a)?, left(*b)?);
                let c = 1.0 + na.dot(&nb);
                if c > MIN_MITER {
                    (na + nb) / c
                } else {
                    unit(na + nb).unwrap_or(na)
                }
            }
            (Some(t), None) | (None, Some(t)) => left(*t)?,
            (None, None) => return None,
        };
        out.push(normal);
    }
    Some(out)
}

impl Evaluator<'_> {
    /// The points of an offset curve.
    pub(crate) fn offset_curve(
        &mut self,
        record: &Record,
        max: usize,
        depth: u32,
    ) -> Step<Vec<Point3>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let basis = attr::reference(named(file, record, "BasisCurve", 0), kind)?;
        let points = self.curve_by_name(basis, max, depth + 1)?;
        let degenerate = || skip(reasons::GEOMETRY_DEGENERATE, kind);
        let mut out = Vec::with_capacity(points.len());
        if kind == "IFCOFFSETCURVEBYDISTANCES" {
            let list = attr::ordered_references(
                &mut self.budget,
                named(file, record, "OffsetValues", 1),
                kind,
                usize::MAX,
            )?;
            let mut rows: Vec<(f64, f64, f64)> = Vec::with_capacity(list.len());
            let mut ignored = false;
            for name in list {
                let row = super::place::expect(file, name, "IFCPOINTBYDISTANCEEXPRESSION")?;
                let value = |this: &Self, attribute: &str, index: usize| -> Step<f64> {
                    Ok(
                        attr::opt_real(named(file, row, attribute, index), this.scale, kind)?
                            .unwrap_or(0.0),
                    )
                };
                let along = value(self, "DistanceAlong", 0)?;
                let lateral = value(self, "OffsetLateral", 1)?;
                let vertical = value(self, "OffsetVertical", 2)?;
                ignored |= value(self, "OffsetLongitudinal", 3)? != 0.0;
                rows.push((along, lateral, vertical));
            }
            if rows.is_empty() {
                return Err(degenerate());
            }
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            if ignored {
                self.log.add(reasons::GEOMETRY_APPROXIMATED, kind, None, 1);
            }
            let normals = normals(&points, Vector3::z()).ok_or_else(degenerate)?;
            let mut along = 0.0;
            for (i, p) in points.iter().enumerate() {
                if i > 0 {
                    along += (p - points[i - 1]).norm();
                }
                let (lateral, vertical) = interpolate(&rows, along);
                let n = normals.get(i).copied().unwrap_or_else(Vector3::zeros);
                out.push(p + n * lateral + Vector3::z() * vertical);
            }
            return Ok(out);
        }
        let distance = attr::real(named(file, record, "Distance", 1), self.scale, kind)?;
        let reference = if kind == "IFCOFFSETCURVE3D" {
            let direction = attr::reference(named(file, record, "RefDirection", 3), kind)?;
            super::place::unit(self.direction(direction)?, kind)?
        } else {
            Vector3::z()
        };
        let normals = normals(&points, reference).ok_or_else(degenerate)?;
        let planar = kind == "IFCOFFSETCURVE2D";
        for (p, n) in points.iter().zip(&normals) {
            let q = p + n * distance;
            out.push(if planar {
                Point3::new(q.x, q.y, 0.0)
            } else {
                q
            });
        }
        Ok(out)
    }

    /// The pieces of an `IfcGeometricSet` or `IfcGeometricCurveSet`.
    pub(crate) fn geometric_set(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let elements = attr::distinct_references(
            &mut self.budget,
            named(self.file, record, "Elements", 0),
            kind,
            usize::MAX,
        )?
        .0;
        let mut lines: Vec<Vec<Point3>> = Vec::new();
        for element in elements {
            self.budget.step()?;
            let Some(target) = self.file.instance(element).and_then(|i| i.records.first()) else {
                self.log.add(
                    reasons::TARGET_MISSING,
                    kind,
                    Some(Source::Decimal(name)),
                    1,
                );
                continue;
            };
            if target.type_name == "IFCCARTESIANPOINT" {
                let p = self.point(element)?;
                lines.push(vec![p, p]);
                continue;
            }
            let found = self.curve_by_name(element, super::MAX_LOOP_VERTICES * 4, 0);
            match found {
                Ok(points) if points.len() >= 2 => lines.push(points),
                Ok(_) => {}
                Err(super::Flow::Skip(s)) => {
                    self.log
                        .add(s.reason, &s.kind, Some(Source::Decimal(element)), 1);
                }
                Err(hard) => return Err(hard),
            }
        }
        if lines.is_empty() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        self.charge_shape()?;
        Ok(Self::own(Shape::Polylines(lines)))
    }
}

/// The lateral and vertical offsets at `along`, interpolated linearly between rows sorted by
/// distance.
fn interpolate(rows: &[(f64, f64, f64)], along: f64) -> (f64, f64) {
    let Some(first) = rows.first() else {
        return (0.0, 0.0);
    };
    if along <= first.0 {
        return (first.1, first.2);
    }
    for w in rows.windows(2) {
        if along <= w[1].0 {
            let span = w[1].0 - w[0].0;
            let t = if span > 0.0 {
                (along - w[0].0) / span
            } else {
                1.0
            };
            return (
                w[0].1 + (w[1].1 - w[0].1) * t,
                w[0].2 + (w[1].2 - w[0].2) * t,
            );
        }
    }
    rows.last().map_or((0.0, 0.0), |r| (r.1, r.2))
}
