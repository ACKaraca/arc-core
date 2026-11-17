//! The built-in function library (CORE-SOLVE-018) and the registry of host functions.
//!
//! The table of functions with their argument and result dimensions is in
//! `docs/crates/arc-solve.md`.

use std::collections::BTreeMap;

use std::sync::Arc;

use arc_base::{Dimension, DisplayFormat, Error, Quantity, Result, Unit};

use crate::error::{at, codes, with_span};
use crate::eval::{Eval, Num, domain, mismatch, offset, wrong_type};
use crate::parse::{Expr, ExprKind};
use crate::units::lookup;
use crate::{Span, Value};

/// The built-in functions with their least and greatest argument counts.
const BUILTINS: [(&str, usize, usize); 24] = [
    ("abs", 1, 1),
    ("min", 1, usize::MAX),
    ("max", 1, usize::MAX),
    ("sqrt", 1, 1),
    ("pow", 2, 2),
    ("sin", 1, 1),
    ("cos", 1, 1),
    ("tan", 1, 1),
    ("asin", 1, 1),
    ("acos", 1, 1),
    ("atan", 1, 1),
    ("atan2", 2, 2),
    ("round", 1, 2),
    ("floor", 1, 2),
    ("ceil", 1, 2),
    ("if", 3, 3),
    ("len", 1, 1),
    ("sum", 1, 1),
    ("first", 1, 1),
    ("last", 1, 1),
    ("format", 2, 3),
    ("concat", 0, usize::MAX),
    // Reserved so that a host function cannot shadow a literal.
    ("true", 0, 0),
    ("false", 0, 0),
];

type HostFn = dyn Fn(&[Value], Option<Unit>) -> Result<Value> + Send + Sync;

struct Host {
    min_args: usize,
    max_args: usize,
    call: Box<HostFn>,
}

/// The pure functions a host adds to the built-in set (the first part of CORE-SOLVE-019).
///
/// A registered function is called with evaluated arguments and the context unit
/// ([`Context::bare_unit`](crate::Context::bare_unit)). A number written without a unit arrives as
/// [`Value::Bare`]; the function decides whether it is a count or a value in the context unit.
/// Its result is checked against the limits of the evaluation without recursion (a result that
/// fails is also dropped without recursion), and an error it returns is passed on with the span of
/// the call.
#[derive(Default)]
pub struct Functions {
    hosts: BTreeMap<String, Host>,
}

impl Functions {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `function` under `name` for calls with `min_args` to `max_args` arguments.
    /// The function receives the arguments and the context unit. It must be pure: the same
    /// arguments and unit give the same result, with no effect.
    ///
    /// # Errors
    ///
    /// `solve.function.invalid` when the name is not an ASCII identifier, names a built-in
    /// function, `true` or `false`, is already registered, or `min_args` exceeds `max_args`.
    pub fn register(
        &mut self,
        name: &str,
        min_args: usize,
        max_args: usize,
        function: impl Fn(&[Value], Option<Unit>) -> Result<Value> + Send + Sync + 'static,
    ) -> Result<()> {
        let identifier = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let taken = BUILTINS.iter().any(|b| b.0 == name) || self.hosts.contains_key(name);
        if !identifier || taken || min_args > max_args {
            return Err(Error::new(codes::FUNCTION_INVALID).with_arg("name", name));
        }
        let host = Host {
            min_args,
            max_args,
            call: Box::new(function),
        };
        self.hosts.insert(name.to_owned(), host);
        Ok(())
    }
}

/// Evaluates the call of `name` with `args`. Only argument evaluation is on the recursion path;
/// the work is in functions that are not inlined.
pub(crate) fn call(
    eval: &mut Eval<'_, '_>,
    name: &str,
    args: &[Expr],
    span: Span,
) -> Result<Value> {
    let builtin = signature(eval, name, args.len(), span)?;
    if let ("if", [condition, then, otherwise]) = (name, args) {
        // Only the chosen branch is evaluated.
        return match eval.eval(condition)? {
            Value::Bool(true) => eval.eval(then),
            Value::Bool(false) => eval.eval(otherwise),
            _ => Err(wrong_type(condition.span())),
        };
    }
    let values = eval.eval_all(args)?;
    if builtin {
        // The unit an increment was written in, so that rounding happens in that unit: the
        // number as written, the scale of the unit and its dimension.
        let increment = args.get(1).and_then(|arg| match arg.kind() {
            ExprKind::Bare(value) => eval
                .ctx
                .bare_unit
                .filter(|unit| unit.offset == 0.0)
                .map(|unit| (*value, unit.scale, unit.dimension)),
            ExprKind::Quantity(q) => arg
                .written()
                .map(|(value, scale)| (value, scale, q.dimension())),
            _ => None,
        });
        apply(eval, name, &values, increment, span)
    } else {
        host(eval, name, &values, span)
    }
}

