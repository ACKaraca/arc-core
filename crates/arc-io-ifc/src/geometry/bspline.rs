//! B-spline curves and surfaces (CORE-IO-015): `IfcBSplineCurveWithKnots`,
//! `IfcRationalBSplineCurveWithKnots`, `IfcBSplineSurfaceWithKnots` and
//! `IfcRationalBSplineSurfaceWithKnots`, evaluated by the de Boor recurrence on homogeneous
//! coordinates (`arc-geom` has the 2D form only).
//!
//! The knot vector is expanded from `KnotMultiplicities` and `Knots` (`UMultiplicities`, `UKnots` and
//! the V forms of a surface); its length must be the number of control points plus the degree plus
//! one, the multiplicities at most degree plus one, the knots non-decreasing and the weights
//! positive, or the item is skipped as malformed. The domain is `[knots[degree],
//! knots[n]]`. The control points and weights are charged to the budget before they are read.
//!
//! A curve is sampled by bisection of each distinct knot span until the midpoint of every piece is
//! within the chord tolerance of its chord (the first split of a span is forced at the degree, so a
//! span with an inflection is not skipped), at most 4096 points. A surface item is the grid of the
//! samples of its spans: the number of cells per span is doubled while the midpoint of any cell of
//! the span is farther than the chord tolerance from the bilinear interpolation of its corners (at
//! most 64 per span); the result is reported as approximated when a cap applies. A surface is an
//! open mesh.

use arc_base::math::Point3;

use crate::error::reasons;
use crate::model::{Parameter, Record};

use super::attr::{self, named, skip};
use super::solid::MeshAcc;
use super::{Evaluator, Flow, Piece, Shape, Step, TRIANGLE_BYTES, VERTEX_BYTES};

/// The most points of a sampled curve.
const MAX_CURVE_POINTS: usize = 4096;
/// The most cells of a span of a surface in one direction.
const MAX_CELLS: usize = 64;

/// A B-spline curve: degree, control points, optional weights and the expanded knot vector.
#[derive(Debug, Clone)]
pub(crate) struct BSplineCurve {
    pub(crate) degree: usize,
    pub(crate) control: Vec<Point3>,
    pub(crate) weights: Option<Vec<f64>>,
    pub(crate) knots: Vec<f64>,
}

/// A B-spline surface: control net `control[i][j]` with `i` along u and `j` along v.
#[derive(Debug, Clone)]
pub(crate) struct BSplineSurface {
    pub(crate) degree: (usize, usize),
    pub(crate) control: Vec<Vec<Point3>>,
    pub(crate) weights: Option<Vec<Vec<f64>>>,
    pub(crate) knots: (Vec<f64>, Vec<f64>),
}

/// The span index `s` with `knots[s] <= t < knots[s + 1]` among `degree..n`, the last span for the
/// end of the domain.
fn span(knots: &[f64], degree: usize, n: usize, t: f64) -> usize {
    let mut s = degree;
    while s + 1 < n && knots.get(s + 1).is_some_and(|&k| k <= t) {
        s += 1;
    }
    s
}

/// De Boor on homogeneous points `[x*w, y*w, z*w, w]`.
fn de_boor(knots: &[f64], degree: usize, ctrl: &[[f64; 4]], t: f64) -> [f64; 4] {
    let n = ctrl.len();
    let s = span(knots, degree, n, t);
    let mut d: Vec<[f64; 4]> = (0..=degree)
        .map(|j| ctrl.get(s + j - degree).copied().unwrap_or([0.0; 4]))
        .collect();
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = s + j - degree;
            let (lo, hi) = (
                knots.get(i).copied().unwrap_or(0.0),
                knots.get(i + degree + 1 - r).copied().unwrap_or(1.0),
            );
            let a = if hi > lo { (t - lo) / (hi - lo) } else { 0.0 };
            let (p, q) = (d[j - 1], d[j]);
            for k in 0..4 {
                d[j][k] = (1.0 - a) * p[k] + a * q[k];
            }
        }
    }
    d[degree]
}

fn homogeneous(p: &Point3, w: f64) -> [f64; 4] {
    [p.x * w, p.y * w, p.z * w, w]
}

fn project(h: [f64; 4]) -> Point3 {
    let w = if h[3] == 0.0 { 1.0 } else { h[3] };
    Point3::new(h[0] / w, h[1] / w, h[2] / w)
}

impl BSplineCurve {
    /// The parameter range.
    pub(crate) fn domain(&self) -> (f64, f64) {
        let n = self.control.len();
        (
            self.knots.get(self.degree).copied().unwrap_or(0.0),
            self.knots.get(n).copied().unwrap_or(1.0),
        )
    }

