//! Writes the expressions, statements and algorithms of a parsed schema as the static trees of
//! `crates/arc-io-ifc/src/schema/tree.rs`, resolving every name: variables become slots, entity,
//! function and defined type names become table indexes, unqualified names in an entity rule
//! become attributes of `SELF`, and enumeration items are checked against the enumerations. A name
//! or construct the trees cannot hold fails the generation instead of being dropped.

use super::ast::{
    AggregateKind, Algorithm, Expr, Local, Logical, RuleDecl, Stmt, TypeSpec, UnaryOp,
};
use super::emit::{Resolved, Target};

/// What unqualified names may refer to besides variables.
#[derive(Clone, Copy)]
pub enum Context {
    /// An entity rule or derived attribute: `SELF` and the attributes of the entity.
    Entity(u16),
    /// A defined type rule: `SELF` is the value.
    Type,
    /// A function: parameters and locals only.
    Function,
}

/// Built-in functions by name, as written in `Builtin`.
const BUILTINS: &[(&str, &str)] = &[
    ("ABS", "Abs"),
    ("ACOS", "Acos"),
    ("ASIN", "Asin"),
    ("ATAN", "Atan"),
    ("BLENGTH", "Blength"),
    ("COS", "Cos"),
    ("EXISTS", "Exists"),
    ("EXP", "Exp"),
    ("HIINDEX", "Hiindex"),
    ("LENGTH", "Length"),
    ("LOINDEX", "Loindex"),
    ("LOG", "Log"),
    ("LOG2", "Log2"),
    ("LOG10", "Log10"),
    ("NVL", "Nvl"),
    ("ODD", "Odd"),
    ("SIN", "Sin"),
    ("SIZEOF", "Sizeof"),
    ("SQRT", "Sqrt"),
    ("TAN", "Tan"),
    ("TYPEOF", "Typeof"),
    ("USEDIN", "Usedin"),
    ("VALUE", "Value"),
];

/// Name resolution state of one rule, derived attribute or algorithm.
pub struct Scope<'a, 'r> {
    r: &'a Resolved<'r>,
    context: Context,
    /// Entities a global rule ranges over.
    population: Vec<(String, u16)>,
    vars: Vec<(String, u16)>,
    next: u16,
}

fn upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

impl<'a, 'r> Scope<'a, 'r> {
    pub fn new(r: &'a Resolved<'r>, context: Context) -> Self {
        Self {
            r,
            context,
            population: Vec::new(),
            vars: Vec::new(),
            next: 0,
        }
    }

