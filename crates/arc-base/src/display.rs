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

/// The written form of a quantity, used by [`DisplayFormat::format_with`] and
/// [`DisplayFormat::parse_with`]. [`Notation::Decimal`] is the form of [`DisplayFormat::format`].
///
/// The feet-and-inch and angle notations write their own units, so they ignore the unit of the
/// format and use its precision (the decimals of the seconds). The rounding increment and the
/// unit symbol apply to [`Notation::Decimal`] only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notation {
    /// A decimal number in the format's unit, with the unit symbol when `show_symbol` is set.
    Decimal,
    /// Feet and inches with a fraction of an inch, such as `5'-3 1/2"`. The denominator is a power
    /// of two from 1 to 256. Values round to the nearest multiple of `1 / denominator` inch, ties
    /// away from zero; the fraction is written in lowest terms.
    Architectural {
        /// The denominator of the inch fraction.
        denominator: u16,
    },
    /// Degrees, minutes and seconds, such as `45°30'15"`. Minutes and seconds have two digits; the
    /// seconds have the format's precision.
    DegreesMinutesSeconds,
    /// A surveyor bearing, such as `N 45°30'15" E`: a north or south letter, an angle from 0 to 90
    /// degrees, and an east or west letter. The angle is written as degrees, minutes and seconds.
    SurveyorBearing,
}

/// The largest number of decimals of the seconds in the angle notations.
const MAX_SECONDS_PRECISION: u8 = 9;
/// The largest denominator of the inch fraction.
const MAX_DENOMINATOR: u16 = 256;
/// The longest text [`DisplayFormat::parse_with`] reads.
const MAX_TEXT_LENGTH: usize = 256;
/// Counts of smallest units at or above this value are out of range; every count stays in `u128`.
const COUNT_LIMIT: f64 = 1.0e30;

impl DisplayFormat {
    /// Formats a quantity in `notation`. The caller passes the decimal separator of the user's
    /// locale; it applies to decimal numbers and to the seconds of the angle notations.
    ///
    /// # Errors
    ///
    /// `base.display.notation_mismatch` when the quantity does not have the dimension the notation
    /// writes (length for feet and inches, angle for the angle notations);
    /// `base.display.invalid_notation` for a denominator that is not a power of two from 1 to 256
    /// or a precision above 9; `base.display.value_out_of_range` for values that are not finite
    /// or too large to write. [`Notation::Decimal`] reports the errors of [`DisplayFormat::format`].
    pub fn format_with(
        &self,
        quantity: Quantity,
        notation: Notation,
        decimal_separator: char,
    ) -> Result<String> {
        match notation {
            Notation::Decimal => self.format(quantity, decimal_separator),
            Notation::Architectural { denominator } => {
                validate_denominator(denominator)?;
                require_dimension(quantity, Dimension::LENGTH)?;
                let inches = Unit::by_name("inch")?.value_of(quantity)?;
                feet_and_inches(inches, denominator)
            }
            Notation::DegreesMinutesSeconds => {
                validate_precision(self.precision)?;
                require_dimension(quantity, Dimension::ANGLE)?;
                let degrees = Unit::by_name("degree")?.value_of(quantity)?;
                degrees_minutes_seconds(degrees, self.precision, self.trailing_zeros)
            }
            Notation::SurveyorBearing => {
                validate_precision(self.precision)?;
                require_dimension(quantity, Dimension::ANGLE)?;
                let degrees = Unit::by_name("degree")?.value_of(quantity)?;
                surveyor_bearing(degrees, self.precision, self.trailing_zeros)
            }
        }
    }

