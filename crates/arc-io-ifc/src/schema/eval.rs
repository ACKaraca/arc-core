//! The EXPRESS interpreter (ISO 10303-11) of the rule trees: values, three-valued logic, operators,
//! aggregates, queries, entity values built by constructors, the built-in functions the IFC
//! schemas use, and the statements of functions and global rules.
//!
//! The interpreter is bounded: every evaluation has a step budget, calls (functions and derived
//! attributes) are limited to the caller's nesting depth, aggregates to its array length, and the
//! cancellation token is checked as steps are taken. Exceeding a bound, an operand of the wrong
//! type, a division by zero or an assignment the interpreter does not support stops the
//! evaluation with a reason; the rule is then not evaluated. Only cancellation is an error.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use arc_base::{Cancel, Limits};

use crate::model::{IfcFile, Parameter};

use super::inverse::InverseIndex;
use super::tree::{Algorithm, BinaryOp, Builtin, Expr, Logical, Stmt, UnaryOp};
use super::{AggregateKind, Bound, Named, SchemaTables, TypeRef};

/// The most steps one evaluation (one rule on one instance, or one global rule) takes.
pub const MAX_STEPS: u64 = 1_000_000;

/// The deepest nesting of calls (functions and derived attributes) whatever the caller's
/// `nesting_depth`: each level takes several interpreter frames of the native stack, and no rule of
/// the IFC schemas nests deeper than a few levels.
pub const MAX_CALL_DEPTH: u32 = 32;

/// Steps between cancellation checks.
const CANCEL_EVERY: u64 = 4096;

/// Why an evaluation stopped.
#[derive(Debug)]
pub enum Stop {
    /// The rule cannot be decided; the reason names the construct or bound.
    NotEvaluated(String),
    /// The caller cancelled.
    Cancelled(arc_base::Error),
}

type Res<T> = std::result::Result<T, Stop>;

fn stop<T>(reason: impl Into<String>) -> Res<T> {
    Err(Stop::NotEvaluated(reason.into()))
}

/// An aggregate value.
#[derive(Debug, Clone)]
pub struct Agg {
    pub kind: AggregateKind,
    /// The first index (1 except for arrays declared otherwise).
    pub lower: i64,
    pub items: Vec<Value>,
}

/// An entity value built by constructors: each part is an entity with its own explicit attributes.
#[derive(Debug, Clone)]
pub struct Built {
    pub parts: Vec<(u16, Vec<Value>)>,
}

/// An entity instance: one of the file, or one built by the rules.
#[derive(Debug, Clone)]
pub enum Inst {
    File(u64),
    Built(Rc<Built>),
}

/// A value.
#[derive(Debug, Clone)]
pub enum Value {
    Indeterminate,
    Integer(i64),
    Real(f64),
    Logical(Logical),
    String(Rc<str>),
    /// The bits, as `0` and `1`.
    Binary(Rc<str>),
    /// An enumeration item in upper case, with its type where known.
    Enum(Option<u16>, Rc<str>),
    Entity(Inst),
    Aggregate(Rc<Agg>),
    /// A value of a defined type.
    Typed(u16, Rc<Value>),
}

impl Value {
    /// The value without its defined types.
    pub fn plain(&self) -> &Value {
        let mut v = self;
        while let Value::Typed(_, inner) = v {
            v = inner;
        }
        v
    }

    fn number(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss, reason = "EXPRESS integers are reals")]
        match self.plain() {
            Value::Integer(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }

    fn is_indeterminate(&self) -> bool {
        matches!(self.plain(), Value::Indeterminate)
    }

    fn string(s: &str) -> Self {
        Value::String(Rc::from(s))
    }

    fn aggregate(kind: AggregateKind, items: Vec<Value>) -> Self {
        Value::Aggregate(Rc::new(Agg {
            kind,
            lower: 1,
            items,
        }))
    }
}

fn and(a: Logical, b: Logical) -> Logical {
    a.min(b)
}

fn or(a: Logical, b: Logical) -> Logical {
    a.max(b)
}

fn not(a: Logical) -> Logical {
    match a {
        Logical::True => Logical::False,
        Logical::False => Logical::True,
        Logical::Unknown => Logical::Unknown,
    }
}

fn logical(b: bool) -> Logical {
    if b { Logical::True } else { Logical::False }
}

/// The bits of a binary parameter as written in a file: the first hexadecimal digit counts the
/// unused leading bits of the rest.
fn bits_of_hex(digits: &str) -> Option<String> {
    let mut chars = digits.chars();
    let unused = usize::try_from(chars.next()?.to_digit(16)?).ok()?;
    if unused > 3 {
        return None;
    }
    let mut bits = String::new();
    for c in chars {
        bits.push_str(&format!("{:04b}", c.to_digit(16)?));
    }
    bits.get(unused..).map(str::to_owned)
}

/// `LIKE` per ISO 10303-11: `@` a letter, `^` an upper-case letter, `!` a lower-case letter, `?`
/// any character, `&` the rest of the string, `#` a digit, `$` a run of characters up to a space
/// or the end, `*` any number of characters, `\` makes the next character literal. Steps are
/// charged per comparison through `budget`.
pub fn like(text: &[char], pattern: &[char], budget: &mut u64) -> Option<bool> {
    *budget = budget.checked_sub(1)?;
    let Some((&p, rest)) = pattern.split_first() else {
        return Some(text.is_empty());
    };
    let mut one = |test: fn(char) -> bool| -> Option<bool> {
        match text.split_first() {
            Some((&c, t)) if test(c) => like(t, rest, budget),
            _ => Some(false),
        }
    };
    match p {
        '@' => one(char::is_alphabetic),
        '^' => one(char::is_uppercase),
        '!' => one(char::is_lowercase),
        '?' => one(|_| true),
        '#' => one(|c| c.is_ascii_digit()),
        // The rest of the string; what follows in the pattern must then match nothing.
        '&' => like(&[], rest, budget),
        '*' => {
            for skip in 0..=text.len() {
                if like(text.get(skip..)?, rest, budget)? {
                    return Some(true);
                }
            }
            Some(false)
        }
        // The characters up to the next space or the end.
        '$' => {
            let run = text.iter().take_while(|c| **c != ' ').count();
            like(text.get(run..)?, rest, budget)
        }
        '\\' => match (rest.split_first(), text.split_first()) {
            (Some((&lit, rest)), Some((&c, t))) if c == lit => like(t, rest, budget),
            _ => Some(false),
        },
        _ => match text.split_first() {
            Some((&c, t)) if c == p => like(t, rest, budget),
            _ => Some(false),
        },
    }
}

/// How a statement list ended.
enum Flow {
    Next,
    Return(Value),
    Escape,
    Skip,
}

/// The variables of one evaluation.
struct Frame {
    this: Value,
    slots: Vec<Value>,
    /// The declared aggregate kind and lower index of aggregate variables.
    lowers: Vec<Option<(AggregateKind, i64)>>,
}

impl Frame {
    fn new(this: Value) -> Self {
        Self {
            this,
            slots: Vec::new(),
            lowers: Vec::new(),
        }
    }

    fn get(&self, slot: u16) -> Value {
        self.slots
            .get(usize::from(slot))
            .cloned()
            .unwrap_or(Value::Indeterminate)
    }

    fn slot_mut(&mut self, slot: u16) -> &mut Value {
        let i = usize::from(slot);
        if self.slots.len() <= i {
            self.slots.resize(i + 1, Value::Indeterminate);
            self.lowers.resize(i + 1, None);
        }
        &mut self.slots[i]
    }

    fn set(&mut self, slot: u16, mut value: Value) {
        let lower = self.lowers.get(usize::from(slot)).copied().flatten();
        // An aggregate assigned to an aggregate variable takes its kind and, for an array, its
        // index range.
        // shortcut: a list with repeats assigned to a SET variable keeps them; the IFC functions
        // only assign empty or computed sets.
        if let (Some((kind, lower)), Value::Aggregate(a)) = (lower, &mut value)
            && (a.kind != kind || a.lower != lower)
        {
            let a = Rc::make_mut(a);
            a.kind = kind;
            a.lower = lower;
        }
        *self.slot_mut(slot) = value;
    }
}

/// A segment of an assignment target.
enum Segment {
    Attribute(&'static str),
    Index(i64),
}

/// The interpreter over one file.
pub struct Interp<'f> {
    file: &'f IfcFile,
    tables: &'static SchemaTables,
    index: &'f InverseIndex,
    cancel: &'f Cancel,
    steps: u64,
    depth: u32,
    max_depth: u32,
    max_len: usize,
    populations: HashMap<u16, Value>,
}

impl<'f> Interp<'f> {
    pub fn new(
        file: &'f IfcFile,
        tables: &'static SchemaTables,
        index: &'f InverseIndex,
        limits: &Limits,
        cancel: &'f Cancel,
    ) -> Self {
        Self {
            file,
            tables,
            index,
            cancel,
            steps: 0,
            depth: 0,
            max_depth: limits.nesting_depth.min(MAX_CALL_DEPTH),
            max_len: usize::try_from(limits.array_length).unwrap_or(usize::MAX),
            populations: HashMap::new(),
        }
    }

