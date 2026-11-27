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
    #[allow(
        clippy::too_many_lines,
        reason = "the profiles, the turn and the rings of one solid"
    )]
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
        let end_points: Option<Vec<Vec<P2>>> = if record.type_name == "IFCREVOLVEDAREASOLIDTAPERED"
        {
            let end = attr::reference(named(file, record, "EndSweptArea", 4), kind)?;
            let end_parts = self.profile(end)?;
            let same = parts.len() == end_parts.len()
                && parts.iter().zip(&end_parts).all(|(a, b)| {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.len() == y.len())
                });
            if !same {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            Some(
                end_parts
                    .into_iter()
                    .map(|loops| loops.into_iter().flatten().collect())
                    .collect(),
            )
        } else {
            None
        };
        let mut acc = MeshAcc::default();
        for (number, part) in self.prepared_parts(parts, kind)?.into_iter().enumerate() {
            let local: Vec<Vector3> = part
                .points
                .iter()
                .map(|p| Vector3::new(p[0], p[1], 0.0) - origin.coords)
                .collect();
            let local_end: Option<Vec<Vector3>> =
                end_points.as_ref().and_then(|e| e.get(number)).map(|e| {
                    e.iter()
                        .map(|p| Vector3::new(p[0], p[1], 0.0) - origin.coords)
                        .collect()
                });
            let radial: Vec<f64> = local.iter().map(|q| (q - k * k.dot(q)).norm()).collect();
            let widest = radial
                .iter()
                .copied()
                .chain(
                    local_end
                        .iter()
                        .flatten()
                        .map(|q| (q - k * k.dot(q)).norm()),
                )
                .fold(0.0_f64, f64::max);
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
            let fixed: Vec<bool> = radial
                .iter()
                .enumerate()
                .map(|(i, &r)| {
                    r <= self.tol.linear
                        && local_end
                            .as_ref()
                            .and_then(|e| e.get(i))
                            .is_none_or(|q| (q - k * k.dot(q)).norm() <= self.tol.linear)
                })
                .collect();
            let mut positions = Vec::with_capacity(rings * part.points.len());
            for j in 0..rings {
                let phi = angle * (j as f64) / (steps as f64);
                let (s, c) = phi.sin_cos();
                let blend = (j as f64) / (steps as f64);
                for (index, q) in local.iter().enumerate() {
                    let q = &match local_end.as_ref().and_then(|e| e.get(index)) {
                        Some(end) => q * (1.0 - blend) + end * blend,
                        None => *q,
                    };
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
        let path = self.sweep_path(directrix, record, kind)?;
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
        let path = self.sweep_path(directrix, record, kind)?;
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

    /// The directrix of a sweep limited by `StartParam` and `EndParam` (attributes 3 and 4), as the
    /// polyline between them; the whole directrix when neither is given. The parameters are those of
    /// the curve: the index of a polyline point, an angle in the plane angle unit of the file on a
    /// circle, an ellipse or a trimmed one of them, the basis parameter on a trimmed line, and the knot
    /// parameter of a B-spline. Another curve with parameters is not evaluated.
    fn sweep_path(&mut self, directrix: u64, record: &Record, kind: &str) -> Step<Vec<Point3>> {
        let file = self.file;
        let given = |name: &str, position: usize| -> Step<Option<f64>> {
            attr::opt_real(named(file, record, name, position), 1.0, kind)
        };
        let (start, end) = (given("StartParam", 3)?, given("EndParam", 4)?);
        if start.is_none() && end.is_none() {
            return self.directrix(directrix);
        }
        let Some(curve) = file.instance(directrix).and_then(|i| i.records.first()) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCCURVE"));
        };
        let mut points = match curve.type_name.as_str() {
            "IFCCIRCLE" | "IFCELLIPSE" => {
                let conic = self.conic(curve)?;
                let (s, e) = (start.unwrap_or(0.0), end.unwrap_or(TAU / self.angle));
                let mut out = Vec::new();
                self.sample_conic(
                    directrix,
                    &conic,
                    s * self.angle,
                    (e - s) * self.angle,
                    usize::MAX,
                    &mut out,
                )?;
                out
            }
            "IFCTRIMMEDCURVE" => {
                let (points, (t1, t2), angular) =
                    self.trimmed_ranged(directrix, curve, usize::MAX)?;
                let factor = if angular { self.angle } else { 1.0 };
                let (s, e) = (
                    start.map_or(t1, |s| s * factor),
                    end.map_or(t2, |e| e * factor),
                );
                let samples: Vec<(f64, Point3)> = points
                    .iter()
                    .enumerate()
                    .map(|(k, p)| {
                        let f = (k as f64) / ((points.len() - 1).max(1) as f64);
                        (t1 + (t2 - t1) * f, *p)
                    })
                    .collect();
                cut(&samples, s, e)
            }
            "IFCPOLYLINE" => {
                let points = self.curve_points(directrix, MAX_LOOP_VERTICES)?;
                let samples: Vec<(f64, Point3)> = points
                    .iter()
                    .enumerate()
                    .map(|(k, p)| (k as f64, *p))
                    .collect();
                let last = (points.len() as f64) - 1.0;
                cut(&samples, start.unwrap_or(0.0), end.unwrap_or(last))
            }
            "IFCBSPLINECURVEWITHKNOTS" | "IFCRATIONALBSPLINECURVEWITHKNOTS" => {
                let spline = self.bspline_curve(curve)?;
                let samples = spline.samples(self.tol.tessellation_chord);
                let (a, b) = spline.domain();
                cut(&samples, start.unwrap_or(a), end.unwrap_or(b))
            }
            other => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        };
        points.dedup_by(|a, b| (*a - *b).norm() <= self.tol.linear);
        if points.len() < 2 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        Ok(points)
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

/// The points of `samples` (parameter, point; parameters ascending or descending) between the
/// parameters `start` and `end`, with the ends interpolated; in the order from `start` to `end`.
fn cut(samples: &[(f64, Point3)], start: f64, end: f64) -> Vec<Point3> {
    let ascending = samples
        .first()
        .zip(samples.last())
        .is_some_and(|(a, b)| a.0 <= b.0);
    let mut sorted: Vec<(f64, Point3)> = samples.to_vec();
    if !ascending {
        sorted.reverse();
    }
    let (lo, hi) = (start.min(end), start.max(end));
    let at = |t: f64| -> Option<Point3> {
        let t = t.clamp(sorted.first()?.0, sorted.last()?.0);
        sorted.windows(2).find_map(|w| {
            (t >= w[0].0 && t <= w[1].0).then(|| {
                let span = w[1].0 - w[0].0;
                let f = if span > 0.0 { (t - w[0].0) / span } else { 0.0 };
                w[0].1 + (w[1].1 - w[0].1) * f
            })
        })
    };
    let mut out = Vec::new();
    out.extend(at(lo));
    out.extend(
        sorted
            .iter()
            .filter(|(t, _)| *t > lo && *t < hi)
            .map(|(_, p)| *p),
    );
    out.extend(at(hi));
    if start > end {
        out.reverse();
    }
    out
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
