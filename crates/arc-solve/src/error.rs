//! The error codes owned by `arc-solve`, and the source span every expression error carries.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-solve.ftl`. An error about an expression carries the byte span of the
//! offending sub-expression as the details `start` and `end`; read it with [`span_of`].

use arc_base::error::Value as Detail;
use arc_base::{Error, ErrorCode};

use crate::Span;

/// The error codes owned by `arc-solve`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// The text is not a well-formed expression.
    pub const EXPR_SYNTAX: ErrorCode = ErrorCode {
        code: "solve.expr.syntax",
        category: Category::InvalidInput,
        message_id: "solve-expr-syntax",
    };
    /// A unit suffix names no built-in unit and no product of built-in units.
    pub const EXPR_UNKNOWN_UNIT: ErrorCode = ErrorCode {
        code: "solve.expr.unknown_unit",
        category: Category::NotFound,
        message_id: "solve-expr-unknown-unit",
    };
    /// A unit with an offset (a temperature scale) is raised to a power or combined with other
    /// units.
    pub const EXPR_OFFSET_UNIT: ErrorCode = ErrorCode {
        code: "solve.expr.offset_unit",
        category: Category::InvalidInput,
        message_id: "solve-expr-offset-unit",
    };
    /// A call names neither a built-in nor a registered function.
    pub const EXPR_UNKNOWN_FUNCTION: ErrorCode = ErrorCode {
        code: "solve.expr.unknown_function",
        category: Category::NotFound,
        message_id: "solve-expr-unknown-function",
    };
    /// A name is not known to the caller's name lookup.
    pub const EXPR_UNKNOWN_NAME: ErrorCode = ErrorCode {
        code: "solve.expr.unknown_name",
        category: Category::NotFound,
        message_id: "solve-expr-unknown-name",
    };
    /// Operands or arguments have dimensions the operation cannot combine, or the result does
    /// not have the dimension the caller expects.
    pub const EXPR_DIMENSION_MISMATCH: ErrorCode = ErrorCode {
        code: "solve.expr.dimension_mismatch",
        category: Category::InvalidInput,
        message_id: "solve-expr-dimension-mismatch",
    };
    /// An operand or argument is a value of the wrong kind (number, truth value, text or list).
    pub const EXPR_TYPE_MISMATCH: ErrorCode = ErrorCode {
        code: "solve.expr.type_mismatch",
        category: Category::InvalidInput,
        message_id: "solve-expr-type-mismatch",
    };
    /// A function was called with too few or too many arguments.
    pub const EXPR_ARGUMENT_COUNT: ErrorCode = ErrorCode {
        code: "solve.expr.argument_count",
        category: Category::InvalidInput,
        message_id: "solve-expr-argument-count",
    };
    /// An argument is outside the domain of the operation, or the result is not a finite
    /// number.
    pub const EXPR_DOMAIN: ErrorCode = ErrorCode {
        code: "solve.expr.domain",
        category: Category::InvalidInput,
        message_id: "solve-expr-domain",
    };
    /// The input length, nesting depth, a list length, a text length, the evaluation steps or a
    /// dimension exponent exceeded its limit.
    pub const EXPR_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "solve.expr.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "solve-expr-limit-exceeded",
    };
    /// A decimal separator is not usable in expressions.
    pub const EXPR_SEPARATOR_INVALID: ErrorCode = ErrorCode {
        code: "solve.expr.separator_invalid",
        category: Category::InvalidInput,
        message_id: "solve-expr-separator-invalid",
    };
    /// A host function name is not an identifier or is already taken, or its arity range is
    /// empty.
    pub const FUNCTION_INVALID: ErrorCode = ErrorCode {
        code: "solve.function.invalid",
        category: Category::InvalidInput,
        message_id: "solve-function-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        EXPR_SYNTAX,
        EXPR_UNKNOWN_UNIT,
        EXPR_OFFSET_UNIT,
        EXPR_UNKNOWN_FUNCTION,
        EXPR_UNKNOWN_NAME,
        EXPR_DIMENSION_MISMATCH,
        EXPR_TYPE_MISMATCH,
        EXPR_ARGUMENT_COUNT,
        EXPR_DOMAIN,
        EXPR_LIMIT_EXCEEDED,
        EXPR_SEPARATOR_INVALID,
        FUNCTION_INVALID,
    ];
}

fn offset(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Adds the span details `start` and `end` to an error.
pub(crate) fn with_span(error: Error, span: Span) -> Error {
    error
        .with_detail("start", offset(span.start))
        .with_detail("end", offset(span.end))
}

/// An error with `code` about the source text in `span`.
pub(crate) fn at(code: ErrorCode, span: Span) -> Error {
    with_span(Error::new(code), span)
}

/// `solve.expr.limit_exceeded` naming the exceeded limit in the detail `limit`.
pub(crate) fn limit(name: &'static str, span: Span) -> Error {
    at(codes::EXPR_LIMIT_EXCEEDED, span).with_detail("limit", name)
}

/// The byte span an expression error carries, if it carries one.
#[must_use]
pub fn span_of(error: &Error) -> Option<Span> {
    let read = |name| match error.details().get(name) {
        Some(Detail::Int(value)) => usize::try_from(*value).ok(),
        _ => None,
    };
    Some(Span {
        start: read("start")?,
        end: read("end")?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::*;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-solve.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-solve.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_arc_base() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(codes::ALL.iter().all(|e| e.code.starts_with("solve.")));
    }

    #[test]
    fn english_catalog_contains_every_registered_message() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(messages.contains(entry.message_id), "{}", entry.code);
        }
    }

    #[test]
    fn every_code_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(
                SPECIFICATION.contains(&cell),
                "{cell} is not in docs/crates/arc-solve.md"
            );
        }
    }

    #[test]
    fn spans_round_trip_through_error_details() {
        let span = Span { start: 3, end: 9 };
        assert_eq!(span_of(&at(codes::EXPR_SYNTAX, span)), Some(span));
        assert_eq!(span_of(&Error::new(codes::EXPR_SYNTAX)), None);
    }
}
