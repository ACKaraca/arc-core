//! Measurement formatting of dimensions (CORE-DRAFT-010).
//!
//! A pure function of a value and a [`NumberFormat`] read from a dimension style. The rules are
//! stated in the crate specification, section "Dimensions, leaders and annotation scaling as
//! implemented".

/// The unit format of a dimension style, with its enum code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UnitFormat {
    /// Decimal digits: `12.50`.
    #[default]
    Decimal = 0,
    /// Feet and decimal inches: `5'-2.38"`; the value is converted to inches.
    Engineering = 1,
    /// Feet and fractional inches: `5'-2 3/8"`; the value is converted to inches.
    Architectural = 2,
    /// A whole number and a binary fraction: `2 3/8`.
    Fractional = 3,
    /// Mantissa and exponent: `1.25E+02`.
    Scientific = 4,
}

impl UnitFormat {
    /// The format of an enum code; unknown codes read as [`UnitFormat::Decimal`].
    #[must_use]
    pub fn of(code: u32) -> Self {
        match code {
            1 => Self::Engineering,
            2 => Self::Architectural,
            3 => Self::Fractional,
            4 => Self::Scientific,
            _ => Self::Decimal,
        }
    }
}

/// How a number is written.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberFormat {
    /// The unit format.
    pub unit_format: UnitFormat,
    /// Digits after the separator, or the binary fraction 1/2^precision; 0 to 8.
    pub precision: u32,
    /// Rounding increment; 0 or less rounds only to the precision.
    pub rounding: f64,
    /// Drops the zero before the separator of a value below 1 (`.5`).
    pub suppress_leading: bool,
    /// Drops zeros after the last significant digit, and the separator with them (`2.5`).
    pub suppress_trailing: bool,
    /// The decimal separator.
    pub separator: char,
    /// A unit symbol appended after a space (not to engineering and architectural values).
    pub suffix: Option<String>,
}

impl Default for NumberFormat {
    fn default() -> Self {
        Self {
            unit_format: UnitFormat::Decimal,
            precision: 2,
            rounding: 0.0,
            suppress_leading: false,
            suppress_trailing: false,
            separator: '.',
            suffix: None,
        }
    }
}

/// The value rounded to the increment, when one is set and the result is finite.
fn rounded(value: f64, increment: f64) -> f64 {
    if increment > 0.0 && increment.is_finite() {
        let q = (value / increment).round() * increment;
        if q.is_finite() {
            return q;
        }
    }
    value
}

/// Decimal digits of `value` under `format`'s precision and suppression, with `.` as the
/// separator.
fn decimal(value: f64, format: &NumberFormat) -> String {
    let precision = format.precision.min(8) as usize;
    let mut text = format!("{value:.precision$}");
    if format.suppress_trailing && text.contains('.') {
        text.truncate(text.trim_end_matches('0').trim_end_matches('.').len());
    }
    // A value that rounds to zero has no sign.
    if text.starts_with('-') && !text.contains(|c: char| ('1'..='9').contains(&c)) {
        text.remove(0);
    }
    if format.suppress_leading {
        if let Some(rest) = text.strip_prefix("0.") {
            text = format!(".{rest}");
        } else if let Some(rest) = text.strip_prefix("-0.") {
            text = format!("-.{rest}");
        }
    }
    text
}

fn separated(text: &str, separator: char) -> String {
    if separator == '.' {
        text.to_owned()
    } else {
        text.replace('.', separator.encode_utf8(&mut [0; 4]))
    }
}

/// The whole part and the reduced binary fraction `(numerator, denominator)` of `|value|`
/// at 1/2^precision; a fraction that rounds to 1 carries.
fn fraction(value: f64, precision: u32) -> (f64, u64, u64) {
    let denominator = 1_u64 << precision.min(8);
    let magnitude = value.abs();
    let mut whole = magnitude.trunc();
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the denominator is at most 256 and the rounded fraction is in [0, 256]"
    )]
    let mut numerator = ((magnitude - whole) * denominator as f64).round() as u64;
    if numerator >= denominator {
        whole += 1.0;
        numerator = 0;
    }
    let mut d = denominator;
    while numerator > 0 && numerator.is_multiple_of(2) && d > 1 {
        numerator /= 2;
        d /= 2;
    }
    (whole, numerator, d)
}

fn fractional(value: f64, precision: u32) -> String {
    let (whole, n, d) = fraction(value, precision);
    let sign = if value < 0.0 && (whole > 0.0 || n > 0) {
        "-"
    } else {
        ""
    };
    match (whole > 0.0, n > 0) {
        (_, false) => format!("{sign}{whole:.0}"),
        (false, true) => format!("{sign}{n}/{d}"),
        (true, true) => format!("{sign}{whole:.0} {n}/{d}"),
    }
}

/// Feet and inches of a value in inches; `inches` writes the inch part.
fn feet_inches(value: f64, inches: impl Fn(f64) -> (String, bool)) -> String {
    let magnitude = value.abs();
    let mut feet = (magnitude / 12.0).trunc();
    let (mut text, carried) = inches(magnitude - feet * 12.0);
    if carried {
        feet += 1.0;
        text = inches(0.0).0;
    }
    let nonzero = feet > 0.0 || text.contains(|c: char| ('1'..='9').contains(&c));
    let sign = if value < 0.0 && nonzero { "-" } else { "" };
    if feet > 0.0 {
        format!("{sign}{feet:.0}'-{text}\"")
    } else {
        format!("{sign}{text}\"")
    }
}

