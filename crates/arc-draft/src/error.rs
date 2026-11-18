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
