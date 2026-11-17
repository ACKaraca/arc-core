//! Roots of trigonometric polynomials of degree 2 on one period.
//!
//! The conic intersections reduce to such polynomials: one conic written implicitly and the
//! other substituted by its eccentric-angle parameterization gives
//! `k0 + k1·cos t + k2·sin t + k3·cos 2t + k4·sin 2t`, and so does the squared distance of a
//! point to an ellipse after differentiation. A nonzero polynomial of this form has at most four
//! roots in a period.

use std::f64::consts::TAU;

use arc_base::{Error, Result};

use crate::error::codes;

/// The number of equal intervals the period is first divided into.
const INITIAL_INTERVALS: u32 = 16;
/// The width, in radians, below which an interval that can be neither excluded nor shown to be
/// monotone is reported as a (numerically double) root at its midpoint.
const MIN_WIDTH: f64 = 1e-10;
/// The cap on subdivision steps. Each of the at most four roots and four critical points costs
/// about two steps per halving from the initial width down to [`MIN_WIDTH`], far below this cap.
const MAX_STEPS: u32 = 20_000;
/// The cap on bisection steps; a bisection reaches adjacent floating-point numbers in at most
/// about 1100 halvings of `f64`, and 200 halvings of an interval no wider than 2π already reach
/// the resolution of `f64` near any angle.
const MAX_BISECTIONS: u32 = 200;

/// `k0 + k1·cos t + k2·sin t + k3·cos 2t + k4·sin 2t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Trig(pub(super) [f64; 5]);

impl Trig {
    pub(super) fn eval(&self, t: f64) -> f64 {
        let [k0, k1, k2, k3, k4] = self.0;
        let (sin, cos) = t.sin_cos();
        let (sin2, cos2) = (2.0 * t).sin_cos();
        k0 + k1 * cos + k2 * sin + k3 * cos2 + k4 * sin2
    }

    pub(super) fn derivative(&self) -> Self {
        let [_, k1, k2, k3, k4] = self.0;
        Self([0.0, k2, -k1, 2.0 * k4, -2.0 * k3])
    }

    /// The sum of the absolute coefficients: a bound on `|f|`.
    pub(super) fn abs_sum(&self) -> f64 {
        self.0.iter().map(|k| k.abs()).sum()
    }

    /// The sum of the absolute non-constant coefficients.
    pub(super) fn variation(&self) -> f64 {
        self.0.iter().skip(1).map(|k| k.abs()).sum()
    }
}

/// A root of `f` in `[x0, x1]`, where `f(x0)` (given as `f0`) and `f(x1)` have opposite signs.
pub(super) fn bisect(f: &Trig, mut x0: f64, mut x1: f64, f0: f64) -> f64 {
    let sign0 = f0.signum();
    for _ in 0..MAX_BISECTIONS {
        let m = 0.5 * (x0 + x1);
        if m <= x0 || m >= x1 {
            break;
        }
        let fm = f.eval(m);
        if fm == 0.0 {
            return m;
        }
        if fm.signum() == sign0 {
            x0 = m;
        } else {
            x1 = m;
        }
    }
    0.5 * (x0 + x1)
}

