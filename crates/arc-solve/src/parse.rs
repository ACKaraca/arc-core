//! The parser: tokens to an expression tree with byte spans (CORE-SOLVE-016).
//!
//! The grammar is in `docs/crates/arc-solve.md`. The parser recurses only for parentheses, lists
//! and calls, counts that nesting, and resolves operators on explicit stacks, so no input exhausts
//! the stack; the depth of the resulting tree is bounded by the same limit. A run of
//! left-associative operators of one precedence level becomes one [`ExprKind::Chain`] node, so
//! its length does not count toward the depth.

use arc_base::{Dimension, Quantity, Result, Unit};

use crate::error::{at, codes, limit, with_span};
use crate::lex::{Tok, Token, check_separator, decode_text, lex};
use crate::units::resolve;
use crate::{ExprLimits, Span};

/// A prefix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnaryOp {
    /// `+x`: a number unchanged.
    Plus,
    /// `-x`.
    Neg,
    /// `!x`.
    Not,
}

/// An infix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BinaryOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`.
    Div,
    /// `^`.
    Pow,
    /// `==`.
    Eq,
    /// `!=`.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// `&&`.
    And,
    /// `||`.
    Or,
}

/// The kind of an expression node.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ExprKind {
    /// A number written without a unit.
    Bare(f64),
    /// A number with a unit or in feet and inches, converted to SI base units.
    Quantity(Quantity),
    /// A reading on a temperature scale with an offset (`20 °C`): an absolute temperature in
    /// kelvin.
    Absolute(Quantity),
    /// `true` or `false`.
    Bool(bool),
    /// A text literal with its escapes resolved.
    Text(String),
    /// A name the caller's lookup resolves.
    Name(String),
    /// `[a, b, ...]`.
    List(Vec<Expr>),
    /// A prefix operator and its operand.
    Unary(UnaryOp, Box<Expr>),
    /// `^` or a comparison and its operands.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A left-associative run of operators of one precedence level (`+` and `-`, `*` and `/`,
    /// `&&`, or `||`): the first operand, then each operator with its right operand. `a - b + c`
    /// is `(a - b) + c`.
    Chain(Box<Expr>, Vec<(BinaryOp, Expr)>),
    /// A function name and its arguments.
    Call(String, Vec<Expr>),
}

/// A parsed expression: a tree of nodes, each with the byte span of its source text.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    kind: ExprKind,
    span: Span,
    depth: u32,
    /// Written in parentheses; a parenthesized chain is not extended.
    grouped: bool,
    /// For a number literal with a unit without offset, the number as written and the scale of
    /// the unit.
    written: Option<(f64, f64)>,
}

impl Expr {
    /// Parses an expression; `.` is the decimal separator.
    ///
    /// # Errors
    ///
    /// `solve.expr.syntax`, `solve.expr.unknown_unit`, `solve.expr.offset_unit`,
    /// `solve.expr.domain` for a literal that is not a finite number, and
    /// `solve.expr.limit_exceeded` for input longer than `limits.text_bytes`, nesting deeper than
    /// the depth limit, or a list or argument list longer than `limits.list_length`. Every error
    /// carries the span of the offending text.
    pub fn parse(text: &str, limits: &ExprLimits) -> Result<Self> {
        Self::parse_localized(text, limits, '.')
    }

