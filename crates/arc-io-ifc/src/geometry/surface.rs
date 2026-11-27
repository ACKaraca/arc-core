//! Surfaces of advanced faces (CORE-IO-015): the parameterizations used to trim and tessellate an
//! `IfcAdvancedFace`.
//!
//! | Surface | Parameters `(u, v)` |
//! |---|---|
//! | `IfcCylindricalSurface` | angle about the axis of `Position`, height |
//! | `IfcConicalSurface` | angle, height (radius `Radius + v * tan(SemiAngle)`) |
//! | `IfcSphericalSurface` | longitude, latitude |
//! | `IfcToroidalSurface` | angle about the axis, angle about the tube |
//! | `IfcSurfaceOfLinearExtrusion` | index parameter along the swept curve polyline, distance along the direction |
//! | `IfcSurfaceOfRevolution` | angle about the axis, index parameter along the swept curve polyline |
//! | `IfcBSplineSurfaceWithKnots` (and rational) | the knot parameters |
//!
//! The swept curve of an extrusion or a revolution is the polyline of the profile curve in the chord
//! tolerance, placed by `Position`; a surface of revolution turns it about `AxisPosition` (in the
//! coordinates of `Position`). The inverse mapping from a point to `(u, v)` is closed-form for the
//! quadrics, by nearest segment for the swept surfaces and by a grid search refined with Newton steps
//! for a B-spline surface, and is unwrapped near a given `(u, v)` for an angle.

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point3, Transform3, Vector3};

use super::bspline::BSplineSurface;

/// The kinds of surface and their dimensions.
pub(crate) enum Kind {
    Cylinder {
        r: f64,
    },
    Cone {
        r: f64,
        tan: f64,
    },
    Sphere {
        r: f64,
    },
    Torus {
        big: f64,
        small: f64,
    },
    Extrusion {
        curve: Vec<Point3>,
        dir: Vector3,
    },
    Revolution {
        curve: Vec<Point3>,
        origin: Point3,
        axis: Vector3,
    },
    Spline(BSplineSurface),
}

/// A surface and its placement (identity for the kinds that carry world coordinates).
pub(crate) struct Surf {
    pub(crate) frame: Transform3,
    pub(crate) kind: Kind,
}

/// `a` moved by multiples of `period` to within half a period of `near`.
pub(crate) fn unwrap_near(a: f64, near: f64, period: f64) -> f64 {
    a + period * ((near - a) / period).round()
}

/// The point of a polyline at the index parameter `s`.
fn along(curve: &[Point3], s: f64) -> Point3 {
    let last = curve.len().saturating_sub(1);
    let s = s.clamp(0.0, last as f64);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped index"
    )]
    let i = (s.floor() as usize).min(last.saturating_sub(1));
    let f = s - i as f64;
    match (curve.get(i), curve.get(i + 1)) {
        (Some(a), Some(b)) => a + (b - a) * f,
        (Some(a), None) => *a,
        _ => Point3::origin(),
    }
}