/// Writes a length `value` in drawing units; `to_inches` converts it for the engineering and
/// architectural formats.
#[must_use]
pub fn format_length(value: f64, format: &NumberFormat, to_inches: f64) -> String {
    let value = rounded(value, format.rounding);
    let suffix = |text: String| match &format.suffix {
        Some(symbol) if !symbol.is_empty() => format!("{text} {symbol}"),
        _ => text,
    };
    if !value.is_finite() {
        return format!("{value}");
    }
    match format.unit_format {
        UnitFormat::Decimal => suffix(separated(&decimal(value, format), format.separator)),
        UnitFormat::Fractional => suffix(fractional(value, format.precision)),
        UnitFormat::Scientific => {
            let precision = format.precision.min(8) as usize;
            let text = format!("{value:.precision$e}");
            let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
            let m: f64 = mantissa.parse().unwrap_or(0.0);
            let e: i32 = exponent.parse().unwrap_or(0);
            let sign = if e < 0 { '-' } else { '+' };
            suffix(format!(
                "{}E{sign}{:02}",
                separated(&decimal(m, format), format.separator),
                e.unsigned_abs()
            ))
        }
        UnitFormat::Engineering => {
            let inches = rounded(value * to_inches, format.rounding);
            let unit = 10_f64.powi(i32::try_from(format.precision.min(8)).unwrap_or(0));
            feet_inches(inches, |i| {
                let carried = (i * unit).round() >= 12.0 * unit;
                (separated(&decimal(i, format), format.separator), carried)
            })
        }
        UnitFormat::Architectural => {
            let inches = rounded(value * to_inches, format.rounding);
            feet_inches(inches, |i| {
                let (whole, n, _) = fraction(i, format.precision);
                let carried = whole >= 12.0 && n == 0;
                (fractional(i, format.precision), carried)
            })
        }
    }
}

/// Writes an angle given in radians as decimal degrees followed by `°`.
#[must_use]
pub fn format_angle(radians: f64, format: &NumberFormat) -> String {
    let degrees = rounded(radians.to_degrees(), format.rounding);
    if !degrees.is_finite() {
        return format!("{degrees}");
    }
    format!(
        "{}°",
        separated(&decimal(degrees, format), format.separator)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(f: impl FnOnce(&mut NumberFormat)) -> NumberFormat {
        let mut format = NumberFormat::default();
        f(&mut format);
        format
    }

    #[test]
    fn decimal_rules() {
        let d = NumberFormat::default();
        assert_eq!(format_length(12.345, &d, 1.0), "12.35");
        assert_eq!(format_length(-0.001, &d, 1.0), "0.00");
        let f = with(|f| {
            f.suppress_leading = true;
            f.suppress_trailing = true;
            f.separator = ',';
            f.suffix = Some("mm".into());
        });
        assert_eq!(format_length(0.5, &f, 1.0), ",5 mm");
        assert_eq!(format_length(-0.5, &f, 1.0), "-,5 mm");
        assert_eq!(format_length(3.0, &f, 1.0), "3 mm");
        let r = with(|f| f.rounding = 0.25);
        assert_eq!(format_length(1.13, &r, 1.0), "1.25");
        assert_eq!(format_length(f64::INFINITY, &r, 1.0), "inf");
    }

    #[test]
    fn other_unit_formats() {
        let frac = with(|f| {
            f.unit_format = UnitFormat::Fractional;
            f.precision = 3;
        });
        assert_eq!(format_length(2.375, &frac, 1.0), "2 3/8");
        assert_eq!(format_length(0.5, &frac, 1.0), "1/2");
        assert_eq!(format_length(1.999, &frac, 1.0), "2");
        let sci = with(|f| f.unit_format = UnitFormat::Scientific);
        assert_eq!(format_length(125.0, &sci, 1.0), "1.25E+02");
        assert_eq!(format_length(0.0125, &sci, 1.0), "1.25E-02");
        let eng = with(|f| f.unit_format = UnitFormat::Engineering);
        assert_eq!(format_length(62.375, &eng, 1.0), "5'-2.38\"");
        assert_eq!(format_length(11.999, &eng, 1.0), "1'-0.00\"");
        let arch = with(|f| {
            f.unit_format = UnitFormat::Architectural;
            f.precision = 3;
        });
        assert_eq!(format_length(62.375, &arch, 1.0), "5'-2 3/8\"");
        assert_eq!(format_length(25.4 * 2.5, &arch, 1.0 / 25.4), "2 1/2\"");
        assert_eq!(format_length(-3.0, &arch, 1.0), "-3\"");
    }

    #[test]
    fn angles_are_degrees() {
        let a = with(|f| f.precision = 1);
        assert_eq!(format_angle(std::f64::consts::FRAC_PI_4, &a), "45.0°");
    }
}