    /// Parses text written in `notation` back to a quantity in SI base units. Decimal text may
    /// end with the unit symbol; feet and inches take the forms `5'-3 1/2"`, `5' 3 1/2"`,
    /// `3 1/2"`, `1/2"` and `5'`; angles take `45°30'15.5"` (minutes and seconds optional) and
    /// `N 45°30'15" E`. Fractions in parsed text may have any denominator from 1 to 256.
    ///
    /// # Errors
    ///
    /// `base.display.parse_invalid` when the text is not a value in the notation, is longer than
    /// 256 characters, or has minutes or seconds of 60 or more; `base.display.invalid_notation` for
    /// an unsupported architectural denominator.
    pub fn parse_with(
        &self,
        text: &str,
        notation: Notation,
        decimal_separator: char,
    ) -> Result<Quantity> {
        if text.len() > MAX_TEXT_LENGTH {
            return Err(parse_error());
        }
        match notation {
            Notation::Decimal => {
                let text = text.trim();
                let text = text
                    .strip_suffix(self.unit.symbol)
                    .map_or(text, str::trim_end);
                let (negative, body) = match text.strip_prefix('-') {
                    Some(rest) => (true, rest),
                    None => (false, text.strip_prefix('+').unwrap_or(text)),
                };
                let value = parse_number(body, decimal_separator)?;
                Ok(self.unit.quantity(if negative { -value } else { value }))
            }
            Notation::Architectural { denominator } => {
                validate_denominator(denominator)?;
                Ok(Unit::by_name("inch")?.quantity(parse_feet_and_inches(text)?))
            }
            Notation::DegreesMinutesSeconds => {
                let degrees = parse_degrees_minutes_seconds(text, decimal_separator)?;
                Ok(Unit::by_name("degree")?.quantity(degrees))
            }
            Notation::SurveyorBearing => {
                let degrees = parse_surveyor_bearing(text, decimal_separator)?;
                Ok(Unit::by_name("degree")?.quantity(degrees))
            }
        }
    }
}

fn invalid_notation() -> Error {
    Error::new(codes::DISPLAY_INVALID_NOTATION)
}

fn parse_error() -> Error {
    Error::new(codes::DISPLAY_PARSE_INVALID)
}

fn validate_denominator(denominator: u16) -> Result<()> {
    if (1..=MAX_DENOMINATOR).contains(&denominator) && denominator.is_power_of_two() {
        Ok(())
    } else {
        Err(invalid_notation())
    }
}

fn validate_precision(precision: u8) -> Result<()> {
    if precision <= MAX_SECONDS_PRECISION {
        Ok(())
    } else {
        Err(invalid_notation())
    }
}

fn require_dimension(quantity: Quantity, dimension: Dimension) -> Result<()> {
    if quantity.dimension() == dimension {
        Ok(())
    } else {
        Err(Error::new(codes::DISPLAY_NOTATION_MISMATCH))
    }
}

