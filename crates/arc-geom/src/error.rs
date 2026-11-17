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
    /// A tolerance is NaN, infinite, negative, or zero where the operation needs a positive one.
    pub const TOLERANCE_INVALID: ErrorCode = ErrorCode {
        code: "geom.tolerance.invalid",
        category: Category::InvalidInput,
        message_id: "geom-tolerance-invalid",
    };
    /// The eye and target of a view frame coincide.
    pub const VIEW_EYE_AT_TARGET: ErrorCode = ErrorCode {
        code: "geom.view.eye_at_target",
        category: Category::InvalidInput,
        message_id: "geom-view-eye-at-target",
    };
    /// The up vector of a view frame is zero or parallel to the view direction.
    pub const VIEW_UP_INVALID: ErrorCode = ErrorCode {
        code: "geom.view.up_invalid",
        category: Category::InvalidInput,
        message_id: "geom-view-up-invalid",
    };
    /// The field of view of a perspective view frame is outside (0, π).
    pub const VIEW_FOV_INVALID: ErrorCode = ErrorCode {
        code: "geom.view.fov_invalid",
        category: Category::InvalidInput,
        message_id: "geom-view-fov-invalid",
    };
    /// The height of an orthographic view frame is not positive.
    pub const VIEW_SCALE_NOT_POSITIVE: ErrorCode = ErrorCode {
        code: "geom.view.scale_not_positive",
        category: Category::InvalidInput,
        message_id: "geom-view-scale-not-positive",
    };
    /// The clip distances of a view frame are not ordered, or the near distance of a perspective frame is not positive.
    pub const VIEW_CLIP_INVALID: ErrorCode = ErrorCode {
        code: "geom.view.clip_invalid",
        category: Category::InvalidInput,
        message_id: "geom-view-clip-invalid",
    };
    /// An aspect ratio is not a finite positive number.
    pub const VIEW_ASPECT_INVALID: ErrorCode = ErrorCode {
        code: "geom.view.aspect_invalid",
        category: Category::InvalidInput,
        message_id: "geom-view-aspect-invalid",
    };

    /// A requested arc length is negative or exceeds the length of the curve.
    pub const LENGTH_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "geom.length.out_of_range",
        category: Category::InvalidInput,
        message_id: "geom-length-out-of-range",
    };
    /// A ray or line was given to an operation that needs a bounded curve.
    pub const CURVE_UNBOUNDED: ErrorCode = ErrorCode {
        code: "geom.curve.unbounded",
        category: Category::Unsupported,
        message_id: "geom-curve-unbounded",
    };
    /// Flattening a curve needs more vertices than the limit allows.
    pub const FLATTEN_TOO_MANY_VERTICES: ErrorCode = ErrorCode {
        code: "geom.flatten.too_many_vertices",
        category: Category::LimitExceeded,
        message_id: "geom-flatten-too-many-vertices",
    };
    /// Two curves have more intersections than the limit allows.
    pub const INTERSECT_TOO_MANY_RESULTS: ErrorCode = ErrorCode {
        code: "geom.intersect.too_many_results",
        category: Category::LimitExceeded,
        message_id: "geom-intersect-too-many-results",
    };

    /// A loop has no curves.
    pub const REGION_LOOP_EMPTY: ErrorCode = ErrorCode {
        code: "geom.region.loop_empty",
        category: Category::InvalidInput,
        message_id: "geom-region-loop-empty",
    };
    /// The end of a curve of a loop is farther than the linear tolerance from the start of the next.
    pub const REGION_LOOP_OPEN: ErrorCode = ErrorCode {
        code: "geom.region.loop_open",
        category: Category::InvalidInput,
        message_id: "geom-region-loop-open",
    };
    /// A loop encloses an area of exactly zero.
    pub const REGION_LOOP_ZERO_AREA: ErrorCode = ErrorCode {
        code: "geom.region.loop_zero_area",
        category: Category::InvalidInput,
        message_id: "geom-region-loop-zero-area",
    };
    /// A region has no parts.
    pub const REGION_EMPTY: ErrorCode = ErrorCode {
        code: "geom.region.empty",
        category: Category::InvalidInput,
        message_id: "geom-region-empty",
    };
    /// The bounding box of a hole is not inside that of its outer loop.
    pub const REGION_HOLE_OUTSIDE: ErrorCode = ErrorCode {
        code: "geom.region.hole_outside",
        category: Category::InvalidInput,
        message_id: "geom-region-hole-outside",
    };
    /// Edges of the flattened loops of a region cross, touch away from a shared vertex, or fold
    /// back.
    pub const TRIANGULATE_EDGES_CROSS: ErrorCode = ErrorCode {
        code: "geom.triangulate.edges_cross",
        category: Category::InvalidInput,
        message_id: "geom-triangulate-edges-cross",
    };
    /// A flattened loop has fewer than three vertices, no area, or the opposite orientation.
    pub const TRIANGULATE_LOOP_COLLAPSED: ErrorCode = ErrorCode {
        code: "geom.triangulate.loop_collapsed",
        category: Category::InvalidInput,
        message_id: "geom-triangulate-loop-collapsed",
    };
    /// The flattened loops of a region are not nested as its parts say.
    pub const TRIANGULATE_NESTING_INVALID: ErrorCode = ErrorCode {
        code: "geom.triangulate.nesting_invalid",
        category: Category::InvalidInput,
        message_id: "geom-triangulate-nesting-invalid",
    };
    /// A triangle mesh has a non-finite vertex, a triangle index outside its vertex list, or a
    /// triangle that is not strictly counterclockwise.
    pub const MESH_INVALID: ErrorCode = ErrorCode {
        code: "geom.mesh.invalid",
        category: Category::InvalidInput,
        message_id: "geom-mesh-invalid",
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
        VIEW_EYE_AT_TARGET,
        VIEW_UP_INVALID,
        VIEW_FOV_INVALID,
        VIEW_SCALE_NOT_POSITIVE,
        VIEW_CLIP_INVALID,
        VIEW_ASPECT_INVALID,
        LENGTH_OUT_OF_RANGE,
        CURVE_UNBOUNDED,
        FLATTEN_TOO_MANY_VERTICES,
        REGION_LOOP_EMPTY,
        REGION_LOOP_OPEN,
        REGION_LOOP_ZERO_AREA,
        REGION_EMPTY,
        REGION_HOLE_OUTSIDE,
        TRIANGULATE_EDGES_CROSS,
        TRIANGULATE_LOOP_COLLAPSED,
        TRIANGULATE_NESTING_INVALID,
        MESH_INVALID,
        INTERSECT_TOO_MANY_RESULTS,
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