    /// Parses an expression typed with the decimal separator of the user's locale. `.` stays a
    /// decimal separator; when `decimal_separator` is `,`, arguments and list elements are
    /// separated by `;` only.
    ///
    /// # Errors
    ///
    /// Those of [`Expr::parse`], and `solve.expr.separator_invalid` when the separator is not
    /// `.`, `,` or a character outside ASCII that is not a letter, digit, space or unit
    /// character.
    pub fn parse_localized(
        text: &str,
        limits: &ExprLimits,
        decimal_separator: char,
    ) -> Result<Self> {
        check_separator(decimal_separator)?;
        let whole = Span {
            start: 0,
            end: text.len(),
        };
        if text.len() > limits.text_bytes {
            return Err(limit("text_bytes", whole));
        }
        let mut parser = Parser {
            text,
            tokens: lex(text, decimal_separator)?,
            pos: 0,
            nesting: 0,
            max_depth: limits.effective_depth(),
            max_list: limits.list_length,
        };
        let expr = parser.expr()?;
        let rest = parser.peek();
        if rest.tok == Tok::End {
            Ok(expr)
        } else {
            Err(at(codes::EXPR_SYNTAX, rest.span))
        }
    }

    /// The kind of the root node.
    #[must_use]
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// The byte span of the source text of this node.
    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }

    /// The depth of the tree: 1 for a leaf.
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// For a number literal with a unit without offset, the number as written in that unit and
    /// the scale of the unit (feet and inches in inches).
    pub(crate) fn written(&self) -> Option<(f64, f64)> {
        self.written
    }
}

const END: Token = Token {
    tok: Tok::End,
    span: Span { start: 0, end: 0 },
};

struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    nesting: u32,
    max_depth: u32,
    max_list: usize,
}

/// An operator waiting on the parser's stack for its right operand.
#[derive(Clone, Copy)]
enum Pending {
    Prefix(UnaryOp),
    Infix(BinaryOp),
}

/// The binding strength of an infix operator: `||`, `&&`, comparisons, sums, products, `^`.
/// Prefix operators bind at 6, between products and `^`.
fn level(op: BinaryOp) -> u8 {
    match op {
        BinaryOp::Or => 1,
        BinaryOp::And => 2,
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            3
        }
        BinaryOp::Add | BinaryOp::Sub => 4,
        BinaryOp::Mul | BinaryOp::Div => 5,
        BinaryOp::Pow => 7,
    }
}

/// Whether `op` forms [`ExprKind::Chain`] runs: the left-associative operators.
fn chains(op: BinaryOp) -> bool {
    !matches!(level(op), 3 | 7)
}

impl Pending {
    fn precedence(self) -> u8 {
        match self {
            Self::Prefix(_) => 6,
            Self::Infix(op) => level(op),
        }
    }
}

fn infix(tok: Tok) -> Option<BinaryOp> {
    Some(match tok {
        Tok::OrOr => BinaryOp::Or,
        Tok::AndAnd => BinaryOp::And,
        Tok::EqEq => BinaryOp::Eq,
        Tok::Ne => BinaryOp::Ne,
        Tok::Lt => BinaryOp::Lt,
        Tok::Le => BinaryOp::Le,
        Tok::Gt => BinaryOp::Gt,
        Tok::Ge => BinaryOp::Ge,
        Tok::Plus => BinaryOp::Add,
        Tok::Minus => BinaryOp::Sub,
        Tok::Star => BinaryOp::Mul,
        Tok::Slash => BinaryOp::Div,
        Tok::Caret => BinaryOp::Pow,
        _ => return None,
    })
}

fn unit_named(name: &str) -> Result<Unit> {
    Unit::by_name(name).copied()
}

