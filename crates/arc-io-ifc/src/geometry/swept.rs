//! Swept solids of revolution and of sweeps along a curve (CORE-IO-015).
//!
//! - `IfcRevolvedAreaSolid`: `SweptArea` 0, `Position` 1, `Axis` 2 (an `IfcAxis1Placement` in the
//!   coordinates of the position, in the plane of the profile) and `Angle` 3 (plane angle; a negative
//!   angle turns the other way; a magnitude of a full turn or more closes the solid without caps). The
//!   profile is turned about the axis in steps whose count keeps the chord deviation of the outermost
//!   profile point within the chord tolerance, at most 512 per turn. A profile
//!   point on the axis is one vertex. The tapered form (`...Tapered`) is not evaluated.
//! - `IfcSweptDiskSolid` and `IfcSweptDiskSolidPolygonal`: a disk of `Radius`, hollow when
//!   `InnerRadius` is given, along the `Directrix` discretized by [`Evaluator::curve_points`]. The disk
//!   is a regular polygon of the chord tolerance. `StartParam` and `EndParam` are not evaluated (the
//!   item is skipped when either is given), and a `FilletRadius` of the polygonal form is not applied
//!   (reported as approximated).
//! - `IfcSurfaceCurveSweptAreaSolid` and `IfcFixedReferenceSweptAreaSolid`: the profile in the plane
//!   of `Position` placed at the start of the directrix, kept perpendicular to the directrix polyline.
//!   `StartParam`/`EndParam` as above.
//!
//! A sweep is the polyline of the directrix with each interior vertex mitered: the profile at a vertex
//! lies in the bisecting plane of the two segments, so each segment is a straight prism and neighbors
//! meet on the plane. The orientation of the profile is carried along by reflecting it in each
//! bisecting plane, which turns it by the least rotation. A turn of more than about 154 degrees at a
//! vertex is not evaluated.

use std::f64::consts::TAU;

use arc_base::math::{Point3, Transform3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, named, skip};
use super::curves::arc_segments;
use super::place::{expect, unit};
use super::polygon::P2;
use super::solid::{MeshAcc, Prepared};
use super::{Evaluator, MAX_LOOP_VERTICES, Piece, Step};

/// The least cosine between a segment and the bisecting plane's normal at a vertex.
const MIN_MITER_COSINE: f64 = 0.1;

