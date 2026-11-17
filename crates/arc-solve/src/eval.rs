//! The evaluator: dimension-checked arithmetic over an expression tree (CORE-SOLVE-017,
//! CORE-SOLVE-021).

use std::sync::Arc;

use arc_base::{Cancel, Dimension, Error, Quantity, Result, Unit};

use crate::error::{at, codes, limit, with_span};
use crate::functions::{self, Functions};
use crate::parse::{BinaryOp, Expr, ExprKind, UnaryOp};
use crate::units::combine;
use crate::value::release;
use crate::{ExprLimits, Span, Value};

/// The lookup of names that are not literals: the hook through which parameters reach
/// expressions. Closures of the same shape implement it.
pub trait Names {
    /// The value of `name`; `Ok(None)` when the name is unknown.
    ///
    /// # Errors
    ///
    /// Any error, for example that of a parameter the name stands for. Evaluation passes it on
    /// with the span of the name.
    fn lookup(&self, name: &str) -> Result<Option<Value>>;
}

impl<F: Fn(&str) -> Result<Option<Value>>> Names for F {
    fn lookup(&self, name: &str) -> Result<Option<Value>> {
        self(name)
    }
}

/// What the caller passes into an evaluation. Nothing else is reachable from an expression.
///
/// Start from [`Context::new`] and add parts with the `with_*` methods.
#[derive(Clone, Copy, Default)]
#[non_exhaustive]
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

impl<'a> Context<'a> {
    /// A context without context unit, names, host functions or token, with the default limits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// This context with `unit` as the context unit of bare numbers.
    #[must_use]
    pub fn with_bare_unit(self, unit: Unit) -> Self {
        Self {
            bare_unit: Some(unit),
            ..self
        }
    }

    /// This context with other limits.
    #[must_use]
    pub fn with_limits(self, limits: ExprLimits) -> Self {
        Self { limits, ..self }
    }

    /// This context with a lookup of names.
    #[must_use]
    pub fn with_names(self, names: &'a dyn Names) -> Self {
        Self {
            names: Some(names),
            ..self
        }
    }

    /// This context with host functions.
    #[must_use]
    pub fn with_functions(self, functions: &'a Functions) -> Self {
        Self {
            functions: Some(functions),
            ..self
        }
    }

    /// This context with a cancellation token.
    #[must_use]
    pub fn with_cancel(self, cancel: &'a Cancel) -> Self {
        Self {
            cancel: Some(cancel),
            ..self
        }
    }
}

/// A number during evaluation.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Num {
    Bare(f64),
    Dim(Quantity),
    /// An absolute temperature.
    Abs(Quantity),
}

impl Num {
    /// The number with an absolute temperature treated as an ordinary quantity.
    fn plain(self) -> Self {
        match self {
            Self::Abs(q) => Self::Dim(q),
            other => other,
        }
    }
}

/// The state of one evaluation.
pub(crate) struct Eval<'c, 'a> {
    pub ctx: &'c Context<'a>,
    steps: u64,
    bytes: u64,
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

/// An operation an absolute temperature does not take.
pub(crate) fn offset(span: Span) -> Error {
    at(codes::EXPR_OFFSET_UNIT, span)
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

    /// Counts `bytes` toward the size of the values this evaluation created or copied.
    pub(crate) fn charge(&mut self, bytes: u64, span: Span) -> Result<()> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > self.ctx.limits.value_bytes {
            return Err(limit("value_bytes", span));
        }
        Ok(())
    }

    /// Counts the elements of a new list toward the size of the values.
    pub(crate) fn charge_list(&mut self, length: usize, span: Span) -> Result<()> {
        let length = u64::try_from(length).unwrap_or(u64::MAX);
        self.charge(length.saturating_mul(ExprLimits::ELEMENT_BYTES), span)
    }

    /// Counts a new text toward the size of the values, after checking its length.
    pub(crate) fn charge_text(&mut self, text: &str, span: Span) -> Result<()> {
        if text.len() > self.ctx.limits.text_bytes {
            return Err(limit("text_bytes", span));
        }
        self.charge(u64::try_from(text.len()).unwrap_or(u64::MAX), span)
    }