impl Parser<'_> {
    fn at(&self, index: usize) -> Token {
        self.tokens
            .get(index)
            .or(self.tokens.last())
            .copied()
            .unwrap_or(END)
    }

    fn peek(&self) -> Token {
        self.at(self.pos)
    }

    fn bump(&mut self) -> Token {
        let token = self.peek();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn source(&self, span: Span) -> &str {
        self.text.get(span.start..span.end).unwrap_or("")
    }

    /// Whether token `index` starts where the token before it ends.
    fn adjacent(&self, index: usize) -> bool {
        index > 0 && self.at(index).span.start == self.at(index - 1).span.end
    }

    fn node(&self, kind: ExprKind, span: Span) -> Result<Expr> {
        let child = |e: &Expr| e.depth;
        let children = match &kind {
            ExprKind::List(items) | ExprKind::Call(_, items) => {
                items.iter().map(child).max().unwrap_or(0)
            }
            ExprKind::Chain(first, rest) => rest
                .iter()
                .map(|(_, e)| e.depth)
                .fold(first.depth, u32::max),
            ExprKind::Unary(_, operand) => operand.depth,
            ExprKind::Binary(_, left, right) => left.depth.max(right.depth),
            _ => 0,
        };
        let depth = children.saturating_add(1);
        if depth > self.max_depth {
            return Err(limit("depth", span));
        }
        Ok(Expr {
            kind,
            span,
            depth,
            grouped: false,
            written: None,
        })
    }

    /// Runs `inner` one nesting level deeper.
    /// Parses one expression, one nesting level deeper. This and [`Self::primary`] are the only
    /// functions on the recursion path; operators are resolved on explicit stacks.
    fn expr(&mut self) -> Result<Expr> {
        if self.nesting >= self.max_depth {
            return Err(limit("depth", self.peek().span));
        }
        self.nesting += 1;
        let result = self.operators();
        self.nesting -= 1;
        result
    }

    /// Whether the minus at the current position is the sign of a reading on an offset scale.
    fn offset_reading(&self) -> bool {
        let number = matches!(self.at(self.pos + 1).tok, Tok::Number { .. });
        if !(number && self.at(self.pos + 2).tok == Tok::Ident) {
            return false;
        }
        self.unit_suffix(self.pos + 2)
            .and_then(|(unit, _)| resolve(self.source(unit)))
            .is_ok_and(|unit| unit.offset != 0.0)
    }

    /// Applies the operator on top of `pending` to the operands on top of `operands`.
    fn reduce(&self, operands: &mut Vec<Expr>, pending: &mut Vec<(Pending, Span)>) -> Result<()> {
        let missing = || at(codes::EXPR_SYNTAX, self.peek().span);
        let (operator, at_span) = pending.pop().ok_or_else(missing)?;
        let right = operands.pop().ok_or_else(missing)?;
        let node = match operator {
            Pending::Prefix(op) => {
                let span = at_span.to(right.span);
                self.node(ExprKind::Unary(op, Box::new(right)), span)?
            }
            Pending::Infix(op) if chains(op) => {
                let mut left = operands.pop().ok_or_else(missing)?;
                let span = left.span.to(right.span);
                if let ExprKind::Chain(_, rest) = &mut left.kind
                    && !left.grouped
                    && rest
                        .first()
                        .is_some_and(|(first, _)| level(*first) == level(op))
                {
                    let depth = left.depth.max(right.depth.saturating_add(1));
                    if depth > self.max_depth {
                        return Err(limit("depth", span));
                    }
                    rest.push((op, right));
                    left.span = span;
                    left.depth = depth;
                    left
                } else {
                    self.node(ExprKind::Chain(Box::new(left), vec![(op, right)]), span)?
                }
            }
            Pending::Infix(op) => {
                let left = operands.pop().ok_or_else(missing)?;
                let span = left.span.to(right.span);
                self.node(ExprKind::Binary(op, Box::new(left), Box::new(right)), span)?
            }
        };
        operands.push(node);
        Ok(())
    }

    /// Operator precedence parsing without recursion: `^` binds tightest and to the right, then
    /// the prefix operators, products, sums, comparisons (which do not chain), `&&` and `||`.
    fn operators(&mut self) -> Result<Expr> {
        let mut operands: Vec<Expr> = Vec::new();
        let mut pending: Vec<(Pending, Span)> = Vec::new();
        loop {
            loop {
                let token = self.peek();
                let prefix = match token.tok {
                    Tok::Minus if !self.offset_reading() => UnaryOp::Neg,
                    Tok::Bang => UnaryOp::Not,
                    Tok::Plus => UnaryOp::Plus,
                    _ => break,
                };
                self.bump();
                pending.push((Pending::Prefix(prefix), token.span));
            }
            operands.push(self.primary()?);
            let token = self.peek();
            let Some(op) = infix(token.tok) else { break };
            let incoming = Pending::Infix(op).precedence();
            while let Some((top, _)) = pending.last() {
                let top = top.precedence();
                // `^` associates to the right; everything else to the left.
                if top < incoming || (top == incoming && op == BinaryOp::Pow) {
                    break;
                }
                if top == incoming && incoming == 3 {
                    // Comparisons do not chain.
                    return Err(at(codes::EXPR_SYNTAX, token.span));
                }
                self.reduce(&mut operands, &mut pending)?;
            }
            self.bump();
            pending.push((Pending::Infix(op), token.span));
        }
        while !pending.is_empty() {
            self.reduce(&mut operands, &mut pending)?;
        }
        operands
            .pop()
            .ok_or_else(|| at(codes::EXPR_SYNTAX, self.peek().span))
    }

    fn primary(&mut self) -> Result<Expr> {
        let token = self.peek();
        match token.tok {
            Tok::Number { .. } => self.number_literal(1.0),
            // On a scale with an offset the sign belongs to the reading: `-40 °C` is 233.15 K.
            Tok::Minus if self.offset_reading() => {
                self.bump();
                let mut literal = self.number_literal(-1.0)?;
                literal.span = token.span.to(literal.span);
                Ok(literal)
            }
            Tok::Text => {
                self.bump();
                let text = decode_text(self.source(token.span));
                self.node(ExprKind::Text(text), token.span)
            }
            Tok::Ident => {
                self.bump();
                let name = self.source(token.span).to_owned();
                if self.peek().tok == Tok::LParen {
                    let (args, span) = self.items(token.span, Tok::RParen)?;
                    return self.node(ExprKind::Call(name, args), span);
                }
                let kind = match name.as_str() {
                    "true" => ExprKind::Bool(true),
                    "false" => ExprKind::Bool(false),
                    _ => ExprKind::Name(name),
                };
                self.node(kind, token.span)
            }
            Tok::LParen => {
                self.bump();
                let mut inner = self.expr()?;
                let close = self.bump();
                if close.tok != Tok::RParen {
                    return Err(at(codes::EXPR_SYNTAX, close.span));
                }
                inner.span = token.span.to(close.span);
                inner.grouped = true;
                Ok(inner)
            }
            Tok::LBracket => {
                let (items, span) = self.items(token.span, Tok::RBracket)?;
                self.node(ExprKind::List(items), span)
            }
            _ => Err(at(codes::EXPR_SYNTAX, token.span)),
        }
    }

    /// The separated expressions between the opening token at `pos` and `close`, and the span
    /// from `start` to the closing token.
    fn items(&mut self, start: Span, close: Tok) -> Result<(Vec<Expr>, Span)> {
        self.bump();
        let mut items = Vec::new();
        if self.peek().tok == close {
            let end = self.bump();
            return Ok((items, start.to(end.span)));
        }
        loop {
            if items.len() >= self.max_list {
                return Err(limit("list_length", start.to(self.peek().span)));
            }
            items.push(self.expr()?);
            let next = self.bump();
            if next.tok == close {
                return Ok((items, start.to(next.span)));
            }
            if next.tok != Tok::Sep {
                return Err(at(codes::EXPR_SYNTAX, next.span));
            }
        }
    }

    fn integer(&self, index: usize) -> Option<f64> {
        match self.at(index).tok {
            Tok::Number {
                value,
                integer: true,
            } => Some(value),
            _ => None,
        }
    }

    /// `n/d"` written without spaces at token `index`: the inches and the index after the mark.
    fn fraction(&self, index: usize) -> Option<(f64, usize)> {
        let numerator = self.integer(index)?;
        let slash = self.at(index + 1).tok == Tok::Slash && self.adjacent(index + 1);
        let denominator = self
            .integer(index + 2)
            .filter(|_| self.adjacent(index + 2))?;
        let mark = self.at(index + 3).tok == Tok::InchMark;
        (slash && mark).then_some((numerator / denominator, index + 4))
    }

    /// Inches at token `index`: `6"`, `6.5"`, `1/2"` or `6 1/2"`.
    fn inches(&self, index: usize) -> Option<(f64, usize)> {
        if let Some(fraction) = self.fraction(index) {
            return Some(fraction);
        }
        let Tok::Number { value, .. } = self.at(index).tok else {
            return None;
        };
        if self.at(index + 1).tok == Tok::InchMark {
            return Some((value, index + 2));
        }
        let (fraction, next) = self.fraction(index + 1)?;
        self.integer(index).map(|whole| (whole + fraction, next))
    }

    /// The extent of a unit suffix starting with the name at token `index`: the factors joined
    /// by `*` or `/` and exponents written without spaces. Returns its span and the next index.
    ///
    /// # Errors
    ///
    /// `solve.expr.syntax` when a `^` directly after the unit is not directly followed by an
    /// integer exponent (`m^+2`, `m^ 2`, `m^0.5`).
    fn unit_suffix(&self, index: usize) -> Result<(Span, usize)> {
        let mut span = self.at(index).span;
        let mut next = index + 1;
        loop {
            let token = self.at(next);
            if token.span.start != span.end {
                break;
            }
            let after = match token.tok {
                Tok::Caret => {
                    let mut k = next + 1;
                    if self.at(k).tok == Tok::Minus && self.adjacent(k) {
                        k += 1;
                    }
                    if !(self.integer(k).is_some() && self.adjacent(k)) {
                        return Err(at(codes::EXPR_SYNTAX, token.span.to(self.at(k).span)));
                    }
                    Some(k)
                }
                Tok::Star | Tok::Slash => (self.at(next + 1).tok == Tok::Ident
                    && self.adjacent(next + 1))
                .then_some(next + 1),
                _ => None,
            };
            let Some(last) = after else { break };
            span.end = self.at(last).span.end;
            next = last + 1;
        }
        Ok((span, next))
    }

    /// The literal at the current number token, its number multiplied by `sign`.
    fn number_literal(&mut self, sign: f64) -> Result<Expr> {
        let index = self.pos;
        let first = self.at(index);
        let Tok::Number { value, .. } = first.tok else {
            return Err(at(codes::EXPR_SYNTAX, first.span));
        };
        let value = sign * value;
        let inch = unit_named("inch")?;
        let mut written = None;
        let (kind, next) = if let Some((inches, next)) = self.inches(index) {
            let si = inch.to_si(inches);
            written = Some((inches, inch.scale));
            (
                ExprKind::Quantity(Quantity::new(si, Dimension::LENGTH)),
                next,
            )
        } else if self.at(index + 1).tok == Tok::FootMark {
            let feet = unit_named("foot")?.to_si(value);
            // A hyphen joins feet and inches only when written without spaces: `8'-6"`.
            let hyphen = self.at(index + 2).tok == Tok::Minus
                && self.adjacent(index + 2)
                && self.adjacent(index + 3);
            let start = if hyphen { index + 3 } else { index + 2 };
            let (si, next) = match self.inches(start) {
                // A foot holds 12 inches; more after a feet part is a typing error.
                Some((inches, next)) if inches >= 12.0 => {
                    let span = self.at(start).span.to(self.at(next - 1).span);
                    return Err(at(codes::EXPR_SYNTAX, span));
                }
                Some((inches, next)) => (feet + inch.to_si(inches), next),
                None => (feet, index + 2),
            };
            let inches = value * 12.0 + self.inches(start).map_or(0.0, |(inches, _)| inches);
            written = Some((inches, inch.scale));
            (
                ExprKind::Quantity(Quantity::new(si, Dimension::LENGTH)),
                next,
            )
        } else if self.at(index + 1).tok == Tok::Ident {
            let (span, next) = self.unit_suffix(index + 1)?;
            let unit = resolve(self.source(span)).map_err(|e| with_span(e, span))?;
            let quantity = Quantity::new(unit.to_si(value), unit.dimension);
            written = (unit.offset == 0.0).then_some((value, unit.scale));
            if unit.offset == 0.0 {
                (ExprKind::Quantity(quantity), next)
            } else {
                (ExprKind::Absolute(quantity), next)
            }
        } else {
            (ExprKind::Bare(value), index + 1)
        };
        let span = first.span.to(self.at(next - 1).span);
        let finite = match &kind {
            ExprKind::Quantity(quantity) | ExprKind::Absolute(quantity) => {
                quantity.value().is_finite()
            }
            _ => value.is_finite(),
        };
        if !finite {
            return Err(at(codes::EXPR_DOMAIN, span));
        }
        self.pos = next;
        let mut literal = self.node(kind, span)?;
        literal.written = written;
        Ok(literal)
    }
}