/// The count of smallest units nearest to `value`, ties away from zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is checked to be finite, non-negative and below COUNT_LIMIT before the cast"
)]
fn nearest_count(value: f64) -> Result<u128> {
    if value.is_finite() && (0.0..COUNT_LIMIT).contains(&value) {
        Ok(value.round() as u128)
    } else {
        Err(Error::new(codes::DISPLAY_VALUE_OUT_OF_RANGE))
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Feet and fractional inches of a signed length in inches, such as `5'-3 1/2"`.
fn feet_and_inches(inches: f64, denominator: u16) -> Result<String> {
    let step = u128::from(denominator);
    let count = nearest_count(inches.abs() * f64::from(denominator))?;
    let per_foot = 12 * step;
    let feet = count / per_foot;
    let rest = count % per_foot;
    let whole = rest / step;
    let numerator = rest % step;

    let mut body = if feet > 0 {
        format!("{feet}'-{whole}")
    } else if whole > 0 {
        whole.to_string()
    } else {
        String::new()
    };
    if numerator > 0 {
        let divisor = gcd(numerator, step);
        let fraction = format!("{}/{}", numerator / divisor, step / divisor);
        if body.is_empty() {
            body = fraction;
        } else {
            body = format!("{body} {fraction}");
        }
    }
    if body.is_empty() {
        body.push('0');
    }
    let sign = if inches < 0.0 && count > 0 { "-" } else { "" };
    Ok(format!("{sign}{body}\""))
}

/// The count of smallest units nearest to a non-negative angle in degrees, in units of
/// `10^-precision` seconds.
fn angle_count(degrees: f64, precision: u8) -> Result<u128> {
    nearest_count(degrees * 3600.0 * 10_f64.powi(i32::from(precision)))
}

/// Appends `count` units of `10^-precision` seconds as `d°mm'ss.ff"`, without a sign.
fn push_degrees_minutes_seconds(
    out: &mut String,
    count: u128,
    precision: u8,
    trailing_zeros: bool,
) {
    let scale = 10_u128.pow(u32::from(precision));
    let degrees = count / (3600 * scale);
    let minutes = count / (60 * scale) % 60;
    let seconds = count / scale % 60;
    let fraction = count % scale;
    out.push_str(&format!("{degrees}°{minutes:02}'{seconds:02}"));
    if precision > 0 {
        let mut digits = format!("{fraction:0width$}", width = usize::from(precision));
        if !trailing_zeros {
            digits.truncate(digits.trim_end_matches('0').len());
        }
        if !digits.is_empty() {
            out.push('.');
            out.push_str(&digits);
        }
    }
    out.push('"');
}

fn degrees_minutes_seconds(degrees: f64, precision: u8, trailing_zeros: bool) -> Result<String> {
    let count = angle_count(degrees.abs(), precision)?;
    let mut out = String::new();
    if degrees < 0.0 && count > 0 {
        out.push('-');
    }
    push_degrees_minutes_seconds(&mut out, count, precision, trailing_zeros);
    Ok(out)
}

/// A surveyor bearing: the angle is measured from north or south towards east or west, and the
/// quadrant intervals are [0, 90], (90, 180], (180, 270] and (270, 360).
fn surveyor_bearing(degrees: f64, precision: u8, trailing_zeros: bool) -> Result<String> {
    let scale = 10_u128.pow(u32::from(precision));
    let turn = 1_296_000 * scale;
    let count = angle_count(degrees.rem_euclid(360.0), precision)? % turn;
    let quarter = turn / 4;
    let half = turn / 2;
    let (north, east, angle) = if count <= quarter {
        (true, true, count)
    } else if count <= half {
        (false, true, half - count)
    } else if count <= half + quarter {
        (false, false, count - half)
    } else {
        (true, false, turn - count)
    };
    let mut out = String::from(if north { "N " } else { "S " });
    push_degrees_minutes_seconds(&mut out, angle, precision, trailing_zeros);
    out.push_str(if east { " E" } else { " W" });
    Ok(out)
}

/// Reads an unsigned decimal number; `separator` is the decimal separator of the text.
fn parse_number(text: &str, separator: char) -> Result<f64> {
    let mut normalized = String::with_capacity(text.len());
    let mut seen_separator = false;
    let mut seen_digit = false;
    for c in text.chars() {
        if c.is_ascii_digit() {
            seen_digit = true;
            normalized.push(c);
        } else if c == separator && !seen_separator {
            seen_separator = true;
            normalized.push('.');
        } else {
            return Err(parse_error());
        }
    }
    if !seen_digit {
        return Err(parse_error());
    }
    normalized.parse::<f64>().map_err(|_| parse_error())
}

/// Reads a non-empty run of ASCII digits as a whole number.
fn digits(text: &str) -> Result<f64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(parse_error());
    }
    text.parse::<f64>().map_err(|_| parse_error())
}

/// Reads `n/d` with 1 <= d <= 256 and n < d.
fn fraction(text: &str) -> Result<f64> {
    let (numerator, denominator) = text.split_once('/').ok_or_else(parse_error)?;
    let numerator = digits(numerator)?;
    let denominator = digits(denominator)?;
    if denominator < 1.0 || denominator > f64::from(MAX_DENOMINATOR) || numerator >= denominator {
        return Err(parse_error());
    }
    Ok(numerator / denominator)
}

/// Reads the inch part of a feet-and-inch text: `3`, `1/2`, or `3 1/2`.
fn inch_part(text: &str) -> Result<f64> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    match tokens.as_slice() {
        [fraction_only] if fraction_only.contains('/') => fraction(fraction_only),
        [whole] => digits(whole),
        [whole, fraction_text] => Ok(digits(whole)? + fraction(fraction_text)?),
        _ => Err(parse_error()),
    }
}

/// Reads feet-and-inch text and returns the length in inches.
fn parse_feet_and_inches(text: &str) -> Result<f64> {
    let text = text.trim();
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let (feet, rest) = match body.split_once('\'') {
        Some((feet_text, rest)) => (
            Some(digits(feet_text)?),
            rest.trim_start_matches([' ', '-']),
        ),
        None => (None, body),
    };
    let inches = if rest.is_empty() {
        if feet.is_none() {
            return Err(parse_error());
        }
        0.0
    } else {
        let inner = rest.strip_suffix('"').ok_or_else(parse_error)?.trim();
        if inner.is_empty() && feet.is_some() {
            0.0
        } else {
            inch_part(inner)?
        }
    };
    let total = feet.unwrap_or(0.0) * 12.0 + inches;
    Ok(if negative { -total } else { total })
}

