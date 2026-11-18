//! The error codes owned by `arc-draft`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-draft.ftl`.

/// The error codes owned by `arc-draft`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A geometry field does not hold the canonical encoding of the curve kind its type stores.
    pub const GEOMETRY_INVALID: ErrorCode = ErrorCode {
        code: "draft.geometry.invalid",
        category: Category::InvalidInput,
        message_id: "draft-geometry-invalid",
    };
    /// The extrusion direction of an entity is the zero vector.
    pub const EXTRUSION_ZERO: ErrorCode = ErrorCode {
        code: "draft.extrusion.zero",
        category: Category::InvalidInput,
        message_id: "draft-extrusion-zero",
    };
    /// A layout space declares the unit `unitless`.
    pub const SPACE_LAYOUT_UNITLESS: ErrorCode = ErrorCode {
        code: "draft.space.layout_unitless",
        category: Category::InvalidInput,
        message_id: "draft-space-layout-unitless",
    };
    /// A linetype pattern has no element of positive length.
    pub const LINETYPE_PATTERN_EMPTY: ErrorCode = ErrorCode {
        code: "draft.linetype.pattern_empty",
        category: Category::InvalidInput,
        message_id: "draft-linetype-pattern-empty",
    };
    /// A linetype scale is not a finite positive number, or the product of the scales is not.
    pub const LINETYPE_SCALE_INVALID: ErrorCode = ErrorCode {
        code: "draft.linetype.scale_invalid",
        category: Category::InvalidInput,
        message_id: "draft-linetype-scale-invalid",
    };
    /// A block reference has a zero scale factor.
    pub const BLOCK_SCALE_ZERO: ErrorCode = ErrorCode {
        code: "draft.block.scale_zero",
        category: Category::InvalidInput,
        message_id: "draft-block-scale-zero",
    };
    /// Block definitions reference each other in a cycle; the detail `cycle` lists it.
    pub const BLOCK_CYCLE: ErrorCode = ErrorCode {
        code: "draft.block.cycle",
        category: Category::InvalidInput,
        message_id: "draft-block-cycle",
    };
    /// A chain of nested block references is deeper than the limit.
    pub const BLOCK_NESTING_TOO_DEEP: ErrorCode = ErrorCode {
        code: "draft.block.nesting_too_deep",
        category: Category::LimitExceeded,
        message_id: "draft-block-nesting-too-deep",
    };
    /// A block reference names an attribute its definition does not define.
    pub const BLOCK_ATTRIBUTE_UNKNOWN: ErrorCode = ErrorCode {
        code: "draft.block.attribute_unknown",
        category: Category::InvalidInput,
        message_id: "draft-block-attribute-unknown",
    };
    /// Another layer, linetype or block definition already has the name.
    pub const NAME_TAKEN: ErrorCode = ErrorCode {
        code: "draft.name.taken",
        category: Category::Conflict,
        message_id: "draft-name-taken",
    };
    /// An object is not of a type the operation accepts.
    pub const OBJECT_WRONG_TYPE: ErrorCode = ErrorCode {
        code: "draft.object.wrong_type",
        category: Category::InvalidInput,
        message_id: "draft-object-wrong-type",
    };
    /// A field value given as JSON does not fit the field, or the field does not exist.
    pub const FIELD_INVALID: ErrorCode = ErrorCode {
        code: "draft.field.invalid",
        category: Category::InvalidInput,
        message_id: "draft-field-invalid",
    };
    /// The reference entity of a draw-order change is one of the entities being moved.
    pub const ORDER_REFERENCE_MOVED: ErrorCode = ErrorCode {
        code: "draft.order.reference_moved",
        category: Category::InvalidInput,
        message_id: "draft-order-reference-moved",
    };
    /// The entities of a draw-order change are not all in the same space or block definition.
    pub const ORDER_CONTAINER_MISMATCH: ErrorCode = ErrorCode {
        code: "draft.order.container_mismatch",
        category: Category::InvalidInput,
        message_id: "draft-order-container-mismatch",
    };

    /// The margins of a layout leave no printable area on its sheet.
    pub const LAYOUT_MARGINS_INVALID: ErrorCode = ErrorCode {
        code: "draft.layout.margins_invalid",
        category: Category::InvalidInput,
        message_id: "draft-layout-margins-invalid",
    };
    /// A layout plots a window without two corners, or with corners not in ascending order.
    pub const LAYOUT_WINDOW_INVALID: ErrorCode = ErrorCode {
        code: "draft.layout.window_invalid",
        category: Category::InvalidInput,
        message_id: "draft-layout-window-invalid",
    };
    /// Layout settings or a viewport are not owned by a layout space.
    pub const LAYOUT_OWNER_INVALID: ErrorCode = ErrorCode {
        code: "draft.layout.owner_invalid",
        category: Category::InvalidInput,
        message_id: "draft-layout-owner-invalid",
    };
    /// A layout space would have two layout settings objects.
    pub const LAYOUT_DUPLICATE: ErrorCode = ErrorCode {
        code: "draft.layout.duplicate",
        category: Category::Conflict,
        message_id: "draft-layout-duplicate",
    };
    /// The clip boundary of a viewport is not a closed polyline.
    pub const VIEWPORT_CLIP_INVALID: ErrorCode = ErrorCode {
        code: "draft.viewport.clip_invalid",
        category: Category::InvalidInput,
        message_id: "draft-viewport-clip-invalid",
    };
    /// The view of a locked viewport cannot change.
    pub const VIEWPORT_LOCKED: ErrorCode = ErrorCode {
        code: "draft.viewport.locked",
        category: Category::Conflict,
        message_id: "draft-viewport-locked",
    };
    /// The boundary of a wipeout is not a closed polyline.
    pub const WIPEOUT_BOUNDARY_OPEN: ErrorCode = ErrorCode {
        code: "draft.wipeout.boundary_open",
        category: Category::InvalidInput,
        message_id: "draft-wipeout-boundary-open",
    };
    /// A dimension style's parent is not a style for all kinds without a parent of its own.
    pub const STYLE_PARENT_INVALID: ErrorCode = ErrorCode {
        code: "draft.style.parent_invalid",
        category: Category::InvalidInput,
        message_id: "draft-style-parent-invalid",
    };
    /// An object selected for purging is in use.
    pub const PURGE_IN_USE: ErrorCode = ErrorCode {
        code: "draft.purge.in_use",
        category: Category::Conflict,
        message_id: "draft-purge-in-use",
    };
    /// A display list would hold more primitives than the bound.
    pub const DISPLAY_TOO_LARGE: ErrorCode = ErrorCode {
        code: "draft.display.too_large",
        category: Category::LimitExceeded,
        message_id: "draft-display-too-large",
    };
    /// A line of a stroke-font file is malformed; the argument `line` names it.
    pub const FONT_SYNTAX: ErrorCode = ErrorCode {
        code: "draft.font.syntax",
        category: Category::InvalidInput,
        message_id: "draft-font-syntax",
    };
    /// A stroke-font file declares a format version this reader does not read.
    pub const FONT_VERSION_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "draft.font.version_unsupported",
        category: Category::Unsupported,
        message_id: "draft-font-version-unsupported",
    };
    /// A stroke-font header value is missing, repeated or inconsistent; the argument `key` names it.
    pub const FONT_HEADER_INVALID: ErrorCode = ErrorCode {
        code: "draft.font.header_invalid",
        category: Category::InvalidInput,
        message_id: "draft-font-header-invalid",
    };
    /// A stroke font defines a glyph twice.
    pub const FONT_GLYPH_DUPLICATE: ErrorCode = ErrorCode {
        code: "draft.font.glyph_duplicate",
        category: Category::InvalidInput,
        message_id: "draft-font-glyph-duplicate",
    };
    /// A stroke font refers to a glyph it does not define.
    pub const FONT_GLYPH_UNKNOWN: ErrorCode = ErrorCode {
        code: "draft.font.glyph_unknown",
        category: Category::InvalidInput,
        message_id: "draft-font-glyph-unknown",
    };
    /// A stroke-font glyph leaves its box or the coordinate range.
    pub const FONT_GLYPH_OUT_OF_BOUNDS: ErrorCode = ErrorCode {
        code: "draft.font.glyph_out_of_bounds",
        category: Category::InvalidInput,
        message_id: "draft-font-glyph-out-of-bounds",
    };
    /// A stroke-font file exceeds a bound of the reader; the argument `limit` names it.
    pub const FONT_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "draft.font.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "draft-font-limit-exceeded",
    };
    /// The font provider lacks the default stroke font.
    pub const FONT_DEFAULT_MISSING: ErrorCode = ErrorCode {
        code: "draft.font.default_missing",
        category: Category::NotFound,
        message_id: "draft-font-default-missing",
    };
    /// A requested font is not available and another font is used in its place.
    pub const FONT_SUBSTITUTED: ErrorCode = ErrorCode {
        code: "draft.font.substituted",
        category: Category::NotFound,
        message_id: "draft-font-substituted",
    };
    /// Outline font bytes cannot be read, or outline text cannot be laid out.
    pub const FONT_OUTLINE_INVALID: ErrorCode = ErrorCode {
        code: "draft.font.outline_invalid",
        category: Category::CorruptData,
        message_id: "draft-font-outline-invalid",
    };
    /// Text holds a control character other than tab, or a size or factor out of range.
    pub const TEXT_CONTENT_INVALID: ErrorCode = ErrorCode {
        code: "draft.text.content_invalid",
        category: Category::InvalidInput,
        message_id: "draft-text-content-invalid",
    };
    /// A text has more paragraphs, runs, tab stops or bytes than the limits allow.
    pub const TEXT_TOO_LARGE: ErrorCode = ErrorCode {
        code: "draft.text.too_large",
        category: Category::LimitExceeded,
        message_id: "draft-text-too-large",
    };
    /// A stacked run lacks a denominator, or an ordinary run has one.
    pub const TEXT_STACK_INVALID: ErrorCode = ErrorCode {
        code: "draft.text.stack_invalid",
        category: Category::InvalidInput,
        message_id: "draft-text-stack-invalid",
    };
    /// Tab stops are not finite, positive and ascending.
    pub const TEXT_TABS_INVALID: ErrorCode = ErrorCode {
        code: "draft.text.tabs_invalid",
        category: Category::InvalidInput,
        message_id: "draft-text-tabs-invalid",
    };

    /// The definition points of a dimension do not number what its kind needs.
    pub const DIMENSION_POINTS_INVALID: ErrorCode = ErrorCode {
        code: "draft.dimension.points_invalid",
        category: Category::InvalidInput,
        message_id: "draft-dimension-points-invalid",
    };
    /// The definition points of a dimension define no measurement: coincident points, parallel
    /// lines, a zero radius or a dimension line through the vertex.
    pub const DIMENSION_DEGENERATE: ErrorCode = ErrorCode {
        code: "draft.dimension.degenerate",
        category: Category::InvalidInput,
        message_id: "draft-dimension-degenerate",
    };
    /// A referenced object and sub-object key give no point.
    pub const DIMENSION_REFERENCE_INVALID: ErrorCode = ErrorCode {
        code: "draft.dimension.reference_invalid",
        category: Category::InvalidInput,
        message_id: "draft-dimension-reference-invalid",
    };
    /// A dimension lost the geometry it referenced and keeps its last points (a warning).
    pub const DIMENSION_DISASSOCIATED: ErrorCode = ErrorCode {
        code: "draft.dimension.disassociated",
        category: Category::NotFound,
        message_id: "draft-dimension-disassociated",
    };
    /// A chain member is not a linear or aligned dimension, or is listed twice.
    pub const CHAIN_MEMBER_INVALID: ErrorCode = ErrorCode {
        code: "draft.chain.member_invalid",
        category: Category::InvalidInput,
        message_id: "draft-chain-member-invalid",
    };
    /// An annotation scale is not a finite positive number, or is listed twice.
    pub const ANNOTATION_SCALE_INVALID: ErrorCode = ErrorCode {
        code: "draft.annotation.scale_invalid",
        category: Category::InvalidInput,
        message_id: "draft-annotation-scale-invalid",
    };
    /// A leader has fewer points than it needs.
    pub const LEADER_POINTS_INVALID: ErrorCode = ErrorCode {
        code: "draft.leader.points_invalid",
        category: Category::InvalidInput,
        message_id: "draft-leader-points-invalid",
    };

    /// A style of a plot style table lacks the key its table kind needs: an index in a
    /// color-mapped table, a name in a named one.
    pub const PLOT_STYLE_ENTRY_INVALID: ErrorCode = ErrorCode {
        code: "draft.plot_style.entry_invalid",
        category: Category::InvalidInput,
        message_id: "draft-plot-style-entry-invalid",
    };
    /// Two styles of a plot style table have the same key; the argument `key` names it.
    pub const PLOT_STYLE_DUPLICATE: ErrorCode = ErrorCode {
        code: "draft.plot_style.duplicate",
        category: Category::Conflict,
        message_id: "draft-plot-style-duplicate",
    };
    /// A plot option is out of range, or the plot area has no extent; the argument `option`
    /// names it.
    pub const PLOT_OPTION_INVALID: ErrorCode = ErrorCode {
        code: "draft.plot.option_invalid",
        category: Category::InvalidInput,
        message_id: "draft-plot-option-invalid",
    };
    /// The object to plot is not a layout space with layout settings, or not a space.
    pub const PLOT_SOURCE_INVALID: ErrorCode = ErrorCode {
        code: "draft.plot.source_invalid",
        category: Category::InvalidInput,
        message_id: "draft-plot-source-invalid",
    };
    /// A plot stream would hold more items than the bound.
    pub const PLOT_TOO_LARGE: ErrorCode = ErrorCode {
        code: "draft.plot.too_large",
        category: Category::LimitExceeded,
        message_id: "draft-plot-too-large",
    };
    /// A raster resolution or size is out of range, or the pixmap cannot be allocated.
    pub const RASTER_INVALID: ErrorCode = ErrorCode {
        code: "draft.raster.invalid",
        category: Category::InvalidInput,
        message_id: "draft-raster-invalid",
    };
    /// A raster output would have more pixels than the bound; the argument `limit` names it.
    pub const RASTER_TOO_LARGE: ErrorCode = ErrorCode {
        code: "draft.raster.too_large",
        category: Category::LimitExceeded,
        message_id: "draft-raster-too-large",
    };
    /// The PNG encoder failed.
    pub const RASTER_ENCODE_FAILED: ErrorCode = ErrorCode {
        code: "draft.raster.encode_failed",
        category: Category::Internal,
        message_id: "draft-raster-encode-failed",
    };

    /// A stored hatch loop is not a closed circle, ellipse or polyline, the hatch has no loop, or
    /// a boundary object cannot bound a region.
    pub const HATCH_BOUNDARY_INVALID: ErrorCode = ErrorCode {
        code: "draft.hatch.boundary_invalid",
        category: Category::InvalidInput,
        message_id: "draft-hatch-boundary-invalid",
    };
    /// A hatch scale is not a finite positive number, or its angle or origin is not finite.
    pub const HATCH_PARAMETER_INVALID: ErrorCode = ErrorCode {
        code: "draft.hatch.parameter_invalid",
        category: Category::InvalidInput,
        message_id: "draft-hatch-parameter-invalid",
    };
    /// A hatch lost a boundary object and keeps its last boundary (a warning).
    pub const HATCH_DISASSOCIATED: ErrorCode = ErrorCode {
        code: "draft.hatch.disassociated",
        category: Category::NotFound,
        message_id: "draft-hatch-disassociated",
    };
    /// A line of a hatch-pattern file is malformed.
    pub const PATTERN_SYNTAX: ErrorCode = ErrorCode {
        code: "draft.pattern.syntax",
        category: Category::InvalidInput,
        message_id: "draft-pattern-syntax",
    };
    /// A hatch-pattern file has a format version this reader does not read.
    pub const PATTERN_VERSION_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "draft.pattern.version_unsupported",
        category: Category::Unsupported,
        message_id: "draft-pattern-version-unsupported",
    };
    /// A hatch-pattern file defines a pattern name twice.
    pub const PATTERN_DUPLICATE: ErrorCode = ErrorCode {
        code: "draft.pattern.duplicate",
        category: Category::InvalidInput,
        message_id: "draft-pattern-duplicate",
    };
    /// A pattern line family breaks a rule: angle, range, spacing or dash list.
    pub const PATTERN_LINE_INVALID: ErrorCode = ErrorCode {
        code: "draft.pattern.line_invalid",
        category: Category::InvalidInput,
        message_id: "draft-pattern-line-invalid",
    };
    /// A pattern, or a pattern hatch, has no line family.
    pub const PATTERN_EMPTY: ErrorCode = ErrorCode {
        code: "draft.pattern.empty",
        category: Category::InvalidInput,
        message_id: "draft-pattern-empty",
    };
    /// A hatch-pattern file exceeds a bound.
    pub const PATTERN_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "draft.pattern.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "draft-pattern-limit-exceeded",
    };
    /// A pattern name is not in the pattern file.
    pub const PATTERN_UNKNOWN: ErrorCode = ErrorCode {
        code: "draft.pattern.unknown",
        category: Category::NotFound,
        message_id: "draft-pattern-unknown",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        GEOMETRY_INVALID,
        EXTRUSION_ZERO,
        SPACE_LAYOUT_UNITLESS,
        LINETYPE_PATTERN_EMPTY,
        LINETYPE_SCALE_INVALID,
        BLOCK_SCALE_ZERO,
        BLOCK_CYCLE,
        BLOCK_NESTING_TOO_DEEP,
        BLOCK_ATTRIBUTE_UNKNOWN,
        NAME_TAKEN,
        OBJECT_WRONG_TYPE,
        FIELD_INVALID,
        ORDER_REFERENCE_MOVED,
        ORDER_CONTAINER_MISMATCH,
        LAYOUT_MARGINS_INVALID,
        LAYOUT_WINDOW_INVALID,
        LAYOUT_OWNER_INVALID,
        LAYOUT_DUPLICATE,
        VIEWPORT_CLIP_INVALID,
        VIEWPORT_LOCKED,
        WIPEOUT_BOUNDARY_OPEN,
        STYLE_PARENT_INVALID,
        PURGE_IN_USE,
        DISPLAY_TOO_LARGE,
        FONT_SYNTAX,
        FONT_VERSION_UNSUPPORTED,
        FONT_HEADER_INVALID,
        FONT_GLYPH_DUPLICATE,
        FONT_GLYPH_UNKNOWN,
        FONT_GLYPH_OUT_OF_BOUNDS,
        FONT_LIMIT_EXCEEDED,
        FONT_DEFAULT_MISSING,
        FONT_SUBSTITUTED,
        FONT_OUTLINE_INVALID,
        TEXT_CONTENT_INVALID,
        TEXT_TOO_LARGE,
        TEXT_STACK_INVALID,
        TEXT_TABS_INVALID,
        DIMENSION_POINTS_INVALID,
        DIMENSION_DEGENERATE,
        DIMENSION_REFERENCE_INVALID,
        DIMENSION_DISASSOCIATED,
        CHAIN_MEMBER_INVALID,
        ANNOTATION_SCALE_INVALID,
        LEADER_POINTS_INVALID,
        PLOT_STYLE_ENTRY_INVALID,
        PLOT_STYLE_DUPLICATE,
        PLOT_OPTION_INVALID,
        PLOT_SOURCE_INVALID,
        PLOT_TOO_LARGE,
        RASTER_INVALID,
        RASTER_TOO_LARGE,
        RASTER_ENCODE_FAILED,
        HATCH_BOUNDARY_INVALID,
        HATCH_PARAMETER_INVALID,
        HATCH_DISASSOCIATED,
        PATTERN_SYNTAX,
        PATTERN_VERSION_UNSUPPORTED,
        PATTERN_DUPLICATE,
        PATTERN_LINE_INVALID,
        PATTERN_EMPTY,
        PATTERN_LIMIT_EXCEEDED,
        PATTERN_UNKNOWN,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-draft.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-draft.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_its_dependencies() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(arc_geom::error::codes::ALL)
            .chain(arc_doc::error::codes::ALL)
            .chain(arc_command::error::codes::ALL)
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(codes::ALL.iter().all(|e| e.code.starts_with("draft.")));
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
            assert!(SPECIFICATION.contains(&cell), "{cell}");
        }
    }
}