/// Parses with the default limits; for tests.
#[cfg(test)]
pub(crate) fn parse(text: &str) -> Result<Expr> {
    Expr::parse(text, &ExprLimits::DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::span_of;

    fn quantity(text: &str) -> Quantity {
        match parse(text).unwrap().kind {
            ExprKind::Quantity(quantity) | ExprKind::Absolute(quantity) => quantity,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= b.abs() * 1e-14
    }

    fn code(text: &str) -> &'static str {
        parse(text).unwrap_err().code()
    }

    #[test]
    fn metric_imperial_and_angle_literals() {
        assert!(close(quantity("2400 mm").value(), 2.4));
        assert!(close(quantity("2400mm").value(), 2.4));
        assert!(close(quantity("2.4 m").value(), 2.4));
        assert!(close(quantity("3 in").value(), 0.0762));
        assert!(close(
            quantity("45 deg").value(),
            std::f64::consts::FRAC_PI_4
        ));
        assert!(close(quantity("90°").value(), std::f64::consts::FRAC_PI_2));
        assert!(close(quantity("0.5 rad").value(), 0.5));
        let line_load = quantity("12 kN/m");
        assert!(close(line_load.value(), 12_000.0));
        assert_eq!(
            line_load.dimension(),
            Dimension::FORCE.div(Dimension::LENGTH)
        );
        assert_eq!(quantity("3 m^2").dimension(), Dimension::AREA);
        assert_eq!(quantity("3 m2").dimension(), Dimension::AREA);
        assert_eq!(quantity("1 kg*m/s^2").dimension(), Dimension::FORCE);
        assert!(close(quantity("20 °C").value(), 293.15));
        assert!(close(quantity("-40 °C").value(), 233.15));
        assert!(close(quantity("-40 degF").value(), 233.15));
        assert!(matches!(
            parse("20 °C").unwrap().kind,
            ExprKind::Absolute(_)
        ));
        assert!(matches!(parse("20 K").unwrap().kind, ExprKind::Quantity(_)));
    }

    #[test]
    fn unit_exponents_are_written_directly() {
        // `^` directly after a unit takes an integer exponent written directly after it.
        for text in ["2 m^+2", "2 m^ 2", "2 m^0.5", "2 m^", "2 m^-x"] {
            assert_eq!(code(text), "solve.expr.syntax", "{text:?}");
        }
        let error = parse("2 m^+2").unwrap_err();
        assert_eq!(span_of(&error), Some(Span { start: 3, end: 5 }));
        // With white space before it, `^` raises the quantity.
        assert!(matches!(
            parse("2 m ^2").unwrap().kind,
            ExprKind::Binary(BinaryOp::Pow, ..)
        ));
        for text in ["2 m0", "2 m^0", "2 µm", "2 \u{3bc}m"] {
            assert_eq!(code(text), "solve.expr.unknown_unit", "{text:?}");
        }
        for text in ["2 m^200", "2 m^-129", "1 m^127*m"] {
            assert_eq!(code(text), "solve.expr.limit_exceeded", "{text:?}");
        }
        // A unit cannot follow a parenthesis.
        assert_eq!(code("(2 + 3) mm"), "solve.expr.syntax");
    }

    #[test]
    fn feet_and_inches() {
        let inch = 0.0254;
        assert!(close(quantity("8' 6\"").value(), 102.0 * inch));
        assert!(close(quantity("8'-6\"").value(), 102.0 * inch));
        assert!(close(quantity("8'6\"").value(), 102.0 * inch));
        assert!(close(quantity("8' 6 1/2\"").value(), 102.5 * inch));
        assert!(close(quantity("8' 1/2\"").value(), 96.5 * inch));
        assert!(close(quantity("6\"").value(), 6.0 * inch));
        assert!(close(quantity("1/2\"").value(), 0.5 * inch));
        assert!(close(quantity("8'").value(), 96.0 * inch));
        assert!(close(quantity("8' 11 3/4\"").value(), 107.75 * inch));
        assert!(close(quantity("13\"").value(), 13.0 * inch));
        // With spaces around the hyphen it subtracts.
        let difference = parse("8' - 6\"").unwrap();
        assert!(matches!(
            difference.kind(),
            ExprKind::Chain(_, rest) if rest[0].0 == BinaryOp::Sub
        ));
        assert_eq!(code("1/0\""), "solve.expr.domain");
        // A foot holds 12 inches.
        for text in ["8' 12\"", "8' 13\"", "8'-12 1/2\"", "8' 25/2\""] {
            assert_eq!(code(text), "solve.expr.syntax", "{text:?}");
        }
        let error = parse("x + 8' 13\"").unwrap_err();
        assert_eq!(span_of(&error), Some(Span { start: 7, end: 10 }));
    }

    #[test]
    fn precedence_and_associativity() {
        let e = parse("1 + 2 * 3 ^ 2 ^ 2").unwrap();
        let ExprKind::Chain(_, sum) = e.kind() else {
            panic!("{e:?}")
        };
        let [(BinaryOp::Add, right)] = sum.as_slice() else {
            panic!()
        };
        let ExprKind::Chain(_, product) = right.kind() else {
            panic!()
        };
        let [(BinaryOp::Mul, power)] = product.as_slice() else {
            panic!()
        };
        let ExprKind::Binary(BinaryOp::Pow, _, exponent) = power.kind() else {
            panic!()
        };
        assert!(matches!(
            exponent.kind(),
            ExprKind::Binary(BinaryOp::Pow, ..)
        ));
        assert!(matches!(
            parse("-2^2").unwrap().kind(),
            ExprKind::Unary(UnaryOp::Neg, _)
        ));
        assert!(matches!(
            parse("a || b && c").unwrap().kind(),
            ExprKind::Chain(_, rest) if rest.len() == 1 && rest[0].0 == BinaryOp::Or
        ));
        assert!(matches!(
            parse("+x").unwrap().kind(),
            ExprKind::Unary(UnaryOp::Plus, _)
        ));
        assert_eq!(code("1 < 2 < 3"), "solve.expr.syntax");
    }

    #[test]
    fn runs_of_one_level_form_one_chain() {
        let ops = |text: &str| match parse(text).unwrap().kind {
            ExprKind::Chain(_, rest) => rest.iter().map(|(op, _)| *op).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        use BinaryOp::{Add, And, Div, Mul, Sub};
        assert_eq!(ops("1 - 2 + 3 - 4"), [Sub, Add, Sub]);
        assert_eq!(ops("1 * 2 / 3"), [Mul, Div]);
        assert_eq!(ops("a && b && c"), [And, And]);
        // A parenthesized run is an operand, not extended.
        assert_eq!(ops("(1 + 2) + 3"), [Add]);
        assert_eq!(ops("1 + (2 + 3)"), [Add]);
        // Thousands of terms stay shallow.
        let long = format!("1{}", " + 1".repeat(5_000));
        let e = parse(&long).unwrap();
        assert_eq!(e.depth(), 2);
        assert_eq!(
            e.span(),
            Span {
                start: 0,
                end: long.len()
            }
        );
    }

    #[test]
    fn calls_lists_names_and_literals() {
        let call = parse("max(1, [2; 3], \"x\", true, w)").unwrap();
        let ExprKind::Call(name, args) = call.kind() else {
            panic!()
        };
        assert_eq!(name, "max");
        assert_eq!(args.len(), 5);
        assert!(matches!(args[1].kind(), ExprKind::List(items) if items.len() == 2));
        assert_eq!(args[2].kind(), &ExprKind::Text("x".to_owned()));
        assert_eq!(args[3].kind(), &ExprKind::Bool(true));
        assert_eq!(args[4].kind(), &ExprKind::Name("w".to_owned()));
        assert_eq!(
            parse("f()").unwrap().kind(),
            &ExprKind::Call("f".into(), vec![])
        );
        assert_eq!(parse("[]").unwrap().kind(), &ExprKind::List(vec![]));
    }

    #[test]
    fn spans_cover_the_source() {
        let e = parse(" (1 m + 2 m) * 3").unwrap();
        assert_eq!(e.span(), Span { start: 1, end: 16 });
        let ExprKind::Chain(left, _) = e.kind() else {
            panic!()
        };
        assert_eq!(left.span(), Span { start: 1, end: 12 });
        let error = parse("1 + 2 furlong").unwrap_err();
        assert_eq!(error.code(), "solve.expr.unknown_unit");
        assert_eq!(span_of(&error), Some(Span { start: 6, end: 13 }));
    }

    #[test]
    fn syntax_errors() {
        for text in [
            "", "1 +", "(1", "max(1,", "[1 2]", "1 2", "2 m)", "2 min(3)", "*",
        ] {
            assert_eq!(code(text), "solve.expr.syntax", "{text:?}");
        }
        assert_eq!(code("1e999"), "solve.expr.domain");
        assert_eq!(code("1e300 km^3"), "solve.expr.domain");
    }

    #[test]
    fn limits_bound_length_depth_and_lists() {
        let tight = ExprLimits::DEFAULT
            .with_text_bytes(9)
            .with_depth(4)
            .with_list_length(2)
            .with_steps(10);
        let limit_of = |text: &str| {
            let error = Expr::parse(text, &tight).unwrap_err();
            assert_eq!(error.code(), "solve.expr.limit_exceeded", "{text}");
            error.details().get("limit").cloned()
        };
        assert_eq!(limit_of("1234567890"), Some("text_bytes".into()));
        assert_eq!(limit_of("((((1))))"), Some("depth".into()));
        assert_eq!(limit_of("2^2^2^2^2"), Some("depth".into()));
        assert_eq!(limit_of("----1"), Some("depth".into()));
        assert_eq!(limit_of("[1,2,3]"), Some("list_length".into()));
        assert!(Expr::parse("1+1+1", &tight).is_ok());
    }

    #[test]
    fn localized_separator() {
        let limits = ExprLimits::DEFAULT;
        let e = Expr::parse_localized("max(1,5; 2)", &limits, ',').unwrap();
        let ExprKind::Call(_, args) = e.kind() else {
            panic!()
        };
        assert_eq!(args[0].kind(), &ExprKind::Bare(1.5));
        assert_eq!(
            Expr::parse_localized("1", &limits, '-').unwrap_err().code(),
            "solve.expr.separator_invalid"
        );
    }
}