/// Checks that `name` is a function taking `count` arguments; `true` for a built-in one.
#[inline(never)]
fn signature(eval: &Eval<'_, '_>, name: &str, count: usize, span: Span) -> Result<bool> {
    let builtin = BUILTINS.iter().find(|b| b.0 == name);
    let host = eval.ctx.functions.and_then(|f| f.hosts.get(name));
    let (min, max) = match (builtin, host) {
        (Some(&(_, min, max)), _) => (min, max),
        (None, Some(host)) => (host.min_args, host.max_args),
        (None, None) => {
            return Err(at(codes::EXPR_UNKNOWN_FUNCTION, span).with_arg("name", name));
        }
    };
    if (min..=max).contains(&count) {
        Ok(builtin.is_some())
    } else {
        Err(at(codes::EXPR_ARGUMENT_COUNT, span))
    }
}

#[inline(never)]
fn host(eval: &mut Eval<'_, '_>, name: &str, values: &[Value], span: Span) -> Result<Value> {
    let unknown = || at(codes::EXPR_UNKNOWN_FUNCTION, span).with_arg("name", name);
    let host = eval.ctx.functions.and_then(|f| f.hosts.get(name));
    let result = (host.ok_or_else(unknown)?.call)(values, eval.ctx.bare_unit)
        .map_err(|e| with_span(e, span))?;
    eval.admit(result, span)
}

/// A dimensionless number: bare or of no dimension.
fn scalar(value: Option<&Value>, span: Span) -> Result<f64> {
    match value {
        Some(Value::Bare(x)) => Ok(*x),
        Some(Value::Number(q)) if q.dimension() == Dimension::NONE => Ok(q.value()),
        Some(Value::Number(_) | Value::Absolute(_)) => Err(mismatch(span)),
        _ => Err(wrong_type(span)),
    }
}

/// The error bound of a quotient of two values converted from units to SI, in units of
/// `f64::EPSILON` relative to the quotient: the two conversions and the division each round once,
/// so `6 in / 1 in` lands within a few units in the last place of 6. This is a bound on
/// floating-point error, not a geometric tolerance.
const QUOTIENT_ERROR_EPSILONS: f64 = 8.0;

/// Whether `quotient` equals `target` within [`QUOTIENT_ERROR_EPSILONS`].
fn within_error(quotient: f64, target: f64) -> bool {
    (quotient - target).abs() <= QUOTIENT_ERROR_EPSILONS * f64::EPSILON * quotient.abs().max(1.0)
}

/// The number of increments `function` (`round`, `floor` or `ceil`) gives for the quotient of a
/// value and its increment. A quotient within the error bound of a whole number is that number;
/// one within the bound of a half is a tie, which `round` takes away from zero.
fn increments(function: &str, quotient: f64) -> f64 {
    let whole = quotient.round();
    if within_error(quotient, whole) {
        return whole;
    }
    let half = quotient.floor() + 0.5;
    match function {
        "round" if within_error(quotient, half) => half + 0.5_f64.copysign(half),
        "round" => quotient.round(),
        "floor" => quotient.floor(),
        _ => quotient.ceil(),
    }
}

/// `count` increments: divided by the reciprocal of the increment when that is a whole number,
/// so that decimal increments give the nearest decimal (`7 / 10` rather than `7 * 0.1`).
fn multiple(count: f64, increment: f64) -> f64 {
    let reciprocal = 1.0 / increment;
    let whole = reciprocal.round();
    if whole >= 1.0 && within_error(reciprocal, whole) {
        count / whole
    } else {
        count * increment
    }
}

fn number(value: Option<&Value>, span: Span) -> Result<Num> {
    value.map_or_else(|| Err(wrong_type(span)), |value| Eval::number(value, span))
}

fn list(value: Option<&Value>, span: Span) -> Result<&[Value]> {
    match value {
        Some(Value::List(items)) => Ok(items),
        _ => Err(wrong_type(span)),
    }
}