    /// Evaluates a domain rule with `SELF` bound to `this`. `?` counts as `UNKNOWN`.
    pub fn rule(&mut self, tree: &'static Expr, this: Value) -> Res<Logical> {
        self.steps = 0;
        self.depth = 0;
        let mut frame = Frame::new(this);
        let value = self.eval(tree, &mut frame)?;
        truth(&value)
    }

    /// Runs the body of a global rule and evaluates each of its domain rules.
    pub fn global(
        &mut self,
        algorithm: &'static Algorithm,
        rules: &'static [super::WhereRule],
    ) -> Vec<Res<Logical>> {
        self.steps = 0;
        self.depth = 0;
        let mut frame = Frame::new(Value::Indeterminate);
        let body = self
            .locals(algorithm, &mut frame)
            .and_then(|()| self.exec(algorithm.body, &mut frame));
        match body {
            Err(Stop::Cancelled(e)) => vec![Err(Stop::Cancelled(e))],
            Err(Stop::NotEvaluated(reason)) => rules
                .iter()
                .map(|_| Err(Stop::NotEvaluated(reason.clone())))
                .collect(),
            Ok(_) => rules
                .iter()
                .map(|r| {
                    self.steps = 0;
                    self.eval(&r.tree, &mut frame).and_then(|v| truth(&v))
                })
                .collect(),
        }
    }

    fn step(&mut self, n: u64) -> Res<()> {
        let before = self.steps;
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_STEPS {
            return stop("step budget exceeded");
        }
        if before / CANCEL_EVERY != self.steps / CANCEL_EVERY {
            self.cancel.check().map_err(Stop::Cancelled)?;
        }
        Ok(())
    }

    fn check_len(&mut self, len: usize) -> Res<()> {
        if len > self.max_len {
            return stop("aggregate larger than the array length limit");
        }
        self.step(u64::try_from(len).unwrap_or(u64::MAX))
    }

    fn enter(&mut self) -> Res<()> {
        if self.depth >= self.max_depth {
            return stop("call depth exceeds the nesting limit");
        }
        self.depth += 1;
        Ok(())
    }

    // ----- Expressions -----

    #[allow(clippy::too_many_lines, reason = "one arm per kind of expression")]
    fn eval(&mut self, e: &'static Expr, frame: &mut Frame) -> Res<Value> {
        self.step(1)?;
        Ok(match e {
            Expr::Integer(i) => Value::Integer(*i),
            Expr::Real(r) => Value::Real(*r),
            Expr::String(s) => Value::string(s),
            Expr::Bits(b) => Value::Binary(Rc::from(*b)),
            Expr::Logical(l) => Value::Logical(*l),
            Expr::Indeterminate => Value::Indeterminate,
            Expr::SelfValue => frame.this.clone(),
            Expr::Variable(slot) => frame.get(*slot),
            Expr::Population(entity) => self.population(*entity)?,
            Expr::EnumItem(item) => Value::Enum(None, Rc::from(*item)),
            Expr::Attribute(base, name) => {
                let base = self.eval(base, frame)?;
                self.attribute(&base, name)?
            }
            Expr::Index(base, index, high) => {
                let base = self.eval(base, frame)?;
                let index = self.eval(index, frame)?;
                let high = match high {
                    Some(h) => Some(self.eval(h, frame)?),
                    None => None,
                };
                self.index(&base, &index, high.as_ref())?
            }
            Expr::Unary(op, x) => {
                let x = self.eval(x, frame)?;
                unary(*op, &x)?
            }
            Expr::Binary(op, a, b) => self.binary(*op, a, b, frame)?,
            Expr::Builtin(b, args) => self.builtin(*b, args, frame)?,
            Expr::Call(f, args) => {
                let mut values = Vec::with_capacity(args.len());
                for a in *args {
                    values.push(self.eval(a, frame)?);
                }
                self.call(*f, values)?
            }
            Expr::Construct(entity, args) => {
                let declared = self
                    .tables
                    .entities
                    .get(usize::from(*entity))
                    .map_or(0, |e| e.attributes.len());
                if declared != args.len() {
                    return stop("an entity constructor with the wrong number of attributes");
                }
                let mut values = Vec::with_capacity(args.len());
                for a in *args {
                    values.push(self.eval(a, frame)?);
                }
                Value::Entity(Inst::Built(Rc::new(Built {
                    parts: vec![(*entity, values)],
                })))
            }
            Expr::Convert(ty, x) => {
                let x = self.eval(x, frame)?;
                if x.is_indeterminate() {
                    x
                } else {
                    Value::Typed(*ty, Rc::new(x.plain().clone()))
                }
            }
            Expr::Aggregate(elements) => {
                let mut items = Vec::new();
                for element in *elements {
                    let value = self.eval(&element.value, frame)?;
                    let count = match element.repeat {
                        None => 1,
                        Some(r) => match self.eval(r, frame)?.plain() {
                            Value::Integer(n) => usize::try_from(*n).unwrap_or(0),
                            Value::Indeterminate => return Ok(Value::Indeterminate),
                            _ => return stop("a repetition count that is not an integer"),
                        },
                    };
                    self.check_len(items.len().saturating_add(count))?;
                    items.extend(std::iter::repeat_n(value, count));
                }
                Value::aggregate(AggregateKind::List, items)
            }
            Expr::Interval {
                low,
                low_inclusive,
                item,
                high_inclusive,
                high,
            } => {
                let low = self.eval(low, frame)?;
                let item = self.eval(item, frame)?;
                let high = self.eval(high, frame)?;
                let first = self.ordered(&low, &item, *low_inclusive)?;
                let second = self.ordered(&item, &high, *high_inclusive)?;
                Value::Logical(and(first, second))
            }
            Expr::Query {
                variable,
                aggregate,
                condition,
            } => {
                let source = self.eval(aggregate, frame)?;
                let agg = match source.plain() {
                    Value::Indeterminate => return Ok(Value::Indeterminate),
                    Value::Aggregate(a) => Rc::clone(a),
                    _ => return stop("QUERY over a value that is not an aggregate"),
                };
                let mut kept = Vec::new();
                for item in &agg.items {
                    frame.set(*variable, item.clone());
                    if truth(&self.eval(condition, frame)?)? == Logical::True {
                        kept.push(item.clone());
                    }
                }
                // shortcut: a query over an array returns a list of the kept elements, not an
                // array with indeterminate gaps; no IFC rule depends on the difference.
                let kind = match agg.kind {
                    AggregateKind::Array => AggregateKind::List,
                    k => k,
                };
                Value::aggregate(kind, kept)
            }
        })
    }

    /// `a < b`, or `a <= b` with `inclusive`.
    fn ordered(&mut self, a: &Value, b: &Value, inclusive: bool) -> Res<Logical> {
        Ok(match compare(a, b)? {
            None => Logical::Unknown,
            Some(o) => logical(o == Ordering::Less || (inclusive && o == Ordering::Equal)),
        })
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        a: &'static Expr,
        b: &'static Expr,
        frame: &mut Frame,
    ) -> Res<Value> {
        let left = self.eval(a, frame)?;
        // AND and OR decide on the left operand where they can, so that an operand which only
        // matters otherwise is not evaluated.
        match op {
            BinaryOp::And if truth(&left)? == Logical::False => {
                return Ok(Value::Logical(Logical::False));
            }
            BinaryOp::Or if truth(&left)? == Logical::True => {
                return Ok(Value::Logical(Logical::True));
            }
            _ => {}
        }
        let right = self.eval(b, frame)?;
        let (l, r) = (&left, &right);
        Ok(match op {
            BinaryOp::And => Value::Logical(and(truth(l)?, truth(r)?)),
            BinaryOp::Or => Value::Logical(or(truth(l)?, truth(r)?)),
            BinaryOp::Xor => {
                let (x, y) = (truth(l)?, truth(r)?);
                Value::Logical(if x == Logical::Unknown || y == Logical::Unknown {
                    Logical::Unknown
                } else {
                    logical(x != y)
                })
            }
            BinaryOp::Eq => Value::Logical(self.equal(l, r, false)?),
            BinaryOp::Ne => Value::Logical(not(self.equal(l, r, false)?)),
            BinaryOp::InstanceEq => Value::Logical(self.equal(l, r, true)?),
            BinaryOp::InstanceNe => Value::Logical(not(self.equal(l, r, true)?)),
            BinaryOp::Lt => Value::Logical(self.ordered(l, r, false)?),
            BinaryOp::Le => Value::Logical(self.ordered(l, r, true)?),
            BinaryOp::Gt => Value::Logical(self.ordered(r, l, false)?),
            BinaryOp::Ge => Value::Logical(self.ordered(r, l, true)?),
            BinaryOp::In => Value::Logical(self.member(l, r)?),
            BinaryOp::Like => match (l.plain(), r.plain()) {
                (Value::Indeterminate, _) | (_, Value::Indeterminate) => {
                    Value::Logical(Logical::Unknown)
                }
                (Value::String(t), Value::String(p)) => {
                    let text: Vec<char> = t.chars().collect();
                    let pattern: Vec<char> = p.chars().collect();
                    let mut budget = MAX_STEPS.saturating_sub(self.steps);
                    let before = budget;
                    let found = like(&text, &pattern, &mut budget);
                    self.step(before - budget)?;
                    match found {
                        Some(found) => Value::Logical(logical(found)),
                        None => return stop("step budget exceeded"),
                    }
                }
                _ => return stop("LIKE on values that are not strings"),
            },
            BinaryOp::Combine => match (l.plain(), r.plain()) {
                (Value::Indeterminate, _) | (_, Value::Indeterminate) => Value::Indeterminate,
                (Value::Entity(Inst::Built(x)), Value::Entity(Inst::Built(y))) => {
                    let mut parts = x.parts.clone();
                    parts.extend(y.parts.iter().cloned());
                    Value::Entity(Inst::Built(Rc::new(Built { parts })))
                }
                _ => return stop("|| on values that are not built entity values"),
            },
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul => self.additive(op, l, r)?,
            BinaryOp::Div | BinaryOp::IntDiv | BinaryOp::Mod | BinaryOp::Power => {
                arithmetic(op, l, r)?
            }
        })
    }

