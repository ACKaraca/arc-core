//! The error codes and loss reasons owned by `arc-io-dxf`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items use
//! the reasons in [`reasons`]. Messages are in `locales/en-US/arc-io-dxf.ftl`.

use arc_base::{Error, ErrorCode};

/// Where in the input a problem was found: a line of an ASCII file or a byte offset of a binary
/// file, of the group or record concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// One-based line number of a group-code line.
    Line(u64),
    /// Zero-based byte offset.
    Offset(u64),
}

impl Position {
    /// Builds an error that carries this position as the detail `line` or `offset`.
    pub(crate) fn error(self, code: ErrorCode) -> Error {
        match self {
            Self::Line(line) => Error::new(code).with_detail("line", clamp(line)),
            Self::Offset(offset) => Error::new(code).with_detail("offset", clamp(offset)),
        }
    }
}

fn clamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The error `io.limit_exceeded` naming the field of [`arc_base::Limits`] that was exceeded.
pub(crate) fn limit(name: &'static str, value: u64) -> Error {
    Error::new(codes::LIMIT_EXCEEDED)
        .with_arg("limit", name)
        .with_detail("value", clamp(value))
}

/// The error codes owned by `arc-io-dxf`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it. The
    /// code and its message are the ones of `arc-io-common`, shared by every reader and writer.
    pub use arc_io_common::error::codes::LIMIT_EXCEEDED;
    /// The file declares a format version this crate does not read, or a write targets one it
    /// does not write.
    pub const UNSUPPORTED_VERSION: ErrorCode = ErrorCode {
        code: "io.unsupported_version",
        category: Category::Unsupported,
        message_id: "io-unsupported-version",
    };
    /// Reading from a stream failed.
    pub const STREAM_FAILED: ErrorCode = ErrorCode {
        code: "io.stream_failed",
        category: Category::Io,
        message_id: "io-stream-failed",
    };
    /// The input ends inside a group, section, table, block or entity sequence.
    pub const DXF_TRUNCATED: ErrorCode = ErrorCode {
        code: "io.dxf.truncated",
        category: Category::CorruptData,
        message_id: "io-dxf-truncated",
    };
    /// A group code is not an integer, is negative, or is unknown in a binary file.
    pub const DXF_GROUP_CODE_INVALID: ErrorCode = ErrorCode {
        code: "io.dxf.group_code_invalid",
        category: Category::CorruptData,
        message_id: "io-dxf-group-code-invalid",
    };
    /// A group value does not parse as its group code's type, is out of range, or is not finite.
    pub const DXF_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.dxf.value_invalid",
        category: Category::CorruptData,
        message_id: "io-dxf-value-invalid",
    };
    /// Sections, tables, blocks or records are not arranged as the format requires.
    pub const DXF_STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.dxf.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-dxf-structure-invalid",
    };
    /// Text of a version 2007 or later file is not valid UTF-8.
    pub const DXF_TEXT_INVALID: ErrorCode = ErrorCode {
        code: "io.dxf.text_invalid",
        category: Category::CorruptData,
        message_id: "io-dxf-text-invalid",
    };
    /// A model value cannot be written: a number is not finite, an integer does not fit its group
    /// code, a value does not match its group code's type, or a binary chunk is too long.
    pub const DXF_WRITE_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.dxf.write_value_invalid",
        category: Category::InvalidInput,
        message_id: "io-dxf-write-value-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        UNSUPPORTED_VERSION,
        STREAM_FAILED,
        DXF_TRUNCATED,
        DXF_GROUP_CODE_INVALID,
        DXF_VALUE_INVALID,
        DXF_STRUCTURE_INVALID,
        DXF_TEXT_INVALID,
        DXF_WRITE_VALUE_INVALID,
    ];
}

/// The stable reason codes of loss-report items produced by `arc-io-dxf`.
pub mod reasons {
    use crate::loss::{Outcome, Reason, Severity};

    /// A section the model does not type was kept as preserved records.
    pub const SECTION_PRESERVED: Reason = Reason {
        code: "io.dxf.section_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-dxf-loss-section-preserved",
    };
    /// A symbol table the model does not type was kept as preserved records.
    pub const TABLE_PRESERVED: Reason = Reason {
        code: "io.dxf.table_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-dxf-loss-table-preserved",
    };
    /// An entity, object or table entry type the model does not type was kept as a preserved
    /// record.
    pub const RECORD_PRESERVED: Reason = Reason {
        code: "io.dxf.record_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-dxf-loss-record-preserved",
    };
    /// Groups of a modeled record that the model does not type were kept as preserved groups.
    pub const GROUPS_PRESERVED: Reason = Reason {
        code: "io.dxf.groups_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-dxf-loss-groups-preserved",
    };
    /// Comment groups (code 999) were skipped.
    pub const COMMENT_DROPPED: Reason = Reason {
        code: "io.dxf.comment_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-dxf-loss-comment-dropped",
    };
    /// The legacy code page is not implemented; non-ASCII bytes were kept as raw-byte characters.
    pub const CODE_PAGE_UNSUPPORTED: Reason = Reason {
        code: "io.dxf.code_page_unsupported",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dxf-loss-code-page-unsupported",
    };
    /// Bytes without a character in the code page were kept as raw-byte characters.
    pub const BYTE_UNMAPPED: Reason = Reason {
        code: "io.dxf.byte_unmapped",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dxf-loss-byte-unmapped",
    };
    /// Spline fit data of hatch boundary edges was not written because the target version has
    /// no place for it.
    pub const FIT_DATA_DROPPED: Reason = Reason {
        code: "io.dxf.fit_data_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-dxf-loss-fit-data-dropped",
    };
    /// Raw-byte characters were written as Unicode characters of a UTF-8 file.
    pub const RAW_BYTES_WRITTEN: Reason = Reason {
        code: "io.dxf.raw_bytes_written",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dxf-loss-raw-bytes-written",
    };
    /// Records without a handle, or with the handle of an earlier record, received a new handle.
    pub const HANDLE_ASSIGNED: Reason = Reason {
        code: "io.dxf.handle_assigned",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "io-dxf-loss-handle-assigned",
    };
    /// The model lacks the block record table that files of the target version require; the
    /// writer does not create it.
    pub const STRUCTURE_INCOMPLETE: Reason = Reason {
        code: "io.dxf.structure_incomplete",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-dxf-loss-structure-incomplete",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        SECTION_PRESERVED,
        TABLE_PRESERVED,
        RECORD_PRESERVED,
        GROUPS_PRESERVED,
        COMMENT_DROPPED,
        CODE_PAGE_UNSUPPORTED,
        BYTE_UNMAPPED,
        FIT_DATA_DROPPED,
        RAW_BYTES_WRITTEN,
        HANDLE_ASSIGNED,
        STRUCTURE_INCOMPLETE,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-dxf.ftl");
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
        assert!(codes::ALL.iter().all(|entry| entry.code.starts_with("io.")));
    }

    #[test]
    fn reasons_are_unique_and_distinct_from_error_codes() {
        let mut seen = BTreeSet::new();
        for reason in reasons::ALL {
            assert!(reason.code.starts_with("io.dxf."), "{}", reason.code);
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
