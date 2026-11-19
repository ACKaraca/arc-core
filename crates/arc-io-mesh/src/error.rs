//! The error codes and loss reasons owned by `arc-io-mesh`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items use
//! the reasons in [`reasons`]. Messages are in `locales/en-US/arc-io-mesh.ftl`.

use arc_base::{Error, ErrorCode};

/// Builds an error that carries a one-based line number as the detail `line`.
pub(crate) fn at_line(code: ErrorCode, line: u64) -> Error {
    Error::new(code).with_detail("line", clamp(line))
}

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

/// The error codes owned by `arc-io-mesh`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "io.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "io-limit-exceeded",
    };
    /// The input ends inside a record, a line, a list or a declared element.
    pub const MESH_TRUNCATED: ErrorCode = ErrorCode {
        code: "io.mesh.truncated",
        category: Category::CorruptData,
        message_id: "io-mesh-truncated",
    };
    /// A value does not parse as its type or is not a finite number.
    pub const MESH_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.value_invalid",
        category: Category::CorruptData,
        message_id: "io-mesh-value-invalid",
    };
    /// A face refers to a vertex that does not exist.
    pub const MESH_INDEX_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "io.mesh.index_out_of_range",
        category: Category::CorruptData,
        message_id: "io-mesh-index-out-of-range",
    };
    /// A face has fewer than three corners.
    pub const MESH_FACE_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.face_invalid",
        category: Category::CorruptData,
        message_id: "io-mesh-face-invalid",
    };
    /// Statements, records or blocks are not arranged as the format requires.
    pub const MESH_STRUCTURE_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.structure_invalid",
        category: Category::CorruptData,
        message_id: "io-mesh-structure-invalid",
    };
    /// The header of a PLY file is malformed or names an unknown format or version.
    pub const MESH_HEADER_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.header_invalid",
        category: Category::CorruptData,
        message_id: "io-mesh-header-invalid",
    };
    /// Text of the file is not valid UTF-8.
    pub const MESH_TEXT_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.text_invalid",
        category: Category::CorruptData,
        message_id: "io-mesh-text-invalid",
    };
    /// The writer was asked for an encoding the format does not have.
    pub const MESH_ENCODING_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "io.mesh.encoding_unsupported",
        category: Category::Unsupported,
        message_id: "io-mesh-encoding-unsupported",
    };
    /// A model value cannot be written: a number is not finite or does not fit the format, or a
    /// name contains a line break or a character the format cannot hold.
    pub const MESH_WRITE_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.mesh.write_value_invalid",
        category: Category::InvalidInput,
        message_id: "io-mesh-write-value-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        LIMIT_EXCEEDED,
        MESH_TRUNCATED,
        MESH_VALUE_INVALID,
        MESH_INDEX_OUT_OF_RANGE,
        MESH_FACE_INVALID,
        MESH_STRUCTURE_INVALID,
        MESH_HEADER_INVALID,
        MESH_TEXT_INVALID,
        MESH_ENCODING_UNSUPPORTED,
        MESH_WRITE_VALUE_INVALID,
    ];
}

/// The stable reason codes of loss-report items produced by `arc-io-mesh`. The item's record
/// kind names the element, property or attribute concerned.
pub mod reasons {
    use crate::loss::{Outcome, Reason, Severity};

    /// Elements or parts of a kind the model does not hold were skipped.
    pub const ELEMENT_SKIPPED: Reason = Reason {
        code: "io.mesh.element_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-mesh-loss-element-skipped",
    };
    /// Properties of a known element that the model does not hold were skipped.
    pub const PROPERTY_SKIPPED: Reason = Reason {
        code: "io.mesh.property_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-mesh-loss-property-skipped",
    };
    /// Polygons with more than three corners were split into triangles by a fan.
    pub const POLYGON_TRIANGULATED: Reason = Reason {
        code: "io.mesh.polygon_triangulated",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "io-mesh-loss-polygon-triangulated",
    };
    /// An attribute or name the target format cannot hold was dropped.
    pub const ATTRIBUTE_DROPPED: Reason = Reason {
        code: "io.mesh.attribute_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-mesh-loss-attribute-dropped",
    };
    /// Some corners of a part lack an attribute, so the attribute was not kept for the part.
    pub const ATTRIBUTE_INCOMPLETE: Reason = Reason {
        code: "io.mesh.attribute_incomplete",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-mesh-loss-attribute-incomplete",
    };
    /// Values were rounded to the precision of the target format.
    pub const PRECISION_REDUCED: Reason = Reason {
        code: "io.mesh.precision_reduced",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-mesh-loss-precision-reduced",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        ELEMENT_SKIPPED,
        PROPERTY_SKIPPED,
        POLYGON_TRIANGULATED,
        ATTRIBUTE_DROPPED,
        ATTRIBUTE_INCOMPLETE,
        PRECISION_REDUCED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-io-mesh.ftl");
    const CATALOG_TR: &str = include_str!("../../../locales/tr-TR/arc-io-mesh.ftl");
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
            assert!(reason.code.starts_with("io.mesh."), "{}", reason.code);
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
