//! Units, the built-in unit registry and unit conversion (CORE-BASE-006, CORE-BASE-007).

use std::f64::consts::PI;

use crate::error::{Error, Result, codes};
use crate::quantity::{Dimension, Quantity};

/// A unit of measurement: a value `v` in this unit is `v * scale + offset` in SI base units.
///
/// Products define domain units by constructing values of this type; the built-in units are in
/// [`Unit::all`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    /// The stable ASCII name, unique in the registry, for example `millimeter`.
    pub name: &'static str,
    /// The display symbol, unique in the registry, for example `mm`.
    pub symbol: &'static str,
    /// The dimension the unit measures.
    pub dimension: Dimension,
    /// The size of one unit in SI base units.
    pub scale: f64,
    /// The SI value of zero in this unit; non-zero only for temperature scales.
    pub offset: f64,
}

impl Unit {
    /// Converts a value in this unit to SI base units.
    #[must_use]
    pub fn to_si(&self, value: f64) -> f64 {
        value * self.scale + self.offset
    }

    /// Converts a value in SI base units to this unit.
    #[must_use]
    pub fn from_si(&self, value: f64) -> f64 {
        (value - self.offset) / self.scale
    }

    /// The quantity of `value` in this unit.
    #[must_use]
    pub fn quantity(&self, value: f64) -> Quantity {
        Quantity::new(self.to_si(value), self.dimension)
    }

    /// The value of a quantity in this unit.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the quantity has another dimension.
    pub fn value_of(&self, quantity: Quantity) -> Result<f64> {
        Ok(self.from_si(quantity.expect_dimension(self.dimension)?.value()))
    }

    /// Every built-in unit.
    #[must_use]
    pub fn all() -> &'static [Unit] {
        UNITS
    }

    /// Looks a built-in unit up by its name.
    ///
    /// # Errors
    ///
    /// `base.unit.unknown` when no built-in unit has the name.
    pub fn by_name(name: &str) -> Result<&'static Unit> {
        UNITS
            .iter()
            .find(|unit| unit.name == name)
            .ok_or_else(|| Error::new(codes::UNIT_UNKNOWN).with_arg("name", name))
    }

    /// Looks a built-in unit up by its symbol.
    ///
    /// # Errors
    ///
    /// `base.unit.unknown` when no built-in unit has the symbol.
    pub fn by_symbol(symbol: &str) -> Result<&'static Unit> {
        UNITS
            .iter()
            .find(|unit| unit.symbol == symbol)
            .ok_or_else(|| Error::new(codes::UNIT_UNKNOWN).with_arg("name", symbol))
    }
}

/// Converts a value between two units of the same dimension.
///
/// A value converted to another unit and back differs from the original by at most four units in
/// the last place; for units with an offset the bound is relative to the larger of the value and
/// the offset.
///
/// # Errors
///
/// `base.unit.dimension_mismatch` when the units measure different dimensions.
pub fn convert(value: f64, from: &Unit, to: &Unit) -> Result<f64> {
    if from.dimension == to.dimension {
        Ok(to.from_si(from.to_si(value)))
    } else {
        Err(Error::new(codes::UNIT_DIMENSION_MISMATCH))
    }
}

const fn unit(name: &'static str, symbol: &'static str, dimension: Dimension, scale: f64) -> Unit {
    Unit {
        name,
        symbol,
        dimension,
        scale,
        offset: 0.0,
    }
}

// Exact definitions the imperial units derive from.
const INCH: f64 = 0.0254;
const FOOT: f64 = 12.0 * INCH;
const YARD: f64 = 3.0 * FOOT;
const MILE: f64 = 1760.0 * YARD;
const POUND: f64 = 0.453_592_37;
const STANDARD_GRAVITY: f64 = 9.806_65;
const POUND_FORCE: f64 = POUND * STANDARD_GRAVITY;
const BTU: f64 = 1_055.055_852_62;
const HOUR: f64 = 3600.0;

use Dimension as D;

