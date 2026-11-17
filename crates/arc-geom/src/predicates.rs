//! Robust orientation and in-circle predicates (CORE-GEOM-031).
//!
//! Both use the adaptive exact arithmetic of the `robust` crate, which returns the exact sign
//! when no intermediate value overflows or underflows. To keep every intermediate value in
//! range, all coordinates are first multiplied by one common power of two, which changes no
//! sign and is exact as long as no scaled coordinate overflows or becomes subnormal.
//!
//! The guarantee is stated in terms of two exponents of the set of coordinates: `E`, the
//! exponent of the largest coordinate magnitude (`2^E ≤ |x| < 2^(E+1)`), and `L`, the exponent
//! of the lowest set bit of any nonzero coordinate (`L` is −1074 for the smallest subnormal and
//! 0 for an odd integer). Zero coordinates are ignored, and all-zero input is exact.
//!
//! - [`orient2d`] is exact when `E − L ≤ 957`; for coordinates with full 53-bit significands that
//!   is a ratio of up to about 2^905 (1e272) between the largest and the smallest magnitude.
//! - [`incircle`] is exact when `E − L ≤ 477`; for full significands that is a ratio of up to
//!   about 2^425 (1e128).
//!
//! Beyond that span no single scale keeps both the largest products in range and the smallest
//! bits representable, and the predicate fails with `geom.precision.insufficient`. `E` and `L`
//! depend only on the set of coordinate values, so for the same points every argument order
//! either returns consistently permuted signs or fails.

use std::ops::Neg;

use arc_base::math::Point2;
use arc_base::{Error, Result};
use robust::Coord;
use serde::{Deserialize, Serialize};

use crate::error::codes;

/// The largest exponent of any scaled intermediate product: products of degree n stay below
/// `2^CEILING`, leaving 64 bits of headroom below the `f64` maximum for sums, the splitting
/// constant of the exact arithmetic and differences of coordinates.
const CEILING: i32 = f64::MAX_EXP - 1 - 64;

/// The smallest exponent any bit of a scaled intermediate product may have: 64 bits above the
/// smallest normal exponent, so that no intermediate value or error bound is subnormal.
const FLOOR: i32 = f64::MIN_EXP - 1 + 64;

/// The exact sign of a predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    /// Below zero: clockwise, or outside the circle.
    Negative,
    /// Exactly zero: collinear, or on the circle.
    Zero,
    /// Above zero: counterclockwise, or inside the circle.
    Positive,
}

impl Neg for Sign {
    type Output = Self;

    fn neg(self) -> Self {
        match self {
            Self::Negative => Self::Positive,
            Self::Zero => Self::Zero,
            Self::Positive => Self::Negative,
        }
    }
}

/// The number of stored fraction bits of an `f64`.
const FRACTION_BITS: u32 = f64::MANTISSA_DIGITS - 1;

/// The exponent bias of an `f64`.
const BIAS: i32 = f64::MAX_EXP - 1;

/// The exponents of the highest and the lowest set bit of a nonzero finite `x`.
fn bit_range(x: f64) -> Option<(i32, i32)> {
    if x == 0.0 || !x.is_finite() {
        return None;
    }
    let bits = x.to_bits();
    let fraction = bits & ((1_u64 << FRACTION_BITS) - 1);
    // The biased exponent has 11 bits, so it always fits an i32.
    let biased = i32::try_from((bits >> FRACTION_BITS) & 0x7ff).unwrap_or(0);
    // A subnormal has the exponent of the smallest normal and no implicit leading bit.
    let (significand, biased) = if biased == 0 {
        (fraction, 1)
    } else {
        (fraction | (1_u64 << FRACTION_BITS), biased)
    };
    let unit = biased - BIAS - FRACTION_BITS.cast_signed();
    let low = unit + significand.trailing_zeros().cast_signed();
    let high = unit + 63 - significand.leading_zeros().cast_signed();
    Some((high, low))
}

/// `x · 2^k`, exact when the result is a normal number.
fn scale_by_power_of_two(x: f64, k: i32) -> f64 {
    // 2^e for e in the normal exponent range, built from its bits.
    let power = |e: i32| f64::from_bits(u64::from((e + BIAS).cast_unsigned()) << FRACTION_BITS);
    // Two factors keep each power in the normal range for every k used here (|k| < 2044).
    let first = k / 2;
    x * power(first) * power(k - first)
}

