//! Physical dimensions and dimensioned quantities (CORE-BASE-005).

use std::cmp::Ordering;
use std::ops::{Div, Mul, Neg};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, codes};

/// The physical dimension of a quantity: integer exponents over the nine base dimensions
/// length, mass, time, electric current, temperature, amount of substance, luminous intensity,
/// angle and solid angle, in that order.
///
/// Exponents saturate at the bounds of `i8` instead of overflowing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Dimension([i8; 9]);

impl Dimension {
    /// A pure number.
    pub const NONE: Self = Self([0; 9]);
    /// Length (meter).
    pub const LENGTH: Self = Self::base(0);
    /// Mass (kilogram).
    pub const MASS: Self = Self::base(1);
    /// Time (second).
    pub const TIME: Self = Self::base(2);
    /// Electric current (ampere).
    pub const CURRENT: Self = Self::base(3);
    /// Thermodynamic temperature (kelvin).
    pub const TEMPERATURE: Self = Self::base(4);
    /// Amount of substance (mole).
    pub const AMOUNT: Self = Self::base(5);
    /// Luminous intensity (candela).
    pub const LUMINOUS_INTENSITY: Self = Self::base(6);
    /// Plane angle (radian).
    pub const ANGLE: Self = Self::base(7);
    /// Solid angle (steradian).
    pub const SOLID_ANGLE: Self = Self::base(8);

    /// Area (square meter).
    pub const AREA: Self = Self::LENGTH.powi(2);
    /// Volume (cubic meter).
    pub const VOLUME: Self = Self::LENGTH.powi(3);
    /// Velocity (meter per second).
    pub const VELOCITY: Self = Self::LENGTH.div(Self::TIME);
    /// Force (newton).
    pub const FORCE: Self = Self::MASS.mul(Self::LENGTH).div(Self::TIME.powi(2));
    /// Pressure (pascal).
    pub const PRESSURE: Self = Self::FORCE.div(Self::AREA);
    /// Density (kilogram per cubic meter).
    pub const DENSITY: Self = Self::MASS.div(Self::VOLUME);
    /// Energy (joule).
    pub const ENERGY: Self = Self::FORCE.mul(Self::LENGTH);
    /// Power (watt).
    pub const POWER: Self = Self::ENERGY.div(Self::TIME);
    /// Luminous flux (lumen).
    pub const LUMINOUS_FLUX: Self = Self::LUMINOUS_INTENSITY.mul(Self::SOLID_ANGLE);
    /// Illuminance (lux).
    pub const ILLUMINANCE: Self = Self::LUMINOUS_FLUX.div(Self::AREA);

    const fn base(index: usize) -> Self {
        let mut exponents = [0; 9];
        exponents[index] = 1;
        Self(exponents)
    }

    /// Creates a dimension from its nine exponents, in the order of the type description.
    #[must_use]
    pub const fn from_exponents(exponents: [i8; 9]) -> Self {
        Self(exponents)
    }

    /// The nine exponents, in the order of the type description.
    #[must_use]
    pub const fn exponents(self) -> [i8; 9] {
        self.0
    }

    /// The dimension of a product: exponents add.
    #[must_use]
    pub const fn mul(self, other: Self) -> Self {
        let mut out = self.0;
        let mut i = 0;
        while i < out.len() {
            out[i] = out[i].saturating_add(other.0[i]);
            i += 1;
        }
        Self(out)
    }

    /// The dimension of a quotient: exponents subtract.
    #[must_use]
    pub const fn div(self, other: Self) -> Self {
        let mut out = self.0;
        let mut i = 0;
        while i < out.len() {
            out[i] = out[i].saturating_sub(other.0[i]);
            i += 1;
        }
        Self(out)
    }

    /// The dimension of an integer power: exponents multiply by `power`.
    #[must_use]
    pub const fn powi(self, power: i8) -> Self {
        let mut out = self.0;
        let mut i = 0;
        while i < out.len() {
            out[i] = out[i].saturating_mul(power);
            i += 1;
        }
        Self(out)
    }
}

