//! The unit-aware expression engine of the Arc suite; geometric constraint solving follows in a
//! later phase.
//!
//! This first part parses and evaluates expressions with units (CORE-SOLVE-016 to CORE-SOLVE-018),
//! bounds parsing and evaluation (CORE-SOLVE-021) and reads numeric field input back from the
//! text that [`arc_base::DisplayFormat`] writes (CORE-SOLVE-022):
//!
//! ```
//! use arc_base::{Dimension, Unit};
//! use arc_solve::{Context, Value, evaluate};
//!
//! let value = evaluate("1200 mm + 3 in", &Context::default()).unwrap();
//! let Value::Number(length) = value else { panic!("not a number") };
//! assert_eq!(length.dimension(), Dimension::LENGTH);
//! assert!((length.value() - 1.2762).abs() < 1e-12);
//!
//! // A bare number takes the context unit.
//! let field = Context::new().with_bare_unit(*Unit::by_symbol("mm").unwrap());
//! let width = arc_solve::parse_quantity("1200 + 300", &field, '.', Dimension::LENGTH).unwrap();
//! assert!((width.value() - 1.5).abs() < 1e-12);
//! ```
//!
//! The grammar, the unit rules, the function library and the limits are specified in
//! `docs/crates/arc-solve.md`. Evaluation is deterministic, bounded and free of side effects.
//!
//! No function in this crate panics, and none recurses deeper than the depth limit
//! ([`ExprLimits::MAX_DEPTH`]) allows. Values that names and host functions supply are checked
//! and, when they fail the limits, dropped without recursion. Dropping a [`Value`] whose lists
//! nest deeply is recursive in Rust itself, so a host must not build or keep such values on its
//! own side.

pub mod error;
pub mod eval;
pub mod functions;
mod lex;
pub mod limits;
pub mod parse;
mod units;
pub mod value;

pub use eval::{Context, Names, evaluate, parse_quantity};
pub use functions::Functions;
pub use limits::ExprLimits;
pub use parse::{BinaryOp, Expr, ExprKind, UnaryOp};
pub use value::Value;

/// A byte range of the source text: `start` inclusive, `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// The offset of the first byte.
    pub start: usize,
    /// The offset after the last byte.
    pub end: usize,
}

impl Span {
    /// The span from the start of `self` to the end of `other`.
    #[must_use]
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start,
            end: other.end,
        }
    }
}
