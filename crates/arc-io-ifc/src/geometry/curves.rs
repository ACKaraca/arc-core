//! Curves as polylines (CORE-IO-015), in 2D or 3D, in meters. A curve is one output shape with at
//! least two points; a closed curve repeats its first point at the end.
//!
//! - `IfcPolyline` keeps every point in order, repeats included.
//! - `IfcIndexedPolyCurve`: `Points` (an `IfcCartesianPointList2D` or `...3D`) and the optional
//!   `Segments`, each an `IfcLineIndex` (one-based points, joined by lines) or an `IfcArcIndex`
//!   (start, a point on the arc, end: the circle through the three points, in 2D or 3D). A repeated
//!   join point is dropped.
//! - `IfcCircle` and `IfcEllipse` (`Position` an axis placement of either dimension) are closed.
//! - `IfcLine` is unbounded and is evaluated only as the basis of a trimmed curve.
//! - `IfcTrimmedCurve` on a line, circle or ellipse, with each trim a parameter value (a length
//!   along the line in units of the vector, an angle in the plane angle unit of the file) or a
//!   cartesian point (projected onto the basis curve). `MasterRepresentation` `.CARTESIAN.` takes the
//!   point and `.PARAMETER.` the parameter when both are given; unspecified takes the parameter.
//!   `SenseAgreement` `.T.` runs in the direction of increasing parameter from `Trim1` to `Trim2`
//!   and `.F.` in the direction of decreasing parameter.
//! - `IfcCompositeCurve`: the parent curves of the segments joined in order, a segment with
//!   `SameSense` `.F.` reversed; nesting is limited to [`MAX_CURVE_NESTING`].
//!
//! Arcs, circles and ellipses are discretized with the chord tolerance: the segment count is the
//! smallest whose sagitta does not exceed it ([`arc_segments`]), at most [`MAX_CIRCLE_SEGMENTS`] per
//! full turn; a curve that needs more is approximated and reported. An ellipse uses its larger
//! semi-axis as the radius, which is conservative. Curves of other kinds (B-splines, offset curves,
//! polylines as the basis of a trim) are not evaluated.

use std::f64::consts::TAU;

use arc_base::math::{Point3, Transform3, Vector3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::{Parameter, Record};
use crate::schema::access::known;

use super::attr::{self, named, skip};
use super::profile::MAX_CIRCLE_SEGMENTS;
use super::{Evaluator, Piece, Shape, Step, VERTEX_BYTES};

/// The deepest nesting of composite and trimmed curves.
pub(crate) const MAX_CURVE_NESTING: u32 = 8;

/// The segment count of an arc of `radius` and `sweep` radians (absolute) whose chord deviation is
/// at most `chord`, and whether the count was limited by [`MAX_CIRCLE_SEGMENTS`] per turn. A segment
/// spans at most 120 degrees. Both `radius` and `chord` must be positive and finite.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the count is a positive ceiling, bounded before the cast"
)]
pub(crate) fn arc_segments(radius: f64, sweep: f64, chord: f64) -> (usize, bool) {
    let sweep = sweep.abs();
    let half = (1.0 - chord / radius).clamp(-1.0, 1.0).acos();
    let raw = sweep / (2.0 * half);
    let limit = MAX_CIRCLE_SEGMENTS as f64 * (sweep / TAU).max(1.0 / 64.0);
    let floor = (sweep / (TAU / 3.0)).ceil().max(1.0);
    // The count stays far below `usize::MAX`, so `count + 1` cannot overflow; the array limit
    // refuses a count this large before anything is allocated.
    let most = f64::from(u32::MAX);
    if !raw.is_finite() || raw > limit {
        return (limit.ceil().max(floor).min(most) as usize, true);
    }
    (raw.ceil().max(floor).min(most) as usize, false)
}

/// A circle or an ellipse: the local frame and the semi-axes along its x and y.
pub(crate) struct Conic {
    pub(crate) frame: Transform3,
    pub(crate) a: f64,
    pub(crate) b: f64,
}

