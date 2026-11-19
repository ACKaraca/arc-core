//! The error codes and loss reasons owned by `arc-io-ifc`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items use
//! the reasons in [`reasons`]. Messages are in `locales/en-US/arc-io-ifc.ftl`.

use arc_base::{Error, ErrorCode};

/// Builds an error that carries a zero-based byte offset as the detail `offset`.
pub(crate) fn at_offset(code: ErrorCode, offset: u64) -> Error {
    Error::new(code).with_detail("offset", clamp(offset))
}

/// The error `io.limit_exceeded` naming the field of [`arc_base::Limits`] that was exceeded.
pub(crate) fn limit(name: &'static str, value: u64) -> Error {
    Error::new(codes::LIMIT_EXCEEDED)
        .with_arg("limit", name)
        .with_detail("value", clamp(value))
}

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The error codes owned by `arc-io-ifc`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "io.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "io-limit-exceeded",
    };
    /// The input ends inside a comment, a string, a parameter list or a record.
    pub const IFC_TRUNCATED: ErrorCode = ErrorCode {
        code: "io.ifc.truncated",
        category: Category::CorruptData,
        message_id: "io-ifc-truncated",
    };
    /// A token does not fit the grammar of the physical file at its position.
    pub const IFC_SYNTAX: ErrorCode = ErrorCode {
        code: "io.ifc.syntax",
        category: Category::CorruptData,
        message_id: "io-ifc-syntax",
    };
    /// The sections of the file (header, data, schema) are missing, out of order or unterminated.
    pub const IFC_STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-ifc-structure-invalid",
    };
    /// A string holds bytes that are not text, an encoded form that is malformed, or a control character.
    pub const IFC_TEXT_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.text_invalid",
        category: Category::CorruptData,
        message_id: "io-ifc-text-invalid",
    };
    /// A number is out of range for its type, or a binary or enumeration token is malformed.
    pub const IFC_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.value_invalid",
        category: Category::CorruptData,
        message_id: "io-ifc-value-invalid",
    };
    /// Two instances carry the same name.
    pub const IFC_INSTANCE_DUPLICATE: ErrorCode = ErrorCode {
        code: "io.ifc.instance_duplicate",
        category: Category::CorruptData,
        message_id: "io-ifc-instance-duplicate",
    };
    /// The ZIP archive holds no entry whose name ends in `.ifc`.
    pub const IFC_ARCHIVE_ENTRY_MISSING: ErrorCode = ErrorCode {
        code: "io.ifc.archive_entry_missing",
        category: Category::NotFound,
        message_id: "io-ifc-archive-entry-missing",
    };
    /// The ZIP archive holds more than one entry whose name ends in `.ifc`.
    pub const IFC_ARCHIVE_ENTRY_AMBIGUOUS: ErrorCode = ErrorCode {
        code: "io.ifc.archive_entry_ambiguous",
        category: Category::Unsupported,
        message_id: "io-ifc-archive-entry-ambiguous",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        LIMIT_EXCEEDED,
        IFC_TRUNCATED,
        IFC_SYNTAX,
        IFC_STRUCTURE_INVALID,
        IFC_TEXT_INVALID,
        IFC_VALUE_INVALID,
        IFC_INSTANCE_DUPLICATE,
        IFC_ARCHIVE_ENTRY_MISSING,
        IFC_ARCHIVE_ENTRY_AMBIGUOUS,
    ];
}

/// The stable reason codes of loss-report items produced by `arc-io-ifc`. The item's record kind
/// names the entity type, attribute or section concerned.
pub mod reasons {
    use crate::loss::{Outcome, Reason, Severity};

    /// An entity type that the typed views do not read is kept as generic instances.
    pub const ENTITY_PRESERVED: Reason = Reason {
        code: "io.ifc.entity_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-ifc-loss-entity-preserved",
    };
    /// A reference names an instance that is not in the file; the reference is kept as it is.
    pub const REFERENCE_UNRESOLVED: Reason = Reason {
        code: "io.ifc.reference_unresolved",
        outcome: Outcome::Preserved,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-reference-unresolved",
    };
    /// The file names a schema that is not one of IFC2X3, IFC4 or IFC4X3; its entities are kept
    /// as generic instances without a schema-specific reading.
    pub const SCHEMA_UNRECOGNIZED: Reason = Reason {
        code: "io.ifc.schema_unrecognized",
        outcome: Outcome::Preserved,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-schema-unrecognized",
    };
    /// An attribute of a typed view does not hold the value the view expects; the value is not read.
    pub const VALUE_UNEXPECTED: Reason = Reason {
        code: "io.ifc.value_unexpected",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-value-unexpected",
    };
    /// A relationship would make an object contain itself; the edge is cut and not followed.
    pub const CYCLE_CUT: Reason = Reason {
        code: "io.ifc.cycle_cut",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-cycle-cut",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        ENTITY_PRESERVED,
        REFERENCE_UNRESOLVED,
        SCHEMA_UNRECOGNIZED,
        VALUE_UNEXPECTED,
        CYCLE_CUT,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-ifc.ftl");
    const CATALOG_TR: &str = include_str!("../../../locales/tr-TR/arc-io-ifc.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-io.md");

    fn catalog_ids(catalog: &str) -> BTreeSet<&str> {
        catalog
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
        assert!(codes::ALL.iter().all(|entry| entry.code.starts_with("io.")));
    }

    #[test]
    fn reasons_are_unique_and_distinct_from_error_codes() {
        let mut seen = BTreeSet::new();
        for reason in reasons::ALL {
            assert!(reason.code.starts_with("io.ifc."), "{}", reason.code);
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
    fn both_catalogs_contain_every_message() {
        let ids: Vec<&str> = codes::ALL
            .iter()
            .map(|c| c.message_id)
            .chain(reasons::ALL.iter().map(|r| r.message_id))
            .collect();
        for catalog in [CATALOG, CATALOG_TR] {
            let defined = catalog_ids(catalog);
            for id in &ids {
                assert!(defined.contains(id), "{id} is not in a catalog");
            }
            assert_eq!(defined.len(), ids.len(), "catalog and registry differ");
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