    /// The point at `t`, clamped to the domain.
    pub(crate) fn point_at(&self, t: f64) -> Point3 {
        let (a, b) = self.domain();
        let ctrl: Vec<[f64; 4]> = self
            .control
            .iter()
            .enumerate()
            .map(|(i, p)| {
                homogeneous(
                    p,
                    self.weights
                        .as_ref()
                        .and_then(|w| w.get(i))
                        .copied()
                        .unwrap_or(1.0),
                )
            })
            .collect();
        project(de_boor(&self.knots, self.degree, &ctrl, t.clamp(a, b)))
    }

    /// The samples `(parameter, point)` within `chord` of the curve, the ends included.
    pub(crate) fn samples(&self, chord: f64) -> Vec<(f64, Point3)> {
        let n = self.control.len();
        let mut breaks: Vec<f64> = self
            .knots
            .get(self.degree..=n)
            .map_or_else(Vec::new, <[f64]>::to_vec);
        breaks.dedup_by(|a, b| (*a - *b).abs() <= 0.0);
        let mut out: Vec<(f64, Point3)> = Vec::new();
        for w in breaks.windows(2) {
            let first = out.is_empty();
            if first {
                out.push((w[0], self.point_at(w[0])));
            }
            let steps = self.degree.max(2);
            for k in 0..steps {
                let (a, b) = (
                    w[0] + (w[1] - w[0]) * (k as f64) / (steps as f64),
                    w[0] + (w[1] - w[0]) * ((k + 1) as f64) / (steps as f64),
                );
                self.refine(a, b, chord, 0, &mut out);
            }
        }
        if out.is_empty() {
            let (a, _) = self.domain();
            out.push((a, self.point_at(a)));
        }
        out
    }

    /// Appends the samples after `a` up to and including `b`.
    fn refine(&self, a: f64, b: f64, chord: f64, depth: u32, out: &mut Vec<(f64, Point3)>) {
        let pa = out.last().map_or_else(|| self.point_at(a), |(_, p)| *p);
        let pb = self.point_at(b);
        let m = 0.5 * (a + b);
        let pm = self.point_at(m);
        let line = pb - pa;
        let len2 = line.norm_squared();
        let t = if len2 > 0.0 {
            ((pm - pa).dot(&line) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let deviation = (pm - (pa + line * t)).norm();
        if deviation > chord && depth < 20 && out.len() < MAX_CURVE_POINTS {
            self.refine(a, m, chord, depth + 1, out);
            self.refine(m, b, chord, depth + 1, out);
        } else {
            out.push((b, pb));
        }
    }
}

impl BSplineSurface {
    /// The parameter ranges of u and v.
    pub(crate) fn domain(&self) -> ((f64, f64), (f64, f64)) {
        let range = |knots: &[f64], degree: usize, n: usize| {
            (
                knots.get(degree).copied().unwrap_or(0.0),
                knots.get(n).copied().unwrap_or(1.0),
            )
        };
        (
            range(&self.knots.0, self.degree.0, self.control.len()),
            range(
                &self.knots.1,
                self.degree.1,
                self.control.first().map_or(0, Vec::len),
            ),
        )
    }

    /// The point at `(u, v)`, clamped to the domain.
    pub(crate) fn point_at(&self, u: f64, v: f64) -> Point3 {
        let ((u0, u1), (v0, v1)) = self.domain();
        let (u, v) = (u.clamp(u0, u1), v.clamp(v0, v1));
        // Evaluate each row (fixed i) along v, then the column along u.
        let column: Vec<[f64; 4]> = self
            .control
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let ctrl: Vec<[f64; 4]> = row
                    .iter()
                    .enumerate()
                    .map(|(j, p)| {
                        let w = self
                            .weights
                            .as_ref()
                            .and_then(|w| w.get(i))
                            .and_then(|r| r.get(j))
                            .copied()
                            .unwrap_or(1.0);
                        homogeneous(p, w)
                    })
                    .collect();
                de_boor(&self.knots.1, self.degree.1, &ctrl, v)
            })
            .collect();
        project(de_boor(&self.knots.0, self.degree.0, &column, u))
    }

    /// The distinct knot values inside the domain of one direction.
    fn breaks(&self, u: bool) -> Vec<f64> {
        let (knots, degree, n) = if u {
            (&self.knots.0, self.degree.0, self.control.len())
        } else {
            (
                &self.knots.1,
                self.degree.1,
                self.control.first().map_or(0, Vec::len),
            )
        };
        let mut b: Vec<f64> = knots.get(degree..=n).map_or_else(Vec::new, <[f64]>::to_vec);
        b.dedup_by(|a, c| (*a - *c).abs() <= 0.0);
        b
    }

