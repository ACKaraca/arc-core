//! Display formatting of quantities and the display units a document stores (CORE-BASE-008,
//! CORE-BASE-011).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result, codes};
use crate::quantity::{Dimension, Quantity};
use crate::unit::Unit;

/// How quantities of one dimension are shown. It affects formatting only, never stored values.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DisplayFormat {
    /// The unit values are shown in. Stored by its registry name.
    // shortcut: only built-in units can be stored; store full unit definitions when products
    // need their own units in document display settings.
    #[serde(serialize_with = "unit_name", deserialize_with = "unit_by_name")]
    pub unit: Unit,
    /// The number of digits after the decimal separator.
    pub precision: u8,
    /// When set, values are rounded to the nearest multiple of this increment, given in `unit`.
    pub rounding_increment: Option<f64>,
    /// Whether zeros at the end of the fraction are kept.
    pub trailing_zeros: bool,
    /// Whether the unit symbol follows the number.
    pub show_symbol: bool,
}

fn unit_name<S: Serializer>(unit: &Unit, serializer: S) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(unit.name)
}

fn unit_by_name<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Unit, D::Error> {
    let name = String::deserialize(deserializer)?;
    Unit::by_name(&name)
        .copied()
        .map_err(serde::de::Error::custom)
}

impl DisplayFormat {
    /// A format in `unit` with three decimals, no rounding increment, trailing zeros removed and
    /// the symbol shown.
    #[must_use]
    pub const fn new(unit: Unit) -> Self {
        Self {
            unit,
            precision: 3,
            rounding_increment: None,
            trailing_zeros: false,
            show_symbol: true,
        }
    }

    /// Formats a quantity. The caller passes the decimal separator of the user's locale.
    ///
    /// # Errors
    ///
    /// `base.unit.dimension_mismatch` when the quantity does not have the dimension of the unit;
    /// `base.display.invalid_increment` when the rounding increment is not a positive finite
    /// number.
    pub fn format(&self, quantity: Quantity, decimal_separator: char) -> Result<String> {
        let mut value = self.unit.value_of(quantity)?;
        if let Some(increment) = self.rounding_increment {
            if !(increment.is_finite() && increment > 0.0) {
                return Err(Error::new(codes::DISPLAY_INVALID_INCREMENT));
            }
            value = (value / increment).round() * increment;
        }

        let mut text = format!("{value:.*}", usize::from(self.precision));
        if !self.trailing_zeros && text.contains('.') && value.is_finite() {
            text.truncate(text.trim_end_matches('0').trim_end_matches('.').len());
        }
        // A value that rounds to zero is shown without a sign.
        if text.starts_with('-') && !text.contains(|c: char| ('1'..='9').contains(&c)) {
            text.remove(0);
        }
        if decimal_separator != '.' {
            text = text.replace('.', decimal_separator.encode_utf8(&mut [0; 4]));
        }
        if self.show_symbol {
            // Degree, minute and second signs attach to the number.
            if !matches!(self.unit.symbol, "°" | "′" | "″") {
                text.push(' ');
            }
            text.push_str(self.unit.symbol);
        }
        Ok(text)
    }
}

/// The display units of a document: at most one [`DisplayFormat`] per dimension, as one record
/// that documents store. Changing it changes formatting only.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DisplayUnits {
    formats: Vec<DisplayFormat>,
}

impl DisplayUnits {
    /// Creates a record without any format.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the format of the dimension its unit measures, replacing an earlier one.
    pub fn set(&mut self, format: DisplayFormat) {
        self.formats
            .retain(|f| f.unit.dimension != format.unit.dimension);
        self.formats.push(format);
    }