/// A value in SI base units together with its dimension.
///
/// Multiplication and division combine dimensions and are available as operators. Addition,
/// subtraction and comparison require equal dimensions and are fallible methods.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quantity {
    value: f64,
    dimension: Dimension,
}

impl Quantity {
    /// Creates a quantity from a value in SI base units.
    #[must_use]
    pub const fn new(value: f64, dimension: Dimension) -> Self {
        Self { value, dimension }
    }

    /// The value in SI base units.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// The dimension.
    #[must_use]
    pub const fn dimension(self) -> Dimension {
        self.dimension
    }

    /// Fails with `base.unit.dimension_mismatch` unless the quantity has `dimension`.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the dimensions differ.
    pub fn expect_dimension(self, dimension: Dimension) -> Result<Self> {
        if self.dimension == dimension {
            Ok(self)
        } else {
            Err(Error::new(codes::UNIT_DIMENSION_MISMATCH))
        }
    }

    /// The sum of two quantities of the same dimension.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the dimensions differ.
    pub fn try_add(self, other: Self) -> Result<Self> {
        other.expect_dimension(self.dimension)?;
        Ok(Self::new(self.value + other.value, self.dimension))
    }

    /// The difference of two quantities of the same dimension.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the dimensions differ.
    pub fn try_sub(self, other: Self) -> Result<Self> {
        other.expect_dimension(self.dimension)?;
        Ok(Self::new(self.value - other.value, self.dimension))
    }

    /// Compares two quantities of the same dimension; `None` when a value is not a number.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the dimensions differ.
    pub fn try_cmp(self, other: Self) -> Result<Option<Ordering>> {
        other.expect_dimension(self.dimension)?;
        Ok(self.value.partial_cmp(&other.value))
    }
}

impl Mul for Quantity {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.value * other.value,
            self.dimension.mul(other.dimension),
        )
    }
}

impl Div for Quantity {
    type Output = Self;

    fn div(self, other: Self) -> Self {
        Self::new(
            self.value / other.value,
            self.dimension.div(other.dimension),
        )
    }
}

impl Mul<f64> for Quantity {
    type Output = Self;

    fn mul(self, factor: f64) -> Self {
        Self::new(self.value * factor, self.dimension)
    }
}

impl Div<f64> for Quantity {
    type Output = Self;

    fn div(self, divisor: f64) -> Self {
        Self::new(self.value / divisor, self.dimension)
    }
}