    pub(crate) fn number(value: &Value, span: Span) -> Result<Num> {
        match value {
            Value::Bare(x) => Ok(Num::Bare(*x)),
            Value::Number(q) => Ok(Num::Dim(*q)),
            Value::Absolute(q) => Ok(Num::Abs(*q)),
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

    /// An absolute temperature from a computed number, which must be finite.
    pub(crate) fn make_absolute(dimension: Dimension, value: f64, span: Span) -> Result<Value> {
        if value.is_finite() {
            Ok(Value::Absolute(Quantity::new(value, dimension)))
        } else {
            Err(domain(span))
        }
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

    /// A bare number added to or subtracted from a value of `dimension`: a difference in the
    /// context unit (its scale only) when that unit has an offset, otherwise as [`Self::adopt`].
    fn adopt_difference(&self, bare: f64, dimension: Dimension, span: Span) -> Result<f64> {
        match self.ctx.bare_unit {
            Some(unit) if unit.offset != 0.0 && unit.dimension == dimension => {
                Ok(bare * unit.scale)
            }
            _ => self.adopt(bare, dimension, span),
        }
    }

    /// Whether a bare number adopted into `dimension` is a reading on a scale with an offset.
    pub(crate) fn adopts_absolute(&self, dimension: Dimension) -> bool {
        self.ctx
            .bare_unit
            .is_some_and(|unit| unit.offset != 0.0 && unit.dimension == dimension)
    }

    /// Two numbers brought to one dimension, for operations that need equal dimensions. The
    /// dimension is `None` when both are bare. Absolute temperatures count as plain quantities.
    pub(crate) fn unify(
        &self,
        a: Num,
        b: Num,
        span: Span,
    ) -> Result<(f64, f64, Option<Dimension>)> {
        match (a.plain(), b.plain()) {
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

    /// `a + b` or `a - b` under the rules for bare numbers and absolute temperatures.
    pub(crate) fn add(&self, a: Num, b: Num, subtract: bool, span: Span) -> Result<Value> {
        let apply = |x: f64, y: f64| if subtract { x - y } else { x + y };
        let same = |p: Quantity, q: Quantity| {
            if p.dimension() == q.dimension() {
                Ok(p.dimension())
            } else {
                Err(mismatch(span))
            }
        };
        match (a, b) {
            (Num::Bare(x), Num::Bare(y)) => Self::make(None, apply(x, y), span),
            (Num::Dim(p), Num::Dim(q)) => {
                Self::make(Some(same(p, q)?), apply(p.value(), q.value()), span)
            }
            // The difference of two readings is a difference; their sum means nothing.
            (Num::Abs(p), Num::Abs(q)) if subtract => {
                Self::make(Some(same(p, q)?), p.value() - q.value(), span)
            }
            (Num::Abs(_), Num::Abs(_)) => Err(offset(span)),
            (Num::Abs(p), Num::Dim(q)) => {
                Self::make_absolute(same(p, q)?, apply(p.value(), q.value()), span)
            }
            (Num::Dim(p), Num::Abs(q)) => {
                let dimension = same(p, q)?;
                if subtract {
                    return Err(offset(span));
                }
                Self::make_absolute(dimension, p.value() + q.value(), span)
            }
            (Num::Abs(p), Num::Bare(y)) => {
                let y = self.adopt_difference(y, p.dimension(), span)?;
                Self::make_absolute(p.dimension(), apply(p.value(), y), span)
            }
            (Num::Bare(x), Num::Abs(q)) => {
                if subtract {
                    return Err(offset(span));
                }
                let x = self.adopt_difference(x, q.dimension(), span)?;
                Self::make_absolute(q.dimension(), x + q.value(), span)
            }
            (Num::Dim(p), Num::Bare(y)) => {
                let y = self.adopt_difference(y, p.dimension(), span)?;
                Self::make(Some(p.dimension()), apply(p.value(), y), span)
            }
            (Num::Bare(x), Num::Dim(q)) => {
                let x = self.adopt_difference(x, q.dimension(), span)?;
                Self::make(Some(q.dimension()), apply(x, q.value()), span)
            }
        }
    }

    pub(crate) fn power(base: Num, exponent: Num, span: Span) -> Result<Value> {
        let exponent = match exponent.plain() {
            Num::Bare(e) => e,
            Num::Dim(q) if q.dimension() == Dimension::NONE => q.value(),
            _ => return Err(mismatch(span)),
        };
        match base {
            Num::Bare(b) => Self::make(None, power_value(b, exponent), span),
            Num::Abs(_) => Err(offset(span)),
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

    /// Admits a value that came from outside the expression (a name or a host function), or a
    /// copy of a part of a value: checks it against the limits without recursion, counting a
    /// step per value and its size toward `value_bytes`. A value that fails is dropped without
    /// recursion.
    pub(crate) fn admit(&mut self, value: Value, span: Span) -> Result<Value> {
        match self.measure(&value, span) {
            Ok(()) => Ok(value),
            Err(error) => {
                release(value);
                Err(error)
            }
        }
    }

    fn measure(&mut self, value: &Value, span: Span) -> Result<()> {
        let limits = self.ctx.limits;
        let mut pending = vec![(value, 0_u32)];
        while let Some((value, depth)) = pending.pop() {
            self.step(span)?;
            match value {
                Value::Number(q) | Value::Absolute(q) if !q.value().is_finite() => {
                    return Err(domain(span));
                }
                Value::Bare(x) if !x.is_finite() => return Err(domain(span)),
                Value::Text(text) => self.charge_text(text, span)?,
                Value::List(items) => {
                    if depth >= limits.effective_depth() {
                        return Err(limit("depth", span));
                    }
                    if items.len() > limits.list_length {
                        return Err(limit("list_length", span));
                    }
                    self.charge_list(items.len(), span)?;
                    pending.extend(items.iter().map(|item| (item, depth + 1)));
                }
                _ => {}
            }
        }
        Ok(())
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
            ExprKind::Binary(op, left, right) => {
                let left = self.eval(left)?;
                let right = self.eval(right)?;
                self.binary(*op, &left, &right, span)
            }
            ExprKind::Chain(first, rest) => self.chain(first, rest, span),
            ExprKind::List(items) => {
                let values = self.eval_all(items)?;
                self.charge_list(values.len(), span)?;
                Ok(Value::List(values.into()))
            }
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

    /// A run of left-associative operators, evaluated left to right without recursion along
    /// the run. An error of one operation carries the span from the first operand to its right
    /// operand, and of the last operation the span of the whole run (with its parentheses);
    /// `&&` and `||` stop at the first operand that decides the result.
    #[inline(never)]
    fn chain(&mut self, first: &Expr, rest: &[(BinaryOp, Expr)], whole: Span) -> Result<Value> {
        let mut value = self.eval(first)?;
        for (index, (op, operand)) in rest.iter().enumerate() {
            let span = if index + 1 == rest.len() {
                whole
            } else {
                first.span().to(operand.span())
            };
            self.step(span)?;
            if matches!(op, BinaryOp::And | BinaryOp::Or) {
                let Value::Bool(left) = &value else {
                    return Err(wrong_type(first.span()));
                };
                if *left == (*op == BinaryOp::Or) {
                    // Every operator of the run is the same, so the result is decided.
                    return Ok(value);
                }
                value = match self.eval(operand)? {
                    Value::Bool(right) => Value::Bool(right),
                    _ => return Err(wrong_type(operand.span())),
                };
            } else {
                let right = self.eval(operand)?;
                value = self.binary(*op, &value, &right, span)?;
            }
        }
        Ok(value)
    }

    #[inline(never)]
    fn leaf(&mut self, expr: &Expr) -> Result<Value> {
        let span = expr.span();
        match expr.kind() {
            ExprKind::Bare(x) => Ok(Value::Bare(*x)),
            ExprKind::Quantity(q) => Ok(Value::Number(*q)),
            ExprKind::Absolute(q) => Ok(Value::Absolute(*q)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Text(text) => {
                self.charge_text(text, span)?;
                Ok(Value::Text(Arc::from(text.as_str())))
            }
            ExprKind::Name(name) => {
                let found = match self.ctx.names {
                    Some(names) => names.lookup(name).map_err(|e| with_span(e, span))?,
                    None => None,
                };
                let value = found.ok_or_else(|| {
                    at(codes::EXPR_UNKNOWN_NAME, span).with_arg("name", name.as_str())
                })?;
                self.admit(value, span)
            }
            _ => Err(wrong_type(span)),
        }
    }

    #[inline(never)]
    fn unary(op: UnaryOp, operand: Value, span: Span) -> Result<Value> {
        match (op, operand) {
            (UnaryOp::Plus, value @ (Value::Bare(_) | Value::Number(_) | Value::Absolute(_))) => {
                Ok(value)
            }
            (UnaryOp::Neg, Value::Bare(x)) => Ok(Value::Bare(-x)),
            (UnaryOp::Neg, Value::Number(q)) => Ok(Value::Number(-q)),
            // A negated reading would be a temperature below absolute zero.
            (UnaryOp::Neg, Value::Absolute(_)) => Err(offset(span)),
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
            Num::Dim(q) | Num::Abs(q) => q,
        };
        let compare = |truth: fn(f64, f64) -> bool| -> Result<Value> {
            let (x, y, _) = self.unify(a, b, span)?;
            Ok(Value::Bool(truth(x, y)))
        };
        match op {
            BinaryOp::Pow => Self::power(a, b, span),
            BinaryOp::Mul | BinaryOp::Div => {
                if let (Num::Abs(_), Num::Abs(_)) = (a, b) {
                    return Err(offset(span));
                }
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
            BinaryOp::Add | BinaryOp::Sub => self.add(a, b, op == BinaryOp::Sub, span),
            BinaryOp::Eq => compare(|x, y| x == y),
            BinaryOp::Ne => compare(|x, y| x != y),
            BinaryOp::Lt => compare(|x, y| x < y),
            BinaryOp::Le => compare(|x, y| x <= y),
            BinaryOp::Gt => compare(|x, y| x > y),
            BinaryOp::Ge => compare(|x, y| x >= y),
            // Only chains hold `&&` and `||`, and they evaluate them.
            BinaryOp::And | BinaryOp::Or => Err(wrong_type(span)),
        }
    }

    /// A bare number as the value it stands for when nothing dimensioned met it: a reading in
    /// the context unit, or dimensionless without one.
    pub(crate) fn settle(&self, bare: f64, span: Span) -> Result<Value> {
        match self.ctx.bare_unit {
            Some(unit) if unit.offset != 0.0 => {
                Self::make_absolute(unit.dimension, unit.to_si(bare), span)
            }
            Some(unit) => Self::make(Some(unit.dimension), unit.to_si(bare), span),
            None => Self::make(Some(Dimension::NONE), bare, span),
        }
    }

    /// Replaces the bare numbers of a result. Lists nest at most twice the depth limit: an
    /// expression's own nesting around a value admitted from outside.
    fn finish(&self, value: Value, span: Span) -> Result<Value> {
        match value {
            Value::Bare(x) => self.settle(x, span),
            Value::List(items) => items
                .iter()
                .map(|item| self.finish(item.clone(), span))
                .collect::<Result<Vec<_>>>()
                .map(|items| Value::List(items.into())),
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
            bytes: 0,
        };
        let value = eval.eval(self)?;
        Ok((value, eval))
    }

    /// Evaluates the expression. A result that is a bare number is given the context unit (a
    /// [`Value::Absolute`] when that unit has an offset), or is dimensionless without one.
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
            Value::Number(q) | Value::Absolute(q) if q.dimension() == dimension => Ok(q),
            Value::Number(_) | Value::Absolute(_) => Err(mismatch(span)),
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
