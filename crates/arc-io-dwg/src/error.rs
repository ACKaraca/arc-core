//! The error codes and loss reasons owned by `arc-io-dwg`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items use
//! the reasons in [`reasons`]. Messages are in `locales/en-US/arc-io-dwg.ftl`. The limit code is
//! the shared `io.limit_exceeded` of `arc-io-dxf`, and cancellation is `base.cancelled`.

use arc_base::Error;
use arc_io_dxf::loss::{Outcome, Reason, Severity};

/// The error `io.limit_exceeded` naming the field of [`arc_base::Limits`] that was exceeded.
pub(crate) fn limit(name: &'static str, value: u64) -> Error {
    Error::new(arc_io_dxf::error::codes::LIMIT_EXCEEDED)
        .with_arg("limit", name)
        .with_detail("value", i64::try_from(value).unwrap_or(i64::MAX))
}

/// The error codes owned by `arc-io-dwg`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// The input does not start with a DWG version signature (`AC` and four digits).
    pub const SIGNATURE_INVALID: ErrorCode = ErrorCode {
        code: "io.dwg.signature_invalid",
        category: Category::CorruptData,
        message_id: "io-dwg-signature-invalid",
    };
    /// The version signature is well formed but names a version this crate does not read.
    pub const VERSION_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "io.dwg.version_unsupported",
        category: Category::Unsupported,
        message_id: "io-dwg-version-unsupported",
    };
    /// The input ends before the structure the file declares is complete.
    pub const TRUNCATED: ErrorCode = ErrorCode {
        code: "io.dwg.truncated",
        category: Category::CorruptData,
        message_id: "io-dwg-truncated",
    };
    /// The file's structures are malformed, or refer to records that do not exist.
    pub const STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.dwg.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-dwg-structure-invalid",
    };
    /// A checksum stored in the file does not match the data it covers.
    pub const CHECKSUM_MISMATCH: ErrorCode = ErrorCode {
        code: "io.dwg.checksum_mismatch",
        category: Category::CorruptData,
        message_id: "io-dwg-checksum-mismatch",
    };
    /// The wrapped library stopped with a panic while reading the file; the read is abandoned.
    pub const LIBRARY_FAILED: ErrorCode = ErrorCode {
        code: "io.dwg.library_failed",
        category: Category::Internal,
        message_id: "io-dwg-library-failed",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        SIGNATURE_INVALID,
        VERSION_UNSUPPORTED,
        TRUNCATED,
        STRUCTURE_INVALID,
        CHECKSUM_MISMATCH,
        LIBRARY_FAILED,
    ];
}

/// The stable reason codes of loss-report items produced by `arc-io-dwg`.
pub mod reasons {
    use super::{Outcome, Reason, Severity};

    /// Header variables other than the ones the drawing model carries were not kept.
    pub const HEADER_DROPPED: Reason = Reason {
        code: "io.dwg.header_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-dwg-loss-header-dropped",
    };
    /// A symbol table the drawing model does not carry was not kept.
    pub const TABLE_DROPPED: Reason = Reason {
        code: "io.dwg.table_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-dwg-loss-table-dropped",
    };
    /// Table entries were kept, but some of their data is not carried by the drawing model.
    pub const TABLE_ENTRY_PARTIAL: Reason = Reason {
        code: "io.dwg.table_entry_partial",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dwg-loss-table-entry-partial",
    };
    /// Non-graphical objects of the file were not kept.
    pub const OBJECT_DROPPED: Reason = Reason {
        code: "io.dwg.object_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-dwg-loss-object-dropped",
    };
    /// An entity of a type the drawing model does not carry, or one outside any block, was not
    /// kept.
    pub const ENTITY_DROPPED: Reason = Reason {
        code: "io.dwg.entity_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-dwg-loss-entity-dropped",
    };
    /// Entities were kept, but some of their data is not carried by the drawing model.
    pub const ENTITY_PARTIAL: Reason = Reason {
        code: "io.dwg.entity_partial",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dwg-loss-entity-partial",
    };
    /// A record had a value the drawing model does not accept (for example a number that is not
    /// finite), so the record was skipped.
    pub const VALUE_SKIPPED: Reason = Reason {
        code: "io.dwg.value_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-dwg-loss-value-skipped",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        HEADER_DROPPED,
        TABLE_DROPPED,
        TABLE_ENTRY_PARTIAL,
        OBJECT_DROPPED,
        ENTITY_DROPPED,
        ENTITY_PARTIAL,
        VALUE_SKIPPED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-dwg.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-io.md");

    fn catalog_ids() -> BTreeSet<&'static str> {
        CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn registry_has_no_problems_alone_or_with_arc_base() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(
            codes::ALL
                .iter()
                .all(|entry| entry.code.starts_with("io.dwg."))
        );
    }

    #[test]
    fn reasons_are_unique_and_distinct_from_error_codes() {
        let mut seen = BTreeSet::new();
        for reason in reasons::ALL {
            assert!(reason.code.starts_with("io.dwg."), "{}", reason.code);
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
        let ids = catalog_ids();
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
            let cell = format!("| `{}` | `{}` |", reason.code, reason.outcome.as_str());
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
    }
}