    /// `+`, `-` and `*` on numbers, strings and aggregates.
    fn additive(&mut self, op: BinaryOp, l: &Value, r: &Value) -> Res<Value> {
        match (l.plain(), r.plain()) {
            (Value::Indeterminate, _) | (_, Value::Indeterminate) => Ok(Value::Indeterminate),
            (Value::String(a), Value::String(b)) if op == BinaryOp::Add => {
                self.check_len(a.len() + b.len())?;
                Ok(Value::string(&format!("{a}{b}")))
            }
            (Value::Aggregate(a), Value::Aggregate(b)) => {
                let a = Rc::clone(a);
                let b = Rc::clone(b);
                self.check_len(a.items.len().saturating_add(b.items.len()))?;
                self.step(
                    u64::try_from(a.items.len().saturating_mul(b.items.len())).unwrap_or(u64::MAX),
                )?;
                let set = a.kind == AggregateKind::Set || b.kind == AggregateKind::Set;
                let mut items = Vec::new();
                match op {
                    BinaryOp::Add => {
                        items.extend(a.items.iter().cloned());
                        for x in &b.items {
                            if !set || !self.contains(&items, x)? {
                                items.push(x.clone());
                            }
                        }
                    }
                    BinaryOp::Sub => {
                        for x in &a.items {
                            if !self.contains(&b.items, x)? {
                                items.push(x.clone());
                            }
                        }
                    }
                    _ => {
                        for x in &a.items {
                            if self.contains(&b.items, x)? && !(set && self.contains(&items, x)?) {
                                items.push(x.clone());
                            }
                        }
                    }
                }
                let kind = if set { AggregateKind::Set } else { a.kind };
                Ok(Value::aggregate(kind, items))
            }
            (Value::Aggregate(a), _) if op != BinaryOp::Mul => {
                let a = Rc::clone(a);
                self.check_len(a.items.len().saturating_add(1))?;
                let present = self.contains(&a.items, r)?;
                let mut items = a.items.clone();
                if op == BinaryOp::Add {
                    if !(a.kind == AggregateKind::Set && present) {
                        items.push(r.clone());
                    }
                } else if let Some(i) = self.position(&items, r)? {
                    items.remove(i);
                }
                Ok(Value::aggregate(a.kind, items))
            }
            (_, Value::Aggregate(b)) if op == BinaryOp::Add => {
                let b = Rc::clone(b);
                self.check_len(b.items.len().saturating_add(1))?;
                let mut items = Vec::with_capacity(b.items.len() + 1);
                if !(b.kind == AggregateKind::Set && self.contains(&b.items, l)?) {
                    items.push(l.clone());
                }
                items.extend(b.items.iter().cloned());
                Ok(Value::aggregate(b.kind, items))
            }
            _ => arithmetic(op, l, r),
        }
    }

