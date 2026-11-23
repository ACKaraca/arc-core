//! The error codes owned by `arc-view`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-view.ftl`.

/// The error codes owned by `arc-view`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A view definition parameter is out of range or not finite.
    pub const DEFINITION_INVALID: ErrorCode = ErrorCode {
        code: "view.definition.invalid",
        category: Category::InvalidInput,
        message_id: "view-definition-invalid",
    };
    /// The objects, triangles, edges, occlusion tests or primitives of a view exceed its limits.
    pub const LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "view.limit.exceeded",
        category: Category::LimitExceeded,
        message_id: "view-limit-exceeded",
    };
    /// An object has a coordinate outside the range a view accepts.
    pub const GEOMETRY_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "view.geometry.out_of_range",
        category: Category::InvalidInput,
        message_id: "view-geometry-out-of-range",
    };
    /// The section of an object at the cut plane is not closed.
    pub const SECTION_OPEN: ErrorCode = ErrorCode {
        code: "view.section.open",
        category: Category::FailedOperation,
        message_id: "view-section-open",
    };
    /// An object part has no triangle higher than the linear tolerance.
    pub const GEOMETRY_DEGENERATE: ErrorCode = ErrorCode {
        code: "view.geometry.degenerate",
        category: Category::InvalidInput,
        message_id: "view-geometry-degenerate",
    };
    /// A line style, cut fill or symbol of an object is invalid and was replaced by a default.
    pub const STYLE_INVALID: ErrorCode = ErrorCode {
        code: "view.style.invalid",
        category: Category::InvalidInput,
        message_id: "view-style-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        DEFINITION_INVALID,
        LIMIT_EXCEEDED,
        GEOMETRY_OUT_OF_RANGE,
        SECTION_OPEN,
        GEOMETRY_DEGENERATE,
        STYLE_INVALID,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-view.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-view.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_its_dependencies() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(arc_geom::error::codes::ALL)
            .chain(arc_mesh::error::codes::ALL)
            .chain(arc_doc::error::codes::ALL)
            .chain(arc_draft::error::codes::ALL)
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(
            codes::ALL
                .iter()
                .all(|entry| entry.code.starts_with("view."))
        );
    }

    #[test]
    fn english_catalog_contains_every_registered_message() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(
                messages.contains(entry.message_id),
                "{} is not in the catalog",
                entry.code
            );
        }
    }

    #[test]
    fn every_code_is_documented_in_the_specification() {
        for entry in codes::ALL {
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(
                SPECIFICATION.contains(&cell),
                "{cell} is not in docs/crates/arc-view.md"
            );
        }
    }
}
