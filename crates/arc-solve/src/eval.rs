//! The evaluator: dimension-checked arithmetic over an expression tree (CORE-SOLVE-017,
//! CORE-SOLVE-021).

use arc_base::{Cancel, Dimension, Error, Quantity, Result, Unit};

use crate::error::{at, codes, limit};
use crate::functions::{self, Functions};
use crate::parse::{BinaryOp, Expr, ExprKind, UnaryOp};
use crate::units::combine;
use crate::{ExprLimits, Span, Value};

/// The lookup of names that are not literals: the hook through which parameters reach
/// expressions. Closures of the same shape implement it.
pub trait Names {
    /// The value of `name`, or `None` when the name is unknown.
    fn lookup(&self, name: &str) -> Option<Value>;
}

impl<F: Fn(&str) -> Option<Value>> Names for F {
    fn lookup(&self, name: &str) -> Option<Value> {
        self(name)
    }
}

/// What the caller passes into an evaluation. Nothing else is reachable from an expression.
#[derive(Clone, Copy, Default)]
pub struct Context<'a> {
    /// The unit bare numbers take where a dimensioned value is needed, for example the display
    /// unit of the field. `None` keeps bare numbers dimensionless, so a bare number where a
    /// dimensioned value is needed fails with `solve.expr.dimension_mismatch`.
    pub bare_unit: Option<Unit>,
    /// The limits of parsing and evaluation.
    pub limits: ExprLimits,
    /// The lookup of names; without it every name is unknown.
    pub names: Option<&'a dyn Names>,
    /// The functions the host registered in addition to the built-in ones.
    pub functions: Option<&'a Functions>,
    /// A token checked during evaluation; a cancelled token fails with `base.cancelled`.
    pub cancel: Option<&'a Cancel>,
}

/// A number during evaluation.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Num {
    Bare(f64),
    Dim(Quantity),
}

/// The state of one evaluation.
pub(crate) struct Eval<'c, 'a> {
    pub ctx: &'c Context<'a>,
    steps: u64,
}

pub(crate) fn mismatch(span: Span) -> Error {
    at(codes::EXPR_DIMENSION_MISMATCH, span)
}

pub(crate) fn wrong_type(span: Span) -> Error {
    at(codes::EXPR_TYPE_MISMATCH, span)
}

pub(crate) fn domain(span: Span) -> Error {
    at(codes::EXPR_DOMAIN, span)
}

/// `base` to an integer power by repeated squaring: multiplications only, so the result is the
/// same on every platform.
fn integer_power(base: f64, exponent: f64) -> f64 {
    // The caller passes an integer of magnitude at most 2^31.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut remaining = exponent.abs() as u64;
    let (mut result, mut square) = (1.0, base);
    while remaining > 0 {
        if remaining & 1 == 1 {
            result *= square;
        }
        square *= square;
        remaining >>= 1;
    }
    if exponent < 0.0 { 1.0 / result } else { result }
}

fn power_value(base: f64, exponent: f64) -> f64 {
    if exponent.fract() == 0.0 && exponent.abs() <= 2_147_483_648.0 {
        integer_power(base, exponent)
    } else if exponent == 0.5 {
        base.sqrt()
    } else {
        base.powf(exponent)
    }
}

/// The dimension of a dimensioned base raised to `exponent`, which must be `n/d` with `d` from
/// 1 to 4 and leave every dimension exponent an integer.
fn power_dimension(dimension: Dimension, exponent: f64, span: Span) -> Result<Dimension> {
    for denominator in 1..=4_i32 {
        let numerator = (exponent * f64::from(denominator)).round();
        if numerator / f64::from(denominator) != exponent {
            continue;
        }
        if numerator.abs() > 508.0 {
            return Err(limit("dimension_exponent", span));
        }
        // Checked above: an integer of magnitude at most 508.
        #[allow(clippy::cast_possible_truncation)]
        let numerator = numerator as i32;
        let mut out = [0_i8; 9];
        for (o, k) in out.iter_mut().zip(dimension.exponents()) {
            let scaled = i32::from(k) * numerator;
            if scaled % denominator != 0 {
                return Err(mismatch(span));
            }
            *o = i8::try_from(scaled / denominator)
                .map_err(|_| limit("dimension_exponent", span))?;
        }
        return Ok(Dimension::from_exponents(out));
    }
    Err(mismatch(span))
}

