//! Unit suffixes resolved against the `arc-base` unit table, and checked dimension algebra.

use arc_base::{Dimension, Error, Result, Unit};

use crate::error::codes;

/// ASCII spellings of built-in units whose symbols are not ASCII, as (alias, unit name).
pub(crate) const ALIASES: [(&str, &str); 5] = [
    ("deg", "degree"),
    ("arcmin", "arcminute"),
    ("arcsec", "arcsecond"),
    ("degC", "degree_celsius"),
    ("degF", "degree_fahrenheit"),
];

/// A built-in unit by symbol, name or alias. The Greek letter mu (U+03BC) reads as the micro
/// sign (U+00B5).
pub(crate) fn lookup(atom: &str) -> Option<Unit> {
    if atom.contains('\u{3bc}') {
        return lookup(&atom.replace('\u{3bc}', "\u{b5}"));
    }
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == atom)
        .map_or(atom, |(_, name)| name);
    Unit::by_symbol(atom)
        .or_else(|_| Unit::by_name(name))
        .ok()
        .copied()
}

/// `a * b^power`, exponent by exponent, or `None` when an exponent leaves the range of `i8`.
pub(crate) fn combine(a: Dimension, b: Dimension, power: i32) -> Option<Dimension> {
    let (a, b) = (a.exponents(), b.exponents());
    let mut out = [0_i8; 9];
    for ((o, x), y) in out.iter_mut().zip(a).zip(b) {
        let value = i32::from(x).checked_add(i32::from(y).checked_mul(power)?)?;
        *o = i8::try_from(value).ok()?;
    }
    Some(Dimension::from_exponents(out))
}

fn unknown(text: &str) -> Error {
    Error::new(codes::EXPR_UNKNOWN_UNIT).with_arg("name", text)
}

fn exponent_limit() -> Error {
    Error::new(codes::EXPR_LIMIT_EXCEEDED).with_detail("limit", "dimension_exponent")
}

/// A unit factor with its non-zero integer exponent: `m`, `m^-2`, `mm2`, `m²`.
fn factor(text: &str) -> Result<(Unit, i32)> {
    if let Some(unit) = lookup(text) {
        return Ok((unit, 1));
    }
    let (base, exponent) = if let Some((base, exponent)) = text.split_once('^') {
        (base, exponent)
    } else if let Some(base) = text.strip_suffix('²') {
        (base, "2")
    } else if let Some(base) = text.strip_suffix('³') {
        (base, "3")
    } else {
        let base = text.trim_end_matches(|c: char| c.is_ascii_digit());
        (base, text.get(base.len()..).unwrap_or(""))
    };
    let unit = lookup(base).ok_or_else(|| unknown(text))?;
    let digits = exponent.strip_prefix('-').unwrap_or(exponent);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(unknown(text));
    }
    // Digits that overflow `i64` are out of range like any exponent beyond `i8`.
    let exponent = exponent.parse::<i64>().map_err(|_| exponent_limit())?;
    if exponent == 0 {
        return Err(unknown(text));
    }
    let exponent = i8::try_from(exponent).map_err(|_| exponent_limit())?;
    Ok((unit, i32::from(exponent)))
}

