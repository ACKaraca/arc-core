//! The built-in function library (CORE-SOLVE-018) and the registry of host functions.
//!
//! The table of functions with their argument and result dimensions is in
//! `docs/crates/arc-solve.md`.

use std::collections::BTreeMap;

use arc_base::{Dimension, DisplayFormat, Error, Quantity, Result};

use crate::error::{at, codes, limit, with_span};
use crate::eval::{Eval, Num, domain, mismatch, wrong_type};
use crate::parse::Expr;
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

type HostFn = dyn Fn(&[Value]) -> Result<Value> + Send + Sync;

struct Host {
    min_args: usize,
    max_args: usize,
    call: Box<HostFn>,
}

/// The pure functions a host adds to the built-in set (the first part of CORE-SOLVE-019).
///
/// A registered function is called with evaluated arguments, in which a number written without
/// a unit is [`Value::Bare`]. Its result is checked against the limits of the evaluation, and an
/// error it returns is passed on with the span of the call.
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
    /// The function must be pure: the same arguments give the same result, with no effect.
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
        function: impl Fn(&[Value]) -> Result<Value> + Send + Sync + 'static,
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
        apply(eval, name, &values, span)
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
    let result = (host.ok_or_else(unknown)?.call)(values).map_err(|e| with_span(e, span))?;
    eval.admit(&result, span, 0)?;
    Ok(result)
}

/// A dimensionless number: bare or of no dimension.
fn scalar(value: Option<&Value>, span: Span) -> Result<f64> {
    match value {
        Some(Value::Bare(x)) => Ok(*x),
        Some(Value::Number(q)) if q.dimension() == Dimension::NONE => Ok(q.value()),
        Some(Value::Number(_)) => Err(mismatch(span)),
        _ => Err(wrong_type(span)),
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
fn apply(eval: &mut Eval<'_, '_>, name: &str, values: &[Value], span: Span) -> Result<Value> {
    let first = values.first();
    let second = values.get(1);
    match name {
        "abs" => Ok(match number(first, span)? {
            Num::Bare(x) => Value::Bare(x.abs()),
            Num::Dim(q) => Value::Number(Quantity::new(q.value().abs(), q.dimension())),
        }),
        "min" | "max" | "sum" => {
            let items = match values {
                [Value::List(items)] => items.as_slice(),
                _ if name == "sum" => return Err(wrong_type(span)),
                _ => values,
            };
            let Some((head, tail)) = items.split_first() else {
                return if name == "sum" {
                    Ok(Value::Bare(0.0))
                } else {
                    Err(domain(span))
                };
            };
            let mut total = head.clone();
            Eval::number(&total, span)?;
            for item in tail {
                eval.step(span)?;
                let (a, b) = (Eval::number(&total, span)?, Eval::number(item, span)?);
                let (x, y, kind) = eval.unify(a, b, span)?;
                let value = match name {
                    "sum" => x + y,
                    "min" => x.min(y),
                    _ => x.max(y),
                };
                total = Eval::make(kind, value, span)?;
            }
            Ok(total)
        }
        "sqrt" => match number(first, span)? {
            Num::Bare(x) => Eval::make(None, x.sqrt(), span),
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
            // An angle in radians (its SI value); a dimensionless argument is read as radians.
            let radians = match number(first, span)? {
                Num::Bare(x) => x,
                Num::Dim(q) if [Dimension::NONE, Dimension::ANGLE].contains(&q.dimension()) => {
                    q.value()
                }
                Num::Dim(_) => return Err(mismatch(span)),
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
                    Num::Dim(_) => Err(mismatch(span)),
                };
            };
            let increment = Eval::number(increment, span)?;
            let (x, step, kind) = eval.unify(number(first, span)?, increment, span)?;
            if step <= 0.0 {
                return Err(domain(span));
            }
            Eval::make(kind, rounded(x / step) * step, span)
        }
        "len" => match first {
            Some(Value::List(items)) => Ok(dimensionless(items.len() as f64)),
            Some(Value::Text(text)) => Ok(dimensionless(text.chars().count() as f64)),
            _ => Err(wrong_type(span)),
        },
        "first" => list(first, span)?
            .first()
            .cloned()
            .ok_or_else(|| domain(span)),
        "last" => list(first, span)?
            .last()
            .cloned()
            .ok_or_else(|| domain(span)),
        "format" => {
            let quantity = match number(first, span)? {
                Num::Bare(x) => eval.settle(x, span)?,
                Num::Dim(q) => q,
            };
            let Some(Value::Text(unit_text)) = second else {
                return Err(wrong_type(span));
            };
            let unit = lookup(unit_text).ok_or_else(|| {
                at(codes::EXPR_UNKNOWN_UNIT, span).with_arg("name", unit_text.as_str())
            })?;
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
            Ok(Value::Text(text))
        }
        "concat" => {
            let mut out = String::new();
            for value in values {
                let Value::Text(text) = value else {
                    return Err(wrong_type(span));
                };
                if out.len() + text.len() > eval.ctx.limits.text_bytes {
                    return Err(limit("text_bytes", span));
                }
                out.push_str(text);
            }
            Ok(Value::Text(out))
        }
        // `true(...)` and `false(...)` are reserved names, not functions.
        _ => Err(at(codes::EXPR_UNKNOWN_FUNCTION, span).with_arg("name", name)),
    }
}
