//! Expression values.

use arc_base::{Dimension, Quantity};

/// A value an expression evaluates to, a name stands for, or a host function takes and returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A number with its dimension, in SI base units.
    Number(Quantity),
    /// A number written without a unit. It takes the context unit where it meets a dimensioned
    /// value or becomes the result; see the specification. Results of
    /// [`Expr::evaluate`](crate::Expr::evaluate) never contain it.
    Bare(f64),
    /// A truth value.
    Bool(bool),
    /// A text.
    Text(String),
    /// A list of values.
    List(Vec<Value>),
}

impl Value {
    /// The number as a quantity; a bare number is dimensionless. `None` for other kinds.
    #[must_use]
    pub fn quantity(&self) -> Option<Quantity> {
        match self {
            Self::Number(quantity) => Some(*quantity),
            Self::Bare(value) => Some(Quantity::new(*value, Dimension::NONE)),
            _ => None,
        }
    }
}