    fn position(&mut self, items: &[Value], x: &Value) -> Res<Option<usize>> {
        for (i, item) in items.iter().enumerate() {
            if self.equal(item, x, true)? == Logical::True {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    fn contains(&mut self, items: &[Value], x: &Value) -> Res<bool> {
        Ok(self.position(items, x)?.is_some())
    }

    /// `x IN aggregate`, by instance equality.
    fn member(&mut self, x: &Value, aggregate: &Value) -> Res<Logical> {
        let agg = match aggregate.plain() {
            Value::Indeterminate => return Ok(Logical::Unknown),
            Value::Aggregate(a) => Rc::clone(a),
            _ => return stop("IN on a value that is not an aggregate"),
        };
        if x.is_indeterminate() {
            return Ok(Logical::Unknown);
        }
        let mut result = Logical::False;
        for item in &agg.items {
            result = or(result, self.equal(x, item, true)?);
            if result == Logical::True {
                break;
            }
        }
        Ok(result)
    }

    /// Value equality, or instance equality with `instance`.
    fn equal(&mut self, a: &Value, b: &Value, instance: bool) -> Res<Logical> {
        self.step(1)?;
        Ok(match (a.plain(), b.plain()) {
            (Value::Indeterminate, _) | (_, Value::Indeterminate) => Logical::Unknown,
            (Value::Entity(x), Value::Entity(y)) => match (x, y) {
                (Inst::File(m), Inst::File(n)) if m == n => Logical::True,
                (Inst::Built(m), Inst::Built(n)) if Rc::ptr_eq(m, n) => Logical::True,
                _ if instance => Logical::False,
                _ => {
                    self.enter()?;
                    let result = self.entity_values_equal(x, y);
                    self.depth -= 1;
                    result?
                }
            },
            (Value::Aggregate(x), Value::Aggregate(y)) => {
                if x.items.len() != y.items.len() {
                    return Ok(Logical::False);
                }
                let (x, y) = (Rc::clone(x), Rc::clone(y));
                let mut result = Logical::True;
                for (p, q) in x.items.iter().zip(&y.items) {
                    result = and(result, self.equal(p, q, instance)?);
                    if result == Logical::False {
                        break;
                    }
                }
                result
            }
            (Value::Enum(_, x), Value::Enum(_, y)) => logical(x == y),
            (p, q) => match compare(p, q)? {
                Some(o) => logical(o == Ordering::Equal),
                None => Logical::False,
            },
        })
    }

    /// Two entity values are equal when their entities and explicit attribute values are.
    fn entity_values_equal(&mut self, x: &Inst, y: &Inst) -> Res<Logical> {
        let (a, b) = (self.explicit_values(x)?, self.explicit_values(y)?);
        if a.len() != b.len() || a.iter().zip(&b).any(|(p, q)| p.0 != q.0) {
            return Ok(Logical::False);
        }
        let mut result = Logical::True;
        for ((_, p), (_, q)) in a.iter().zip(&b) {
            if p.len() != q.len() {
                return Ok(Logical::False);
            }
            for (u, v) in p.iter().zip(q) {
                result = and(result, self.equal(u, v, false)?);
                if result == Logical::False {
                    return Ok(result);
                }
            }
        }
        Ok(result)
    }

    /// The parts of an entity value, each declaring entity with its explicit attribute values,
    /// sorted by entity.
    fn explicit_values(&mut self, x: &Inst) -> Res<Vec<(u16, Vec<Value>)>> {
        let mut out = match x {
            Inst::Built(b) => b.parts.clone(),
            Inst::File(n) => {
                let tables = self.tables;
                let Some(instance) = self.file.instance(*n) else {
                    return stop("a reference to a missing instance");
                };
                let mut out = Vec::new();
                for record in &instance.records {
                    let Some(entity) = tables.entity_index(&record.type_name) else {
                        return stop("an instance of an unknown entity");
                    };
                    let positions = if instance.records.len() == 1 {
                        tables.positions(entity)
                    } else {
                        Vec::new()
                    };
                    if instance.records.len() == 1 {
                        // One part per declaring entity of the chain.
                        let mut parts: Vec<(u16, Vec<Value>)> = Vec::new();
                        for (position, parameter) in positions.iter().zip(&record.parameters) {
                            let value = self.read(parameter, Some(position.attribute.ty))?;
                            match parts.last_mut() {
                                Some((e, values)) if *e == position.declared.entity => {
                                    values.push(value);
                                }
                                _ => parts.push((position.declared.entity, vec![value])),
                            }
                        }
                        out.extend(parts);
                    } else {
                        let attributes = tables
                            .entities
                            .get(usize::from(entity))
                            .map_or(&[][..], |e| e.attributes);
                        let mut values = Vec::new();
                        for (a, parameter) in attributes.iter().zip(&record.parameters) {
                            values.push(self.read(parameter, Some(a.ty))?);
                        }
                        out.push((entity, values));
                    }
                }
                out
            }
        };
        out.sort_by_key(|(e, _)| *e);
        Ok(out)
    }

    fn index(&mut self, base: &Value, index: &Value, high: Option<&Value>) -> Res<Value> {
        let int = |v: &Value| -> Res<Option<i64>> {
            match v.plain() {
                Value::Integer(i) => Ok(Some(*i)),
                Value::Indeterminate => Ok(None),
                _ => stop("an index that is not an integer"),
            }
        };
        let (Some(i), high) = (int(index)?, high.map(int).transpose()?) else {
            return Ok(Value::Indeterminate);
        };
        if high == Some(None) {
            return Ok(Value::Indeterminate);
        }
        let high = high.flatten();
        match base.plain() {
            Value::Indeterminate => Ok(Value::Indeterminate),
            Value::Aggregate(a) => {
                let at = |k: i64| {
                    k.checked_sub(a.lower)
                        .and_then(|k| usize::try_from(k).ok())
                        .filter(|k| *k < a.items.len())
                };
                match high {
                    None => Ok(at(i).map_or(Value::Indeterminate, |k| a.items[k].clone())),
                    Some(h) => match (at(i), at(h)) {
                        (Some(s), Some(e)) if s <= e => Ok(Value::aggregate(
                            a.kind,
                            a.items
                                .get(s..=e)
                                .map(<[Value]>::to_vec)
                                .unwrap_or_default(),
                        )),
                        _ => Ok(Value::Indeterminate),
                    },
                }
            }
            Value::String(s) | Value::Binary(s) => {
                let chars: Vec<char> = s.chars().collect();
                let h = high.unwrap_or(i);
                let range = usize::try_from(i - 1)
                    .ok()
                    .zip(usize::try_from(h).ok())
                    .filter(|(s, e)| s < e && *e <= chars.len());
                Ok(match range {
                    Some((s, e)) => {
                        let text: String = chars[s..e].iter().collect();
                        if matches!(base.plain(), Value::Binary(_)) {
                            Value::Binary(Rc::from(text))
                        } else {
                            Value::string(&text)
                        }
                    }
                    None => Value::Indeterminate,
                })
            }
            _ => stop("an index on a value that is not an aggregate or string"),
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm per built-in function")]
    fn builtin(&mut self, b: Builtin, args: &'static [Expr], frame: &mut Frame) -> Res<Value> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(self.eval(a, frame)?);
        }
        let arity = match b {
            Builtin::Atan | Builtin::Nvl | Builtin::Usedin => 2,
            _ => 1,
        };
        if values.len() != arity {
            return stop("a built-in function with the wrong number of arguments");
        }
        let v = &values[0];
        let number = |f: fn(f64) -> f64| -> Res<Value> {
            match v.plain() {
                Value::Indeterminate => Ok(Value::Indeterminate),
                _ => match v.number().map(f) {
                    Some(r) if r.is_finite() => Ok(Value::Real(r)),
                    Some(_) => stop("a numeric function outside its domain"),
                    None => stop("a numeric function on a value that is not a number"),
                },
            }
        };
        match b {
            Builtin::Abs => match v.plain() {
                Value::Integer(i) => i
                    .checked_abs()
                    .map(Value::Integer)
                    .map_or_else(|| stop("integer overflow"), Ok),
                _ => number(f64::abs),
            },
            Builtin::Acos => number(f64::acos),
            Builtin::Asin => number(f64::asin),
            Builtin::Atan => match (v.number(), values[1].number()) {
                _ if v.is_indeterminate() || values[1].is_indeterminate() => {
                    Ok(Value::Indeterminate)
                }
                (Some(y), Some(x)) => Ok(Value::Real(if x == 0.0 {
                    std::f64::consts::FRAC_PI_2.copysign(y)
                } else {
                    (y / x).atan()
                })),
                _ => stop("ATAN on values that are not numbers"),
            },
            Builtin::Cos => number(f64::cos),
            Builtin::Sin => number(f64::sin),
            Builtin::Tan => number(f64::tan),
            Builtin::Exp => number(f64::exp),
            Builtin::Log => number(f64::ln),
            Builtin::Log2 => number(f64::log2),
            Builtin::Log10 => number(f64::log10),
            Builtin::Sqrt => number(f64::sqrt),
            Builtin::Blength | Builtin::Length => match v.plain() {
                Value::Indeterminate => Ok(Value::Indeterminate),
                Value::Binary(s) if b == Builtin::Blength => Ok(count(s.chars().count())),
                Value::String(s) if b == Builtin::Length => Ok(count(s.chars().count())),
                _ => stop("BLENGTH or LENGTH on a value of another type"),
            },
            Builtin::Exists => Ok(Value::Logical(logical(!v.is_indeterminate()))),
            Builtin::Nvl => Ok(if v.is_indeterminate() {
                values[1].clone()
            } else {
                v.clone()
            }),
            Builtin::Odd => match v.plain() {
                Value::Indeterminate => Ok(Value::Logical(Logical::Unknown)),
                Value::Integer(i) => Ok(Value::Logical(logical(i % 2 != 0))),
                _ => stop("ODD on a value that is not an integer"),
            },
            Builtin::Sizeof | Builtin::Hiindex | Builtin::Loindex => match v.plain() {
                Value::Indeterminate => Ok(Value::Indeterminate),
                Value::Aggregate(a) => {
                    let len = i64::try_from(a.items.len()).unwrap_or(i64::MAX);
                    Ok(Value::Integer(match (b, a.kind) {
                        (Builtin::Sizeof, _) => len,
                        (Builtin::Hiindex, AggregateKind::Array) => a.lower + len - 1,
                        (Builtin::Hiindex, _) => len,
                        (_, AggregateKind::Array) => a.lower,
                        _ => 1,
                    }))
                }
                _ => stop("SIZEOF, HIINDEX or LOINDEX on a value that is not an aggregate"),
            },
            Builtin::Typeof => {
                let names = self.type_names(v);
                Ok(Value::aggregate(
                    AggregateKind::Set,
                    names.iter().map(|n| Value::string(n)).collect(),
                ))
            }
            Builtin::Usedin => self.used_in(v, &values[1]),
            Builtin::Value => match v.plain() {
                Value::Indeterminate => Ok(Value::Indeterminate),
                Value::String(s) => {
                    let s = s.trim();
                    Ok(s.parse::<i64>().map(Value::Integer).unwrap_or_else(|_| {
                        s.parse::<f64>()
                            .ok()
                            .filter(|r| r.is_finite())
                            .map_or(Value::Indeterminate, Value::Real)
                    }))
                }
                _ => stop("VALUE on a value that is not a string"),
            },
        }
    }

    /// The qualified names `TYPEOF` returns, upper case: the entities of every part with their
    /// supertypes, or a defined type with the types it is based on, ending with the simple or
    /// aggregation type. Empty for `?`.
    pub fn type_names(&self, v: &Value) -> Vec<String> {
        let tables = self.tables;
        let qualified = |name: &str| format!("{}.{}", tables.name, name).to_ascii_uppercase();
        let mut names = Vec::new();
        let mut current = v;
        loop {
            match current {
                Value::Typed(d, inner) => {
                    let mut ty = Some(*d);
                    // The chain of defined types the value's type is based on.
                    for _ in 0..32 {
                        let Some(i) = ty else { break };
                        let Some(def) = tables.defined_types.get(usize::from(i)) else {
                            break;
                        };
                        names.push(qualified(def.name));
                        ty = match def.underlying {
                            TypeRef::Named(Named::Defined(j)) => Some(j),
                            _ => None,
                        };
                    }
                    current = inner;
                }
                Value::Entity(inst) => {
                    for part in self.parts(inst) {
                        for e in tables.supertypes(part) {
                            if let Some(entity) = tables.entities.get(usize::from(e)) {
                                let name = qualified(entity.name);
                                if !names.contains(&name) {
                                    names.push(name);
                                }
                            }
                        }
                    }
                    break;
                }
                Value::Enum(ty, _) => {
                    if let Some(e) = ty.and_then(|t| tables.enumerations.get(usize::from(t))) {
                        names.push(qualified(e.name));
                    }
                    break;
                }
                Value::Indeterminate => break,
                simple => {
                    names.push(
                        match simple {
                            Value::Integer(_) => "INTEGER",
                            Value::Real(_) => "REAL",
                            Value::Logical(_) => "LOGICAL",
                            Value::String(_) => "STRING",
                            Value::Binary(_) => "BINARY",
                            Value::Aggregate(a) => match a.kind {
                                AggregateKind::Array => "ARRAY",
                                AggregateKind::List => "LIST",
                                AggregateKind::Set => "SET",
                                AggregateKind::Bag => "BAG",
                            },
                            _ => "",
                        }
                        .to_owned(),
                    );
                    break;
                }
            }
        }
        names
    }

    /// The entity indexes of an instance's parts; empty for a dangling reference or an unknown
    /// entity.
    fn parts(&self, inst: &Inst) -> Vec<u16> {
        match inst {
            Inst::Built(b) => b.parts.iter().map(|(e, _)| *e).collect(),
            Inst::File(n) => self
                .file
                .instance(*n)
                .map(|i| {
                    i.records
                        .iter()
                        .filter_map(|r| self.tables.entity_index(&r.type_name))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// `USEDIN(T, 'SCHEMA.ENTITY.ATTRIBUTE')`: the instances that refer to `T` through the
    /// attribute, one per reference; with `''`, through any attribute.
    fn used_in(&mut self, target: &Value, role: &Value) -> Res<Value> {
        let role = match role.plain() {
            Value::String(s) => s.to_ascii_uppercase(),
            Value::Indeterminate => return Ok(Value::Indeterminate),
            _ => return stop("USEDIN with a role that is not a string"),
        };
        let n = match target.plain() {
            Value::Entity(Inst::File(n)) => *n,
            Value::Indeterminate => return Ok(Value::Indeterminate),
            Value::Entity(Inst::Built(_)) => {
                return Ok(Value::aggregate(AggregateKind::Bag, Vec::new()));
            }
            _ => return stop("USEDIN on a value that is not an entity instance"),
        };
        let tables = self.tables;
        let wanted = if role.is_empty() {
            None
        } else {
            let mut segments = role.split('.');
            let (Some(_schema), Some(entity), Some(attribute), None) = (
                segments.next(),
                segments.next(),
                segments.next(),
                segments.next(),
            ) else {
                return stop("USEDIN with a malformed role");
            };
            let Some(entity) = tables.entity_index(entity) else {
                return stop("USEDIN with a role naming an unknown entity");
            };
            Some((entity, attribute.to_owned()))
        };
        let referrers = self.index.referrers(n);
        self.check_len(referrers.len())?;
        let mut items = Vec::new();
        for referrer in referrers {
            let keep = match &wanted {
                None => true,
                Some((entity, attribute)) => self
                    .file
                    .instance(referrer.instance)
                    .and_then(|i| {
                        let record = i.records.get(usize::try_from(referrer.record).ok()?)?;
                        let part = tables.entity_index(&record.type_name)?;
                        if !tables.is_subtype_of(part, *entity) {
                            return Some(false);
                        }
                        let position = if i.records.len() == 1 {
                            tables.attribute_position(part, attribute)?.0
                        } else {
                            tables
                                .entities
                                .get(usize::from(part))?
                                .attributes
                                .iter()
                                .position(|a| a.name.eq_ignore_ascii_case(attribute))?
                        };
                        Some(usize::try_from(referrer.position).ok()? == position)
                    })
                    .unwrap_or(false),
            };
            if keep {
                items.push(Value::Entity(Inst::File(referrer.instance)));
            }
        }
        Ok(Value::aggregate(AggregateKind::Bag, items))
    }

    /// The instances of `entity` and its subtypes in file order, as a set.
    fn population(&mut self, entity: u16) -> Res<Value> {
        if let Some(v) = self.populations.get(&entity) {
            return Ok(v.clone());
        }
        let tables = self.tables;
        let mut items = Vec::new();
        for instance in self.file.instances() {
            let member = instance.records.iter().any(|r| {
                tables
                    .entity_index(&r.type_name)
                    .is_some_and(|e| tables.is_subtype_of(e, entity))
            });
            if member {
                items.push(Value::Entity(Inst::File(instance.name)));
            }
        }
        self.check_len(items.len())?;
        let v = Value::aggregate(AggregateKind::Set, items);
        self.populations.insert(entity, v.clone());
        Ok(v)
    }

    // ----- Attributes -----

    /// The attribute `name` of an entity value: a derived attribute, then an explicit one, then an
    /// inverse one, over the supertypes of every part. `?` for `?` or an attribute the entity
    /// does not have.
    fn attribute(&mut self, base: &Value, name: &str) -> Res<Value> {
        let inst = match base.plain() {
            Value::Indeterminate => return Ok(Value::Indeterminate),
            Value::Entity(inst) => inst.clone(),
            _ => return stop("an attribute of a value that is not an entity instance"),
        };
        let tables = self.tables;
        let parts = self.parts(&inst);
        if parts.is_empty() {
            return match inst {
                Inst::File(n) if self.file.instance(n).is_none() => Ok(Value::Indeterminate),
                _ => stop("an instance of an unknown entity"),
            };
        }
        let chain = |part: u16| {
            tables
                .supertypes(part)
                .filter_map(|e| tables.entities.get(usize::from(e)).map(|d| (e, d)))
        };
        for &part in &parts {
            for (_, e) in chain(part) {
                if let Some(d) = e.derived.iter().find(|d| d.name.eq_ignore_ascii_case(name)) {
                    self.enter()?;
                    let mut frame = Frame::new(Value::Entity(inst.clone()));
                    let value = self.eval(&d.tree, &mut frame);
                    self.depth -= 1;
                    return value;
                }
            }
        }
        for &part in &parts {
            for (owner, e) in chain(part) {
                let Some(i) = e
                    .attributes
                    .iter()
                    .position(|a| a.name.eq_ignore_ascii_case(name))
                else {
                    continue;
                };
                let ty = e.attributes[i].ty;
                return match &inst {
                    Inst::Built(b) => Ok(b
                        .parts
                        .iter()
                        .find(|(p, _)| *p == owner)
                        .and_then(|(_, values)| values.get(i).cloned())
                        .unwrap_or(Value::Indeterminate)),
                    Inst::File(n) => {
                        let Some(instance) = self.file.instance(*n) else {
                            return Ok(Value::Indeterminate);
                        };
                        let parameter = if instance.records.len() == 1 {
                            tables
                                .attribute_position(part, name)
                                .and_then(|(p, _)| instance.records[0].parameters.get(p))
                        } else {
                            // A part of a complex instance holds the attributes its entity
                            // declares.
                            instance
                                .records
                                .iter()
                                .find(|r| tables.entity_index(&r.type_name) == Some(owner))
                                .and_then(|r| r.parameters.get(i))
                        };
                        match parameter {
                            Some(p) => self.read(p, Some(ty)),
                            None => stop("a record without the attribute's position"),
                        }
                    }
                };
            }
        }
        for &part in &parts {
            for (_, e) in chain(part) {
                if let Some(inverse) = e
                    .inverses
                    .iter()
                    .find(|v| v.name.eq_ignore_ascii_case(name))
                {
                    let Inst::File(n) = inst else {
                        return Ok(Value::Indeterminate);
                    };
                    let members = self.index.members(self.file, tables, n, inverse);
                    self.check_len(members.len())?;
                    let items: Vec<Value> = members
                        .into_iter()
                        .map(|m| Value::Entity(Inst::File(m)))
                        .collect();
                    return Ok(match inverse.aggregate {
                        None => items.into_iter().next().unwrap_or(Value::Indeterminate),
                        Some(a) => Value::aggregate(a.kind, items),
                    });
                }
            }
        }
        Ok(Value::Indeterminate)
    }

    /// A parameter of the file as a value of the declared type `ty` (`None` when unknown).
    pub fn read(&mut self, parameter: &Parameter, ty: Option<TypeRef>) -> Res<Value> {
        self.read_at(parameter, ty, 0)
    }

    fn read_at(&mut self, parameter: &Parameter, ty: Option<TypeRef>, depth: u32) -> Res<Value> {
        if depth > crate::step::MAX_NESTING {
            return stop("a value nested too deeply");
        }
        let tables = self.tables;
        let underlying = ty.map(|t| tables.underlying(t));
        let plain = match parameter {
            Parameter::Omitted | Parameter::Derived => return Ok(Value::Indeterminate),
            Parameter::Integer(i) => match underlying {
                #[allow(clippy::cast_precision_loss, reason = "an integer written for a real")]
                Some(TypeRef::Real) => Value::Real(*i as f64),
                _ => Value::Integer(*i),
            },
            Parameter::Real(r) => Value::Real(*r),
            Parameter::Text(t) => Value::string(t),
            Parameter::Binary(b) => match bits_of_hex(b) {
                Some(bits) => Value::Binary(Rc::from(bits)),
                None => return stop("a malformed binary value"),
            },
            Parameter::Enumeration(e) => {
                let item = e.to_ascii_uppercase();
                match underlying {
                    Some(TypeRef::Boolean | TypeRef::Logical) => match item.as_str() {
                        "T" => Value::Logical(Logical::True),
                        "F" => Value::Logical(Logical::False),
                        "U" => Value::Logical(Logical::Unknown),
                        _ => return stop("a malformed logical value"),
                    },
                    Some(TypeRef::Named(Named::Enumeration(i))) => {
                        Value::Enum(Some(i), Rc::from(item))
                    }
                    _ => Value::Enum(None, Rc::from(item)),
                }
            }
            Parameter::Reference(n) => Value::Entity(Inst::File(*n)),
            Parameter::Typed { type_name, value } => {
                return match tables.find(type_name) {
                    Some(member @ (Named::Defined(_) | Named::Enumeration(_))) => {
                        self.read_at(value, Some(TypeRef::Named(member)), depth + 1)
                    }
                    _ => stop("a typed value of an unknown type"),
                };
            }
            Parameter::List(items) => {
                let (kind, lower, element) = match underlying {
                    Some(TypeRef::Aggregate(a)) => (
                        a.kind,
                        match (a.kind, a.lower) {
                            (AggregateKind::Array, Bound::Value(v)) => i64::from(v),
                            _ => 1,
                        },
                        Some(a.element),
                    ),
                    _ => (AggregateKind::List, 1, None),
                };
                self.check_len(items.len())?;
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.read_at(item, element, depth + 1)?);
                }
                Value::Aggregate(Rc::new(Agg {
                    kind,
                    lower,
                    items: values,
                }))
            }
        };
        Ok(match ty {
            Some(TypeRef::Named(Named::Defined(d))) => Value::Typed(d, Rc::new(plain)),
            _ => plain,
        })
    }

    // ----- Algorithms -----

    fn call(&mut self, function: u16, args: Vec<Value>) -> Res<Value> {
        let Some(f) = self.tables.functions.get(usize::from(function)) else {
            return stop("a call of an unknown function");
        };
        if args.len() != usize::from(f.tree.parameters) {
            return stop("a function call with the wrong number of arguments");
        }
        self.enter()?;
        let mut frame = Frame::new(Value::Indeterminate);
        frame.slots = args;
        frame.lowers = vec![None; frame.slots.len()];
        let result = self.locals(&f.tree, &mut frame).and_then(|()| {
            Ok(match self.exec(f.tree.body, &mut frame)? {
                Flow::Return(v) => v,
                _ => Value::Indeterminate,
            })
        });
        self.depth -= 1;
        result
    }

    fn locals(&mut self, algorithm: &'static Algorithm, frame: &mut Frame) -> Res<()> {
        for local in algorithm.locals {
            let lower = match &local.lower {
                Some(e) => match self.eval(e, frame)?.plain() {
                    Value::Integer(i) => *i,
                    _ => return stop("an array bound that is not an integer"),
                },
                None => 1,
            };
            let lower = local.kind.map(|kind| (kind, lower));
            let value = match &local.init {
                Some(e) => self.eval(e, frame)?,
                None => Value::Indeterminate,
            };
            frame.slot_mut(local.slot);
            frame.lowers[usize::from(local.slot)] = lower;
            frame.set(local.slot, value);
        }
        Ok(())
    }

    fn exec(&mut self, body: &'static [Stmt], frame: &mut Frame) -> Res<Flow> {
        for s in body {
            match self.stmt(s, frame)? {
                Flow::Next => {}
                flow => return Ok(flow),
            }
        }
        Ok(Flow::Next)
    }

    fn stmt(&mut self, s: &'static Stmt, frame: &mut Frame) -> Res<Flow> {
        self.step(1)?;
        Ok(match s {
            Stmt::Null => Flow::Next,
            Stmt::Assign(target, value) => {
                let value = self.eval(value, frame)?;
                self.assign(target, value, frame)?;
                Flow::Next
            }
            Stmt::If {
                condition,
                then,
                otherwise,
            } => {
                let c = self.eval(condition, frame)?;
                if truth(&c)? == Logical::True {
                    self.exec(then, frame)?
                } else {
                    self.exec(otherwise, frame)?
                }
            }
            Stmt::Case {
                selector,
                branches,
                otherwise,
            } => {
                let selector = self.eval(selector, frame)?;
                for branch in *branches {
                    for label in branch.labels {
                        let label = self.eval(label, frame)?;
                        if self.equal(&selector, &label, false)? == Logical::True {
                            return self.stmt(&branch.body, frame);
                        }
                    }
                }
                match otherwise {
                    Some(o) => self.stmt(o, frame)?,
                    None => Flow::Next,
                }
            }
            Stmt::Repeat {
                increment,
                while_,
                until,
                body,
            } => self.repeat(*increment, *while_, *until, body, frame)?,
            Stmt::Return(value) => Flow::Return(match value {
                Some(v) => self.eval(v, frame)?,
                None => Value::Indeterminate,
            }),
            Stmt::Escape => Flow::Escape,
            Stmt::Skip => Flow::Skip,
            Stmt::Compound(body) => self.exec(body, frame)?,
        })
    }

    fn repeat(
        &mut self,
        increment: Option<&'static super::tree::Increment>,
        while_: Option<&'static Expr>,
        until: Option<&'static Expr>,
        body: &'static [Stmt],
        frame: &mut Frame,
    ) -> Res<Flow> {
        let mut counter = None;
        if let Some(inc) = increment {
            let from = self.eval(&inc.from, frame)?;
            let to = self.eval(&inc.to, frame)?;
            let by = match &inc.by {
                Some(b) => self.eval(b, frame)?,
                None => Value::Integer(1),
            };
            match (from.plain(), to.plain(), by.plain()) {
                (Value::Integer(f), Value::Integer(t), Value::Integer(b)) if *b != 0 => {
                    counter = Some((inc.variable, *f, *t, *b));
                }
                // An indeterminate bound skips the loop.
                (Value::Indeterminate, _, _)
                | (_, Value::Indeterminate, _)
                | (_, _, Value::Indeterminate) => return Ok(Flow::Next),
                _ => return stop("a REPEAT bound that is not an integer or a zero step"),
            }
        }
        let mut i = counter.map_or(0, |c| c.1);
        loop {
            if let Some((slot, _, to, by)) = counter {
                if (by > 0 && i > to) || (by < 0 && i < to) {
                    break;
                }
                frame.set(slot, Value::Integer(i));
            }
            if let Some(w) = while_
                && truth(&self.eval(w, frame)?)? != Logical::True
            {
                break;
            }
            match self.exec(body, frame)? {
                Flow::Return(v) => return Ok(Flow::Return(v)),
                Flow::Escape => break,
                Flow::Next | Flow::Skip => {}
            }
            if let Some(u) = until
                && truth(&self.eval(u, frame)?)? == Logical::True
            {
                break;
            }
            match counter {
                Some((_, _, _, by)) => match i.checked_add(by) {
                    Some(next) => i = next,
                    None => break,
                },
                None => self.step(1)?,
            }
        }
        Ok(Flow::Next)
    }

    fn assign(&mut self, target: &'static Expr, value: Value, frame: &mut Frame) -> Res<()> {
        let mut segments = Vec::new();
        let mut current = target;
        let slot = loop {
            match current {
                Expr::Variable(slot) => break *slot,
                Expr::Attribute(base, name) => {
                    segments.push(Segment::Attribute(name));
                    current = base;
                }
                Expr::Index(base, index, None) => {
                    match self.eval(index, frame)?.plain() {
                        Value::Integer(i) => segments.push(Segment::Index(*i)),
                        _ => return stop("an assignment index that is not an integer"),
                    }
                    current = base;
                }
                _ => return stop("an assignment target that is not a variable"),
            }
        };
        if segments.is_empty() {
            frame.set(slot, value);
            return Ok(());
        }
        segments.reverse();
        let mut root = std::mem::replace(frame.slot_mut(slot), Value::Indeterminate);
        let result = self.assign_path(&mut root, &segments, value);
        *frame.slot_mut(slot) = root;
        result
    }

    fn assign_path(&mut self, target: &mut Value, path: &[Segment], value: Value) -> Res<()> {
        let Some((first, rest)) = path.split_first() else {
            *target = value;
            return Ok(());
        };
        let mut inner = target;
        while let Value::Typed(_, v) = inner {
            inner = Rc::make_mut(v);
        }
        match (first, inner) {
            (Segment::Index(i), Value::Aggregate(a)) => {
                let a = Rc::make_mut(a);
                let k = i
                    .checked_sub(a.lower)
                    .and_then(|k| usize::try_from(k).ok())
                    .filter(|k| *k < a.items.len());
                match k {
                    Some(k) => self.assign_path(&mut a.items[k], rest, value),
                    None => stop("an assignment index outside the aggregate"),
                }
            }
            (Segment::Attribute(name), Value::Entity(Inst::Built(b))) => {
                let tables = self.tables;
                let b = Rc::make_mut(b);
                for (entity, values) in &mut b.parts {
                    let declared = tables
                        .entities
                        .get(usize::from(*entity))
                        .map_or(&[][..], |e| e.attributes);
                    if let Some(i) = declared
                        .iter()
                        .position(|a| a.name.eq_ignore_ascii_case(name))
                        && let Some(slot) = values.get_mut(i)
                    {
                        return self.assign_path(slot, rest, value);
                    }
                }
                stop("an assignment to an attribute the entity value does not hold")
            }
            _ => stop("an assignment into a value of the file or of another type"),
        }
    }
}

fn count(n: usize) -> Value {
    Value::Integer(i64::try_from(n).unwrap_or(i64::MAX))
}

/// The truth value of a rule result: `?` is `UNKNOWN`.
fn truth(v: &Value) -> Res<Logical> {
    match v.plain() {
        Value::Logical(l) => Ok(*l),
        Value::Indeterminate => Ok(Logical::Unknown),
        _ => stop("a condition that is not a logical value"),
    }
}

/// The order of two values of the same kind; `None` when one is `?`.
fn compare(a: &Value, b: &Value) -> Res<Option<Ordering>> {
    Ok(match (a.plain(), b.plain()) {
        (Value::Indeterminate, _) | (_, Value::Indeterminate) => None,
        (Value::Integer(x), Value::Integer(y)) => Some(x.cmp(y)),
        (Value::String(x), Value::String(y)) | (Value::Binary(x), Value::Binary(y)) => {
            Some(x.cmp(y))
        }
        (Value::Logical(x), Value::Logical(y)) => Some(x.cmp(y)),
        (Value::Enum(_, x), Value::Enum(_, y)) if x == y => Some(Ordering::Equal),
        _ => match (a.number(), b.number()) {
            (Some(x), Some(y)) => match x.partial_cmp(&y) {
                Some(o) => Some(o),
                None => return stop("a comparison of numbers that are not ordered"),
            },
            _ => return stop("a comparison of values of different types"),
        },
    })
}

fn unary(op: UnaryOp, x: &Value) -> Res<Value> {
    if x.is_indeterminate() {
        return Ok(match op {
            UnaryOp::Not => Value::Logical(Logical::Unknown),
            _ => Value::Indeterminate,
        });
    }
    match (op, x.plain()) {
        (UnaryOp::Not, _) => Ok(Value::Logical(not(truth(x)?))),
        (UnaryOp::Plus, Value::Integer(_) | Value::Real(_)) => Ok(x.plain().clone()),
        (UnaryOp::Minus, Value::Integer(i)) => i
            .checked_neg()
            .map(Value::Integer)
            .map_or_else(|| stop("integer overflow"), Ok),
        (UnaryOp::Minus, Value::Real(r)) => Ok(Value::Real(-r)),
        _ => stop("a sign on a value that is not a number"),
    }
}

/// Arithmetic on numbers; `?` gives `?`.
fn arithmetic(op: BinaryOp, l: &Value, r: &Value) -> Res<Value> {
    if l.is_indeterminate() || r.is_indeterminate() {
        return Ok(Value::Indeterminate);
    }
    let overflow = || Stop::NotEvaluated("integer overflow".into());
    if let (Value::Integer(a), Value::Integer(b)) = (l.plain(), r.plain()) {
        let (a, b) = (*a, *b);
        return match op {
            BinaryOp::Add => a.checked_add(b).map(Value::Integer).ok_or_else(overflow),
            BinaryOp::Sub => a.checked_sub(b).map(Value::Integer).ok_or_else(overflow),
            BinaryOp::Mul => a.checked_mul(b).map(Value::Integer).ok_or_else(overflow),
            BinaryOp::IntDiv | BinaryOp::Mod if b == 0 => stop("division by zero"),
            BinaryOp::IntDiv => a
                .checked_div_euclid(b)
                .map(Value::Integer)
                .ok_or_else(overflow),
            BinaryOp::Mod => a
                .checked_rem_euclid(b)
                .map(Value::Integer)
                .ok_or_else(overflow),
            BinaryOp::Power if b >= 0 => u32::try_from(b)
                .ok()
                .and_then(|b| a.checked_pow(b))
                .map(Value::Integer)
                .ok_or_else(overflow),
            _ => real(op, l, r),
        };
    }
    real(op, l, r)
}

fn real(op: BinaryOp, l: &Value, r: &Value) -> Res<Value> {
    let (Some(a), Some(b)) = (l.number(), r.number()) else {
        return stop("arithmetic on values that are not numbers");
    };
    let v = match op {
        BinaryOp::Add => a + b,
        BinaryOp::Sub => a - b,
        BinaryOp::Mul => a * b,
        BinaryOp::Div if b == 0.0 => return stop("division by zero"),
        BinaryOp::Div => a / b,
        BinaryOp::Power => a.powf(b),
        _ => return stop("integer division of values that are not integers"),
    };
    if v.is_finite() {
        Ok(Value::Real(v))
    } else {
        stop("arithmetic outside the range of reals")
    }
}

#[cfg(test)]
mod tests {
    use super::super::tree::{Element, Increment, Local};
    use super::super::{DefinedType, Function};
    use super::*;
    use crate::{ReadOptions, read};

    static LOOP: [Stmt; 2] = [
        Stmt::Repeat {
            increment: Some(&Increment {
                variable: 1,
                from: Expr::Integer(1),
                to: Expr::Variable(0),
                by: None,
            }),
            while_: None,
            until: None,
            body: &[Stmt::Null],
        },
        Stmt::Return(Some(&Expr::Logical(Logical::True))),
    ];

    /// `ARRAY [0:2]` filled by index assignment, then returned.
    static ARRAY: [Stmt; 3] = [
        Stmt::Assign(
            &Expr::Variable(0),
            &Expr::Aggregate(&[Element {
                value: Expr::Integer(0),
                repeat: Some(&Expr::Integer(3)),
            }]),
        ),
        Stmt::Assign(
            &Expr::Index(&Expr::Variable(0), &Expr::Integer(2), None),
            &Expr::Integer(7),
        ),
        Stmt::Return(Some(&Expr::Variable(0))),
    ];

    static TABLES: SchemaTables = SchemaTables {
        name: "T",
        source_blake3: "",
        entities: &[],
        defined_types: &[
            DefinedType {
                name: "IfcLengthMeasure",
                underlying: TypeRef::Real,
                where_rules: &[],
            },
            DefinedType {
                name: "IfcPositiveLengthMeasure",
                underlying: TypeRef::Named(Named::Defined(0)),
                where_rules: &[],
            },
        ],
        enumerations: &[],
        selects: &[],
        functions: &[
            // FUNCTION A : ARRAY OF INTEGER; LOCAL r : ARRAY [0:2] OF INTEGER; END_LOCAL; ...
            Function {
                name: "A",
                source: "",
                tree: Algorithm {
                    parameters: 0,
                    locals: &[Local {
                        slot: 0,
                        init: None,
                        kind: Some(AggregateKind::Array),
                        lower: Some(Expr::Integer(0)),
                    }],
                    body: &ARRAY,
                },
            },
            // FUNCTION Loop(n : INTEGER) : LOGICAL; REPEAT i := 1 TO n; END_REPEAT; RETURN(TRUE);
            Function {
                name: "Loop",
                source: "",
                tree: Algorithm {
                    parameters: 1,
                    locals: &[],
                    body: &LOOP,
                },
            },
            // FUNCTION Rec(n) : LOGICAL; RETURN(Rec(n));
            Function {
                name: "Rec",
                source: "",
                tree: Algorithm {
                    parameters: 1,
                    locals: &[],
                    body: &[Stmt::Return(Some(&Expr::Call(2, &[Expr::Variable(0)])))],
                },
            },
        ],
        rules: &[],
    };

    fn file() -> IfcFile {
        let text = "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
            FILE_NAME('','',(''),(''),'','','');FILE_SCHEMA(('IFC4'));ENDSEC;DATA;ENDSEC;\
            END-ISO-10303-21;";
        read(text.as_bytes(), &mut ReadOptions::default())
            .unwrap()
            .file
    }

    /// Evaluates `e` with `SELF` bound to `this`.
    fn eval_with(e: &'static Expr, this: Value, cancel: &Cancel) -> Res<Value> {
        let file = file();
        let index = InverseIndex::default();
        let mut interp = Interp::new(&file, &TABLES, &index, &Limits::DEFAULT, cancel);
        let mut frame = Frame::new(this);
        interp.eval(e, &mut frame)
    }

    fn eval(e: &'static Expr) -> Res<Value> {
        eval_with(e, Value::Indeterminate, &Cancel::new())
    }

    fn truth_of(e: &'static Expr) -> Logical {
        truth(&eval(e).unwrap()).unwrap()
    }

    fn reason(e: &'static Expr) -> String {
        match eval(e) {
            Err(Stop::NotEvaluated(r)) => r,
            other => format!("{other:?}"),
        }
    }

    fn int(e: &'static Expr) -> i64 {
        match eval(e).unwrap().plain() {
            Value::Integer(i) => *i,
            other => panic!("{other:?}"),
        }
    }

    fn real(e: &'static Expr) -> f64 {
        eval(e).unwrap().number().unwrap()
    }

    const T: Expr = Expr::Logical(Logical::True);
    const F: Expr = Expr::Logical(Logical::False);
    const U: Expr = Expr::Logical(Logical::Unknown);
    const I: Expr = Expr::Indeterminate;

    #[test]
    fn three_valued_logic() {
        use Logical::{False, True, Unknown};
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::And, &T, &U)), Unknown);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::And, &F, &U)), False);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::And, &U, &F)), False);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::Or, &F, &U)), Unknown);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::Or, &U, &T)), True);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::Xor, &T, &F)), True);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::Xor, &T, &T)), False);
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::Xor, &T, &U)), Unknown);
        assert_eq!(truth_of(&Expr::Unary(UnaryOp::Not, &U)), Unknown);
        assert_eq!(truth_of(&Expr::Unary(UnaryOp::Not, &F)), True);
        // `?` is UNKNOWN in a logical context and in comparisons.
        assert_eq!(truth_of(&Expr::Binary(BinaryOp::And, &T, &I)), Unknown);
        assert_eq!(truth_of(&I), Unknown);
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::Eq, &I, &Expr::Integer(1))),
            Unknown
        );
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::Lt, &U, &T)),
            True,
            "FALSE < UNKNOWN < TRUE"
        );
        // AND and OR decide on their left operand without evaluating the right one.
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::Or, &T, &Expr::Integer(1))),
            True
        );
        assert!(reason(&Expr::Binary(BinaryOp::And, &T, &Expr::Integer(1))).contains("logical"));
    }

    #[test]
    fn numbers_mix_integers_and_reals() {
        const ONE: Expr = Expr::Integer(1);
        const HALF: Expr = Expr::Real(0.5);
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::Eq, &ONE, &Expr::Real(1.0))),
            Logical::True
        );
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::Gt, &ONE, &HALF)),
            Logical::True
        );
        assert!((real(&Expr::Binary(BinaryOp::Div, &ONE, &Expr::Integer(4))) - 0.25).abs() < 1e-12);
        assert_eq!(
            int(&Expr::Binary(
                BinaryOp::IntDiv,
                &Expr::Integer(-7),
                &Expr::Integer(2)
            )),
            -4
        );
        assert_eq!(
            int(&Expr::Binary(
                BinaryOp::Mod,
                &Expr::Integer(-7),
                &Expr::Integer(2)
            )),
            1
        );
        assert_eq!(
            int(&Expr::Binary(
                BinaryOp::Power,
                &Expr::Integer(2),
                &Expr::Integer(10)
            )),
            1024
        );
        assert!((real(&Expr::Builtin(Builtin::Sqrt, &[Expr::Integer(9)])) - 3.0).abs() < 1e-12);
        assert_eq!(int(&Expr::Builtin(Builtin::Abs, &[Expr::Integer(-3)])), 3);
        assert!(matches!(
            eval(&Expr::Binary(BinaryOp::Add, &ONE, &I)).unwrap(),
            Value::Indeterminate
        ));
        // Errors stop the evaluation instead of passing.
        assert!(reason(&Expr::Binary(BinaryOp::Div, &ONE, &Expr::Integer(0))).contains("zero"));
        assert!(
            reason(&Expr::Binary(
                BinaryOp::Mul,
                &Expr::Integer(i64::MAX),
                &Expr::Integer(2)
            ))
            .contains("overflow")
        );
        assert!(reason(&Expr::Builtin(Builtin::Sqrt, &[Expr::Integer(-1)])).contains("domain"));
        assert!(reason(&Expr::Binary(BinaryOp::Lt, &ONE, &Expr::String("a"))).contains("types"));
    }

    const LIST: Expr = Expr::Aggregate(&[
        Element {
            value: Expr::Integer(1),
            repeat: None,
        },
        Element {
            value: Expr::Integer(2),
            repeat: Some(&Expr::Integer(2)),
        },
    ]);

    #[test]
    fn aggregates() {
        assert_eq!(int(&Expr::Builtin(Builtin::Sizeof, &[LIST])), 3);
        assert_eq!(int(&Expr::Builtin(Builtin::Hiindex, &[LIST])), 3);
        assert_eq!(int(&Expr::Builtin(Builtin::Loindex, &[LIST])), 1);
        assert_eq!(int(&Expr::Index(&LIST, &Expr::Integer(2), None)), 2);
        assert!(matches!(
            eval(&Expr::Index(&LIST, &Expr::Integer(4), None)).unwrap(),
            Value::Indeterminate
        ));
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::In, &Expr::Integer(2), &LIST)),
            Logical::True
        );
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::In, &Expr::Integer(5), &LIST)),
            Logical::False
        );
        assert_eq!(
            truth_of(&Expr::Binary(BinaryOp::In, &I, &LIST)),
            Logical::Unknown
        );
        // A list keeps repeats; a set of TYPEOF does not.
        assert_eq!(
            int(&Expr::Builtin(
                Builtin::Sizeof,
                &[Expr::Binary(BinaryOp::Add, &LIST, &LIST)]
            )),
            6
        );
        assert_eq!(
            int(&Expr::Builtin(
                Builtin::Sizeof,
                &[Expr::Binary(BinaryOp::Sub, &LIST, &Expr::Integer(2))]
            )),
            2
        );
        const TYPES: Expr = Expr::Builtin(Builtin::Typeof, &[Expr::Integer(1)]);
        assert_eq!(
            int(&Expr::Builtin(
                Builtin::Sizeof,
                &[Expr::Binary(BinaryOp::Add, &TYPES, &TYPES)]
            )),
            1
        );
        assert_eq!(
            int(&Expr::Builtin(
                Builtin::Sizeof,
                &[Expr::Binary(
                    BinaryOp::Mul,
                    &LIST,
                    &Expr::Aggregate(&[Element {
                        value: Expr::Integer(2),
                        repeat: None,
                    }])
                )]
            )),
            2
        );
        assert!(matches!(
            eval(&Expr::Builtin(Builtin::Sizeof, &[I])).unwrap(),
            Value::Indeterminate
        ));
        assert!(matches!(
            eval(&Expr::Builtin(Builtin::Nvl, &[I, Expr::Integer(4)])).unwrap(),
            Value::Integer(4)
        ));
        assert_eq!(
            truth_of(&Expr::Builtin(Builtin::Exists, &[I])),
            Logical::False
        );
        // { 1 <= x < 3 }
        assert_eq!(
            truth_of(&Expr::Interval {
                low: &Expr::Integer(1),
                low_inclusive: true,
                item: &Expr::Integer(1),
                high_inclusive: false,
                high: &Expr::Integer(3),
            }),
            Logical::True
        );
        assert_eq!(
            truth_of(&Expr::Interval {
                low: &Expr::Integer(1),
                low_inclusive: false,
                item: &Expr::Integer(1),
                high_inclusive: false,
                high: &Expr::Integer(3),
            }),
            Logical::False
        );
        // A repetition beyond the array length limit is not built.
        const HUGE: Expr = Expr::Aggregate(&[Element {
            value: Expr::Integer(0),
            repeat: Some(&Expr::Integer(i64::MAX)),
        }]);
        assert!(reason(&HUGE).contains("limit"));
    }

    #[test]
    fn query_keeps_the_elements_the_condition_holds_for() {
        // QUERY(x <* [1, 2, 2] | x > 1)
        const Q: Expr = Expr::Query {
            variable: 0,
            aggregate: &LIST,
            condition: &Expr::Binary(BinaryOp::Gt, &Expr::Variable(0), &Expr::Integer(1)),
        };
        assert_eq!(int(&Expr::Builtin(Builtin::Sizeof, &[Q])), 2);
        // A condition that is UNKNOWN does not keep the element.
        const UQ: Expr = Expr::Query {
            variable: 0,
            aggregate: &LIST,
            condition: &Expr::Logical(Logical::Unknown),
        };
        assert_eq!(int(&Expr::Builtin(Builtin::Sizeof, &[UQ])), 0);
        const IQ: Expr = Expr::Query {
            variable: 0,
            aggregate: &I,
            condition: &T,
        };
        assert!(matches!(eval(&IQ).unwrap(), Value::Indeterminate));
    }

    #[test]
    fn typeof_names_the_defined_types_a_value_is_based_on() {
        let file = file();
        let index = InverseIndex::default();
        let cancel = Cancel::new();
        let interp = Interp::new(&file, &TABLES, &index, &Limits::DEFAULT, &cancel);
        let v = Value::Typed(1, Rc::new(Value::Real(2.0)));
        assert_eq!(
            interp.type_names(&v),
            ["T.IFCPOSITIVELENGTHMEASURE", "T.IFCLENGTHMEASURE", "REAL"]
        );
        assert!(interp.type_names(&Value::Indeterminate).is_empty());
        // SELF > 0 compares the value inside its defined types.
        assert_eq!(
            truth(
                &eval_with(
                    &Expr::Binary(BinaryOp::Gt, &Expr::SelfValue, &Expr::Integer(0)),
                    v,
                    &cancel
                )
                .unwrap()
            )
            .unwrap(),
            Logical::True
        );
    }

    #[test]
    fn like_follows_the_express_wildcards() {
        let check = |text: &str, pattern: &str| {
            let t: Vec<char> = text.chars().collect();
            let p: Vec<char> = pattern.chars().collect();
            like(&t, &p, &mut 10_000).unwrap()
        };
        assert!(check("A1", "@#"));
        assert!(!check("11", "@#"));
        assert!(check("Ab", "^!"));
        assert!(!check("ab", "^!"));
        assert!(check("abc", "a?c"));
        assert!(check("abcdef", "ab&"));
        assert!(!check("abcdef", "ab&f"));
        assert!(check("abcdef", "a*f"));
        assert!(check("af", "a*f"));
        assert!(!check("abcde", "a*f"));
        assert!(check("one two", "$ two"));
        assert!(check("a*b", "a\\*b"));
        assert!(!check("axb", "a\\*b"));
        assert!(check("", ""));
        // The budget bounds backtracking.
        let t: Vec<char> = "a".repeat(40).chars().collect();
        let p: Vec<char> = "*a*a*a*a*b".chars().collect();
        assert!(like(&t, &p, &mut 1000).is_none());
    }

    #[test]
    fn functions_assign_into_arrays_with_their_lower_bound() {
        const CALL: Expr = Expr::Call(0, &[]);
        assert_eq!(int(&Expr::Builtin(Builtin::Loindex, &[CALL])), 0);
        assert_eq!(int(&Expr::Builtin(Builtin::Hiindex, &[CALL])), 2);
        assert_eq!(int(&Expr::Index(&CALL, &Expr::Integer(2), None)), 7);
        assert_eq!(int(&Expr::Index(&CALL, &Expr::Integer(0), None)), 0);
    }

    #[test]
    fn evaluations_are_bounded() {
        assert_eq!(
            truth_of(&Expr::Call(1, &[Expr::Integer(100)])),
            Logical::True
        );
        assert!(reason(&Expr::Call(1, &[Expr::Integer(10_000_000)])).contains("step budget"));
        assert!(reason(&Expr::Call(2, &[Expr::Integer(1)])).contains("depth"));
        assert!(reason(&Expr::Call(1, &[])).contains("arguments"));
        let cancel = Cancel::new();
        cancel.cancel();
        assert!(matches!(
            eval_with(
                &Expr::Call(1, &[Expr::Integer(100_000)]),
                Value::Indeterminate,
                &cancel
            ),
            Err(Stop::Cancelled(_))
        ));
    }
}