    /// The format of a dimension, if one is set.
    #[must_use]
    pub fn get(&self, dimension: Dimension) -> Option<&DisplayFormat> {
        self.formats.iter().find(|f| f.unit.dimension == dimension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str) -> Unit {
        *Unit::by_name(name).unwrap()
    }

    fn meters(value: f64) -> Quantity {
        Quantity::new(value, Dimension::LENGTH)
    }

    #[test]
    fn default_format_trims_zeros_and_shows_the_symbol() {
        let format = DisplayFormat::new(unit("millimeter"));
        assert_eq!(format.format(meters(1.25), '.').unwrap(), "1250 mm");
        assert_eq!(format.format(meters(0.0012346), '.').unwrap(), "1.235 mm");
        assert_eq!(format.format(meters(0.0015), '.').unwrap(), "1.5 mm");
        assert_eq!(format.format(meters(100.0), '.').unwrap(), "100000 mm");
    }

    #[test]
    fn precision_and_trailing_zeros_are_applied() {
        let mut format = DisplayFormat::new(unit("meter"));
        format.trailing_zeros = true;
        format.precision = 2;
        assert_eq!(format.format(meters(1.5), '.').unwrap(), "1.50 m");
        format.precision = 0;
        assert_eq!(format.format(meters(1.5), '.').unwrap(), "2 m");
        format.show_symbol = false;
        assert_eq!(format.format(meters(-12.0), '.').unwrap(), "-12");
    }

    #[test]
    fn locale_decimal_separator_is_used() {
        let format = DisplayFormat::new(unit("meter"));
        assert_eq!(format.format(meters(1234.5), ',').unwrap(), "1234,5 m");
        assert_eq!(format.format(meters(0.25), '٫').unwrap(), "0٫25 m");
    }

    #[test]
    fn rounding_increment_rounds_to_the_nearest_multiple() {
        let mut format = DisplayFormat::new(unit("millimeter"));
        format.rounding_increment = Some(5.0);
        assert_eq!(format.format(meters(0.0123), '.').unwrap(), "10 mm");
        assert_eq!(format.format(meters(0.0126), '.').unwrap(), "15 mm");
        format.rounding_increment = Some(0.25);
        assert_eq!(format.format(meters(0.0011), '.').unwrap(), "1 mm");
        assert_eq!(format.format(meters(0.00115), '.').unwrap(), "1.25 mm");
    }

    #[test]
    fn invalid_rounding_increments_are_rejected() {
        let mut format = DisplayFormat::new(unit("meter"));
        for increment in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            format.rounding_increment = Some(increment);
            let error = format.format(meters(1.0), '.').unwrap_err();
            assert_eq!(error.code(), "base.display.invalid_increment");
        }
    }

    #[test]
    fn values_that_round_to_zero_have_no_sign() {
        let mut format = DisplayFormat::new(unit("meter"));
        assert_eq!(format.format(meters(-0.0001), '.').unwrap(), "0 m");
        format.trailing_zeros = true;
        assert_eq!(format.format(meters(-0.0001), '.').unwrap(), "0.000 m");
        assert_eq!(format.format(meters(-0.001), '.').unwrap(), "-0.001 m");
    }

    #[test]
    fn angle_signs_attach_and_offset_units_convert() {
        let degrees = DisplayFormat::new(unit("degree"));
        let right_angle = Quantity::new(std::f64::consts::FRAC_PI_2, Dimension::ANGLE);
        assert_eq!(degrees.format(right_angle, '.').unwrap(), "90°");
        let celsius = DisplayFormat::new(unit("degree_celsius"));
        let boiling = Quantity::new(373.15, Dimension::TEMPERATURE);
        assert_eq!(celsius.format(boiling, '.').unwrap(), "100 °C");
    }

    #[test]
    fn formatting_another_dimension_fails() {
        let format = DisplayFormat::new(unit("meter"));
        let error = format
            .format(Quantity::new(1.0, Dimension::TIME), '.')
            .unwrap_err();
        assert_eq!(error.code(), "base.unit.dimension_mismatch");
    }

    #[test]
    fn display_units_hold_one_format_per_dimension() {
        let mut units = DisplayUnits::new();
        units.set(DisplayFormat::new(unit("meter")));
        units.set(DisplayFormat::new(unit("degree")));
        units.set(DisplayFormat::new(unit("foot")));
        assert_eq!(units.get(Dimension::LENGTH).unwrap().unit.name, "foot");
        assert_eq!(units.get(Dimension::ANGLE).unwrap().unit.name, "degree");
        assert!(units.get(Dimension::MASS).is_none());
    }

    #[test]
    fn changing_display_units_changes_formatting_only() {
        let stored = meters(3.048);
        let mut units = DisplayUnits::new();
        units.set(DisplayFormat::new(unit("meter")));
        let metric = units
            .get(Dimension::LENGTH)
            .unwrap()
            .format(stored, '.')
            .unwrap();
        units.set(DisplayFormat::new(unit("foot")));
        let imperial = units
            .get(Dimension::LENGTH)
            .unwrap()
            .format(stored, '.')
            .unwrap();
        assert_eq!((metric.as_str(), imperial.as_str()), ("3.048 m", "10 ft"));
        assert_eq!(stored, meters(3.048));
    }

    #[test]
    fn display_units_round_trip_through_their_stored_form() {
        let mut units = DisplayUnits::new();
        units.set(DisplayFormat {
            rounding_increment: Some(0.5),
            ..DisplayFormat::new(unit("inch"))
        });
        let json = serde_json::to_string(&units).unwrap();
        assert!(json.contains("\"unit\":\"inch\""), "{json}");
        assert_eq!(serde_json::from_str::<DisplayUnits>(&json).unwrap(), units);

        let unknown = json.replace("inch", "cubit");
        assert!(serde_json::from_str::<DisplayUnits>(&unknown).is_err());
    }
}