/// Every root of `f` in [0, 2π), sorted. A polynomial without non-constant terms has no
/// isolated roots and yields none.
///
/// The period is subdivided; an interval is dropped when a Taylor bound shows `f` has no zero
/// in it, and solved by bisection when a Taylor bound shows `f'` has no zero in it (so `f` is
/// monotone). An interval that is neither and narrower than [`MIN_WIDTH`] holds a numerically
/// double root, reported at its midpoint.
///
/// # Errors
///
/// `geom.precision.insufficient` when the subdivision does not finish within [`MAX_STEPS`],
/// which happens only when rounding hides the structure of the polynomial.
pub(super) fn roots(f: &Trig) -> Result<Vec<f64>> {
    if f.variation() == 0.0 {
        return Ok(Vec::new());
    }
    let d1 = f.derivative();
    let d2 = d1.derivative();
    // Bounds on |f''| and |f'''| over the whole period.
    let m2 = d2.abs_sum();
    let m3 = d2.derivative().abs_sum();
    let width = TAU / f64::from(INITIAL_INTERVALS);
    let mut stack: Vec<(f64, f64)> = (0..INITIAL_INTERVALS)
        .rev()
        .map(|i| (width * f64::from(i), width * f64::from(i + 1)))
        .collect();
    let mut found = Vec::new();
    let mut steps = 0;
    while let Some((x0, x1)) = stack.pop() {
        steps += 1;
        if steps > MAX_STEPS {
            return Err(Error::new(codes::PRECISION_INSUFFICIENT));
        }
        let h = x1 - x0;
        let m = 0.5 * (x0 + x1);
        let (fm, d1m) = (f.eval(m), d1.eval(m));
        // |f(x)| ≥ |f(m)| − |f'(m)|·h/2 − max|f''|·h²/8 on the interval.
        if fm.abs() > d1m.abs() * h / 2.0 + m2 * h * h / 8.0 {
            continue;
        }
        if d1m.abs() > d2.eval(m).abs() * h / 2.0 + m3 * h * h / 8.0 {
            // f' has no zero: f is monotone. The interval is half-open so a root on a shared
            // boundary is found once.
            let (f0, f1) = (f.eval(x0), f.eval(x1));
            if f0 == 0.0 {
                found.push(x0);
            } else if f1 != 0.0 && f0.signum() != f1.signum() {
                found.push(bisect(f, x0, x1, f0));
            }
            continue;
        }
        if h <= MIN_WIDTH {
            found.push(m);
            continue;
        }
        stack.push((m, x1));
        stack.push((x0, m));
    }
    // Roots closer than the resolution, or joined by a run where f rounds to exactly zero
    // (around a double root), are one root.
    let same = |x: f64, y: f64| {
        y - x <= 2.0 * MIN_WIDTH
            || (f.eval(x) == 0.0 && f.eval(y) == 0.0 && f.eval(0.5 * (x + y)) == 0.0)
    };
    found.sort_by(f64::total_cmp);
    found.dedup_by(|later, earlier| same(*earlier, *later));
    if found.len() > 1
        && let (Some(first), Some(last)) = (found.first(), found.last())
        && same(*last, first + TAU)
    {
        found.pop();
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn simple_and_double_roots_are_found() {
        // cos t has roots at π/2 and 3π/2.
        let roots = roots(&Trig([0.0, 1.0, 0.0, 0.0, 0.0])).unwrap();
        assert_eq!(roots.len(), 2);
        assert!((roots[0] - std::f64::consts::FRAC_PI_2).abs() < 1e-14);
        // 1 − cos t has a double root at 0 only.
        let double = super::roots(&Trig([1.0, -1.0, 0.0, 0.0, 0.0])).unwrap();
        assert_eq!(double.len(), 1);
        assert!(double[0].min(TAU - double[0]) < 1e-6);
        // A constant has no isolated roots, a positive polynomial none at all.
        assert!(super::roots(&Trig([0.0; 5])).unwrap().is_empty());
        assert!(
            super::roots(&Trig([3.0, 1.0, 0.5, 0.2, 0.1]))
                .unwrap()
                .is_empty()
        );
    }

    proptest! {
        #[test]
        fn roots_of_products_of_shifted_sines_are_recovered(
            a in 0.0..TAU, b in 0.0..TAU,
        ) {
            // sin(t − a)·sin(t − b) = (cos(a − b) − cos(2t − a − b)) / 2: roots a, b, a + π, b + π.
            let s = a + b;
            let f = Trig([0.5 * (a - b).cos(), 0.0, 0.0, -0.5 * s.cos(), -0.5 * s.sin()]);
            let found = roots(&f).unwrap();
            let gap = (a - b).rem_euclid(std::f64::consts::PI);
            prop_assume!(gap > 1e-3 && gap < std::f64::consts::PI - 1e-3);
            prop_assert_eq!(found.len(), 4);
            for y in [a, b, a + std::f64::consts::PI, b + std::f64::consts::PI] {
                let near = found.iter().any(|x| {
                    let d = (x - y).rem_euclid(TAU);
                    d.min(TAU - d) < 1e-9
                });
                prop_assert!(near, "{y} not among {found:?}");
            }
        }
    }
}
