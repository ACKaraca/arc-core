//! Parent curves of a curve segment, in their own intrinsic frame.
//!
//! Only the shape matters: a curve segment places the point at its start and the tangent there
//! with its `Placement`, so the `Position` of a parent curve drops out. A parent curve gives, for a
//! pair of parameters `s0` and `s`, the displacement from the point at `s0` to the point at `s` and
//! the heading at `s`, both in the frame of the parent. Lengths are in the unit of the file.

use std::f64::consts::PI;

use super::num::{heading_integral, sampled_bound};

/// The shape of a transition: how the curvature (or cant) moves from its start value to its end
/// value over the segment, as a function of `xi = s / L`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// Constant value: no change.
    Constant,
    /// Linear change (clothoid, linear transition).
    Linear,
    /// Helmert: two parabolas.
    Helmert,
    /// Bloss: `(3 - 2 xi) xi^2`.
    Bloss,
    /// Cosine: `(1 - cos(pi xi)) / 2`.
    Cosine,
    /// Sine: `xi - sin(2 pi xi) / (2 pi)`.
    Sine,
    /// Viennese bend: `xi^4 (35 - 84 xi + 70 xi^2 - 20 xi^3)`.
    Viennese,
}

impl Shape {
    /// The change fraction at `xi`, 0 at the start and 1 at the end.
    pub(crate) fn value(self, xi: f64) -> f64 {
        match self {
            Self::Constant => 0.0,
            Self::Linear => xi,
            Self::Helmert => {
                if xi < 0.5 {
                    2.0 * xi * xi
                } else {
                    1.0 - 2.0 * (1.0 - xi) * (1.0 - xi)
                }
            }
            Self::Bloss => (3.0 - 2.0 * xi) * xi * xi,
            Self::Cosine => 0.5 * (1.0 - (PI * xi).cos()),
            Self::Sine => xi - (2.0 * PI * xi).sin() / (2.0 * PI),
            Self::Viennese => xi.powi(4) * (35.0 - 84.0 * xi + 70.0 * xi * xi - 20.0 * xi.powi(3)),
        }
    }

    /// The integral of [`Self::value`] from 0 to `xi`.
    pub(crate) fn integral(self, xi: f64) -> f64 {
        match self {
            Self::Constant => 0.0,
            Self::Linear => 0.5 * xi * xi,
            Self::Helmert => {
                if xi < 0.5 {
                    2.0 * xi.powi(3) / 3.0
                } else {
                    1.0 / 12.0 + (xi - 0.5) + (2.0 / 3.0) * ((1.0 - xi).powi(3) - 0.125)
                }
            }
            Self::Bloss => xi.powi(3) - 0.5 * xi.powi(4),
            Self::Cosine => 0.5 * (xi - (PI * xi).sin() / PI),
            Self::Sine => 0.5 * xi * xi + ((2.0 * PI * xi).cos() - 1.0) / (4.0 * PI * PI),
            Self::Viennese => {
                7.0 * xi.powi(5) - 14.0 * xi.powi(6) + 10.0 * xi.powi(7) - 2.5 * xi.powi(8)
            }
        }
    }
}

/// A plane curve described by its heading as a function of the length `s` along it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Spiral {
    /// Curvature `sum c[k] s^k`: the clothoid and the second, third and seventh order polynomial
    /// spirals.
    Polynomial(Vec<f64>),
    /// `IfcCosineSpiral`: curvature `c0 + c1 cos(pi s / len)`.
    Cosine { c0: f64, c1: f64, len: f64 },
    /// `IfcSineSpiral`: heading `c0 s + t2 s^2 + t3 (1 - cos(2 pi s / len))`, so the curvature is
    /// `c0 + 2 t2 s + 2 pi t3 / len sin(2 pi s / len)`.
    Sine { c0: f64, t2: f64, t3: f64, len: f64 },
    /// A transition of an alignment layout: curvature `k1 + dk f(xi)`, and for the Viennese bend
    /// the extra term `-vc xi^2 (1 - 4 xi + 5 xi^2 - 2 xi^3)`, where `xi = s / len`.
    Blend {
        k1: f64,
        dk: f64,
        len: f64,
        shape: Shape,
        vc: f64,
    },
}

impl Spiral {
    /// The heading at `s`, 0 at `s = 0`.
    pub(crate) fn theta(&self, s: f64) -> f64 {
        match self {
            Self::Polynomial(c) => c
                .iter()
                .enumerate()
                .map(|(k, c)| c * s.powi(i32::try_from(k + 1).unwrap_or(i32::MAX)) / (k + 1) as f64)
                .sum(),
            Self::Cosine { c0, c1, len } => c0 * s + c1 * len / PI * (PI * s / len).sin(),
            Self::Sine { c0, t2, t3, len } => {
                c0 * s + t2 * s * s + t3 * (1.0 - (2.0 * PI * s / len).cos())
            }
            Self::Blend {
                k1,
                dk,
                len,
                shape,
                vc,
            } => {
                let xi = s / len;
                let base = k1 * xi + dk * shape.integral(xi);
                let extra = xi.powi(3) / 3.0 - xi.powi(4) + xi.powi(5) - xi.powi(6) / 3.0;
                len * (base - vc * extra)
            }
        }
    }