impl Eval<'_, '_> {
    /// Counts one step and checks the cancellation token every 1,024 steps, starting with the
    /// first.
    pub(crate) fn step(&mut self, span: Span) -> Result<()> {
        self.steps += 1;
        if self.steps > self.ctx.limits.steps {
            return Err(limit("steps", span));
        }
        match self.ctx.cancel {
            Some(cancel) if self.steps % 1024 == 1 => cancel.check(),
            _ => Ok(()),
        }
    }

    pub(crate) fn number(value: &Value, span: Span) -> Result<Num> {
        match value {
            Value::Bare(x) => Ok(Num::Bare(*x)),
            Value::Number(q) => Ok(Num::Dim(*q)),
            _ => Err(wrong_type(span)),
        }
    }

    /// A value of `dimension` (`None`: bare) from a computed number, which must be finite.
    pub(crate) fn make(kind: Option<Dimension>, value: f64, span: Span) -> Result<Value> {
        if !value.is_finite() {
            return Err(domain(span));
        }
        Ok(match kind {
            None => Value::Bare(value),
            Some(dimension) => Value::Number(Quantity::new(value, dimension)),
        })
    }

    /// A bare number where a value of `dimension` is needed: unchanged for a dimensionless
    /// value, otherwise read in the context unit.
    pub(crate) fn adopt(&self, bare: f64, dimension: Dimension, span: Span) -> Result<f64> {
        if dimension == Dimension::NONE {
            return Ok(bare);
        }
        match self.ctx.bare_unit {
            Some(unit) if unit.dimension == dimension => Ok(unit.to_si(bare)),
            _ => Err(mismatch(span)),
        }
    }

    /// Two numbers brought to one dimension, for operations that need equal dimensions. The
    /// dimension is `None` when both are bare.
    pub(crate) fn unify(
        &self,
        a: Num,
        b: Num,
        span: Span,
    ) -> Result<(f64, f64, Option<Dimension>)> {
        match (a, b) {
            (Num::Bare(x), Num::Bare(y)) => Ok((x, y, None)),
            (Num::Dim(p), Num::Dim(q)) if p.dimension() == q.dimension() => {
                Ok((p.value(), q.value(), Some(p.dimension())))
            }
            (Num::Bare(x), Num::Dim(q)) => {
                let x = self.adopt(x, q.dimension(), span)?;
                Ok((x, q.value(), Some(q.dimension())))
            }
            (Num::Dim(p), Num::Bare(y)) => {
                let y = self.adopt(y, p.dimension(), span)?;
                Ok((p.value(), y, Some(p.dimension())))
            }
            _ => Err(mismatch(span)),
        }
    }

    pub(crate) fn power(base: Num, exponent: Num, span: Span) -> Result<Value> {
        let exponent = match exponent {
            Num::Bare(e) => e,
            Num::Dim(q) if q.dimension() == Dimension::NONE => q.value(),
            Num::Dim(_) => return Err(mismatch(span)),
        };
        match base {
            Num::Bare(b) => Self::make(None, power_value(b, exponent), span),
            Num::Dim(q) => {
                let dimension = if q.dimension() == Dimension::NONE {
                    Dimension::NONE
                } else {
                    power_dimension(q.dimension(), exponent, span)?
                };
                Self::make(Some(dimension), power_value(q.value(), exponent), span)
            }
        }
    }

    /// Checks a value that came from outside the expression (a name or a host function) against
    /// the limits, counting a step per value.
    pub(crate) fn admit(&mut self, value: &Value, span: Span, depth: u32) -> Result<()> {
        self.step(span)?;
        let limits = &self.ctx.limits;
        match value {
            Value::Number(q) if !q.value().is_finite() => Err(domain(span)),
            Value::Bare(x) if !x.is_finite() => Err(domain(span)),
            Value::Text(text) if text.len() > limits.text_bytes => Err(limit("text_bytes", span)),
            Value::List(items) => {
                if depth >= limits.effective_depth() {
                    return Err(limit("depth", span));
                }
                if items.len() > limits.list_length {
                    return Err(limit("list_length", span));
                }
                items
                    .iter()
                    .try_for_each(|item| self.admit(item, span, depth + 1))
            }
            _ => Ok(()),
        }
    }