impl Neg for Quantity {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.value, self.dimension)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::{Strategy, prop_assert, prop_assert_eq, proptest};

    use super::*;

    /// Dimensions whose exponents are small enough that no combination below saturates.
    fn dimension() -> impl Strategy<Value = Dimension> {
        proptest::array::uniform9(-20_i8..=20).prop_map(Dimension::from_exponents)
    }

    fn value() -> impl Strategy<Value = f64> {
        -1e100_f64..1e100
    }

    #[test]
    fn derived_dimensions_have_the_si_exponents() {
        assert_eq!(Dimension::AREA.exponents(), [2, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(Dimension::FORCE.exponents(), [1, 1, -2, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            Dimension::PRESSURE.exponents(),
            [-1, 1, -2, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(Dimension::POWER.exponents(), [2, 1, -3, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            Dimension::ILLUMINANCE.exponents(),
            [-2, 0, 0, 0, 0, 0, 1, 0, 1]
        );
        assert_eq!(Dimension::default(), Dimension::NONE);
    }

    #[test]
    fn exponents_saturate_instead_of_overflowing() {
        let huge = Dimension::LENGTH.powi(i8::MAX);
        assert_eq!(huge.mul(huge).exponents()[0], i8::MAX);
        assert_eq!(Dimension::NONE.div(huge).div(huge).exponents()[0], i8::MIN);
    }

    #[test]
    fn adding_and_comparing_different_dimensions_fails() {
        let length = Quantity::new(1.0, Dimension::LENGTH);
        let time = Quantity::new(1.0, Dimension::TIME);
        assert_eq!(
            length.try_add(time).unwrap_err().code(),
            "base.unit.dimension_mismatch"
        );
        assert_eq!(
            length.try_sub(time).unwrap_err().code(),
            "base.unit.dimension_mismatch"
        );
        assert_eq!(
            length.try_cmp(time).unwrap_err().code(),
            "base.unit.dimension_mismatch"
        );
    }

    #[test]
    fn arithmetic_on_matching_dimensions_works() {
        let a = Quantity::new(3.0, Dimension::LENGTH);
        let b = Quantity::new(2.0, Dimension::LENGTH);
        assert_eq!(a.try_add(b).unwrap(), Quantity::new(5.0, Dimension::LENGTH));
        assert_eq!(a.try_sub(b).unwrap(), Quantity::new(1.0, Dimension::LENGTH));
        assert_eq!(a.try_cmp(b).unwrap(), Some(Ordering::Greater));
        assert_eq!(a * b, Quantity::new(6.0, Dimension::AREA));
        assert_eq!(a / b, Quantity::new(1.5, Dimension::NONE));
        assert_eq!(a * 2.0, Quantity::new(6.0, Dimension::LENGTH));
        assert_eq!(a / 2.0, Quantity::new(1.5, Dimension::LENGTH));
        assert_eq!(-a, Quantity::new(-3.0, Dimension::LENGTH));
        let nan = Quantity::new(f64::NAN, Dimension::LENGTH);
        assert_eq!(a.try_cmp(nan).unwrap(), None);
    }

    #[test]
    fn speed_is_length_over_time() {
        let speed = Quantity::new(100.0, Dimension::LENGTH) / Quantity::new(9.58, Dimension::TIME);
        assert_eq!(speed.dimension(), Dimension::VELOCITY);
    }

    proptest! {
        #[test]
        fn dimension_product_is_commutative_and_associative(
            a in dimension(), b in dimension(), c in dimension(),
        ) {
            prop_assert_eq!(a.mul(b), b.mul(a));
            prop_assert_eq!(a.mul(b).mul(c), a.mul(b.mul(c)));
        }

        #[test]
        fn dimension_division_inverts_multiplication(a in dimension(), b in dimension()) {
            prop_assert_eq!(a.mul(b).div(b), a);
            prop_assert_eq!(a.div(a), Dimension::NONE);
            prop_assert_eq!(a.mul(Dimension::NONE), a);
        }

        #[test]
        fn dimension_powers_are_repeated_products(a in dimension(), n in 0_i8..=5) {
            let mut product = Dimension::NONE;
            for _ in 0..n {
                product = product.mul(a);
            }
            prop_assert_eq!(a.powi(n), product);
            prop_assert_eq!(a.powi(-1), Dimension::NONE.div(a));
        }

        #[test]
        fn quantity_products_combine_dimensions(
            x in value(), y in value(), a in dimension(), b in dimension(),
        ) {
            let (p, q) = (Quantity::new(x, a), Quantity::new(y, b));
            prop_assert_eq!((p * q).dimension(), a.mul(b));
            prop_assert_eq!((p / q).dimension(), a.div(b));
            prop_assert_eq!(p * q, q * p);
        }

        #[test]
        fn addition_needs_equal_dimensions(
            x in value(), y in value(), a in dimension(), b in dimension(),
        ) {
            let (p, q) = (Quantity::new(x, a), Quantity::new(y, b));
            prop_assert_eq!(p.try_add(q).is_ok(), a == b);
            prop_assert_eq!(p.try_cmp(q).is_ok(), a == b);
            let sum = p.try_add(Quantity::new(y, a)).unwrap();
            prop_assert_eq!(sum, Quantity::new(y, a).try_add(p).unwrap());
            prop_assert!(sum.dimension() == a);
        }

        #[test]
        fn quantities_survive_serialization(x in value(), a in dimension()) {
            let quantity = Quantity::new(x, a);
            let json = serde_json::to_string(&quantity).unwrap();
            let back: Quantity = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(back, quantity);
        }
    }
}
