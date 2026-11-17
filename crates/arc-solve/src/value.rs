//! Expression values.

use std::sync::Arc;

use arc_base::{Dimension, Quantity};

/// A value an expression evaluates to, a name stands for, or a host function takes and returns.
///
/// Texts and lists are shared: cloning a value copies a reference, not its content.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// A number with its dimension, in SI base units. A temperature of this kind is a
    /// temperature difference.
    Number(Quantity),
    /// An absolute temperature: a reading on a scale with an offset (`20 °C`), in kelvin. Two of
    /// them cannot be added or multiplied and one cannot be negated; see the specification.
    Absolute(Quantity),
    /// A number written without a unit. It takes the context unit where it meets a dimensioned
    /// value or becomes the result; see the specification. Results of
    /// [`Expr::evaluate`](crate::Expr::evaluate) never contain it.
    Bare(f64),
    /// A truth value.
    Bool(bool),
    /// A text.
    Text(Arc<str>),
    /// A list of values.
    List(Arc<[Value]>),
}

impl Value {
    /// A text value.
    #[must_use]
    pub fn text(text: impl Into<Arc<str>>) -> Self {
        Self::Text(text.into())
    }

    /// A list value.
    #[must_use]
    pub fn list(items: impl Into<Arc<[Value]>>) -> Self {
        Self::List(items.into())
    }

    /// The number as a quantity; a bare number is dimensionless. `None` for other kinds.
    #[must_use]
    pub fn quantity(&self) -> Option<Quantity> {
        match self {
            Self::Number(quantity) | Self::Absolute(quantity) => Some(*quantity),
            Self::Bare(value) => Some(Quantity::new(*value, Dimension::NONE)),
            _ => None,
        }
    }
}

/// Drops `value` without recursion, however deeply its lists nest. Lists shared with other
/// values are only released, never descended into.
pub(crate) fn release(value: Value) {
    let mut pending = vec![value];
    while let Some(mut value) = pending.pop() {
        if let Value::List(items) = &mut value
            && let Some(items) = Arc::get_mut(items)
        {
            for item in items.iter_mut() {
                if matches!(item, Value::List(_)) {
                    pending.push(std::mem::replace(item, Value::Bool(false)));
                }
            }
        }
    }
}
