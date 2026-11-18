//! The error codes and loss reasons owned by `arc-io-pdf`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items use
//! the reasons in [`reasons`]. Messages are in `locales/en-US/arc-io-pdf.ftl`.

use arc_base::Error;

/// The error `io.limit_exceeded` naming the field of [`arc_base::Limits`] that was exceeded.
pub(crate) fn limit(name: &'static str) -> Error {
    Error::new(codes::LIMIT_EXCEEDED).with_arg("limit", name)
}

/// The error `io.pdf.option_invalid` naming the option.
pub(crate) fn option(name: &'static str) -> Error {
    Error::new(codes::OPTION_INVALID).with_arg("option", name)
}

/// The error `io.pdf.stream_invalid` naming what is wrong.
pub(crate) fn stream(what: &'static str) -> Error {
    Error::new(codes::STREAM_INVALID).with_arg("what", what)
}

/// The error codes owned by `arc-io-pdf`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "io.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "io-pdf-limit-exceeded",
    };
    /// An option is out of range or malformed; the argument `option` names it.
    pub const OPTION_INVALID: ErrorCode = ErrorCode {
        code: "io.pdf.option_invalid",
        category: Category::InvalidInput,
        message_id: "io-pdf-option-invalid",
    };
    /// A plot stream cannot be written: a page size, coordinate or clip index is out of range;
    /// the argument `what` names it.
    pub const STREAM_INVALID: ErrorCode = ErrorCode {
        code: "io.pdf.stream_invalid",
        category: Category::InvalidInput,
        message_id: "io-pdf-stream-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[LIMIT_EXCEEDED, OPTION_INVALID, STREAM_INVALID];
}

/// The stable reason codes of loss-report items produced by `arc-io-pdf`.
pub mod reasons {
    use crate::loss::{Outcome, Reason, Severity};

    /// Items written without change.
    pub const ITEMS_WRITTEN: Reason = Reason {
        code: "io.pdf.items_written",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "io-pdf-loss-items-written",
    };
    /// Outline glyph runs were not written; stroke text is the supported path.
    pub const GLYPHS_DROPPED: Reason = Reason {
        code: "io.pdf.glyphs_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-pdf-loss-glyphs-dropped",
    };
    /// NURBS curves were written as polylines within the chord tolerance.
    pub const NURBS_FLATTENED: Reason = Reason {
        code: "io.pdf.nurbs_flattened",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "io-pdf-loss-nurbs-flattened",
    };
    /// Circular and elliptical arcs were written as cubic Bezier curves within the chord
    /// tolerance (PDF has no arc operator).
    pub const ARCS_APPROXIMATED: Reason = Reason {
        code: "io.pdf.arcs_approximated",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "io-pdf-loss-arcs-approximated",
    };
    /// A polyline with vertex widths was stroked with one width instead of a taper.
    pub const POLYLINE_WIDTH_APPROXIMATED: Reason = Reason {
        code: "io.pdf.polyline_width_approximated",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-pdf-loss-polyline-width-approximated",
    };
    /// Items that could not be written: an unbounded curve without pieces, or a curve that
    /// cannot be flattened.
    pub const ITEMS_FAILED: Reason = Reason {
        code: "io.pdf.items_failed",
        outcome: Outcome::Failed,
        severity: Severity::Error,
        message_id: "io-pdf-loss-items-failed",
    };
    /// Entities the plot stream already left out because they could not be listed, mapped or
    /// cut.
    pub const SOURCES_FAILED: Reason = Reason {
        code: "io.pdf.sources_failed",
        outcome: Outcome::Failed,
        severity: Severity::Error,
        message_id: "io-pdf-loss-sources-failed",
    };
    /// Layers have no optional content group in SVG output and were not written.
    pub const LAYERS_DROPPED: Reason = Reason {
        code: "io.pdf.layers_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-pdf-loss-layers-dropped",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        ITEMS_WRITTEN,
        GLYPHS_DROPPED,
        NURBS_FLATTENED,
        ARCS_APPROXIMATED,
        POLYLINE_WIDTH_APPROXIMATED,
        ITEMS_FAILED,
        SOURCES_FAILED,
        LAYERS_DROPPED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-pdf.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-io.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_arc_base() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(codes::ALL.iter().all(|entry| entry.code.starts_with("io.")));
    }

    #[test]
    fn reasons_are_unique_and_distinct_from_error_codes() {
        let mut seen = BTreeSet::new();
        for reason in reasons::ALL {
            assert!(reason.code.starts_with("io.pdf."), "{}", reason.code);
            assert!(seen.insert(reason.code), "{} twice", reason.code);
            assert!(
                seen.insert(reason.message_id),
                "{} twice",
                reason.message_id
            );
        }
        for entry in codes::ALL {
            assert!(!seen.contains(entry.code) && !seen.contains(entry.message_id));
        }
    }

    #[test]
    fn english_catalog_contains_every_message() {
        let ids: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for id in codes::ALL
            .iter()
            .map(|c| c.message_id)
            .chain(reasons::ALL.iter().map(|r| r.message_id))
        {
            assert!(ids.contains(id), "{id} is not in the catalog");
        }
    }

    #[test]
    fn every_code_and_reason_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
        for reason in reasons::ALL {
            let outcome = serde_json::to_value(reason.outcome).unwrap();
            let cell = format!("| `{}` | `{}` |", reason.code, outcome.as_str().unwrap());
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
    }
}