/// The index parameter of the polyline point nearest to `q` under `measure`, which maps a polyline
/// point to the space `q` is in.
fn nearest(curve: &[Point3], q: &Vector3, measure: &dyn Fn(&Point3) -> Vector3) -> f64 {
    let mut best = (f64::MAX, 0.0);
    for (i, w) in curve.windows(2).enumerate() {
        let (a, b) = (measure(&w[0]), measure(&w[1]));
        let line = b - a;
        let len2 = line.norm_squared();
        let t = if len2 > 0.0 {
            ((q - a).dot(&line) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = (q - (a + line * t)).norm();
        if d < best.0 {
            best = (d, i as f64 + t);
        }
    }
    best.1
}

impl Surf {
    /// Whether `u` and `v` repeat: their periods.
    pub(crate) fn periods(&self) -> (Option<f64>, Option<f64>) {
        match self.kind {
            Kind::Cylinder { .. }
            | Kind::Cone { .. }
            | Kind::Sphere { .. }
            | Kind::Revolution { .. } => (Some(TAU), None),
            Kind::Torus { .. } => (Some(TAU), Some(TAU)),
            _ => (None, None),
        }
    }

    /// Whether the parameterization's normal is opposite to the surface normal of the entity.
    pub(crate) fn reversed(&self) -> bool {
        matches!(self.kind, Kind::Revolution { .. })
    }

    /// The point at `(u, v)`.
    pub(crate) fn point(&self, u: f64, v: f64) -> Point3 {
        let local = match &self.kind {
            Kind::Cylinder { r } => Point3::new(r * u.cos(), r * u.sin(), v),
            Kind::Cone { r, tan } => {
                let rho = r + v * tan;
                Point3::new(rho * u.cos(), rho * u.sin(), v)
            }
            Kind::Sphere { r } => {
                Point3::new(r * v.cos() * u.cos(), r * v.cos() * u.sin(), r * v.sin())
            }
            Kind::Torus { big, small } => {
                let rho = big + small * v.cos();
                Point3::new(rho * u.cos(), rho * u.sin(), small * v.sin())
            }
            Kind::Extrusion { curve, dir } => return along(curve, u) + dir * v,
            Kind::Revolution {
                curve,
                origin,
                axis,
            } => {
                let q = along(curve, v) - origin;
                let (s, c) = u.sin_cos();
                return origin + q * c + axis.cross(&q) * s + axis * (axis.dot(&q) * (1.0 - c));
            }
            Kind::Spline(s) => return s.point_at(u, v),
        };
        self.frame.transform_point(&local)
    }

    /// The `(u, v)` of a point on the surface, unwrapped near `hint` for the periodic parameters.
    pub(crate) fn uv(&self, p: &Point3, hint: Option<(f64, f64)>) -> (f64, f64) {
        let (mut u, mut v) = match &self.kind {
            Kind::Cylinder { .. } | Kind::Cone { .. } => {
                let l = self.frame.inverse().transform_point(p);
                (angle(l.y, l.x, hint.map(|h| h.0)), l.z)
            }
            Kind::Sphere { .. } => {
                let l = self.frame.inverse().transform_point(p);
                let rho = l.x.hypot(l.y);
                (angle(l.y, l.x, hint.map(|h| h.0)), l.z.atan2(rho))
            }
            Kind::Torus { big, .. } => {
                let l = self.frame.inverse().transform_point(p);
                let rho = l.x.hypot(l.y);
                (angle(l.y, l.x, hint.map(|h| h.0)), l.z.atan2(rho - big))
            }
            Kind::Extrusion { curve, dir } => {
                let flat = |q: &Point3| q.coords - dir * dir.dot(&q.coords);
                let s = nearest(curve, &flat(p), &flat);
                (s, (p - along(curve, s)).dot(dir))
            }
            Kind::Revolution {
                curve,
                origin,
                axis,
            } => {
                let q = p - origin;
                let (z, rho) = (axis.dot(&q), (q - axis * axis.dot(&q)).norm());
                let radial = |c: &Point3| {
                    let w = c - origin;
                    Vector3::new(axis.dot(&w), (w - axis * axis.dot(&w)).norm(), 0.0)
                };
                let s = nearest(curve, &Vector3::new(z, rho, 0.0), &radial);
                let c = along(curve, s) - origin;
                let (cr, pr) = (c - axis * axis.dot(&c), q - axis * axis.dot(&q));
                let theta = axis.dot(&cr.cross(&pr)).atan2(cr.dot(&pr));
                (theta, s)
            }
            Kind::Spline(s) => spline_uv(s, p, hint),
        };
        let (pu, pv) = self.periods();
        if let (Some(period), Some(h)) = (pu, hint) {
            u = unwrap_near(u, h.0, period);
        }
        if let (Some(period), Some(h)) = (pv, hint) {
            v = unwrap_near(v, h.1, period);
        }
        (u, v)
    }

    /// The lengths per unit of `u` and of `v` near `(u, v)`, for scaling the parameters to a metric.
    pub(crate) fn scales(&self, u: f64, v: f64) -> (f64, f64) {
        let h = 1e-3;
        let p = self.point(u, v);
        let su = (self.point(u + h, v) - p).norm() / h;
        let sv = (self.point(u, v + h) - p).norm() / h;
        let fix = |x: f64| if x.is_finite() && x > 1e-9 { x } else { 1.0 };
        (fix(su), fix(sv))
    }

    /// The `u` and `v` values of a lattice of interior points over the box, for the surfaces whose
    /// flat triangles would leave the surface by more than `chord`; `None` for a ruled surface,
    /// whose boundary points are enough.
    pub(crate) fn lattice(
        &self,
        chord: f64,
        lo: (f64, f64),
        hi: (f64, f64),
    ) -> Option<(Vec<f64>, Vec<f64>)> {
        let step = |radius: f64| {
            2.0 * (1.0 - chord / radius.max(chord * 1.0001))
                .clamp(-1.0, 1.0)
                .acos()
        };
        let (du, dv) = match &self.kind {
            Kind::Sphere { r } => (step(*r), step(*r)),
            Kind::Torus { big, small } => (step(big + small), step(*small)),
            Kind::Revolution {
                curve,
                origin,
                axis,
            } => {
                let widest = curve
                    .iter()
                    .map(|c| {
                        let w = c - origin;
                        (w - axis * axis.dot(&w)).norm()
                    })
                    .fold(0.0, f64::max);
                (step(widest), 1.0)
            }
            Kind::Spline(s) => {
                let (us, vs, _) = s.grid(chord);
                let keep = |values: Vec<f64>, a: f64, b: f64| -> Vec<f64> {
                    values.into_iter().filter(|x| *x > a && *x < b).collect()
                };
                return Some((keep(us, lo.0, hi.0), keep(vs, lo.1, hi.1)));
            }
            _ => return None,
        };
        let lattice = |a: f64, b: f64, d: f64| -> Vec<f64> {
            if d.is_nan() || d <= 1e-4 {
                return Vec::new();
            }
            let n = ((b - a) / d).ceil().max(1.0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bounded below"
            )]
            let n = n.min(4096.0) as usize;
            (1..n)
                .map(|i| a + (b - a) * (i as f64) / (n as f64))
                .collect()
        };
        Some((lattice(lo.0, hi.0, du), lattice(lo.1, hi.1, dv)))
    }

    /// The `v` of the degenerate row (a pole) toward which `v` increases when `up`, if any.
    pub(crate) fn pole(&self, up: bool) -> Option<f64> {
        match &self.kind {
            Kind::Sphere { .. } => Some(if up { PI / 2.0 } else { -PI / 2.0 }),
            Kind::Cone { r, tan } if tan.abs() > 1e-12 => {
                let apex = -r / tan;
                // The radius shrinks toward the apex: toward +v when tan < 0.
                ((*tan < 0.0) == up).then_some(apex)
            }
            _ => None,
        }
    }
}

fn angle(y: f64, x: f64, hint: Option<f64>) -> f64 {
    if x == 0.0 && y == 0.0 {
        hint.unwrap_or(0.0)
    } else {
        y.atan2(x)
    }
}

/// The parameters of the point of a B-spline surface nearest to `p`: the best of a grid of samples,
/// refined by a few steps of coordinate descent.
fn spline_uv(s: &BSplineSurface, p: &Point3, hint: Option<(f64, f64)>) -> (f64, f64) {
    let ((u0, u1), (v0, v1)) = s.domain();
    let (us, vs, _) = s.grid(1.0e30);
    let (mut us, mut vs) = (us, vs);
    for k in 0..=8 {
        us.push(u0 + (u1 - u0) * f64::from(k) / 8.0);
        vs.push(v0 + (v1 - v0) * f64::from(k) / 8.0);
    }
    let mut best = (f64::MAX, (u0, v0));
    let seeds = hint
        .into_iter()
        .chain(us.iter().flat_map(|&u| vs.iter().map(move |&v| (u, v))));
    for (u, v) in seeds {
        let d = (s.point_at(u, v) - p).norm_squared();
        if d < best.0 {
            best = (d, (u, v));
        }
    }
    let (mut u, mut v) = best.1;
    let (mut su, mut sv) = ((u1 - u0) / 16.0, (v1 - v0) / 16.0);
    for _ in 0..40 {
        let mut moved = false;
        for (du, dv) in [(su, 0.0), (-su, 0.0), (0.0, sv), (0.0, -sv)] {
            let (nu, nv) = ((u + du).clamp(u0, u1), (v + dv).clamp(v0, v1));
            let d = (s.point_at(nu, nv) - p).norm_squared();
            if d < best.0 {
                best = (d, (nu, nv));
                (u, v) = (nu, nv);
                moved = true;
            }
        }
        if !moved {
            su *= 0.5;
            sv *= 0.5;
        }
    }
    (u, v)
}