    /// An upper bound of the curvature magnitude on `[a, b]`, for the step of the integration.
    fn curvature_bound(&self, a: f64, b: f64) -> f64 {
        sampled_bound(&|s| self.curvature(s), a, b)
    }

    /// The curvature at `s`.
    pub(crate) fn curvature(&self, s: f64) -> f64 {
        match self {
            Self::Polynomial(c) => c.iter().rev().fold(0.0, |acc, c| acc * s + c),
            Self::Cosine { c0, c1, len } => c0 + c1 * (PI * s / len).cos(),
            Self::Sine { c0, t2, t3, len } => {
                c0 + 2.0 * t2 * s + 2.0 * PI * t3 / len * (2.0 * PI * s / len).sin()
            }
            Self::Blend {
                k1,
                dk,
                len,
                shape,
                vc,
            } => {
                let xi = s / len;
                let f = shape.value(xi);
                let g = xi * xi * (1.0 - 4.0 * xi + 5.0 * xi * xi - 2.0 * xi.powi(3));
                k1 + dk * f - vc * g
            }
        }
    }
}

/// A parent curve.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Parent {
    /// A straight line; the parameter is the length.
    Line,
    /// A circle of the given radius, counterclockwise for increasing length.
    Circle(f64),
    /// A polynomial curve `x(t), y(t)` by ascending coefficients; the parameter is `t`, or, when
    /// `by_length`, the length along the curve.
    Polynomial {
        x: Vec<f64>,
        y: Vec<f64>,
        by_length: bool,
    },
    /// A curve given by its heading.
    Spiral(Spiral),
}

fn horner(c: &[f64], t: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, c| acc * t + c)
}

fn derivative(c: &[f64]) -> Vec<f64> {
    c.iter()
        .enumerate()
        .skip(1)
        .map(|(k, c)| c * k as f64)
        .collect()
}

impl Parent {
    /// The displacement from the point at `s0` to the point at `s`, and the heading at `s`, in the
    /// frame of the curve (heading 0 at the start of a spiral or a line).
    pub(crate) fn delta(&self, s0: f64, s: f64) -> ([f64; 2], f64) {
        match self {
            Self::Line => ([s - s0, 0.0], 0.0),
            Self::Circle(r) => (
                [
                    r * ((s / r).sin() - (s0 / r).sin()),
                    r * ((s0 / r).cos() - (s / r).cos()),
                ],
                s / r,
            ),
            Self::Polynomial { x, y, by_length } => {
                let (t0, t) = if *by_length {
                    (self.param(x, y, s0), self.param(x, y, s))
                } else {
                    (s0, s)
                };
                let heading = horner(&derivative(y), t).atan2(horner(&derivative(x), t));
                (
                    [horner(x, t) - horner(x, t0), horner(y, t) - horner(y, t0)],
                    heading,
                )
            }
            Self::Spiral(spiral) => {
                let (lo, hi) = (s0.min(s), s0.max(s));
                let bound = spiral.curvature_bound(lo, hi);
                // The curvature of a Helmert transition has a kink at its middle; integrate each
                // half on its own.
                let mut knots = vec![s0];
                if let Spiral::Blend {
                    shape: Shape::Helmert,
                    len,
                    ..
                } = spiral
                    && lo < 0.5 * len
                    && 0.5 * len < hi
                {
                    knots.push(0.5 * len);
                }
                knots.push(s);
                let mut d = [0.0, 0.0];
                for pair in knots.windows(2) {
                    let part = heading_integral(&|u| spiral.theta(u), bound, pair[0], pair[1]);
                    d = [d[0] + part[0], d[1] + part[1]];
                }
                (d, spiral.theta(s))
            }
        }
    }

    /// The parameter `t` of a polynomial curve at the length `s` from `t = 0`, by Newton steps on
    /// the arc length.
    fn param(&self, x: &[f64], y: &[f64], s: f64) -> f64 {
        let (dx, dy) = (derivative(x), derivative(y));
        let speed = |t: f64| horner(&dx, t).hypot(horner(&dy, t));
        let length = |t: f64| gauss_speed(&speed, t);
        let mut t = s;
        for _ in 0..8 {
            let v = speed(t);
            if v <= f64::EPSILON {
                break;
            }
            let step = (length(t) - s) / v;
            t -= step;
            if step.abs() < 1e-14 * (1.0 + t.abs()) {
                break;
            }
        }
        t
    }
}

/// The integral of `speed` from 0 to `t` on 8 panels of the 8-point rule.
fn gauss_speed(speed: &dyn Fn(f64) -> f64, t: f64) -> f64 {
    const NODES: [f64; 4] = [
        0.183_434_642_495_649_8,
        0.525_532_409_916_329,
        0.796_666_477_413_626_7,
        0.960_289_856_497_536_3,
    ];
    const WEIGHTS: [f64; 4] = [
        0.362_683_783_378_362,
        0.313_706_645_877_887_3,
        0.222_381_034_453_374_47,
        0.101_228_536_290_376_26,
    ];
    let panels = 8usize;
    let h = t / panels as f64;
    let mut sum = 0.0;
    for i in 0..panels {
        let mid = h * (i as f64 + 0.5);
        for (n, w) in NODES.iter().zip(WEIGHTS) {
            sum += w * (speed(mid + 0.5 * h * n) + speed(mid - 0.5 * h * n));
        }
    }
    sum * 0.5 * h
}