impl Evaluator<'_> {
    /// The pieces of a revolved area solid.
    pub(crate) fn revolution(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = "IFCREVOLVEDAREASOLID";
        let file = self.file;
        let profile = attr::reference(named(file, record, "SweptArea", 0), kind)?;
        let position = self.axis3(attr::opt_reference(
            named(file, record, "Position", 1),
            kind,
        )?)?;
        let axis = attr::reference(named(file, record, "Axis", 2), kind)?;
        let axis = expect(file, axis, "IFCAXIS1PLACEMENT")?;
        let origin = match attr::opt_reference(named(file, axis, "Location", 0), kind)? {
            Some(p) => self.point(p)?,
            None => Point3::origin(),
        };
        let k = match attr::opt_reference(named(file, axis, "Axis", 1), kind)? {
            Some(d) => unit(self.direction(d)?, kind)?,
            None => Vector3::z(),
        };
        let angle = attr::real(named(file, record, "Angle", 3), self.angle, kind)?;
        if angle == 0.0 || !angle.is_finite() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let full = angle.abs() >= TAU - 1e-9;
        let angle = if full { TAU.copysign(angle) } else { angle };
        let parts = self.profile(profile)?;
        let mut acc = MeshAcc::default();
        for part in self.prepared_parts(parts, kind)? {
            let local: Vec<Vector3> = part
                .points
                .iter()
                .map(|p| Vector3::new(p[0], p[1], 0.0) - origin.coords)
                .collect();
            let radial: Vec<f64> = local.iter().map(|q| (q - k * k.dot(q)).norm()).collect();
            let widest = radial.iter().copied().fold(0.0_f64, f64::max);
            if widest <= self.tol.linear {
                return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
            }
            let (steps, limited) = arc_segments(widest, angle, self.tol.tessellation_chord);
            if limited {
                self.log.add(
                    reasons::GEOMETRY_APPROXIMATED,
                    kind,
                    Some(Source::Decimal(name)),
                    1,
                );
            }
            let rings = if full { steps } else { steps + 1 };
            self.budget.array(rings.saturating_mul(part.points.len()))?;
            let fixed: Vec<bool> = radial.iter().map(|&r| r <= self.tol.linear).collect();
            let mut positions = Vec::with_capacity(rings * part.points.len());
            for j in 0..rings {
                let phi = angle * (j as f64) / (steps as f64);
                let (s, c) = phi.sin_cos();
                for q in &local {
                    let turned = q * c + k.cross(q) * s + k * (k.dot(q) * (1.0 - c));
                    positions.push(position.transform_point(&Point3::from(turned + origin.coords)));
                }
            }
            self.ring_part(&mut acc, &part, positions, rings, full, &fixed)?;
        }
        self.finish_mesh(acc)
    }

    /// The pieces of a swept disk solid.
    pub(crate) fn swept_disk(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let directrix = attr::reference(named(file, record, "Directrix", 0), kind)?;
        let radius = attr::real(named(file, record, "Radius", 1), self.scale, kind)?;
        let inner = attr::opt_real(named(file, record, "InnerRadius", 2), self.scale, kind)?;
        if radius <= 0.0 || inner.is_some_and(|r| r <= 0.0 || r >= radius) {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        self.reject_parameters(record, kind)?;
        if kind == "IFCSWEPTDISKSOLIDPOLYGONAL" {
            let fillet = attr::opt_real(named(file, record, "FilletRadius", 5), self.scale, kind)?;
            if fillet.is_some_and(|r| r > 0.0) {
                self.log.add(
                    reasons::GEOMETRY_APPROXIMATED,
                    kind,
                    Some(Source::Decimal(name)),
                    1,
                );
            }
        }
        let path = self.directrix(directrix)?;
        let loops = self.circle_loops(name, kind, &Transform3::identity(), radius, inner)?;
        let part = self.prepare_part(loops, kind)?;
        let (u0, v0) = perpendicular_frame(&path)?;
        let mut acc = MeshAcc::default();
        self.sweep(&mut acc, &part, &path, u0, v0, kind)?;
        self.finish_mesh(acc)
    }

    /// The pieces of an `IfcSurfaceCurveSweptAreaSolid` or `IfcFixedReferenceSweptAreaSolid`.
    pub(crate) fn directrix_sweep(&mut self, record: &Record) -> Step<Vec<Piece>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let profile = attr::reference(named(file, record, "SweptArea", 0), kind)?;
        let position = self.axis3(attr::opt_reference(
            named(file, record, "Position", 1),
            kind,
        )?)?;
        let directrix = attr::reference(named(file, record, "Directrix", 2), kind)?;
        self.reject_parameters(record, kind)?;
        let path = self.directrix(directrix)?;
        let start = *path
            .first()
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        let (u0, v0) = perpendicular_frame(&path)?;
        // The profile plane of the position, in the coordinates of the first section.
        let ex = position.transform_vector(&Vector3::x());
        let ey = position.transform_vector(&Vector3::y());
        let origin = position.transform_point(&Point3::origin());
        let parts = self.profile(profile)?;
        let mut acc = MeshAcc::default();
        for loops in parts {
            let moved: Vec<Vec<P2>> = loops
                .iter()
                .map(|l| {
                    l.iter()
                        .map(|p| {
                            let q = origin + ex * p[0] + ey * p[1] - start;
                            [q.dot(&u0), q.dot(&v0)]
                        })
                        .collect()
                })
                .collect();
            let part = self.prepare_part(moved, kind)?;
            self.sweep(&mut acc, &part, &path, u0, v0, kind)?;
        }
        self.finish_mesh(acc)
    }

    /// A swept solid that is limited by parameters is not evaluated.
    fn reject_parameters(&self, record: &Record, kind: &str) -> Step<()> {
        let file = self.file;
        let given = |name: &str, position: usize| {
            !matches!(
                named(file, record, name, position),
                None | Some(crate::model::Parameter::Omitted | crate::model::Parameter::Derived)
            )
        };
        let (start, end) = (3, 4);
        if given("StartParam", start) || given("EndParam", end) {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        Ok(())
    }

    /// The points of a directrix without consecutive repeats, at least two.
    fn directrix(&mut self, name: u64) -> Step<Vec<Point3>> {
        let kind = "IFCDIRECTRIX";
        let points = self.curve_points(name, MAX_LOOP_VERTICES)?;
        let mut out: Vec<Point3> = Vec::with_capacity(points.len());
        for p in points {
            if out.last().is_none_or(|l| (l - p).norm() > self.tol.linear) {
                out.push(p);
            }
        }
        if out.len() < 2 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        Ok(out)
    }

    /// Sweeps the prepared profile, whose points are coordinates in the frame `(u0, v0)` at the
    /// start of `path`, along the mitered polyline and adds the closed part to `acc`.
    fn sweep(
        &mut self,
        acc: &mut MeshAcc,
        part: &Prepared,
        path: &[Point3],
        u0: Vector3,
        v0: Vector3,
        kind: &str,
    ) -> Step<()> {
        let segments = path.len() - 1;
        let mut tangents: Vec<Vector3> = Vec::with_capacity(segments);
        for w in path.windows(2) {
            tangents.push(unit(w[1] - w[0], kind)?);
        }
        let mut frames: Vec<(Vector3, Vector3)> = vec![(u0, v0)];
        let mut normals: Vec<Vector3> = vec![tangents[0]];
        for i in 1..segments {
            let (previous, next) = (tangents[i - 1], tangents[i]);
            let bisector = unit(previous + next, kind)
                .map_err(|_| skip(reasons::GEOMETRY_NOT_EVALUATED, kind))?;
            if previous.dot(&bisector) < MIN_MITER_COSINE {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            let reflect = |v: Vector3| v - bisector * (2.0 * v.dot(&bisector));
            let (u, v) = frames[i - 1];
            frames.push((reflect(u), reflect(v)));
            normals.push(bisector);
        }
        normals.push(tangents[segments - 1]);
        let n = part.points.len();
        self.budget.array((segments + 1).saturating_mul(n))?;
        let mut positions = Vec::with_capacity((segments + 1) * n);
        for (i, c) in path.iter().enumerate() {
            let frame = frames[i.saturating_sub(1).min(segments - 1)];
            let frame = if i == 0 { frames[0] } else { frame };
            let tangent = tangents[i.saturating_sub(1).min(segments - 1)];
            let normal = normals[i];
            let along = tangent.dot(&normal);
            for p in &part.points {
                let q = frame.0 * p[0] + frame.1 * p[1];
                let s = -q.dot(&normal) / along;
                positions.push(c + q + tangent * s);
            }
        }
        self.ring_part(acc, part, positions, segments + 1, false, &[])
    }
}

/// A frame `(u, v)` perpendicular to the first segment of `path`, right-handed with its direction.
fn perpendicular_frame(path: &[Point3]) -> Step<(Vector3, Vector3)> {
    let (Some(a), Some(b)) = (path.first(), path.get(1)) else {
        return Err(skip(reasons::GEOMETRY_DEGENERATE, "IFCDIRECTRIX"));
    };
    let t = unit(b - a, "IFCDIRECTRIX")?;
    let helper = if t.x.abs() < 0.9 {
        Vector3::x()
    } else {
        Vector3::y()
    };
    let u = unit(helper - t * helper.dot(&t), "IFCDIRECTRIX")?;
    Ok((u, t.cross(&u)))
}