    /// The parameter values of the grid lines within `chord`, per direction; the flag tells that a
    /// cap applied.
    pub(crate) fn grid(&self, chord: f64) -> (Vec<f64>, Vec<f64>, bool) {
        let mut capped = false;
        let mut lines = |u_direction: bool| -> Vec<f64> {
            let other = self.breaks(!u_direction);
            let mut out: Vec<f64> = Vec::new();
            let breaks = self.breaks(u_direction);
            for w in breaks.windows(2) {
                let mut cells = self.degree_of(u_direction).max(1);
                loop {
                    let mut worst = 0.0_f64;
                    for c in 0..cells {
                        let a = w[0] + (w[1] - w[0]) * (c as f64) / (cells as f64);
                        let b = w[0] + (w[1] - w[0]) * ((c + 1) as f64) / (cells as f64);
                        let m = 0.5 * (a + b);
                        for o in other.windows(2) {
                            let t = 0.5 * (o[0] + o[1]);
                            let at = |x: f64, y: f64| {
                                if u_direction {
                                    self.point_at(x, y)
                                } else {
                                    self.point_at(y, x)
                                }
                            };
                            let mid = at(m, t);
                            let chord_mid = at(a, t).coords * 0.5 + at(b, t).coords * 0.5;
                            worst = worst.max((mid.coords - chord_mid).norm());
                        }
                    }
                    if worst <= chord || cells >= MAX_CELLS {
                        capped |= worst > chord;
                        break;
                    }
                    cells *= 2;
                }
                if out.is_empty() {
                    out.push(w[0]);
                }
                for c in 1..=cells {
                    out.push(w[0] + (w[1] - w[0]) * (c as f64) / (cells as f64));
                }
            }
            out
        };
        let u = lines(true);
        let v = lines(false);
        (u, v, capped)
    }

    fn degree_of(&self, u: bool) -> usize {
        if u { self.degree.0 } else { self.degree.1 }
    }
}

/// Expands `multiplicities` and `knots` into one knot vector and checks it against `count` control
/// points of `degree`.
fn expand(multiplicities: &[i64], knots: &[f64], degree: usize, count: usize) -> Option<Vec<f64>> {
    if multiplicities.len() != knots.len() {
        return None;
    }
    let mut out = Vec::new();
    for (&m, &k) in multiplicities.iter().zip(knots) {
        let m = usize::try_from(m)
            .ok()
            .filter(|&m| m >= 1 && m <= degree + 1)?;
        out.extend(std::iter::repeat_n(k, m));
    }
    let sorted = out.windows(2).all(|w| w[0] <= w[1]);
    (sorted && out.len() == count + degree + 1 && out.iter().all(|k| k.is_finite())).then_some(out)
}