impl Conic {
    fn at(&self, t: f64) -> Point3 {
        self.frame
            .transform_point(&Point3::new(self.a * t.cos(), self.b * t.sin(), 0.0))
    }

    /// The parameter of the point of the curve nearest to `p` by angle.
    fn parameter_of(&self, p: &Point3) -> f64 {
        let local = self.frame.inverse().transform_point(p);
        (local.y / self.b).atan2(local.x / self.a)
    }
}

/// A trim of a trimmed curve.
#[derive(Clone, Copy)]
enum Trim {
    Parameter(f64),
    Point(Point3),
}

impl Evaluator<'_> {
    /// The pieces of a curve item.
    pub(crate) fn curve(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let points = self.curve_points_of(name, record, usize::MAX, 0)?;
        if points.len() < 2 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, &record.type_name));
        }
        self.charge_shape()?;
        Ok(Self::own(Shape::Polylines(vec![points])))
    }

    /// The points of the curve named `name`. A curve of more than `max` points is refused before its
    /// points are built.
    pub(crate) fn curve_points(&mut self, name: u64, max: usize) -> Step<Vec<Point3>> {
        self.curve_by_name(name, max, 0)
    }

    pub(crate) fn curve_by_name(&mut self, name: u64, max: usize, depth: u32) -> Step<Vec<Point3>> {
        let file = self.file;
        let Some(record) = file.instance(name).and_then(|i| i.records.first()) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCCURVE"));
        };
        self.curve_points_of(name, record, max, depth)
    }

    /// Charges `count` more points on a curve of `have` points: the bounds of the call and of the
    /// array limit, and the memory.
    fn grow(&mut self, have: usize, count: usize, max: usize, kind: &str) -> Step<()> {
        let total = have.saturating_add(count);
        self.budget.array(total)?;
        if total > max {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        self.budget.charge(
            u64::try_from(count)
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        self.budget.step()?;
        Ok(())
    }

    fn curve_points_of(
        &mut self,
        name: u64,
        record: &Record,
        max: usize,
        depth: u32,
    ) -> Step<Vec<Point3>> {
        if depth > MAX_CURVE_NESTING {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        match record.type_name.as_str() {
            "IFCPOLYLINE" => {
                let kind = "IFCPOLYLINE";
                let names = attr::ordered_references(
                    &mut self.budget,
                    self.file.attr(record, known::POLYLINE_POINTS),
                    kind,
                    max,
                )?;
                self.points_of(&names)
            }
            "IFCINDEXEDPOLYCURVE" => self.indexed_polycurve(name, record, max),
            "IFCCIRCLE" | "IFCELLIPSE" => {
                let conic = self.conic(record)?;
                let mut out = Vec::new();
                self.sample_conic(name, &conic, 0.0, TAU, max, &mut out)?;
                Ok(out)
            }
            "IFCBSPLINECURVEWITHKNOTS" | "IFCRATIONALBSPLINECURVEWITHKNOTS" => {
                self.bspline_points(record, max)
            }
            "IFCOFFSETCURVE2D" | "IFCOFFSETCURVE3D" | "IFCOFFSETCURVEBYDISTANCES" => {
                self.offset_curve(record, max, depth)
            }
            "IFCTRIMMEDCURVE" => self.trimmed(name, record, max),
            "IFCCOMPOSITECURVE" => self.composite(record, max, depth),
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    /// The points of a list of point instances, in order, with the budget charged and stepped.
    pub(crate) fn points_of(&mut self, names: &[u64]) -> Step<Vec<Point3>> {
        self.budget.charge(
            u64::try_from(names.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        let mut points = Vec::with_capacity(names.len());
        for &name in names {
            self.budget.step()?;
            points.push(self.point(name)?);
        }
        Ok(points)
    }

    /// The frame and semi-axes of an `IfcCircle` or `IfcEllipse`.
    pub(crate) fn conic(&self, record: &Record) -> Step<Conic> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let frame = self.axis_any(attr::opt_reference(
            named(file, record, "Position", 0),
            kind,
        )?)?;
        let (a, b) = if kind == "IFCCIRCLE" {
            let r = positive(named(file, record, "Radius", 1), self.scale, kind)?;
            (r, r)
        } else {
            (
                positive(named(file, record, "SemiAxis1", 1), self.scale, kind)?,
                positive(named(file, record, "SemiAxis2", 2), self.scale, kind)?,
            )
        };
        Ok(Conic { frame, a, b })
    }

    /// Appends the points of the arc of `conic` from parameter `t0` over `sweep` radians (signed),
    /// including both ends.
    pub(crate) fn sample_conic(
        &mut self,
        name: u64,
        conic: &Conic,
        t0: f64,
        sweep: f64,
        max: usize,
        out: &mut Vec<Point3>,
    ) -> Step<()> {
        let kind = "IFCCIRCLE";
        let (count, limited) =
            arc_segments(conic.a.max(conic.b), sweep, self.tol.tessellation_chord);
        if limited {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        self.grow(out.len(), count + 1, max, kind)?;
        for k in 0..=count {
            out.push(conic.at(t0 + sweep * (k as f64) / (count as f64)));
        }
        Ok(())
    }

    fn indexed_polycurve(&mut self, name: u64, record: &Record, max: usize) -> Step<Vec<Point3>> {
        let kind = "IFCINDEXEDPOLYCURVE";
        let list = attr::reference(self.file.attr(record, known::POLYCURVE_POINTS), kind)?;
        let coordinates = self.point_list(list)?;
        let count = coordinates.len();
        let mut out: Vec<Point3> = Vec::new();
        let segments = match self.file.attr(record, known::POLYCURVE_SEGMENTS) {
            None | Some(Parameter::Omitted | Parameter::Derived) => {
                self.grow(0, count, max, kind)?;
                return Ok(coordinates);
            }
            Some(Parameter::List(segments)) => segments,
            Some(_) => return Err(attr::wrong(kind)),
        };
        for segment in segments {
            let (type_name, value) = match segment {
                Parameter::Typed { type_name, value } => (type_name.as_str(), value.as_ref()),
                Parameter::List(_) => ("IFCLINEINDEX", segment),
                _ => return Err(attr::wrong(kind)),
            };
            let indices = attr::integers(Some(value), kind)?;
            let mut picked: Vec<Point3> = Vec::with_capacity(indices.len());
            for one_based in indices {
                self.budget.step()?;
                let zero_based = one_based
                    .checked_sub(1)
                    .and_then(|k| usize::try_from(k).ok())
                    .filter(|&k| k < count)
                    .ok_or_else(|| skip(reasons::INDEX_INVALID, kind))?;
                picked.push(
                    coordinates
                        .get(zero_based)
                        .copied()
                        .unwrap_or(Point3::origin()),
                );
            }
            match (type_name, picked.as_slice()) {
                ("IFCLINEINDEX", points) => {
                    self.grow(out.len(), points.len(), max, kind)?;
                    append(&mut out, points, 0.0);
                }
                ("IFCARCINDEX", [a, b, c]) => {
                    let arc = self.arc_through(name, a, b, c, max, out.len())?;
                    append(&mut out, &arc, 0.0);
                }
                ("IFCARCINDEX", _) => return Err(skip(reasons::INDEX_INVALID, kind)),
                (other, _) => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
            }
        }
        Ok(out)
    }

    /// The points of the arc from `a` through `b` to `c`, the circle through the three points; a
    /// collinear triple is the two straight segments.
    fn arc_through(
        &mut self,
        name: u64,
        a: &Point3,
        b: &Point3,
        c: &Point3,
        max: usize,
        have: usize,
    ) -> Step<Vec<Point3>> {
        let kind = "IFCARCINDEX";
        let u = b - a;
        let v = c - a;
        let n = u.cross(&v);
        let n2 = n.norm_squared();
        let scale = u.norm_squared().max(v.norm_squared());
        if !n2.is_finite() || n2 <= 1e-12 * scale * scale {
            self.grow(have, 3, max, kind)?;
            return Ok(vec![*a, *b, *c]);
        }
        let center =
            a + ((v.norm_squared() * u * -1.0 + u.norm_squared() * v).cross(&n)) / (2.0 * n2);
        let radius = (a - center).norm();
        let x = (a - center) / radius;
        let z = n / n2.sqrt();
        let y = z.cross(&x);
        let to_c = c - center;
        let mut sweep = to_c.dot(&y).atan2(to_c.dot(&x));
        if sweep <= 0.0 {
            sweep += TAU;
        }
        let (count, limited) = arc_segments(radius, sweep, self.tol.tessellation_chord);
        if limited {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        self.grow(have, count + 1, max, kind)?;
        let mut out = Vec::with_capacity(count + 1);
        for k in 0..=count {
            let t = sweep * (k as f64) / (count as f64);
            out.push(center + (x * t.cos() + y * t.sin()) * radius);
        }
        // The ends are the given points exactly.
        if let Some(first) = out.first_mut() {
            *first = *a;
        }
        if let Some(last) = out.last_mut() {
            *last = *c;
        }
        Ok(out)
    }

    /// The two trims of a set, preferring by the master representation.
    fn trim_of(&self, set: Option<&Parameter>, master: Option<&str>, kind: &str) -> Step<Trim> {
        let items = attr::list(set, kind)?;
        let mut parameter = None;
        let mut point = None;
        for item in items {
            match item {
                Parameter::Reference(name) => {
                    if point.is_none() {
                        point = Some(self.point(*name)?);
                    }
                }
                other => {
                    if parameter.is_none() {
                        parameter = Some(attr::real(Some(other), 1.0, kind)?);
                    }
                }
            }
        }
        let by_point = master == Some("CARTESIAN");
        let chosen = if by_point {
            point.map(Trim::Point).or(parameter.map(Trim::Parameter))
        } else {
            parameter.map(Trim::Parameter).or(point.map(Trim::Point))
        };
        chosen.ok_or_else(|| attr::wrong(kind))
    }

    fn trimmed(&mut self, name: u64, record: &Record, max: usize) -> Step<Vec<Point3>> {
        Ok(self.trimmed_ranged(name, record, max)?.0)
    }

    /// The points of a trimmed curve with the range of its basis parameter they span (radians for
    /// a circle or an ellipse) and whether that parameter is an angle.
    pub(crate) fn trimmed_ranged(
        &mut self,
        name: u64,
        record: &Record,
        max: usize,
    ) -> Step<(Vec<Point3>, (f64, f64), bool)> {
        let kind = "IFCTRIMMEDCURVE";
        let file = self.file;
        let basis = attr::reference(named(file, record, "BasisCurve", 0), kind)?;
        let sense = attr::boolean(named(file, record, "SenseAgreement", 3)).unwrap_or(true);
        let master = attr::enumeration(named(file, record, "MasterRepresentation", 4));
        let first = self.trim_of(named(file, record, "Trim1", 1), master, kind)?;
        let second = self.trim_of(named(file, record, "Trim2", 2), master, kind)?;
        let Some(basis_record) = file.instance(basis).and_then(|i| i.records.first()) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCCURVE"));
        };
        match basis_record.type_name.as_str() {
            "IFCLINE" => {
                let (origin, dir) = self.line(basis_record)?;
                let length2 = dir.norm_squared();
                let param = |trim: Trim| match trim {
                    Trim::Parameter(t) => t,
                    Trim::Point(p) => (p - origin).dot(&dir) / length2,
                };
                let (s1, s2) = (param(first), param(second));
                self.grow(0, 2, max, kind)?;
                Ok((vec![origin + dir * s1, origin + dir * s2], (s1, s2), false))
            }
            "IFCCIRCLE" | "IFCELLIPSE" => {
                let conic = self.conic(basis_record)?;
                let angle = |trim: Trim| match trim {
                    Trim::Parameter(t) => t * self.angle,
                    Trim::Point(p) => conic.parameter_of(&p),
                };
                let (t1, mut t2) = (angle(first), angle(second));
                // The next turn of `t2` after (or before) `t1`, computed in one step: repeated
                // additions of a turn never end on parameters far beyond 2π.
                if sense && t2 <= t1 {
                    t2 = t1 + (TAU - (t1 - t2).rem_euclid(TAU));
                } else if !sense && t2 >= t1 {
                    t2 = t1 - (TAU - (t2 - t1).rem_euclid(TAU));
                }
                if !(t2 - t1).is_finite() || (t2 - t1).abs() > TAU * (1.0 + 1e-9) {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                let mut out = Vec::new();
                self.sample_conic(name, &conic, t1, t2 - t1, max, &mut out)?;
                Ok((out, (t1, t2), true))
            }
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    /// The point and the direction vector (with its magnitude, in meters) of an `IfcLine`.
    fn line(&self, record: &Record) -> Step<(Point3, Vector3)> {
        let kind = "IFCLINE";
        let file = self.file;
        let origin = self.point(attr::reference(named(file, record, "Pnt", 0), kind)?)?;
        let vector = attr::reference(named(file, record, "Dir", 1), kind)?;
        let vector = super::place::expect(file, vector, "IFCVECTOR")?;
        let orientation = attr::reference(named(file, vector, "Orientation", 0), kind)?;
        let magnitude = attr::real(named(file, vector, "Magnitude", 1), self.scale, kind)?;
        let unit = super::place::unit(self.direction(orientation)?, kind)?;
        if magnitude <= 0.0 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        Ok((origin, unit * magnitude))
    }

    fn composite(&mut self, record: &Record, max: usize, depth: u32) -> Step<Vec<Point3>> {
        let kind = "IFCCOMPOSITECURVE";
        let file = self.file;
        let segments = attr::ordered_references(
            &mut self.budget,
            named(file, record, "Segments", 0),
            kind,
            max,
        )?;
        let mut out: Vec<Point3> = Vec::new();
        for segment in segments {
            self.budget.step()?;
            let segment = super::place::expect(file, segment, "IFCCOMPOSITECURVESEGMENT")?;
            let same = attr::boolean(named(file, segment, "SameSense", 1)).unwrap_or(true);
            let parent = attr::reference(named(file, segment, "ParentCurve", 2), kind)?;
            let mut points = self.curve_by_name(parent, max, depth + 1)?;
            if !same {
                points.reverse();
            }
            self.grow(out.len(), points.len(), max, kind)?;
            append(&mut out, &points, self.tol.linear);
        }
        Ok(out)
    }
}

/// Appends `points`, dropping the first when it coincides (within `tolerance`) with the last point of
/// `out`.
fn append(out: &mut Vec<Point3>, points: &[Point3], tolerance: f64) {
    for (k, p) in points.iter().enumerate() {
        if k == 0
            && out
                .last()
                .is_some_and(|last| (last - p).norm() <= tolerance)
        {
            continue;
        }
        out.push(*p);
    }
}

/// A positive length of a curve, scaled to meters.
fn positive(parameter: Option<&Parameter>, scale: f64, kind: &str) -> Step<f64> {
    let value = attr::real(parameter, scale, kind)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
    }
}

impl Evaluator<'_> {
    /// The points of an `IfcCartesianPointList2D` or `IfcCartesianPointList3D`, scaled to meters.
    /// The row count is checked against `array_length` and charged before the points are built.
    pub(crate) fn point_list(&mut self, name: u64) -> Step<Vec<Point3>> {
        let kind = "IFCCARTESIANPOINTLIST";
        let file = self.file;
        let record = file
            .instance(name)
            .and_then(|i| i.records.first())
            .ok_or_else(|| skip(reasons::TARGET_MISSING, kind))?;
        if record.type_name != "IFCCARTESIANPOINTLIST2D"
            && record.type_name != "IFCCARTESIANPOINTLIST3D"
        {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        let rows = attr::list(file.attr(record, known::POINT_LIST_COORDINATES), kind)?;
        self.budget.array(rows.len())?;
        self.budget.charge(
            u64::try_from(rows.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        let mut points = Vec::with_capacity(rows.len());
        for row in rows {
            self.budget.step()?;
            points.push(match row {
                Parameter::List(values) => attr::point3(values, self.scale, kind)?,
                _ => return Err(attr::wrong(kind)),
            });
        }
        Ok(points)
    }
}