fn dimensionless(value: f64) -> Value {
    Value::Number(Quantity::new(value, Dimension::NONE))
}

#[allow(clippy::too_many_lines)] // One arm per function of the closed set.
#[inline(never)]
fn apply(
    eval: &mut Eval<'_, '_>,
    name: &str,
    values: &[Value],
    increment_unit: Option<(f64, f64, Dimension)>,
    span: Span,
) -> Result<Value> {
    let first = values.first();
    let second = values.get(1);
    match name {
        "abs" => Ok(match number(first, span)? {
            Num::Bare(x) => Value::Bare(x.abs()),
            Num::Dim(q) => Value::Number(Quantity::new(q.value().abs(), q.dimension())),
            Num::Abs(q) => Value::Absolute(Quantity::new(q.value().abs(), q.dimension())),
        }),
        "min" | "max" => {
            let items = match values {
                [Value::List(items)] => &items[..],
                _ => values,
            };
            let Some((head, tail)) = items.split_first() else {
                return Err(domain(span));
            };
            let mut best = Eval::number(head, span)?;
            for item in tail {
                eval.step(span)?;
                let next = Eval::number(item, span)?;
                let (x, y, kind) = eval.unify(best, next, span)?;
                let take = if name == "min" { y < x } else { y > x };
                let (chosen, value) = if take { (next, y) } else { (best, x) };
                // A bare number that met a dimensioned one is now a value of its dimension.
                best = match (kind, chosen) {
                    (None, _) => Num::Bare(value),
                    (Some(dimension), Num::Abs(_)) => Num::Abs(Quantity::new(value, dimension)),
                    (Some(dimension), Num::Bare(_)) if eval.adopts_absolute(dimension) => {
                        Num::Abs(Quantity::new(value, dimension))
                    }
                    (Some(dimension), _) => Num::Dim(Quantity::new(value, dimension)),
                };
            }
            Ok(match best {
                Num::Bare(x) => Value::Bare(x),
                Num::Dim(q) => Value::Number(q),
                Num::Abs(q) => Value::Absolute(q),
            })
        }
        "sum" => {
            let [Value::List(items)] = values else {
                return Err(wrong_type(span));
            };
            let Some((head, tail)) = items.split_first() else {
                return Ok(Value::Bare(0.0));
            };
            let mut total = head.clone();
            Eval::number(&total, span)?;
            for item in tail {
                eval.step(span)?;
                let (a, b) = (Eval::number(&total, span)?, Eval::number(item, span)?);
                total = eval.add(a, b, false, span)?;
            }
            Ok(total)
        }
        "sqrt" => match number(first, span)? {
            Num::Bare(x) => Eval::make(None, x.sqrt(), span),
            Num::Abs(_) => Err(offset(span)),
            Num::Dim(q) => {
                let mut half = q.dimension().exponents();
                if half.iter().any(|k| k % 2 != 0) {
                    return Err(mismatch(span));
                }
                half.iter_mut().for_each(|k| *k /= 2);
                Eval::make(
                    Some(Dimension::from_exponents(half)),
                    q.value().sqrt(),
                    span,
                )
            }
        },
        "pow" => Eval::power(number(first, span)?, number(second, span)?, span),
        "sin" | "cos" | "tan" => {
            // An angle in radians (its SI value). A bare argument is read in the context unit
            // when that is an angle, otherwise as radians, like a dimensionless one.
            let radians = match number(first, span)? {
                Num::Bare(x) => match eval.ctx.bare_unit {
                    Some(unit) if unit.dimension == Dimension::ANGLE => unit.to_si(x),
                    _ => x,
                },
                Num::Dim(q) if [Dimension::NONE, Dimension::ANGLE].contains(&q.dimension()) => {
                    q.value()
                }
                Num::Dim(_) | Num::Abs(_) => return Err(mismatch(span)),
            };
            let value = match name {
                "sin" => radians.sin(),
                "cos" => radians.cos(),
                _ => radians.tan(),
            };
            Eval::make(Some(Dimension::NONE), value, span)
        }
        "asin" | "acos" | "atan" => {
            let x = scalar(first, span)?;
            let value = match name {
                "asin" => x.asin(),
                "acos" => x.acos(),
                _ => x.atan(),
            };
            Eval::make(Some(Dimension::ANGLE), value, span)
        }
        "atan2" => {
            let (y, x, _) = eval.unify(number(first, span)?, number(second, span)?, span)?;
            Eval::make(Some(Dimension::ANGLE), y.atan2(x), span)
        }
        "round" | "floor" | "ceil" => {
            let rounded: fn(f64) -> f64 = match name {
                "round" => f64::round,
                "floor" => f64::floor,
                _ => f64::ceil,
            };
            let Some(increment) = second else {
                // Without an increment the result would depend on the unit of a dimensioned value.
                return match number(first, span)? {
                    Num::Bare(x) => Eval::make(None, rounded(x), span),
                    Num::Dim(q) if q.dimension() == Dimension::NONE => {
                        Eval::make(Some(Dimension::NONE), rounded(q.value()), span)
                    }
                    Num::Abs(_) => Err(offset(span)),
                    Num::Dim(_) => Err(mismatch(span)),
                };
            };
            let (value, increment) = (number(first, span)?, Eval::number(increment, span)?);
            if matches!(value, Num::Abs(_)) || matches!(increment, Num::Abs(_)) {
                // The multiples of an increment on a scale with an offset depend on the scale.
                return Err(offset(span));
            }
            let (x, step, kind) = eval.unify(value, increment, span)?;
            if step <= 0.0 {
                return Err(domain(span));
            }
            // In the unit the increment was written in, when it is known and has the dimension
            // of the operation, so that multiples written in that unit are exact.
            let written = increment_unit
                .filter(|(_, _, dimension)| kind == Some(*dimension))
                .map(|(step, scale, _)| (step, scale));
            let (x_written, step_written) = match written {
                Some((step, scale)) => (x / scale, step),
                None => (x, step),
            };
            let quotient = x_written / step_written;
            if !quotient.is_finite() {
                return Err(domain(span));
            }
            let count = increments(name, quotient);
            let result = if within_error(quotient, count) {
                // A value that is already a multiple stays exactly as it was.
                x
            } else if let Some((step, scale)) = written {
                // The multiple as a literal in that unit would give it.
                multiple(count, step) * scale
            } else {
                multiple(count, step)
            };
            Eval::make(kind, result, span)
        }
        "len" => match first {
            Some(Value::List(items)) => Ok(dimensionless(items.len() as f64)),
            Some(Value::Text(text)) => Ok(dimensionless(text.chars().count() as f64)),
            _ => Err(wrong_type(span)),
        },
        "first" | "last" => {
            let items = list(first, span)?;
            let item = if name == "first" {
                items.first()
            } else {
                items.last()
            };
            let item = item.cloned().ok_or_else(|| domain(span))?;
            eval.admit(item, span)
        }
        "format" => {
            let quantity = match number(first, span)? {
                Num::Bare(x) => eval
                    .settle(x, span)?
                    .quantity()
                    .ok_or_else(|| wrong_type(span))?,
                Num::Dim(q) | Num::Abs(q) => q,
            };
            let Some(Value::Text(unit_text)) = second else {
                return Err(wrong_type(span));
            };
            let unit = lookup(unit_text)
                .ok_or_else(|| at(codes::EXPR_UNKNOWN_UNIT, span).with_arg("name", &**unit_text))?;
            if unit.dimension != quantity.dimension() {
                return Err(mismatch(span));
            }
            let precision = match values.get(2) {
                None => 3,
                decimals => {
                    let decimals = scalar(decimals, span)?;
                    (0..=17_u8)
                        .find(|d| f64::from(*d) == decimals)
                        .ok_or_else(|| domain(span))?
                }
            };
            let format = DisplayFormat {
                precision,
                ..DisplayFormat::new(unit)
            };
            let text = format
                .format(quantity, '.')
                .map_err(|e| with_span(e, span))?;
            eval.charge_text(&text, span)?;
            Ok(Value::Text(text.into()))
        }
        "concat" => {
            let mut out = String::new();
            for value in values {
                let Value::Text(text) = value else {
                    return Err(wrong_type(span));
                };
                if out.len() + text.len() > eval.ctx.limits.text_bytes {
                    return Err(crate::error::limit("text_bytes", span));
                }
                out.push_str(text);
            }
            eval.charge_text(&out, span)?;
            Ok(Value::Text(Arc::from(out)))
        }
        // `true(...)` and `false(...)` are reserved names, not functions.
        _ => Err(at(codes::EXPR_UNKNOWN_FUNCTION, span).with_arg("name", name)),
    }
}