impl Evaluator<'_> {
    fn real_list(&mut self, parameter: Option<&Parameter>, kind: &str) -> Step<Vec<f64>> {
        let list = attr::list(parameter, kind)?;
        self.budget.array(list.len())?;
        self.budget.charge(
            u64::try_from(list.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(8),
        )?;
        list.iter()
            .map(|p| attr::real(Some(p), 1.0, kind))
            .collect()
    }

    fn degree(parameter: Option<&Parameter>, kind: &str) -> Step<usize> {
        let Some(Parameter::Integer(value)) = parameter else {
            return Err(attr::wrong(kind));
        };
        usize::try_from(*value)
            .ok()
            .filter(|&d| (1..=25).contains(&d))
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))
    }

    /// The B-spline curve of an `IfcBSplineCurveWithKnots` or its rational form.
    pub(crate) fn bspline_curve(&mut self, record: &Record) -> Step<BSplineCurve> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let degree = Self::degree(named(file, record, "Degree", 0), kind)?;
        let points = attr::ordered_references(
            &mut self.budget,
            named(file, record, "ControlPointsList", 1),
            kind,
            usize::MAX,
        )?;
        let control = self.points_of(&points)?;
        let multiplicities = attr::integers(named(file, record, "KnotMultiplicities", 5), kind)?;
        let knots = self.real_list(named(file, record, "Knots", 6), kind)?;
        let weights = if kind == "IFCRATIONALBSPLINECURVEWITHKNOTS" {
            Some(self.real_list(named(file, record, "WeightsData", 8), kind)?)
        } else {
            None
        };
        if control.len() <= degree
            || weights.as_ref().is_some_and(|w| {
                w.len() != control.len() || !w.iter().all(|&x| x.is_finite() && x > 0.0)
            })
        {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let knots = expand(&multiplicities, &knots, degree, control.len())
            .ok_or_else(|| skip(reasons::GEOMETRY_DEGENERATE, kind))?;
        Ok(BSplineCurve {
            degree,
            control,
            weights,
            knots,
        })
    }

    /// The B-spline surface of an `IfcBSplineSurfaceWithKnots` or its rational form.
    pub(crate) fn bspline_surface(&mut self, record: &Record) -> Step<BSplineSurface> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let du = Self::degree(named(file, record, "UDegree", 0), kind)?;
        let dv = Self::degree(named(file, record, "VDegree", 1), kind)?;
        let rows = attr::list(named(file, record, "ControlPointsList", 2), kind)?;
        self.budget.array(rows.len())?;
        let mut control: Vec<Vec<Point3>> = Vec::with_capacity(rows.len());
        let mut total = 0_usize;
        for row in rows {
            let names = attr::ordered_references(&mut self.budget, Some(row), kind, usize::MAX)?;
            total = total.saturating_add(names.len());
            self.budget.array(total)?;
            control.push(self.points_of(&names)?);
        }
        let width = control.first().map_or(0, Vec::len);
        let um = attr::integers(named(file, record, "UMultiplicities", 7), kind)?;
        let vm = attr::integers(named(file, record, "VMultiplicities", 8), kind)?;
        let uk = self.real_list(named(file, record, "UKnots", 9), kind)?;
        let vk = self.real_list(named(file, record, "VKnots", 10), kind)?;
        let weights = if kind == "IFCRATIONALBSPLINESURFACEWITHKNOTS" {
            let rows = attr::list(named(file, record, "WeightsData", 12), kind)?;
            let mut w = Vec::with_capacity(rows.len());
            for row in rows {
                w.push(self.real_list(Some(row), kind)?);
            }
            Some(w)
        } else {
            None
        };
        let ragged = control.iter().any(|r| r.len() != width);
        if control.len() <= du
            || width <= dv
            || ragged
            || weights.as_ref().is_some_and(|w| {
                w.len() != control.len()
                    || w.iter()
                        .any(|r| r.len() != width || !r.iter().all(|&x| x.is_finite() && x > 0.0))
            })
        {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let (Some(uk), Some(vk)) = (
            expand(&um, &uk, du, control.len()),
            expand(&vm, &vk, dv, width),
        ) else {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        };
        Ok(BSplineSurface {
            degree: (du, dv),
            control,
            weights,
            knots: (uk, vk),
        })
    }

    /// The polyline of a B-spline curve item.
    pub(crate) fn bspline_points(&mut self, record: &Record, max: usize) -> Step<Vec<Point3>> {
        let curve = self.bspline_curve(record)?;
        let samples = curve.samples(self.tol.tessellation_chord);
        self.budget.array(samples.len())?;
        if samples.len() > max {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        self.budget.charge(
            u64::try_from(samples.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        Ok(samples.into_iter().map(|(_, p)| p).collect())
    }

    /// The pieces of a B-spline surface item: an open mesh of the grid.
    pub(crate) fn bspline_surface_item(&mut self, name: u64, record: &Record) -> Step<Vec<Piece>> {
        let surface = self.bspline_surface(record)?;
        let (us, vs, capped) = surface.grid(self.tol.tessellation_chord);
        if capped {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                &record.type_name,
                Some(crate::loss::Source::Decimal(name)),
                1,
            );
        }
        let count = us.len().saturating_mul(vs.len());
        self.budget.array(count)?;
        self.budget.charge(
            u64::try_from(count)
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES + 2 * TRIANGLE_BYTES),
        )?;
        let mut positions = Vec::with_capacity(count);
        for &u in &us {
            self.budget.step()?;
            for &v in &vs {
                positions.push(surface.point_at(u, v));
            }
        }
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        let n = vs.len();
        for i in 0..us.len().saturating_sub(1) {
            for j in 0..n.saturating_sub(1) {
                let (a, b, c, d) = (
                    i * n + j,
                    (i + 1) * n + j,
                    (i + 1) * n + j + 1,
                    i * n + j + 1,
                );
                for t in [[a, b, c], [a, c, d]] {
                    triangles.push([
                        super::index32(t[0])?,
                        super::index32(t[1])?,
                        super::index32(t[2])?,
                    ]);
                }
            }
        }
        let acc = MeshAcc {
            positions,
            triangles,
        };
        self.charge_shape()?;
        let mesh = arc_mesh::TriMeshBuilder::new(acc.positions, acc.triangles)
            .build(&self.limits)
            .map_err(Flow::Hard)?;
        Ok(Self::own(Shape::Mesh(mesh)))
    }
}