    fn push(&mut self, name: &str) -> Result<u16, String> {
        let slot = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| "too many variables".to_owned())?;
        self.vars.push((upper(name), slot));
        Ok(slot)
    }

    fn var(&self, name: &str) -> Option<u16> {
        let name = upper(name);
        self.vars
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .map(|(_, s)| *s)
    }

    /// Whether the entity or a supertype declares an explicit, derived or inverse attribute
    /// `name`.
    fn has_attribute(&self, entity: u16, name: &str) -> bool {
        let mut current = Some(entity);
        for _ in 0..self.r.entities.len() {
            let Some(i) = current else { return false };
            let Some(e) = self.r.entities.get(usize::from(i)) else {
                return false;
            };
            let plain = |n: &super::ast::AttrName| match n {
                super::ast::AttrName::Plain(n) => n.eq_ignore_ascii_case(name),
                super::ast::AttrName::Redeclared { attribute, .. } => {
                    attribute.eq_ignore_ascii_case(name)
                }
            };
            if e.explicit.iter().any(|a| plain(&a.name))
                || e.derived.iter().any(|d| plain(&d.name))
                || e.inverse.iter().any(|v| plain(&v.name))
            {
                return true;
            }
            current = self.r.supertype(e).ok().flatten();
        }
        false
    }

    fn enum_item(&self, item: &str) -> Option<String> {
        self.r.enumerations.iter().find_map(|t| match &t.body {
            super::ast::TypeBody::Enumeration { items, .. } => items
                .iter()
                .find(|i| i.eq_ignore_ascii_case(item))
                .map(|i| upper(i)),
            _ => None,
        })
    }

    fn name(&self, name: &str) -> Result<String, String> {
        if let Some(slot) = self.var(name) {
            return Ok(format!("Expr::Variable({slot})"));
        }
        let is_self = name.eq_ignore_ascii_case("SELF");
        match self.context {
            Context::Entity(_) | Context::Type if is_self => return Ok("Expr::SelfValue".into()),
            Context::Entity(e) if self.has_attribute(e, name) => {
                return Ok(format!("Expr::Attribute(&Expr::SelfValue, {name:?})"));
            }
            _ => {}
        }
        if let Some((_, e)) = self
            .population
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            return Ok(format!("Expr::Population({e})"));
        }
        if name.eq_ignore_ascii_case("PI") {
            return Ok("Expr::Real(std::f64::consts::PI)".into());
        }
        if name.eq_ignore_ascii_case("CONST_E") {
            return Ok("Expr::Real(std::f64::consts::E)".into());
        }
        if let Some(item) = self.enum_item(name) {
            return Ok(format!("Expr::EnumItem({item:?})"));
        }
        Err(format!("unresolved name {name}"))
    }

    fn list(&mut self, items: &[Expr]) -> Result<String, String> {
        let mut out = String::from("&[");
        for item in items {
            out.push_str(&self.expr(item)?);
            out.push_str(", ");
        }
        out.push(']');
        Ok(out)
    }

    fn boxed(&mut self, e: &Expr) -> Result<String, String> {
        Ok(format!("&{}", self.expr(e)?))
    }

    /// The Rust source of an `Expr` value.
    #[allow(clippy::too_many_lines, reason = "one arm per kind of expression")]
    pub fn expr(&mut self, e: &Expr) -> Result<String, String> {
        Ok(match e {
            Expr::Integer(v) => format!(
                "Expr::Integer({})",
                i64::try_from(*v).map_err(|_| format!("integer {v} is too large"))?
            ),
            Expr::Real(v) if v.is_finite() => format!("Expr::Real({v:?})"),
            Expr::Real(v) => return Err(format!("real {v} is not finite")),
            Expr::Str(s) => format!("Expr::String({s:?})"),
            Expr::Encoded(_) => return Err("encoded string literals are not supported".into()),
            Expr::Bits(b) => format!("Expr::Bits({b:?})"),
            Expr::Logical(l) => format!(
                "Expr::Logical(Logical::{})",
                match l {
                    Logical::True => "True",
                    Logical::False => "False",
                    Logical::Unknown => "Unknown",
                }
            ),
            Expr::Indeterminate => "Expr::Indeterminate".into(),
            Expr::Name(n) => self.name(n)?,
            Expr::Unary(op, x) => {
                let op = match op {
                    UnaryOp::Plus => "Plus",
                    UnaryOp::Minus => "Minus",
                    UnaryOp::Not => "Not",
                };
                format!("Expr::Unary(UnaryOp::{op}, {})", self.boxed(x)?)
            }
            Expr::Binary(op, a, b) => format!(
                "Expr::Binary(BinaryOp::{op:?}, {}, {})",
                self.boxed(a)?,
                self.boxed(b)?
            ),
            Expr::Call(name, args) => self.call(name, args)?,
            Expr::Attribute(base, attribute) => {
                // `Type.ITEM` names an item of an enumeration type.
                if let Expr::Name(n) = base.as_ref()
                    && self.var(n).is_none()
                    && matches!(self.r.target(n), Some(Target::Enumeration(_)))
                {
                    let item = self
                        .enum_item(attribute)
                        .ok_or_else(|| format!("{n} has no item {attribute}"))?;
                    return Ok(format!("Expr::EnumItem({item:?})"));
                }
                format!("Expr::Attribute({}, {attribute:?})", self.boxed(base)?)
            }
            Expr::Group(base, entity) => {
                if !matches!(self.r.target(entity), Some(Target::Entity(_))) {
                    return Err(format!("{entity} is not an entity"));
                }
                self.expr(base)?
            }
            Expr::Index(base, index, high) => {
                let high = match high {
                    Some(h) => format!("Some({})", self.boxed(h)?),
                    None => "None".into(),
                };
                format!(
                    "Expr::Index({}, {}, {high})",
                    self.boxed(base)?,
                    self.boxed(index)?
                )
            }
            Expr::AggregateInit(elements) => {
                let mut out = String::from("Expr::Aggregate(&[");
                for (value, repeat) in elements {
                    let repeat = match repeat {
                        Some(r) => format!("Some({})", self.boxed(r)?),
                        None => "None".into(),
                    };
                    out.push_str(&format!(
                        "Element {{ value: {}, repeat: {repeat} }}, ",
                        self.expr(value)?
                    ));
                }
                out.push_str("])");
                out
            }
            Expr::Interval {
                low,
                low_inclusive,
                item,
                high_inclusive,
                high,
            } => format!(
                "Expr::Interval {{ low: {}, low_inclusive: {low_inclusive}, item: {}, \
                 high_inclusive: {high_inclusive}, high: {} }}",
                self.boxed(low)?,
                self.boxed(item)?,
                self.boxed(high)?
            ),
            Expr::Query {
                variable,
                aggregate,
                condition,
            } => {
                let aggregate = self.boxed(aggregate)?;
                let slot = self.push(variable)?;
                let condition = self.boxed(condition);
                self.vars.pop();
                format!(
                    "Expr::Query {{ variable: {slot}, aggregate: {aggregate}, condition: {} }}",
                    condition?
                )
            }
        })
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> Result<String, String> {
        if let Some((_, b)) = BUILTINS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Ok(format!("Expr::Builtin(Builtin::{b}, {})", self.list(args)?));
        }
        match self.r.target(name) {
            Some(Target::Function) => {
                let i = self
                    .r
                    .functions
                    .iter()
                    .position(|f| f.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| format!("{name} is not a function"))?;
                Ok(format!("Expr::Call({i}, {})", self.list(args)?))
            }
            Some(Target::Entity(i)) => Ok(format!("Expr::Construct({i}, {})", self.list(args)?)),
            Some(Target::Defined(i)) => match args {
                [value] => Ok(format!("Expr::Convert({i}, {})", self.boxed(value)?)),
                _ => Err(format!("{name} converts one value")),
            },
            _ => Err(format!("{name} is not a function, entity or defined type")),
        }
    }

    fn stmts(&mut self, body: &[Stmt]) -> Result<String, String> {
        let mut out = String::from("&[");
        for s in body {
            out.push_str(&self.stmt(s)?);
            out.push_str(", ");
        }
        out.push(']');
        Ok(out)
    }

    fn opt(&mut self, e: Option<&Expr>) -> Result<String, String> {
        Ok(match e {
            Some(e) => format!("Some({})", self.boxed(e)?),
            None => "None".into(),
        })
    }

    fn stmt(&mut self, s: &Stmt) -> Result<String, String> {
        Ok(match s {
            Stmt::Null => "Stmt::Null".into(),
            Stmt::Assign(target, value) => {
                if root(target).is_none_or(|n| self.var(n).is_none()) {
                    return Err("an assignment target is not a variable".into());
                }
                let target = self.boxed(target)?;
                format!("Stmt::Assign({target}, {})", self.boxed(value)?)
            }
            Stmt::If {
                condition,
                then,
                otherwise,
            } => format!(
                "Stmt::If {{ condition: {}, then: {}, otherwise: {} }}",
                self.boxed(condition)?,
                self.stmts(then)?,
                self.stmts(otherwise)?
            ),
            Stmt::Case {
                selector,
                branches,
                otherwise,
            } => {
                let selector = self.boxed(selector)?;
                let mut out = String::from("&[");
                for (labels, body) in branches {
                    out.push_str(&format!(
                        "Branch {{ labels: {}, body: {} }}, ",
                        self.list(labels)?,
                        self.stmt(body)?
                    ));
                }
                out.push(']');
                let otherwise = match otherwise {
                    Some(o) => format!("Some(&{})", self.stmt(o)?),
                    None => "None".into(),
                };
                format!(
                    "Stmt::Case {{ selector: {selector}, branches: {out}, otherwise: {otherwise} }}"
                )
            }
            Stmt::Repeat {
                increment,
                while_,
                until,
                body,
            } => {
                let mut pushed = false;
                let increment = match increment {
                    Some((variable, from, to, by)) => {
                        let from = self.expr(from)?;
                        let to = self.expr(to)?;
                        let by = match by {
                            Some(b) => format!("Some({})", self.expr(b)?),
                            None => "None".into(),
                        };
                        let slot = self.push(variable)?;
                        pushed = true;
                        format!(
                            "Some(&Increment {{ variable: {slot}, from: {from}, to: {to}, by: {by} }})"
                        )
                    }
                    None => "None".into(),
                };
                let rest = (|| {
                    Ok::<_, String>(format!(
                        "while_: {}, until: {}, body: {}",
                        self.opt(while_.as_ref())?,
                        self.opt(until.as_ref())?,
                        self.stmts(body)?
                    ))
                })();
                if pushed {
                    self.vars.pop();
                }
                format!("Stmt::Repeat {{ increment: {increment}, {} }}", rest?)
            }
            Stmt::Return(value) => format!("Stmt::Return({})", self.opt(value.as_ref())?),
            Stmt::Escape => "Stmt::Escape".into(),
            Stmt::Skip => "Stmt::Skip".into(),
            Stmt::Compound(body) => format!("Stmt::Compound({})", self.stmts(body)?),
            Stmt::Call(_) => return Err("procedure calls are not supported".into()),
        })
    }

    fn locals(&mut self, locals: &[&Local]) -> Result<String, String> {
        let mut out = String::from("&[");
        for local in locals {
            let init = match &local.init {
                Some(e) => format!("Some({})", self.expr(e)?),
                None => "None".into(),
            };
            let (kind, lower) = match &local.ty {
                Some(TypeSpec::Aggregate(a)) => {
                    let kind = match a.kind {
                        AggregateKind::Array => "Some(AggregateKind::Array)",
                        AggregateKind::List => "Some(AggregateKind::List)",
                        AggregateKind::Set => "Some(AggregateKind::Set)",
                        AggregateKind::Bag => "Some(AggregateKind::Bag)",
                        AggregateKind::General => "None",
                    };
                    let lower = match &a.bounds {
                        Some((low, _)) if a.kind == AggregateKind::Array => {
                            format!("Some({})", self.expr(&low.expr)?)
                        }
                        _ => "None".into(),
                    };
                    (kind, lower)
                }
                _ => ("None", "None".into()),
            };
            // The variable is visible to the initializers of the locals after it.
            let slot = self.push(&local.name)?;
            out.push_str(&format!(
                "Local {{ slot: {slot}, init: {init}, kind: {kind}, lower: {lower} }}, "
            ));
        }
        out.push(']');
        Ok(out)
    }

    /// The `Algorithm` of a function.
    pub fn function(&mut self, f: &Algorithm) -> Result<String, String> {
        for (name, _) in &f.parameters {
            self.push(name)?;
        }
        let locals: Vec<&Local> = f.constants.iter().chain(&f.locals).collect();
        let locals = self.locals(&locals)?;
        let body = self.stmts(&f.body)?;
        Ok(format!(
            "Algorithm {{ parameters: {}, locals: {locals}, body: {body} }}",
            f.parameters.len()
        ))
    }

    /// The `Algorithm` of a global rule; the scope keeps its variables for the WHERE rules.
    pub fn rule(&mut self, rule: &RuleDecl) -> Result<String, String> {
        for name in &rule.entities {
            match self.r.target(name) {
                Some(Target::Entity(i)) => self.population.push((name.clone(), i)),
                _ => return Err(format!("{name} is not an entity")),
            }
        }
        let locals: Vec<&Local> = rule.locals.iter().collect();
        let locals = self.locals(&locals)?;
        let body = self.stmts(&rule.body)?;
        Ok(format!(
            "Algorithm {{ parameters: 0, locals: {locals}, body: {body} }}"
        ))
    }
}

/// The name an assignment target starts from, when it is a name followed by qualifiers.
fn root(e: &Expr) -> Option<&str> {
    match e {
        Expr::Name(n) => Some(n),
        Expr::Attribute(base, _) | Expr::Group(base, _) | Expr::Index(base, _, None) => root(base),
        _ => None,
    }
}