/// Reads `[-]d°[mm'][ss[.f]"]` and returns the signed angle in degrees.
fn parse_degrees_minutes_seconds(text: &str, separator: char) -> Result<f64> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (degrees_text, rest) = body.split_once('°').ok_or_else(parse_error)?;
    let degrees = digits(degrees_text)?;
    let (minutes, rest) = match rest.split_once(['\'', '′']) {
        Some((minutes_text, after)) => (digits(minutes_text)?, after),
        None => (0.0, rest),
    };
    let seconds = match rest.strip_suffix('"').or_else(|| rest.strip_suffix('″')) {
        Some(seconds_text) => parse_number(seconds_text, separator)?,
        None if rest.is_empty() => 0.0,
        None => return Err(parse_error()),
    };
    if minutes >= 60.0 || seconds >= 60.0 {
        return Err(parse_error());
    }
    let value = degrees + minutes / 60.0 + seconds / 3600.0;
    Ok(if negative { -value } else { value })
}

/// Reads `N|S angle E|W` and returns the azimuth in degrees from north, in [0, 360).
fn parse_surveyor_bearing(text: &str, separator: char) -> Result<f64> {
    let mut parts = text.split_whitespace();
    let (Some(first), Some(angle), Some(last), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(parse_error());
    };
    let north = match first.to_ascii_uppercase().as_str() {
        "N" => true,
        "S" => false,
        _ => return Err(parse_error()),
    };
    let east = match last.to_ascii_uppercase().as_str() {
        "E" => true,
        "W" => false,
        _ => return Err(parse_error()),
    };
    if angle.starts_with('-') {
        return Err(parse_error());
    }
    let offset = parse_degrees_minutes_seconds(angle, separator)?;
    if !(0.0..=90.0).contains(&offset) {
        return Err(parse_error());
    }
    let azimuth = match (north, east) {
        (true, true) => offset,
        (false, true) => 180.0 - offset,
        (false, false) => 180.0 + offset,
        (true, false) => 360.0 - offset,
    };
    Ok(azimuth.rem_euclid(360.0))
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

    use proptest::prelude::{prop_assert, proptest};

    fn inch_quantity(value: f64) -> Quantity {
        Unit::by_name("inch").unwrap().quantity(value)
    }

    fn inches_of(quantity: Quantity) -> f64 {
        unit("inch").value_of(quantity).unwrap()
    }

    fn degree_quantity(value: f64) -> Quantity {
        unit("degree").quantity(value)
    }

    fn feet_format() -> DisplayFormat {
        DisplayFormat::new(unit("foot"))
    }

    fn architectural(value: f64, denominator: u16) -> String {
        feet_format()
            .format_with(
                inch_quantity(value),
                Notation::Architectural { denominator },
                '.',
            )
            .unwrap()
    }

    fn code_of(result: Result<String>) -> &'static str {
        result.unwrap_err().code()
    }

    #[test]
    fn architectural_text_uses_reduced_fractions_and_carries() {
        assert_eq!(architectural(63.5, 16), "5'-3 1/2\"");
        assert_eq!(architectural(63.5, 2), "5'-3 1/2\"");
        assert_eq!(architectural(3.5, 4), "3 1/2\"");
        assert_eq!(architectural(2.5, 8), "2 1/2\"");
        assert_eq!(architectural(0.5, 256), "1/2\"");
        assert_eq!(architectural(1.0 / 256.0, 256), "1/256\"");
        assert_eq!(architectural(3.0, 16), "3\"");
        assert_eq!(architectural(144.0, 16), "12'-0\"");
        assert_eq!(architectural(12.0, 16), "1'-0\"");
        // 11 31/32 keeps its fraction; 11.99 rounds up to the next foot.
        assert_eq!(architectural(11.0 + 31.0 / 32.0, 32), "11 31/32\"");
        assert_eq!(architectural(11.99, 32), "1'-0\"");
        assert_eq!(architectural(0.2, 4), "1/4\"");
    }

    #[test]
    fn architectural_zero_and_negative_values() {
        assert_eq!(architectural(0.0, 16), "0\"");
        assert_eq!(architectural(-0.001, 16), "0\"");
        assert_eq!(architectural(-63.5, 16), "-5'-3 1/2\"");
        assert_eq!(architectural(-3.0, 16), "-3\"");
    }

    #[test]
    fn architectural_rejects_bad_denominators_and_dimensions() {
        for denominator in [0, 3, 512] {
            let notation = Notation::Architectural { denominator };
            let result = feet_format().format_with(inch_quantity(1.0), notation, '.');
            assert_eq!(code_of(result), "base.display.invalid_notation");
        }
        let seconds = Quantity::new(1.0, Dimension::TIME);
        let result =
            feet_format().format_with(seconds, Notation::Architectural { denominator: 2 }, '.');
        assert_eq!(code_of(result), "base.display.notation_mismatch");
    }

    #[test]
    fn architectural_text_parses_every_documented_form() {
        let parse = |text: &str| {
            inches_of(
                feet_format()
                    .parse_with(text, Notation::Architectural { denominator: 16 }, '.')
                    .unwrap(),
            )
        };
        let close = |text: &str, inches: f64| (parse(text) - inches).abs() < 1e-9;
        assert!(close("5'-3 1/2\"", 63.5));
        assert!(close("5' 3 1/2\"", 63.5));
        assert!(close("3 1/2\"", 3.5));
        assert!(close("1/2\"", 0.5));
        assert!(close("-5'-3 1/2\"", -63.5));
        assert!(close("12\"", 12.0));
        assert!(close("5'", 60.0));
        assert!(close("0\"", 0.0));
    }

    #[test]
    fn architectural_text_rejects_malformed_input() {
        let notation = Notation::Architectural { denominator: 16 };
        let format = feet_format();
        for text in [
            "5'-3 1/2",
            "3 2/1\"",
            "3 1/0\"",
            "3 1/2/3\"",
            "1/2 3\"",
            "x\"",
            "",
            "-",
            "5'-3 1/2\"\"",
        ] {
            let result = format.parse_with(text, notation, '.');
            assert_eq!(
                code_of(result.map(|_| String::new())),
                "base.display.parse_invalid",
                "{text}"
            );
        }
        let long = "1".repeat(300);
        let result = format.parse_with(&long, notation, '.');
        assert_eq!(
            code_of(result.map(|_| String::new())),
            "base.display.parse_invalid"
        );
    }

    proptest! {
        #[test]
        fn architectural_round_trip_rounds_to_the_denominator(
            inches in -5000.0_f64..5000.0,
            exponent in 0_u32..=8,
        ) {
            let denominator = 1_u16 << exponent;
            let notation = Notation::Architectural { denominator };
            let format = feet_format();
            let text = format.format_with(inch_quantity(inches), notation, '.').unwrap();
            let back = inches_of(format.parse_with(&text, notation, '.').unwrap());
            let step = f64::from(denominator);
            let expected = (inches.abs() * step).round() / step * inches.signum();
            prop_assert!((back - expected).abs() <= 1e-9, "{text}: {back} != {expected}");
        }
    }

    fn dms(degrees: f64, precision: u8, trailing_zeros: bool) -> String {
        let format = DisplayFormat {
            precision,
            trailing_zeros,
            ..DisplayFormat::new(unit("degree"))
        };
        format
            .format_with(
                degree_quantity(degrees),
                Notation::DegreesMinutesSeconds,
                '.',
            )
            .unwrap()
    }

    #[test]
    fn degrees_minutes_seconds_text_and_carry() {
        assert_eq!(dms(45.5, 3, false), "45°30'00\"");
        assert_eq!(dms(-45.5, 3, false), "-45°30'00\"");
        assert_eq!(dms(0.0, 3, false), "0°00'00\"");
        assert_eq!(dms(-0.0, 0, false), "0°00'00\"");
        // 59.9996 seconds rounds up to the next minute.
        assert_eq!(dms(10.0 + 59.9996 / 3600.0, 3, false), "10°01'00\"");
        assert_eq!(dms(10.0 + 1.5 / 3600.0, 2, true), "10°00'01.50\"");
        assert_eq!(dms(10.0 + 1.5 / 3600.0, 2, false), "10°00'01.5\"");
        assert_eq!(dms(10.0 + 1.5 / 3600.0, 0, false), "10°00'02\"");
        assert_eq!(dms(10.0 + 1.24 / 3600.0, 1, false), "10°00'01.2\"");
        assert_eq!(dms(10.0 + 1.24 / 3600.0, 1, true), "10°00'01.2\"");
    }

    #[test]
    fn angle_notations_reject_bad_values() {
        let format = DisplayFormat {
            precision: 10,
            ..DisplayFormat::new(unit("degree"))
        };
        let result = format.format_with(degree_quantity(1.0), Notation::DegreesMinutesSeconds, '.');
        assert_eq!(code_of(result), "base.display.invalid_notation");
        let format = DisplayFormat::new(unit("degree"));
        for value in [f64::NAN, f64::INFINITY, 1e300] {
            let result =
                format.format_with(degree_quantity(value), Notation::DegreesMinutesSeconds, '.');
            assert_eq!(
                code_of(result),
                "base.display.value_out_of_range",
                "{value}"
            );
        }
        let length = Quantity::new(1.0, Dimension::LENGTH);
        let result = format.format_with(length, Notation::SurveyorBearing, '.');
        assert_eq!(code_of(result), "base.display.notation_mismatch");
    }

    #[test]
    fn degrees_minutes_seconds_text_parses() {
        let parse = |text: &str| {
            unit("degree")
                .value_of(
                    DisplayFormat::new(unit("degree"))
                        .parse_with(text, Notation::DegreesMinutesSeconds, '.')
                        .unwrap(),
                )
                .unwrap()
        };
        assert!((parse("45°30'15\"") - (45.0 + 30.0 / 60.0 + 15.0 / 3600.0)).abs() < 1e-12);
        assert!((parse("-45°30'15.5\"") + (45.0 + 30.0 / 60.0 + 15.5 / 3600.0)).abs() < 1e-12);
        assert!((parse("45°30′15″") - (45.0 + 30.0 / 60.0 + 15.0 / 3600.0)).abs() < 1e-12);
        assert!((parse("45°") - 45.0).abs() < 1e-12);
        assert!((parse("45°30'") - 45.5).abs() < 1e-12);
        let format = DisplayFormat::new(unit("degree"));
        for text in [
            "45°60'00\"",
            "45 30",
            "45°30'15",
            "45°30'15.5.5\"",
            "°",
            "45°30'15\"x",
        ] {
            let result = format.parse_with(text, Notation::DegreesMinutesSeconds, '.');
            assert_eq!(
                code_of(result.map(|_| String::new())),
                "base.display.parse_invalid",
                "{text}"
            );
        }
    }

    proptest! {
        #[test]
        fn degrees_minutes_seconds_round_trip_within_the_rounding_step(
            degrees in -1000.0_f64..1000.0,
            precision in 0_u8..=6,
        ) {
            let format = DisplayFormat {
                precision,
                ..DisplayFormat::new(unit("degree"))
            };
            let text = format
                .format_with(degree_quantity(degrees), Notation::DegreesMinutesSeconds, '.')
                .unwrap();
            let back = unit("degree")
                .value_of(format.parse_with(&text, Notation::DegreesMinutesSeconds, '.').unwrap())
                .unwrap();
            let step = 0.5 / (3600.0 * 10_f64.powi(i32::from(precision)));
            prop_assert!((back - degrees).abs() <= step + 1e-9, "{text}: {back} vs {degrees}");
        }
    }

    fn bearing(degrees: f64, precision: u8) -> String {
        let format = DisplayFormat {
            precision,
            ..DisplayFormat::new(unit("degree"))
        };
        format
            .format_with(degree_quantity(degrees), Notation::SurveyorBearing, '.')
            .unwrap()
    }

    #[test]
    fn surveyor_bearings_cover_the_quadrants_and_boundaries() {
        assert_eq!(bearing(45.0, 3), "N 45°00'00\" E");
        assert_eq!(bearing(135.0, 3), "S 45°00'00\" E");
        assert_eq!(bearing(225.0, 3), "S 45°00'00\" W");
        assert_eq!(bearing(315.0, 3), "N 45°00'00\" W");
        assert_eq!(bearing(0.0, 3), "N 0°00'00\" E");
        assert_eq!(bearing(360.0, 3), "N 0°00'00\" E");
        assert_eq!(bearing(90.0, 3), "N 90°00'00\" E");
        assert_eq!(bearing(180.0, 3), "S 0°00'00\" E");
        assert_eq!(bearing(270.0, 3), "S 90°00'00\" W");
        assert_eq!(bearing(-45.0, 3), "N 45°00'00\" W");
        assert_eq!(
            bearing(45.0 + 30.0 / 60.0 + 15.0 / 3600.0, 0),
            "N 45°30'15\" E"
        );
    }

    #[test]
    fn surveyor_bearings_carry_into_the_next_quadrant() {
        // Rounds to exactly 90 degrees, so the north-east quadrant applies.
        assert_eq!(bearing(89.999_999_9, 0), "N 90°00'00\" E");
        // Rounds to exactly 270 degrees, which belongs to the south-west quadrant.
        assert_eq!(bearing(269.999_999_9, 0), "S 90°00'00\" W");
        assert_eq!(bearing(359.999_999_9, 0), "N 0°00'00\" E");
    }

    #[test]
    fn surveyor_bearings_parse_and_reject_malformed_text() {
        let parse = |text: &str| {
            unit("degree")
                .value_of(
                    DisplayFormat::new(unit("degree"))
                        .parse_with(text, Notation::SurveyorBearing, '.')
                        .unwrap(),
                )
                .unwrap()
        };
        assert!((parse("N 45°00'00\" E") - 45.0).abs() < 1e-12);
        assert!((parse("S 0°00'00\" E") - 180.0).abs() < 1e-12);
        assert!((parse("S 90°00'00\" W") - 270.0).abs() < 1e-12);
        assert!((parse("N 0°00'00\" W")).abs() < 1e-12);
        assert!((parse("n 90°00'00\" e") - 90.0).abs() < 1e-12);
        let format = DisplayFormat::new(unit("degree"));
        for text in [
            "N 91°00'00\" E",
            "X 45°00'00\" E",
            "N 45°00'00\" X",
            "45°00'00\" E",
            "N -45°00'00\" E",
            "N 45°00'00\" E extra",
        ] {
            let result = format.parse_with(text, Notation::SurveyorBearing, '.');
            assert_eq!(
                code_of(result.map(|_| String::new())),
                "base.display.parse_invalid",
                "{text}"
            );
        }
    }

    proptest! {
        #[test]
        fn surveyor_bearing_round_trip_within_the_rounding_step(
            degrees in 0.0_f64..360.0,
            precision in 0_u8..=4,
        ) {
            let format = DisplayFormat {
                precision,
                ..DisplayFormat::new(unit("degree"))
            };
            let text = format
                .format_with(degree_quantity(degrees), Notation::SurveyorBearing, '.')
                .unwrap();
            let back = unit("degree")
                .value_of(format.parse_with(&text, Notation::SurveyorBearing, '.').unwrap())
                .unwrap();
            let apart = (back - degrees).abs();
            let around = apart.min(360.0 - apart);
            let step = 0.5 / (3600.0 * 10_f64.powi(i32::from(precision)));
            prop_assert!(around <= step + 1e-9, "{text}: {back} vs {degrees}");
        }
    }

    #[test]
    fn decimal_notation_matches_format_and_parses_back() {
        let format = DisplayFormat::new(unit("millimeter"));
        let quantity = meters(1.25);
        assert_eq!(
            format
                .format_with(quantity, Notation::Decimal, '.')
                .unwrap(),
            format.format(quantity, '.').unwrap()
        );
        let parsed = format
            .parse_with("1250 mm", Notation::Decimal, '.')
            .unwrap();
        assert!((parsed.value() - 1.25).abs() < 1e-12);
        let comma = DisplayFormat::new(unit("meter"));
        let parsed = comma.parse_with("-1,5 m", Notation::Decimal, ',').unwrap();
        assert!((parsed.value() + 1.5).abs() < 1e-12);
        for text in ["1.2.3 m", "abc", "", "-", "1,5 m"] {
            let result = comma.parse_with(text, Notation::Decimal, '.');
            assert_eq!(
                code_of(result.map(|_| String::new())),
                "base.display.parse_invalid",
                "{text}"
            );
        }
    }

    proptest! {
        #[test]
        fn decimal_round_trip_is_within_the_precision(value in -1000.0_f64..1000.0) {
            let format = DisplayFormat::new(unit("meter"));
            let text = format.format(meters(value), ',').unwrap();
            let back = format.parse_with(&text, Notation::Decimal, ',').unwrap().value();
            prop_assert!((back - value).abs() <= 0.5e-3 + 1e-9, "{text}: {back} vs {value}");
        }
    }
}
