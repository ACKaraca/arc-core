//! The clear text encoding of ISO 10303-21 for writing: records, parameters, strings and reals.
//!
//! Strings keep printable ASCII as it is, double the apostrophe and the backslash, and write every
//! other character (non-ASCII and control characters) in `\X2\...\X0\` runs of four uppercase
//! hexadecimal digits per character, or `\X4\...\X0\` runs of eight digits for the characters above
//! U+FFFF. Reals always carry a point and are written in their shortest form that
//! reads back to the same value; magnitudes outside `[1E-3, 1E15)` use an exponent of at least two
//! digits with its sign (`1.E-05`). The output is the same for the same input on every platform.

use std::fmt::Write as _;

use arc_base::{Error, Limits, Result};

use crate::error::{codes, limit};
use crate::model::Parameter;

/// Writes `#name=TYPE(parameters);` and a line feed.
pub(crate) fn record(
    out: &mut String,
    name: u64,
    type_name: &str,
    parameters: &[Parameter],
    limits: &Limits,
) -> Result<()> {
    let _ = write!(out, "#{name}=");
    header_record(out, type_name, parameters, limits)
}

/// Writes `TYPE(parameters);` and a line feed, the form of a header record.
pub(crate) fn header_record(
    out: &mut String,
    type_name: &str,
    parameters: &[Parameter],
    limits: &Limits,
) -> Result<()> {
    out.push_str(type_name);
    list(out, parameters, limits, 1)?;
    out.push_str(";\n");
    Ok(())
}

fn list(out: &mut String, items: &[Parameter], limits: &Limits, depth: u32) -> Result<()> {
    let length = u64::try_from(items.len()).unwrap_or(u64::MAX);
    if length > limits.array_length {
        return Err(limit("array_length", length));
    }
    if depth > limits.nesting_depth.min(crate::step::MAX_NESTING) {
        return Err(limit("nesting_depth", u64::from(depth)));
    }
    out.push('(');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        parameter(out, item, limits, depth)?;
    }
    out.push(')');
    Ok(())
}

fn parameter(out: &mut String, value: &Parameter, limits: &Limits, depth: u32) -> Result<()> {
    match value {
        Parameter::Integer(number) => {
            let _ = write!(out, "{number}");
        }
        Parameter::Real(number) => real(out, *number)?,
        Parameter::Text(content) => {
            let length = u64::try_from(content.len()).unwrap_or(u64::MAX);
            if length > limits.string_bytes {
                return Err(limit("string_bytes", length));
            }
            text(out, content)?;
        }
        Parameter::Enumeration(name) => {
            let _ = write!(out, ".{name}.");
        }
        Parameter::Binary(digits) => {
            let _ = write!(out, "\"{digits}\"");
        }
        Parameter::Reference(name) => {
            let _ = write!(out, "#{name}");
        }
        Parameter::Omitted => out.push('$'),
        Parameter::Derived => out.push('*'),
        Parameter::Typed { type_name, value } => {
            if depth >= limits.nesting_depth.min(crate::step::MAX_NESTING) {
                return Err(limit("nesting_depth", u64::from(depth) + 1));
            }
            out.push_str(type_name);
            out.push('(');
            parameter(out, value, limits, depth + 1)?;
            out.push(')');
        }
        Parameter::List(items) => list(out, items, limits, depth + 1)?,
    }
    Ok(())
}

/// Writes a string between apostrophes. A NUL character cannot be written (a reader refuses it)
/// and is `io.ifc.write_value_invalid`.
pub(crate) fn text(out: &mut String, content: &str) -> Result<()> {
    out.push('\'');
    // The encoded run in effect: none, `\X2\` (UTF-16 code units) or `\X4\` (code points).
    let mut run: Option<bool> = None;
    for c in content.chars() {
        if c == '\0' {
            return Err(Error::new(codes::IFC_WRITE_VALUE_INVALID).with_arg("value", "text"));
        }
        let wanted = if (' '..='~').contains(&c) {
            None
        } else {
            Some(u32::from(c) > 0xFFFF)
        };
        if wanted != run {
            if run.is_some() {
                out.push_str("\\X0\\");
            }
            match wanted {
                Some(true) => out.push_str("\\X4\\"),
                Some(false) => out.push_str("\\X2\\"),
                None => {}
            }
            run = wanted;
        }
        match (wanted, c) {
            (None, '\'') => out.push_str("''"),
            (None, '\\') => out.push_str("\\\\"),
            (None, _) => out.push(c),
            (Some(true), _) => {
                let _ = write!(out, "{:08X}", u32::from(c));
            }
            (Some(false), _) => {
                let _ = write!(out, "{:04X}", u32::from(c));
            }
        }
    }
    if run.is_some() {
        out.push_str("\\X0\\");
    }
    out.push('\'');
    Ok(())
}