/// The coordinates scaled by one common power of two so that a homogeneous polynomial of
/// `degree` in them is evaluated exactly, or `geom.precision.insufficient` when no such scale
/// exists.
fn prescaled<const N: usize>(points: [&Point2; N], degree: i32) -> Result<[Coord<f64>; N]> {
    if !points.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return Err(Error::new(codes::INPUT_NOT_FINITE));
    }
    let range = points
        .iter()
        .flat_map(|p| [p.x, p.y])
        .filter_map(bit_range)
        .reduce(|(high, low), (h, l)| (high.max(h), low.min(l)));
    let Some((high, low)) = range else {
        // Every coordinate is zero.
        return Ok(points.map(|p| Coord { x: p.x, y: p.y }));
    };
    // Put the largest magnitude just below 2^(CEILING / degree), then check that the lowest
    // bit of every degree-n product stays above 2^FLOOR.
    let k = CEILING.div_euclid(degree) - (high + 1);
    if degree * (low + k) < FLOOR {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT)
            .with_detail("exponent_span", i64::from(high - low)));
    }
    Ok(points.map(|p| Coord {
        x: scale_by_power_of_two(p.x, k),
        y: scale_by_power_of_two(p.y, k),
    }))
}

fn sign_of(value: f64) -> Result<Sign> {
    if value > 0.0 {
        Ok(Sign::Positive)
    } else if value < 0.0 {
        Ok(Sign::Negative)
    } else if value == 0.0 {
        Ok(Sign::Zero)
    } else {
        // NaN: cannot happen after prescaling; reported rather than guessed.
        Err(Error::new(codes::PRECISION_INSUFFICIENT))
    }
}

/// The orientation of the triangle `a`, `b`, `c`: positive when counterclockwise, negative when
/// clockwise, zero when the points are collinear.
///
/// The sign is exact under the condition in the [module documentation](self).
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite coordinate; `geom.precision.insufficient` when the
/// coordinates span more binary exponents than the exact computation can hold.
pub fn orient2d(a: &Point2, b: &Point2, c: &Point2) -> Result<Sign> {
    let [a, b, c] = prescaled([a, b, c], 2)?;
    sign_of(robust::orient2d(a, b, c))
}