    /// Evaluates a node. The arms that do work are separate functions that are not inlined, so
    /// that the frame of this recursive function stays small.
    pub(crate) fn eval(&mut self, expr: &Expr) -> Result<Value> {
        let span = expr.span();
        self.step(span)?;
        match expr.kind() {
            ExprKind::Unary(op, operand) => {
                let operand = self.eval(operand)?;
                Self::unary(*op, operand, span)
            }
            ExprKind::Binary(op @ (BinaryOp::And | BinaryOp::Or), left, right) => {
                let Value::Bool(first) = self.eval(left)? else {
                    return Err(wrong_type(left.span()));
                };
                if first == (*op == BinaryOp::Or) {
                    return Ok(Value::Bool(first));
                }
                match self.eval(right)? {
                    Value::Bool(second) => Ok(Value::Bool(second)),
                    _ => Err(wrong_type(right.span())),
                }
            }
            ExprKind::Binary(op, left, right) => {
                let left = self.eval(left)?;
                let right = self.eval(right)?;
                self.binary(*op, &left, &right, span)
            }
            ExprKind::List(items) => self.eval_all(items).map(Value::List),
            ExprKind::Call(name, args) => functions::call(self, name, args, span),
            _ => self.leaf(expr),
        }
    }

    /// The values of `items`, in order.
    #[inline(never)]
    pub(crate) fn eval_all(&mut self, items: &[Expr]) -> Result<Vec<Value>> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.eval(item)?);
        }
        Ok(values)
    }

    #[inline(never)]
    fn leaf(&mut self, expr: &Expr) -> Result<Value> {
        let span = expr.span();
        match expr.kind() {
            ExprKind::Bare(x) => Ok(Value::Bare(*x)),
            ExprKind::Quantity(q) => Ok(Value::Number(*q)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Text(text) => Ok(Value::Text(text.clone())),
            ExprKind::Name(name) => {
                let found = self.ctx.names.and_then(|names| names.lookup(name));
                let value = found.ok_or_else(|| {
                    at(codes::EXPR_UNKNOWN_NAME, span).with_arg("name", name.as_str())
                })?;
                self.admit(&value, span, 0)?;
                Ok(value)
            }
            _ => Err(wrong_type(span)),
        }
    }

    #[inline(never)]
    fn unary(op: UnaryOp, operand: Value, span: Span) -> Result<Value> {
        match (op, operand) {
            (UnaryOp::Neg, Value::Bare(x)) => Ok(Value::Bare(-x)),
            (UnaryOp::Neg, Value::Number(q)) => Ok(Value::Number(-q)),
            (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            _ => Err(wrong_type(span)),
        }
    }

    #[inline(never)]
    fn binary(&self, op: BinaryOp, left: &Value, right: &Value, span: Span) -> Result<Value> {
        if matches!(op, BinaryOp::Eq | BinaryOp::Ne) {
            let equal = match (left, right) {
                (Value::Bool(a), Value::Bool(b)) => Some(a == b),
                (Value::Text(a), Value::Text(b)) => Some(a == b),
                _ => None,
            };
            if let Some(equal) = equal {
                return Ok(Value::Bool(equal == (op == BinaryOp::Eq)));
            }
        }
        let (a, b) = (Self::number(left, span)?, Self::number(right, span)?);
        let scale = |a: Num| match a {
            Num::Bare(x) => Quantity::new(x, Dimension::NONE),
            Num::Dim(q) => q,
        };
        match op {
            BinaryOp::Pow => Self::power(a, b, span),
            BinaryOp::Mul | BinaryOp::Div => {
                let divide = op == BinaryOp::Div;
                let value = |x: f64, y: f64| if divide { x / y } else { x * y };
                if let (Num::Bare(x), Num::Bare(y)) = (a, b) {
                    return Self::make(None, value(x, y), span);
                }
                let (p, q) = (scale(a), scale(b));
                let power = if divide { -1 } else { 1 };
                let dimension = combine(p.dimension(), q.dimension(), power)
                    .ok_or_else(|| limit("dimension_exponent", span))?;
                Self::make(Some(dimension), value(p.value(), q.value()), span)
            }
            _ => {
                let (x, y, kind) = self.unify(a, b, span)?;
                let truth = match op {
                    BinaryOp::Add => return Self::make(kind, x + y, span),
                    BinaryOp::Sub => return Self::make(kind, x - y, span),
                    BinaryOp::Eq => x == y,
                    BinaryOp::Ne => x != y,
                    BinaryOp::Lt => x < y,
                    BinaryOp::Le => x <= y,
                    BinaryOp::Gt => x > y,
                    _ => x >= y,
                };
                Ok(Value::Bool(truth))
            }
        }
    }

    /// A bare number as the quantity it stands for when nothing dimensioned met it.
    pub(crate) fn settle(&self, bare: f64, span: Span) -> Result<Quantity> {
        let quantity = match self.ctx.bare_unit {
            Some(unit) => unit.quantity(bare),
            None => Quantity::new(bare, Dimension::NONE),
        };
        if quantity.value().is_finite() {
            Ok(quantity)
        } else {
            Err(domain(span))
        }
    }

    /// Replaces the bare numbers of a result; list depth is bounded by the depth limit.
    fn finish(&self, value: Value, span: Span) -> Result<Value> {
        match value {
            Value::Bare(x) => self.settle(x, span).map(Value::Number),
            Value::List(items) => items
                .into_iter()
                .map(|item| self.finish(item, span))
                .collect::<Result<_>>()
                .map(Value::List),
            other => Ok(other),
        }
    }
}

impl Expr {
    fn run<'c, 'a>(&self, context: &'c Context<'a>) -> Result<(Value, Eval<'c, 'a>)> {
        if self.depth() > context.limits.effective_depth() {
            return Err(limit("depth", self.span()));
        }
        let mut eval = Eval {
            ctx: context,
            steps: 0,
        };
        let value = eval.eval(self)?;
        Ok((value, eval))
    }

    /// Evaluates the expression. A result that is a bare number is given the context unit, or
    /// is dimensionless without one.
    ///
    /// # Errors
    ///
    /// The `solve.expr.*` codes of the specification's error table, each with the span of the
    /// offending sub-expression; `base.cancelled` when the context's token was cancelled; and
    /// whatever a host function returns.
    pub fn evaluate(&self, context: &Context<'_>) -> Result<Value> {
        let (value, eval) = self.run(context)?;
        eval.finish(value, self.span())
    }

    /// Evaluates the expression to a quantity of `dimension`. A bare result is read in the
    /// context unit when that unit has the dimension.
    ///
    /// # Errors
    ///
    /// Those of [`Expr::evaluate`]; `solve.expr.dimension_mismatch` when the result has another
    /// dimension and `solve.expr.type_mismatch` when it is not a number, both with the span of
    /// the whole expression.
    pub fn evaluate_as(&self, context: &Context<'_>, dimension: Dimension) -> Result<Quantity> {
        let (value, eval) = self.run(context)?;
        let span = self.span();
        match value {
            Value::Bare(x) => {
                let si = eval.adopt(x, dimension, span)?;
                if si.is_finite() {
                    Ok(Quantity::new(si, dimension))
                } else {
                    Err(domain(span))
                }
            }
            Value::Number(q) if q.dimension() == dimension => Ok(q),
            Value::Number(_) => Err(mismatch(span)),
            _ => Err(wrong_type(span)),
        }
    }
}

/// Parses `text` with `.` as the decimal separator and evaluates it.
///
/// # Errors
///
/// Those of [`Expr::parse`] and [`Expr::evaluate`].
pub fn evaluate(text: &str, context: &Context<'_>) -> Result<Value> {
    Expr::parse(text, &context.limits)?.evaluate(context)
}

/// Reads the text of a numeric input field: parses it with the decimal separator of the user's
/// locale and evaluates it to a quantity of `dimension`.
///
/// Text written by [`arc_base::DisplayFormat::format`] with the same separator reads back as
/// the displayed value when the context unit is the format's unit (CORE-SOLVE-022).
///
/// # Errors
///
/// Those of [`Expr::parse_localized`] and [`Expr::evaluate_as`].
pub fn parse_quantity(
    text: &str,
    context: &Context<'_>,
    decimal_separator: char,
    dimension: Dimension,
) -> Result<Quantity> {
    Expr::parse_localized(text, &context.limits, decimal_separator)?.evaluate_as(context, dimension)
}