static UNITS: &[Unit] = &[
    // Length.
    unit("meter", "m", D::LENGTH, 1.0),
    unit("millimeter", "mm", D::LENGTH, 1e-3),
    unit("centimeter", "cm", D::LENGTH, 1e-2),
    unit("kilometer", "km", D::LENGTH, 1e3),
    unit("inch", "in", D::LENGTH, INCH),
    unit("foot", "ft", D::LENGTH, FOOT),
    unit("yard", "yd", D::LENGTH, YARD),
    unit("mile", "mi", D::LENGTH, MILE),
    unit("us_survey_foot", "ftUS", D::LENGTH, 1200.0 / 3937.0),
    // Area.
    unit("square_meter", "m²", D::AREA, 1.0),
    unit("square_millimeter", "mm²", D::AREA, 1e-6),
    unit("square_centimeter", "cm²", D::AREA, 1e-4),
    unit("square_kilometer", "km²", D::AREA, 1e6),
    unit("hectare", "ha", D::AREA, 1e4),
    unit("square_inch", "in²", D::AREA, INCH * INCH),
    unit("square_foot", "ft²", D::AREA, FOOT * FOOT),
    unit("square_yard", "yd²", D::AREA, YARD * YARD),
    unit("acre", "ac", D::AREA, 43_560.0 * FOOT * FOOT),
    // Volume.
    unit("cubic_meter", "m³", D::VOLUME, 1.0),
    unit("cubic_millimeter", "mm³", D::VOLUME, 1e-9),
    unit("cubic_centimeter", "cm³", D::VOLUME, 1e-6),
    unit("liter", "L", D::VOLUME, 1e-3),
    unit("cubic_inch", "in³", D::VOLUME, INCH * INCH * INCH),
    unit("cubic_foot", "ft³", D::VOLUME, FOOT * FOOT * FOOT),
    unit("cubic_yard", "yd³", D::VOLUME, YARD * YARD * YARD),
    unit("us_gallon", "gal", D::VOLUME, 231.0 * INCH * INCH * INCH),
    // Angle.
    unit("radian", "rad", D::ANGLE, 1.0),
    unit("degree", "°", D::ANGLE, PI / 180.0),
    unit("arcminute", "′", D::ANGLE, PI / 10_800.0),
    unit("arcsecond", "″", D::ANGLE, PI / 648_000.0),
    unit("gradian", "gon", D::ANGLE, PI / 200.0),
    // Mass.
    unit("kilogram", "kg", D::MASS, 1.0),
    unit("gram", "g", D::MASS, 1e-3),
    unit("tonne", "t", D::MASS, 1e3),
    unit("pound", "lb", D::MASS, POUND),
    unit("ounce", "oz", D::MASS, POUND / 16.0),
    unit("short_ton", "ton", D::MASS, 2000.0 * POUND),
    // Time.
    unit("second", "s", D::TIME, 1.0),
    unit("millisecond", "ms", D::TIME, 1e-3),
    unit("minute", "min", D::TIME, 60.0),
    unit("hour", "h", D::TIME, HOUR),
    unit("day", "d", D::TIME, 24.0 * HOUR),
    // Temperature.
    unit("kelvin", "K", D::TEMPERATURE, 1.0),
    Unit {
        offset: 273.15,
        ..unit("degree_celsius", "°C", D::TEMPERATURE, 1.0)
    },
    Unit {
        offset: 459.67 * 5.0 / 9.0,
        ..unit("degree_fahrenheit", "°F", D::TEMPERATURE, 5.0 / 9.0)
    },
    // Force.
    unit("newton", "N", D::FORCE, 1.0),
    unit("kilonewton", "kN", D::FORCE, 1e3),
    unit("pound_force", "lbf", D::FORCE, POUND_FORCE),
    unit("kip", "kip", D::FORCE, 1000.0 * POUND_FORCE),
    // Pressure.
    unit("pascal", "Pa", D::PRESSURE, 1.0),
    unit("kilopascal", "kPa", D::PRESSURE, 1e3),
    unit("megapascal", "MPa", D::PRESSURE, 1e6),
    unit("bar", "bar", D::PRESSURE, 1e5),
    unit(
        "pound_per_square_inch",
        "psi",
        D::PRESSURE,
        POUND_FORCE / (INCH * INCH),
    ),
    unit(
        "pound_per_square_foot",
        "psf",
        D::PRESSURE,
        POUND_FORCE / (FOOT * FOOT),
    ),
    // Velocity.
    unit("meter_per_second", "m/s", D::VELOCITY, 1.0),
    unit("kilometer_per_hour", "km/h", D::VELOCITY, 1e3 / HOUR),
    unit("foot_per_second", "ft/s", D::VELOCITY, FOOT),
    unit("mile_per_hour", "mph", D::VELOCITY, MILE / HOUR),
    // Density.
    unit("kilogram_per_cubic_meter", "kg/m³", D::DENSITY, 1.0),
    unit("gram_per_cubic_centimeter", "g/cm³", D::DENSITY, 1e3),
    unit(
        "pound_per_cubic_foot",
        "lb/ft³",
        D::DENSITY,
        POUND / (FOOT * FOOT * FOOT),
    ),
    // Energy.
    unit("joule", "J", D::ENERGY, 1.0),
    unit("kilojoule", "kJ", D::ENERGY, 1e3),
    unit("kilowatt_hour", "kWh", D::ENERGY, 1e3 * HOUR),
    unit("british_thermal_unit", "BTU", D::ENERGY, BTU),
    unit("foot_pound_force", "ft·lbf", D::ENERGY, FOOT * POUND_FORCE),
    // Power.
    unit("watt", "W", D::POWER, 1.0),
    unit("kilowatt", "kW", D::POWER, 1e3),
    unit("horsepower", "hp", D::POWER, 550.0 * FOOT * POUND_FORCE),
    unit("btu_per_hour", "BTU/h", D::POWER, BTU / HOUR),
    // Luminous quantities.
    unit("candela", "cd", D::LUMINOUS_INTENSITY, 1.0),
    unit("lumen", "lm", D::LUMINOUS_FLUX, 1.0),
    // Illuminance.
    unit("lux", "lx", D::ILLUMINANCE, 1.0),
    unit("foot_candle", "fc", D::ILLUMINANCE, 1.0 / (FOOT * FOOT)),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::{Strategy, prop_assert, prop_assert_eq, prop_oneof, proptest};

    use super::*;

    fn by_name(name: &str) -> &'static Unit {
        Unit::by_name(name).unwrap()
    }

    /// Distance between two finite values of the same sign, in units in the last place.
    fn ulps(a: f64, b: f64) -> u64 {
        a.to_bits().abs_diff(b.to_bits())
    }

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() <= expected.abs() * 1e-12
    }

    /// Positive and negative magnitudes far from overflow and underflow.
    fn value() -> impl Strategy<Value = f64> {
        prop_oneof![1e-100_f64..1e100, -1e100_f64..-1e-100]
    }

    fn any_unit() -> impl Strategy<Value = Unit> {
        proptest::sample::select(Unit::all())
    }

    #[test]
    fn names_and_symbols_are_unique_and_well_formed() {
        let names: BTreeSet<_> = Unit::all().iter().map(|u| u.name).collect();
        let symbols: BTreeSet<_> = Unit::all().iter().map(|u| u.symbol).collect();
        assert_eq!(names.len(), Unit::all().len());
        assert_eq!(symbols.len(), Unit::all().len());
        for unit in Unit::all() {
            assert!(
                unit.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                unit.name
            );
            assert!(unit.scale.is_finite() && unit.scale > 0.0, "{}", unit.name);
            assert_eq!(Unit::by_name(unit.name).unwrap(), unit);
            assert_eq!(Unit::by_symbol(unit.symbol).unwrap(), unit);
        }
    }

    #[test]
    fn every_required_dimension_has_metric_and_imperial_units() {
        let pairs = [
            ("meter", "foot"),
            ("square_meter", "square_foot"),
            ("cubic_meter", "cubic_foot"),
            ("kilogram", "pound"),
            ("degree_celsius", "degree_fahrenheit"),
            ("newton", "pound_force"),
            ("pascal", "pound_per_square_inch"),
            ("meter_per_second", "mile_per_hour"),
            ("kilogram_per_cubic_meter", "pound_per_cubic_foot"),
            ("joule", "british_thermal_unit"),
            ("watt", "horsepower"),
            ("lux", "foot_candle"),
        ];
        for (metric, imperial) in pairs {
            assert_eq!(
                by_name(metric).dimension,
                by_name(imperial).dimension,
                "{metric}"
            );
        }
        for single in ["radian", "degree", "second", "candela", "lumen"] {
            assert!(Unit::by_name(single).is_ok());
        }
    }

    #[test]
    fn unknown_units_are_reported() {
        assert_eq!(
            Unit::by_name("cubit").unwrap_err().code(),
            "base.unit.unknown"
        );
        assert_eq!(
            Unit::by_symbol("cb").unwrap_err().code(),
            "base.unit.unknown"
        );
    }

    #[test]
    fn known_conversions_hold() {
        let cases = [
            (1.0, "inch", "millimeter", 25.4),
            (1.0, "mile", "foot", 5280.0),
            (1.0, "acre", "square_meter", 4_046.856_422_4),
            (1.0, "us_gallon", "liter", 3.785_411_784),
            (180.0, "degree", "radian", PI),
            (200.0, "gradian", "degree", 180.0),
            (1.0, "degree", "arcsecond", 3600.0),
            (1.0, "pound", "gram", 453.592_37),
            (1.0, "day", "minute", 1440.0),
            (0.0, "degree_celsius", "kelvin", 273.15),
            (32.0, "degree_fahrenheit", "degree_celsius", 0.0),
            (212.0, "degree_fahrenheit", "kelvin", 373.15),
            (1.0, "pound_force", "newton", 4.448_221_615_260_5),
            (1.0, "pound_per_square_inch", "pascal", 6_894.757_293_168),
            (1.0, "bar", "kilopascal", 100.0),
            (60.0, "mile_per_hour", "foot_per_second", 88.0),
            (36.0, "kilometer_per_hour", "meter_per_second", 10.0),
            (
                1.0,
                "gram_per_cubic_centimeter",
                "pound_per_cubic_foot",
                62.427_960_576_145,
            ),
            (1.0, "kilowatt_hour", "kilojoule", 3600.0),
            (1.0, "horsepower", "watt", 745.699_871_582_27),
            (1.0, "foot_candle", "lux", 10.763_910_416_71),
        ];
        for (value, from, to, expected) in cases {
            let actual = convert(value, by_name(from), by_name(to)).unwrap();
            let ok = if expected == 0.0 {
                actual.abs() < 1e-12
            } else {
                close(actual, expected)
            };
            assert!(ok, "{value} {from} -> {to}: {actual} != {expected}");
        }
    }

    #[test]
    fn converting_between_dimensions_fails() {
        let error = convert(1.0, by_name("meter"), by_name("second")).unwrap_err();
        assert_eq!(error.code(), "base.unit.dimension_mismatch");
        let length = by_name("foot").quantity(1.0);
        assert_eq!(length.dimension(), Dimension::LENGTH);
        assert!(close(by_name("inch").value_of(length).unwrap(), 12.0));
        assert!(by_name("second").value_of(length).is_err());
    }

    proptest! {
        #[test]
        fn to_a_unit_and_back_is_within_four_ulps(si in value(), unit in any_unit()) {
            let back = unit.to_si(unit.from_si(si));
            if unit.offset == 0.0 {
                prop_assert!(ulps(back, si) <= 4, "{}: {si} -> {back}", unit.name);
            } else {
                let magnitude = si.abs().max(unit.offset.abs());
                prop_assert!((back - si).abs() <= 4.0 * f64::EPSILON * magnitude);
            }
        }

        #[test]
        fn conversion_has_an_inverse(v in value(), from in any_unit(), to in any_unit()) {
            let forward = convert(v, &from, &to);
            prop_assert_eq!(forward.is_ok(), from.dimension == to.dimension);
            if let Ok(converted) = forward {
                let back = convert(converted, &to, &from).unwrap();
                if from.offset == 0.0 && to.offset == 0.0 {
                    prop_assert!(ulps(back, v) <= 4, "{} -> {}: {v} -> {back}", from.name, to.name);
                } else {
                    // Offsets are stored in SI; compare there.
                    let magnitude = from.to_si(v).abs().max(from.offset).max(to.offset);
                    let error = (from.to_si(back) - from.to_si(v)).abs();
                    prop_assert!(error <= 8.0 * f64::EPSILON * magnitude);
                }
            }
        }

        #[test]
        fn quantity_of_a_unit_value_reads_back(v in value(), unit in any_unit()) {
            let read = unit.value_of(unit.quantity(v)).unwrap();
            if unit.offset == 0.0 {
                prop_assert!(ulps(read, v) <= 4);
            }
        }
    }
}