/// The position of `d` relative to the circle through `a`, `b`, `c`, which must be in
/// counterclockwise order: positive inside, negative outside, zero on the circle. Clockwise
/// order flips the sign.
///
/// The sign is exact under the condition in the [module documentation](self).
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite coordinate; `geom.precision.insufficient` when the
/// coordinates span more binary exponents than the exact computation can hold.
pub fn incircle(a: &Point2, b: &Point2, c: &Point2, d: &Point2) -> Result<Sign> {
    let [a, b, c, d] = prescaled([a, b, c, d], 4)?;
    sign_of(robust::incircle(a, b, c, d))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testing::point;

    /// Points that are often exactly or nearly collinear or cocircular.
    fn tricky_point() -> impl Strategy<Value = Point2> {
        prop_oneof![
            point(),
            (0.0..1.0_f64).prop_map(|t| Point2::new(0.5 + t * 12.0, 0.5 + t * 24.0)),
            (-5_i32..5).prop_map(|k| Point2::new(f64::from(k), 1.0)),
            (0.0..std::f64::consts::TAU).prop_map(|a| Point2::new(a.cos(), a.sin())),
        ]
    }

    #[test]
    fn known_orientations() {
        let (o, x, y) = (
            Point2::origin(),
            Point2::new(1.0, 0.0),
            Point2::new(0.0, 1.0),
        );
        assert_eq!(orient2d(&o, &x, &y).unwrap(), Sign::Positive);
        assert_eq!(orient2d(&o, &y, &x).unwrap(), Sign::Negative);
        assert_eq!(
            orient2d(&o, &x, &Point2::new(3.0, 0.0)).unwrap(),
            Sign::Zero
        );
        // Nearly collinear points whose naive determinant rounds to the wrong sign or to zero.
        let a = Point2::new(0.5, 0.5);
        let b = Point2::new(12.0, 12.0);
        let c = Point2::new(24.0, 24.0);
        assert_eq!(orient2d(&a, &b, &c).unwrap(), Sign::Zero);
        let c = Point2::new(24.0, 24.000_000_000_000_004);
        assert_eq!(orient2d(&a, &b, &c).unwrap(), Sign::Positive);
        let west = Point2::new(-1.0, 0.0);
        assert_eq!(incircle(&x, &y, &west, &o).unwrap(), Sign::Positive);
        assert_eq!(
            incircle(&x, &y, &west, &Point2::new(0.0, -1.0)).unwrap(),
            Sign::Zero
        );
        assert_eq!(
            incircle(&x, &y, &west, &Point2::new(5.0, 5.0)).unwrap(),
            Sign::Negative
        );
    }

    #[test]
    fn non_finite_input_is_reported() {
        let o = Point2::origin();
        let bad = Point2::new(f64::NAN, 0.0);
        assert_eq!(
            orient2d(&o, &o, &bad).unwrap_err().code(),
            "geom.input.not_finite"
        );
        assert_eq!(
            incircle(&o, &o, &o, &Point2::new(0.0, f64::INFINITY))
                .unwrap_err()
                .code(),
            "geom.input.not_finite"
        );
    }

    const INSUFFICIENT: &str = "geom.precision.insufficient";

    #[test]
    fn tiny_and_huge_coordinates_are_decided_exactly() {
        let o = Point2::origin();
        for s in [
            5e-324,
            1e-300,
            1e-200,
            1e-160,
            1e-150,
            1e150,
            1e160,
            1e300,
            f64::MAX,
        ] {
            let (x, y) = (Point2::new(s, 0.0), Point2::new(0.0, s));
            assert_eq!(orient2d(&o, &x, &y).unwrap(), Sign::Positive, "{s:e}");
            assert_eq!(orient2d(&o, &y, &x).unwrap(), Sign::Negative);
            let west = Point2::new(-s, 0.0);
            assert_eq!(incircle(&x, &y, &west, &o).unwrap(), Sign::Positive);
            assert_eq!(
                incircle(&x, &y, &west, &Point2::new(0.0, -s)).unwrap(),
                Sign::Zero
            );
            let south = Point2::new(0.0, -0.5 * s);
            for [a, b, c] in [[x, y, west], [y, west, x], [west, x, y]] {
                assert_eq!(incircle(&a, &b, &c, &south).unwrap(), Sign::Positive);
                assert_eq!(orient2d(&a, &b, &c).unwrap(), Sign::Positive);
            }
        }
    }

    #[test]
    fn the_documented_exponent_spans_are_the_limits() {
        let o = Point2::origin();
        let big = |e: i32| scale_by_power_of_two(1.0, e);
        // orient2d: E − L ≤ 957.
        let a = Point2::new(big(500), 0.0);
        assert!(orient2d(&o, &a, &Point2::new(0.0, big(-457))).is_ok());
        assert_eq!(
            orient2d(&o, &a, &Point2::new(0.0, big(-458)))
                .unwrap_err()
                .code(),
            INSUFFICIENT
        );
        // incircle: E − L ≤ 477.
        let (b, c) = (Point2::new(0.0, 1.0), Point2::new(-1.0, 0.0));
        let far = Point2::new(big(240), big(240));
        assert!(incircle(&Point2::new(big(-237), 0.0), &b, &c, &far).is_ok());
        assert_eq!(
            incircle(&Point2::new(big(-238), 0.0), &b, &c, &far)
                .unwrap_err()
                .code(),
            INSUFFICIENT
        );
        // A point far out with unit points: 1e100 is decided, 1e160 is not, in every order.
        let a = Point2::new(1.0, 0.0);
        let d = Point2::new(1e100, 1e100);
        assert_eq!(incircle(&a, &b, &c, &d).unwrap(), Sign::Negative);
        let d = Point2::new(1e160, 1e160);
        for [p, q, r, s] in [[a, b, c, d], [b, c, a, d], [d, b, c, a], [a, d, c, b]] {
            assert_eq!(incircle(&p, &q, &r, &s).unwrap_err().code(), INSUFFICIENT);
        }
    }

    #[test]
    fn bit_ranges_are_exact() {
        assert_eq!(bit_range(1.0), Some((0, 0)));
        assert_eq!(bit_range(-3.0), Some((1, 0)));
        assert_eq!(bit_range(0.75), Some((-1, -2)));
        assert_eq!(bit_range(f64::from_bits(1)), Some((-1074, -1074)));
        assert_eq!(bit_range(f64::MAX), Some((1023, 971)));
        assert_eq!(bit_range(f64::MIN_POSITIVE), Some((-1022, -1022)));
        assert_eq!(bit_range(0.0), None);
    }

    /// Any finite value, with exponents spread across the whole range of `f64`, or zero.
    fn any_coordinate() -> impl Strategy<Value = f64> {
        prop_oneof![
            4 => any::<u64>().prop_map(f64::from_bits).prop_filter("finite", |x| x.is_finite()),
            1 => Just(0.0),
        ]
    }

    /// Points with coordinates anywhere in the range of `f64`, and nearly degenerate points
    /// scaled together by one power of two.
    fn wide_points<const N: usize>() -> impl Strategy<Value = [Point2; N]> {
        prop_oneof![
            prop::array::uniform::<_, N>((any_coordinate(), any_coordinate()))
                .prop_map(|a| a.map(|(x, y)| Point2::new(x, y))),
            (
                prop::array::uniform::<_, N>(tricky_point()),
                -1300_i32..1300
            )
                .prop_map(|(a, k)| {
                    a.map(|p| {
                        Point2::new(scale_by_power_of_two(p.x, k), scale_by_power_of_two(p.y, k))
                    })
                }),
        ]
        .prop_filter("finite", |a| {
            a.iter().all(|p| p.x.is_finite() && p.y.is_finite())
        })
    }

    /// Whether the documented exponent span allows an exact answer for `points`.
    fn within_span(points: &[Point2], limit: i32) -> bool {
        points
            .iter()
            .flat_map(|p| [p.x, p.y])
            .filter_map(bit_range)
            .reduce(|(h, l), (h2, l2)| (h.max(h2), l.min(l2)))
            .is_none_or(|(high, low)| high - low <= limit)
    }

    fn signed(result: Result<Sign>) -> std::result::Result<Sign, &'static str> {
        result.map_err(|error| error.code())
    }

    proptest! {
        #[test]
        fn wide_orientations_are_permutation_consistent_or_fail_alike([a, b, c] in wide_points::<3>()) {
            let s = signed(orient2d(&a, &b, &c));
            prop_assert_eq!(s.is_ok(), within_span(&[a, b, c], 957));
            if let Err(code) = s {
                prop_assert_eq!(code, INSUFFICIENT);
            }
            let negated = s.map(|sign| -sign);
            prop_assert_eq!(signed(orient2d(&b, &c, &a)), s);
            prop_assert_eq!(signed(orient2d(&c, &a, &b)), s);
            prop_assert_eq!(signed(orient2d(&b, &a, &c)), negated);
            prop_assert_eq!(signed(orient2d(&a, &c, &b)), negated);
            prop_assert_eq!(signed(orient2d(&c, &b, &a)), negated);
        }

        #[test]
        fn wide_incircles_are_permutation_consistent_or_fail_alike([a, b, c, d] in wide_points::<4>()) {
            let s = signed(incircle(&a, &b, &c, &d));
            prop_assert_eq!(s.is_ok(), within_span(&[a, b, c, d], 477));
            if let Err(code) = s {
                prop_assert_eq!(code, INSUFFICIENT);
            }
            let negated = s.map(|sign| -sign);
            prop_assert_eq!(signed(incircle(&b, &c, &a, &d)), s);
            prop_assert_eq!(signed(incircle(&c, &a, &b, &d)), s);
            prop_assert_eq!(signed(incircle(&b, &a, &c, &d)), negated);
            prop_assert_eq!(signed(incircle(&a, &b, &d, &c)), negated);
            prop_assert_eq!(signed(incircle(&d, &b, &c, &a)), negated);
        }

        #[test]
        fn exact_signs_do_not_change_under_power_of_two_scaling(
            a in tricky_point(), b in tricky_point(), c in tricky_point(), d in tricky_point(),
            k in -900_i32..1000,
        ) {
            let up = |p: Point2| Point2::new(scale_by_power_of_two(p.x, k), scale_by_power_of_two(p.y, k));
            prop_assert_eq!(orient2d(&up(a), &up(b), &up(c)).unwrap(), orient2d(&a, &b, &c).unwrap());
            prop_assert_eq!(
                incircle(&up(a), &up(b), &up(c), &up(d)).unwrap(),
                incircle(&a, &b, &c, &d).unwrap()
            );
        }

        #[test]
        fn orientation_is_consistent_under_permutation(
            a in tricky_point(), b in tricky_point(), c in tricky_point(),
        ) {
            let s = orient2d(&a, &b, &c).unwrap();
            prop_assert_eq!(orient2d(&b, &c, &a).unwrap(), s);
            prop_assert_eq!(orient2d(&c, &a, &b).unwrap(), s);
            prop_assert_eq!(orient2d(&b, &a, &c).unwrap(), -s);
            prop_assert_eq!(orient2d(&a, &c, &b).unwrap(), -s);
            prop_assert_eq!(orient2d(&c, &b, &a).unwrap(), -s);
        }

        #[test]
        fn incircle_is_consistent_under_permutation(
            a in tricky_point(), b in tricky_point(), c in tricky_point(), d in tricky_point(),
        ) {
            let s = incircle(&a, &b, &c, &d).unwrap();
            prop_assert_eq!(incircle(&b, &c, &a, &d).unwrap(), s);
            prop_assert_eq!(incircle(&c, &a, &b, &d).unwrap(), s);
            prop_assert_eq!(incircle(&b, &a, &c, &d).unwrap(), -s);
            prop_assert_eq!(incircle(&a, &c, &b, &d).unwrap(), -s);
            // The lifted determinant alternates in all four points.
            prop_assert_eq!(incircle(&a, &b, &d, &c).unwrap(), -s);
        }
    }
}