/// Resolves a unit suffix: a built-in unit, or factors joined by `*`, `/` or `·`, applied left to
/// right. The result of a product has an empty name and symbol.
///
/// # Errors
///
/// `solve.expr.unknown_unit` for a factor that is no unit or has the exponent 0,
/// `solve.expr.offset_unit` for a unit with an offset inside a product or power,
/// `solve.expr.limit_exceeded` when an exponent or a dimension exponent leaves the range of `i8`, and `solve.expr.domain` when the scale is not a positive
/// finite number.
pub(crate) fn resolve(text: &str) -> Result<Unit> {
    if let Some(unit) = lookup(text) {
        return Ok(unit);
    }
    let mut scale = 1.0;
    let mut dimension = Dimension::NONE;
    let mut divide = false;
    let mut rest = text;
    loop {
        let end = rest.find(['*', '/', '·']).unwrap_or(rest.len());
        let (part, tail) = rest.split_at(end);
        let (unit, exponent) = factor(part)?;
        if unit.offset != 0.0 {
            return Err(Error::new(codes::EXPR_OFFSET_UNIT).with_arg("name", part));
        }
        let power = if divide { -exponent } else { exponent };
        dimension = combine(dimension, unit.dimension, power).ok_or_else(exponent_limit)?;
        for _ in 0..power.unsigned_abs() {
            if power > 0 {
                scale *= unit.scale;
            } else {
                scale /= unit.scale;
            }
        }
        let mut tail = tail.chars();
        match tail.next() {
            None => break,
            Some(operator) => divide = operator == '/',
        }
        rest = tail.as_str();
    }
    if !(scale.is_finite() && scale > 0.0) {
        return Err(Error::new(codes::EXPR_DOMAIN));
    }
    Ok(Unit {
        name: "",
        symbol: "",
        dimension,
        scale,
        offset: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= b.abs() * 1e-14
    }

    #[test]
    fn symbols_names_and_aliases_resolve_to_table_units() {
        assert_eq!(resolve("mm").unwrap().name, "millimeter");
        assert_eq!(resolve("inch").unwrap().name, "inch");
        assert_eq!(resolve("deg").unwrap().name, "degree");
        assert_eq!(resolve("°C").unwrap().name, "degree_celsius");
        assert_eq!(resolve("kg/m³").unwrap().name, "kilogram_per_cubic_meter");
        assert_eq!(resolve("ft·lbf").unwrap().name, "foot_pound_force");
    }

    #[test]
    fn products_combine_scales_and_dimensions() {
        let line_load = resolve("kN/m").unwrap();
        assert_eq!(line_load.dimension, Dimension::FORCE.div(Dimension::LENGTH));
        assert!(close(line_load.scale, 1e3));
        assert_eq!(resolve("m^2").unwrap().dimension, Dimension::AREA);
        assert_eq!(resolve("mm2").unwrap().dimension, Dimension::AREA);
        assert!(close(resolve("mm2").unwrap().scale, 1e-6));
        assert_eq!(resolve("kg*m/s^2").unwrap().dimension, Dimension::FORCE);
        assert_eq!(resolve("N·m").unwrap().dimension, Dimension::ENERGY);
        assert_eq!(
            resolve("m^-2").unwrap().dimension,
            Dimension::NONE.div(Dimension::AREA)
        );
        assert_eq!(resolve("kN/m/s").unwrap().dimension.exponents()[2], -3);
    }

    #[test]
    fn unknown_offset_and_overflowing_units_fail() {
        for text in [
            "cubit", "m^x", "m//s", "", "2", "m0", "m^0", "m^-0", "m^+2", "µm", "μm",
        ] {
            assert_eq!(
                resolve(text).unwrap_err().code(),
                "solve.expr.unknown_unit",
                "{text}"
            );
        }
        assert_eq!(
            resolve("°C/m").unwrap_err().code(),
            "solve.expr.offset_unit"
        );
        assert_eq!(
            resolve("degC^2").unwrap_err().code(),
            "solve.expr.offset_unit"
        );
        assert_eq!(
            resolve("m^100*m^100").unwrap_err().code(),
            "solve.expr.limit_exceeded"
        );
        assert_eq!(resolve("km^120").unwrap_err().code(), "solve.expr.domain");
        // One code for every exponent out of range, written or combined.
        for text in ["m^999", "m^-129", "m^128", "m^99999999999999999999999"] {
            let error = resolve(text).unwrap_err();
            assert_eq!(error.code(), "solve.expr.limit_exceeded", "{text}");
            assert_eq!(
                error.details().get("limit"),
                Some(&"dimension_exponent".into())
            );
        }
        assert!(resolve("m^-128").is_ok() && resolve("m^127").is_ok());
    }

    #[test]
    fn combine_checks_the_exponent_range() {
        assert_eq!(
            combine(Dimension::LENGTH, Dimension::LENGTH, 2),
            Some(Dimension::VOLUME)
        );
        assert_eq!(combine(Dimension::NONE, Dimension::LENGTH, 128), None);
        assert_eq!(combine(Dimension::NONE, Dimension::LENGTH, i32::MAX), None);
    }
}
