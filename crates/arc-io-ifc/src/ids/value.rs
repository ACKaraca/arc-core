//! IFC values as the checker compares them, and the comparison with IDS values.

use super::model::{Restriction, Value};
use crate::model::Parameter;

/// The tolerance of the comparison of real numbers (IDS 1.0).
const EPSILON: f64 = 1e-6;

/// A value of an instance.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    /// `$`, or a derived attribute.
    Null,
    Str(String),
    Int(i64),
    Real(f64),
    Bool(bool),
    /// A logical `.U.`.
    Unknown,
    /// An enumeration item such as `.NOTDEFINED.`, as its text.
    Enum(String),
    /// A reference to an instance.
    Object(u64),
    List(Vec<Val>),
}

impl Val {
    /// The value of a parameter; a typed parameter gives its wrapped value.
    pub(crate) fn of(parameter: &Parameter) -> Self {
        match parameter {
            Parameter::Omitted | Parameter::Derived => Self::Null,
            Parameter::Text(t) => Self::Str(t.clone()),
            Parameter::Integer(i) => Self::Int(*i),
            Parameter::Real(r) => Self::Real(*r),
            Parameter::Enumeration(e) => match e.as_str() {
                "T" => Self::Bool(true),
                "F" => Self::Bool(false),
                "U" => Self::Unknown,
                other => Self::Enum(other.to_owned()),
            },
            Parameter::Binary(b) => Self::Str(b.clone()),
            Parameter::Reference(n) => Self::Object(*n),
            Parameter::Typed { value, .. } => Self::of(value),
            Parameter::List(items) => Self::List(items.iter().map(Self::of).collect()),
        }
    }

    /// Whether the value carries data: not null, not an empty string, not an empty list, not
    /// a logical unknown.
    pub(crate) fn usable(&self) -> bool {
        match self {
            Self::Null | Self::Unknown => false,
            Self::Str(s) => !s.is_empty(),
            Self::List(l) => !l.is_empty(),
            _ => true,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Real(r) => Some(*r),
            _ => None,
        }
    }
}

/// Equality of reals within the IDS tolerance. The bounds of the interval are included, as the
/// decimal values at the bounds are; the slack absorbs the rounding of a binary double there.
pub(crate) fn approx(x: f64, v: f64) -> bool {
    let slack = 1e-15 * (1.0 + v.abs());
    let delta = v.abs() * EPSILON + EPSILON + slack;
    x >= v - delta && x <= v + delta
}

/// The text as a number, only in the formats of IDS: digits with an optional sign, a decimal point
/// and an exponent; no `NaN`, no infinity, no thousands separators.
fn parse_number(text: &str) -> Option<f64> {
    let t = text.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    let (mantissa, exponent) = match digits.find(['e', 'E']) {
        Some(i) => (&digits[..i], Some(&digits[i + 1..])),
        None => (digits, None),
    };
    let ok_mantissa = !mantissa.is_empty()
        && mantissa.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && mantissa.bytes().filter(|b| *b == b'.').count() <= 1
        && mantissa.bytes().any(|b| b.is_ascii_digit());
    let ok_exponent = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && e.bytes().all(|b| b.is_ascii_digit())
    });
    if ok_mantissa && ok_exponent {
        t.parse::<f64>().ok().filter(|v| v.is_finite())
    } else {
        None
    }
}

fn parse_integer(text: &str) -> Option<i64> {
    let t = text.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        t.parse().ok()
    } else {
        None
    }
}

/// Whether a simple IDS value equals an IFC value.
fn simple_matches(text: &str, v: &Val) -> bool {
    match v {
        Val::Str(s) | Val::Enum(s) => s == text,
        Val::Int(i) => parse_integer(text).is_some_and(|t| t == *i),
        Val::Real(r) => parse_number(text).is_some_and(|t| approx(*r, t)),
        Val::Bool(b) => text == if *b { "true" } else { "false" },
        _ => false,
    }
}

/// Whether `v` satisfies the restriction; `Val::Str` only for the string facets.
fn restriction_matches(r: &Restriction, v: &Val) -> bool {
    let as_text = match v {
        Val::Str(s) | Val::Enum(s) => Some(s.as_str()),
        _ => None,
    };
    if !r.enumeration.is_empty() && !r.enumeration.iter().any(|e| simple_matches(e, v)) {
        return false;
    }
    if !r.patterns.is_empty()
        && !as_text.is_some_and(|t| r.patterns.iter().any(|(_, p)| p.is_match(t)))
    {
        return false;
    }
    let bounds = [
        (&r.min_inclusive, 0u8),
        (&r.min_exclusive, 1),
        (&r.max_inclusive, 2),
        (&r.max_exclusive, 3),
    ];
    if bounds.iter().any(|(b, _)| b.is_some()) {
        let Some(x) = v.number() else {
            return false;
        };
        for (bound, kind) in bounds {
            let Some(bound) = bound else { continue };
            let Some(b) = parse_number(bound) else {
                return false;
            };
            let ok = match kind {
                0 => x >= b,
                1 => x > b,
                2 => x <= b,
                _ => x < b,
            };
            if !ok {
                return false;
            }
        }
    }
    if r.length.is_some() || r.min_length.is_some() || r.max_length.is_some() {
        let Some(t) = as_text else {
            return false;
        };
        let n = t.chars().count();
        if r.length.is_some_and(|l| n != l)
            || r.min_length.is_some_and(|l| n < l)
            || r.max_length.is_some_and(|l| n > l)
        {
            return false;
        }
    }
    true
}

/// Whether an IFC value satisfies an IDS value. A list satisfies it when one of its items does.
pub(crate) fn matches(value: &Value, v: &Val) -> bool {
    if let Val::List(items) = v {
        return items.iter().any(|i| matches(value, i));
    }
    match value {
        Value::Simple(text) => simple_matches(text, v),
        Value::Restriction(r) => restriction_matches(r, v),
    }
}

/// Whether a name (class, attribute, property set, ...) satisfies an IDS value.
pub(crate) fn name_matches(value: &Value, name: &str) -> bool {
    match value {
        Value::Simple(text) => text == name,
        Value::Restriction(r) => restriction_matches(r, &Val::Str(name.to_owned())),
    }
}

/// Whether a restriction has a range bound.
pub(crate) fn has_range(value: &Value) -> bool {
    matches!(value, Value::Restriction(r) if r.min_inclusive.is_some() || r.min_exclusive.is_some()
        || r.max_inclusive.is_some() || r.max_exclusive.is_some())
}
