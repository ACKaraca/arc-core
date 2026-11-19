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
    /// The degree of a NURBS curve is 0, above the largest supported degree, or not below the number of control points.
    pub const NURBS_DEGREE_INVALID: ErrorCode = ErrorCode {
        code: "geom.nurbs.degree_invalid",
        category: Category::InvalidInput,
        message_id: "geom-nurbs-degree-invalid",
    };
    /// The knot vector of a NURBS curve has the wrong length, decreases, is not clamped, repeats a value too often or has an empty range.
    pub const NURBS_KNOTS_INVALID: ErrorCode = ErrorCode {
        code: "geom.nurbs.knots_invalid",
        category: Category::InvalidInput,
        message_id: "geom-nurbs-knots-invalid",
    };
    /// The weights of a NURBS curve do not match its control points in number, or one is not positive.
    pub const NURBS_WEIGHTS_INVALID: ErrorCode = ErrorCode {
        code: "geom.nurbs.weights_invalid",
        category: Category::InvalidInput,
        message_id: "geom-nurbs-weights-invalid",
    };
    /// All control points of a NURBS curve are equal.
    pub const NURBS_DEGENERATE: ErrorCode = ErrorCode {
        code: "geom.nurbs.degenerate",
        category: Category::InvalidInput,
        message_id: "geom-nurbs-degenerate",
    };
    /// A NURBS curve has more control points than the limit allows.
    pub const NURBS_TOO_MANY_CONTROL_POINTS: ErrorCode = ErrorCode {
        code: "geom.nurbs.too_many_control_points",
        category: Category::LimitExceeded,
        message_id: "geom-nurbs-too-many-control-points",
    };
    /// A knot cannot be removed as often as asked without moving the curve by more than the tolerance.
    pub const NURBS_KNOT_NOT_REMOVABLE: ErrorCode = ErrorCode {
        code: "geom.nurbs.knot_not_removable",
        category: Category::FailedOperation,
        message_id: "geom-nurbs-knot-not-removable",
    };
    /// The points to fit are too few for the degree, or two consecutive ones are equal.
    pub const FIT_POINTS_INVALID: ErrorCode = ErrorCode {
        code: "geom.fit.points_invalid",
        category: Category::InvalidInput,
        message_id: "geom-fit-points-invalid",
    };
    /// A parameter lies outside the range an operation accepts, or is NaN.
    pub const PARAMETER_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "geom.parameter.out_of_range",
        category: Category::InvalidInput,
        message_id: "geom-parameter-out-of-range",
    };
    /// An operation does not support the kind of a curve it was given yet.
    pub const CURVE_KIND_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "geom.curve.kind_unsupported",
        category: Category::Unsupported,
        message_id: "geom-curve-kind-unsupported",
    };
    /// A point given to split a curve is farther than the linear tolerance from it.
    pub const EDIT_POINT_OFF_CURVE: ErrorCode = ErrorCode {
        code: "geom.edit.point_off_curve",
        category: Category::InvalidInput,
        message_id: "geom-edit-point-off-curve",
    };
    /// The boundary curves do not meet the curve, or its extension, where a trim or extension needs them.
    pub const EDIT_NO_INTERSECTION: ErrorCode = ErrorCode {
        code: "geom.edit.no_intersection",
        category: Category::FailedOperation,
        message_id: "geom-edit-no-intersection",
    };
    /// A closed curve was given to an operation that needs an open one.
    pub const EDIT_CURVE_CLOSED: ErrorCode = ErrorCode {
        code: "geom.edit.curve_closed",
        category: Category::InvalidInput,
        message_id: "geom-edit-curve-closed",
    };
    /// A composite curve has no curves.
    pub const COMPOSITE_EMPTY: ErrorCode = ErrorCode {
        code: "geom.composite.empty",
        category: Category::InvalidInput,
        message_id: "geom-composite-empty",
    };
    /// The end of a curve of a composite is farther than the linear tolerance from the start of the next.
    pub const COMPOSITE_NOT_CONNECTED: ErrorCode = ErrorCode {
        code: "geom.composite.not_connected",
        category: Category::InvalidInput,
        message_id: "geom-composite-not-connected",
    };
    /// An offset of a circle or arc reaches or passes its center, or nothing of an offset remains.
    pub const OFFSET_COLLAPSED: ErrorCode = ErrorCode {
        code: "geom.offset.collapsed",
        category: Category::FailedOperation,
        message_id: "geom-offset-collapsed",
    };
    /// A curve to offset has a point without a tangent direction.
    pub const OFFSET_DEGENERATE: ErrorCode = ErrorCode {
        code: "geom.offset.degenerate",
        category: Category::InvalidInput,
        message_id: "geom-offset-degenerate",
    };
    /// A miter limit is not a finite number of at least 1.
    pub const OFFSET_JOIN_INVALID: ErrorCode = ErrorCode {
        code: "geom.offset.join_invalid",
        category: Category::InvalidInput,
        message_id: "geom-offset-join-invalid",
    };
    /// An offset needs more pieces or samples than the limit allows.
    pub const OFFSET_TOO_MANY_PIECES: ErrorCode = ErrorCode {
        code: "geom.offset.too_many_pieces",
        category: Category::LimitExceeded,
        message_id: "geom-offset-too-many-pieces",
    };
    /// A point index is built from more points than the limit allows.
    pub const INDEX_TOO_MANY_POINTS: ErrorCode = ErrorCode {
        code: "geom.index.too_many_points",
        category: Category::LimitExceeded,
        message_id: "geom-index-too-many-points",
    };

    /// The curves given to build a region cross or touch themselves.
    pub const REGION_SELF_INTERSECTING: ErrorCode = ErrorCode {
        code: "geom.region.self_intersecting",
        category: Category::InvalidInput,
        message_id: "geom-region-self-intersecting",
    };
    /// Two of the closed curves given to build a region cross, touch or overlap.
    pub const REGION_CURVES_INTERSECT: ErrorCode = ErrorCode {
        code: "geom.region.curves_intersect",
        category: Category::InvalidInput,
        message_id: "geom-region-curves-intersect",
    };
    /// An arrangement of curves needs more elements, candidate pairs, intersections or edges
    /// than the limit allows.
    pub const ARRANGEMENT_TOO_LARGE: ErrorCode = ErrorCode {
        code: "geom.arrangement.too_large",
        category: Category::LimitExceeded,
        message_id: "geom-arrangement-too-large",
    };
    /// No closed boundary of the curves encloses the picked point.
    pub const TRACE_NOT_ENCLOSED: ErrorCode = ErrorCode {
        code: "geom.trace.not_enclosed",
        category: Category::FailedOperation,
        message_id: "geom-trace-not-enclosed",
    };
    /// The picked point lies on one of the curves.
    pub const TRACE_ON_BOUNDARY: ErrorCode = ErrorCode {
        code: "geom.trace.on_boundary",
        category: Category::InvalidInput,
        message_id: "geom-trace-on-boundary",
    };

    /// A fillet radius is NaN, infinite or negative.
    pub const CORNER_RADIUS_INVALID: ErrorCode = ErrorCode {
        code: "geom.corner.radius_invalid",
        category: Category::InvalidInput,
        message_id: "geom-corner-radius-invalid",
    };
    /// A chamfer distance is NaN, infinite or negative, or a chamfer angle is outside (0, π).
    pub const CORNER_DISTANCE_INVALID: ErrorCode = ErrorCode {
        code: "geom.corner.distance_invalid",
        category: Category::InvalidInput,
        message_id: "geom-corner-distance-invalid",
    };
    /// A pick hint of a corner construction is farther than the linear tolerance from its curve.
    pub const CORNER_PICK_OFF_CURVE: ErrorCode = ErrorCode {
        code: "geom.corner.pick_off_curve",
        category: Category::InvalidInput,
        message_id: "geom-corner-pick-off-curve",
    };
    /// No solution of a corner construction keeps both pick hints on the parts that remain.
    pub const CORNER_PICK_NO_SOLUTION: ErrorCode = ErrorCode {
        code: "geom.corner.pick_no_solution",
        category: Category::FailedOperation,
        message_id: "geom-corner-pick-no-solution",
    };
    /// No circle of the fillet radius is tangent to both curves on the corner.
    pub const CORNER_RADIUS_TOO_LARGE: ErrorCode = ErrorCode {
        code: "geom.corner.radius_too_large",
        category: Category::FailedOperation,
        message_id: "geom-corner-radius-too-large",
    };
    /// The supports of two curves of a corner construction do not meet.
    pub const CORNER_NO_INTERSECTION: ErrorCode = ErrorCode {
        code: "geom.corner.no_intersection",
        category: Category::FailedOperation,
        message_id: "geom-corner-no-intersection",
    };
    /// Two lines of a corner construction are parallel; no corner point or unique joint exists.
    pub const CORNER_PARALLEL: ErrorCode = ErrorCode {
        code: "geom.corner.parallel",
        category: Category::FailedOperation,
        message_id: "geom-corner-parallel",
    };
    /// Two curves of a corner construction lie on the same line or circle.
    pub const CORNER_COINCIDENT: ErrorCode = ErrorCode {
        code: "geom.corner.coincident",
        category: Category::InvalidInput,
        message_id: "geom-corner-coincident",
    };
    /// A part of a curve that a corner construction keeps would be empty.
    pub const CORNER_CURVE_TOO_SHORT: ErrorCode = ErrorCode {
        code: "geom.corner.curve_too_short",
        category: Category::FailedOperation,
        message_id: "geom-corner-curve-too-short",
    };
    /// A corner construction is not defined for the kinds of its curves.
    pub const CORNER_UNSUPPORTED_PAIR: ErrorCode = ErrorCode {
        code: "geom.corner.unsupported_pair",
        category: Category::Unsupported,
        message_id: "geom-corner-unsupported-pair",
    };
    /// A polyline segment index is outside the polyline, or the two indices are equal.
    pub const CORNER_SEGMENT_INVALID: ErrorCode = ErrorCode {
        code: "geom.corner.segment_invalid",
        category: Category::InvalidInput,
        message_id: "geom-corner-segment-invalid",
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
        NURBS_DEGREE_INVALID,
        NURBS_KNOTS_INVALID,
        NURBS_WEIGHTS_INVALID,
        NURBS_DEGENERATE,
        NURBS_TOO_MANY_CONTROL_POINTS,
        NURBS_KNOT_NOT_REMOVABLE,
        FIT_POINTS_INVALID,
        PARAMETER_OUT_OF_RANGE,
        CURVE_KIND_UNSUPPORTED,
        EDIT_POINT_OFF_CURVE,
        EDIT_NO_INTERSECTION,
        EDIT_CURVE_CLOSED,
        COMPOSITE_EMPTY,
        COMPOSITE_NOT_CONNECTED,
        OFFSET_COLLAPSED,
        OFFSET_DEGENERATE,
        OFFSET_JOIN_INVALID,
        OFFSET_TOO_MANY_PIECES,
        INDEX_TOO_MANY_POINTS,
        REGION_SELF_INTERSECTING,
        REGION_CURVES_INTERSECT,
        ARRANGEMENT_TOO_LARGE,
        TRACE_NOT_ENCLOSED,
        TRACE_ON_BOUNDARY,
        CORNER_RADIUS_INVALID,
        CORNER_DISTANCE_INVALID,
        CORNER_PICK_OFF_CURVE,
        CORNER_PICK_NO_SOLUTION,
        CORNER_RADIUS_TOO_LARGE,
        CORNER_NO_INTERSECTION,
        CORNER_PARALLEL,
        CORNER_COINCIDENT,
        CORNER_CURVE_TOO_SHORT,
        CORNER_UNSUPPORTED_PAIR,
        CORNER_SEGMENT_INVALID,
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
