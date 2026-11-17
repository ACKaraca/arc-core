//! The error codes owned by `arc-geom`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-geom.ftl`.

/// The error codes owned by `arc-geom`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A coordinate, length, angle, factor or transform entry is NaN or infinite.
    pub const INPUT_NOT_FINITE: ErrorCode = ErrorCode {
        code: "geom.input.not_finite",
        category: Category::InvalidInput,
        message_id: "geom-input-not-finite",
    };
    /// A segment starts and ends at the same point.
    pub const SEGMENT_ZERO_LENGTH: ErrorCode = ErrorCode {
        code: "geom.segment.zero_length",
        category: Category::InvalidInput,
        message_id: "geom-segment-zero-length",
    };
    /// The direction of a ray or line is the zero vector.
    pub const DIRECTION_ZERO: ErrorCode = ErrorCode {
        code: "geom.direction.zero",
        category: Category::InvalidInput,
        message_id: "geom-direction-zero",
    };
    /// The radius of a circle or arc is not positive.
    pub const RADIUS_NOT_POSITIVE: ErrorCode = ErrorCode {
        code: "geom.radius.not_positive",
        category: Category::InvalidInput,
        message_id: "geom-radius-not-positive",
    };
    /// The sweep of an arc is zero or larger than a full turn.
    pub const SWEEP_INVALID: ErrorCode = ErrorCode {
        code: "geom.sweep.invalid",
        category: Category::InvalidInput,
        message_id: "geom-sweep-invalid",
    };
    /// An ellipse has a zero major axis or a minor-to-major ratio outside (0, 1].
    pub const ELLIPSE_INVALID_AXIS: ErrorCode = ErrorCode {
        code: "geom.ellipse.invalid_axis",
        category: Category::InvalidInput,
        message_id: "geom-ellipse-invalid-axis",
    };
    /// A polyline has fewer than two vertices.
    pub const POLYLINE_TOO_FEW_VERTICES: ErrorCode = ErrorCode {
        code: "geom.polyline.too_few_vertices",
        category: Category::InvalidInput,
        message_id: "geom-polyline-too-few-vertices",
    };
    /// Two consecutive vertices of a polyline coincide.
    pub const POLYLINE_ZERO_LENGTH_SEGMENT: ErrorCode = ErrorCode {
        code: "geom.polyline.zero_length_segment",
        category: Category::InvalidInput,
        message_id: "geom-polyline-zero-length-segment",
    };
    /// A polyline width is negative.
    pub const POLYLINE_NEGATIVE_WIDTH: ErrorCode = ErrorCode {
        code: "geom.polyline.negative_width",
        category: Category::InvalidInput,
        message_id: "geom-polyline-negative-width",
    };
    /// A bounding box has a minimum corner above its maximum corner.
    pub const BBOX_INVALID: ErrorCode = ErrorCode {
        code: "geom.bbox.invalid",
        category: Category::InvalidInput,
        message_id: "geom-bbox-invalid",
    };
    /// A transform is not invertible.
    pub const TRANSFORM_SINGULAR: ErrorCode = ErrorCode {
        code: "geom.transform.singular",
        category: Category::InvalidInput,
        message_id: "geom-transform-singular",
    };
    /// A polyline with arcs or widths was given a transform that is not a similarity.
    pub const TRANSFORM_NOT_SIMILARITY: ErrorCode = ErrorCode {
        code: "geom.transform.not_similarity",
        category: Category::Unsupported,
        message_id: "geom-transform-not-similarity",
    };
    /// A computation exceeded the range or precision of `f64`.
    pub const PRECISION_INSUFFICIENT: ErrorCode = ErrorCode {
        code: "geom.precision.insufficient",
        category: Category::FailedOperation,
        message_id: "geom-precision-insufficient",
    };
    /// Encoded geometry is malformed or oversized.
    pub const DECODE_INVALID: ErrorCode = ErrorCode {
        code: "geom.decode.invalid",
        category: Category::CorruptData,
        message_id: "geom-decode-invalid",
    };
    /// A linear or angular tolerance is NaN, infinite or negative.
    pub const TOLERANCE_INVALID: ErrorCode = ErrorCode {
        code: "geom.tolerance.invalid",
        category: Category::InvalidInput,
        message_id: "geom-tolerance-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        INPUT_NOT_FINITE,
        SEGMENT_ZERO_LENGTH,
        DIRECTION_ZERO,
        RADIUS_NOT_POSITIVE,
        SWEEP_INVALID,
        ELLIPSE_INVALID_AXIS,
        POLYLINE_TOO_FEW_VERTICES,
        POLYLINE_ZERO_LENGTH_SEGMENT,
        POLYLINE_NEGATIVE_WIDTH,
        BBOX_INVALID,
        TRANSFORM_SINGULAR,
        TRANSFORM_NOT_SIMILARITY,
        PRECISION_INSUFFICIENT,
        DECODE_INVALID,
        TOLERANCE_INVALID,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-geom.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-geom.md");

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
                .all(|entry| entry.code.starts_with("geom."))
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
                "{cell} is not in docs/crates/arc-geom.md"
            );
        }
    }
}