/// Writes a real with a point, in the shortest form that reads back to the same value. Negative
/// zero is written as `0.`. A value that is not finite is `io.ifc.write_value_invalid`.
pub(crate) fn real(out: &mut String, value: f64) -> Result<()> {
    if !value.is_finite() {
        return Err(Error::new(codes::IFC_WRITE_VALUE_INVALID).with_arg("value", "real"));
    }
    let value = if value == 0.0 { 0.0 } else { value };
    let magnitude = value.abs();
    if magnitude == 0.0 || (1e-3..1e15).contains(&magnitude) {
        // `Display` of `f64` is the shortest round-trip form and never uses an exponent.
        let plain = value.to_string();
        out.push_str(&plain);
        if !plain.contains('.') {
            out.push('.');
        }
    } else {
        let scientific = format!("{value:e}");
        let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
        out.push_str(mantissa);
        if !mantissa.contains('.') {
            out.push('.');
        }
        let exponent: i32 = exponent.parse().unwrap_or(0);
        let _ = write!(out, "E{exponent:+03}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]

    use super::*;
    use crate::text::decode;

    fn real_text(value: f64) -> String {
        let mut out = String::new();
        real(&mut out, value).unwrap();
        out
    }

    fn text_of(value: &str) -> String {
        let mut out = String::new();
        text(&mut out, value).unwrap();
        out
    }

    #[test]
    fn reals_carry_a_point_and_use_an_exponent_only_at_the_extremes() {
        assert_eq!(real_text(1.0), "1.");
        assert_eq!(real_text(0.25), "0.25");
        assert_eq!(real_text(-3.5), "-3.5");
        assert_eq!(real_text(0.0), "0.");
        assert_eq!(real_text(-0.0), "0.");
        assert_eq!(real_text(1e-5), "1.E-05");
        assert_eq!(real_text(-2.5e-7), "-2.5E-07");
        assert_eq!(real_text(1e20), "1.E+20");
        assert_eq!(real_text(1.5e300), "1.5E+300");
        assert_eq!(real_text(0.001), "0.001");
        assert_eq!(real_text(123_456_789.0), "123456789.");
        assert_eq!(real_text(0.1 + 0.2), "0.30000000000000004");
    }

    #[test]
    fn reals_read_back_to_the_same_bits() {
        for value in [
            1.0,
            0.1,
            1.0 / 3.0,
            1e-5,
            -2.5e-7,
            1e20,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324,
            999_999_999_999_999.9,
        ] {
            let written = real_text(value);
            assert_eq!(written.parse::<f64>().unwrap(), value, "{written}");
        }
    }

    #[test]
    fn non_finite_reals_are_refused() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = real(&mut String::new(), value).unwrap_err();
            assert_eq!(error.code(), "io.ifc.write_value_invalid");
        }
    }

    #[test]
    fn strings_escape_apostrophes_backslashes_and_non_ascii() {
        assert_eq!(text_of("Wall"), "'Wall'");
        assert_eq!(text_of("it's"), "'it''s'");
        assert_eq!(text_of("a\\b"), "'a\\\\b'");
        assert_eq!(text_of("Kat ğ"), "'Kat \\X2\\011F\\X0\\'");
        assert_eq!(text_of("çöü!"), "'\\X2\\00E700F600FC\\X0\\!'");
        assert_eq!(text_of("a\nb"), "'a\\X2\\000A\\X0\\b'");
        // Characters above U+FFFF are code points of eight digits in `\X4\`.
        assert_eq!(text_of("\u{1F600}"), "'\\X4\\0001F600\\X0\\'");
        assert_eq!(
            text_of("a\u{1F600}\u{1F3E0}ğb"),
            "'a\\X4\\0001F6000001F3E0\\X0\\\\X2\\011F\\X0\\b'"
        );
        assert_eq!(text_of(""), "''");
        let error = text(&mut String::new(), "a\0").unwrap_err();
        assert_eq!(error.code(), "io.ifc.write_value_invalid");
    }

    #[test]
    fn strings_decode_back_to_the_same_text() {
        for value in ["it's", "a\\b", "Kat ğ \u{1F600} \t end", "''\\\\", "\\X2\\"] {
            let written = text_of(value);
            let inner = written
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap();
            assert_eq!(decode(inner.as_bytes(), 0).unwrap().text, value);
        }
    }

    #[test]
    fn records_write_every_parameter_kind() {
        let mut out = String::new();
        let parameters = [
            Parameter::Text("x".into()),
            Parameter::Omitted,
            Parameter::Derived,
            Parameter::Enumeration("T".into()),
            Parameter::Integer(-3),
            Parameter::Reference(7),
            Parameter::List(vec![Parameter::Real(1.0), Parameter::Real(0.5)]),
            Parameter::Typed {
                type_name: "IFCLABEL".into(),
                value: Box::new(Parameter::Text("y".into())),
            },
            Parameter::List(Vec::new()),
        ];
        record(&mut out, 12, "IFCTEST", &parameters, &Limits::CONSTRAINED).unwrap();
        assert_eq!(
            out,
            "#12=IFCTEST('x',$,*,.T.,-3,#7,(1.,0.5),IFCLABEL('y'),());\n"
        );
    }

    #[test]
    fn limits_bound_strings_and_lists() {
        let limits = Limits {
            string_bytes: 3,
            array_length: 2,
            ..Limits::CONSTRAINED
        };
        let long = [Parameter::Text("abcd".into())];
        let error = record(&mut String::new(), 1, "X", &long, &limits).unwrap_err();
        assert_eq!(error.code(), "io.limit_exceeded");
        let wide = [
            Parameter::Integer(1),
            Parameter::Integer(2),
            Parameter::Omitted,
        ];
        let error = record(&mut String::new(), 1, "X", &wide, &limits).unwrap_err();
        assert_eq!(error.code(), "io.limit_exceeded");
    }
}
