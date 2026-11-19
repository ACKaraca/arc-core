//! The error codes and loss reasons owned by `arc-draft-dxf`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; loss-report items
//! name one of the [`reasons`]. Messages are in `locales/en-US/arc-draft-dxf.ftl`.

/// The error codes owned by `arc-draft-dxf`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// The target of an import or export is not a model space.
    pub const SPACE_INVALID: ErrorCode = ErrorCode {
        code: "draft.dxf.space_invalid",
        category: Category::InvalidInput,
        message_id: "draft-dxf-space-invalid",
    };
    /// A stored drafting object does not have the shape its type declares.
    pub const DOCUMENT_INVALID: ErrorCode = ErrorCode {
        code: "draft.dxf.document_invalid",
        category: Category::CorruptData,
        message_id: "draft-dxf-document-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[SPACE_INVALID, DOCUMENT_INVALID];
}

/// The stable reason codes of the loss-report items this crate adds to those of `arc-io-dxf`.
pub mod reasons {
    use arc_io_dxf::loss::{Outcome, Reason, Severity};

    /// An entity of a kind without a drafting counterpart was not imported.
    pub const ENTITY_UNMAPPED: Reason = Reason {
        code: "draft.dxf.entity_unmapped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-entity-unmapped",
    };

    /// A record without a drafting counterpart (an entity or a record of the OBJECTS section
    /// of a type the neutral model does not type) was kept as a foreign data row of the
    /// document and is written back by the export.
    pub const RECORD_PRESERVED: Reason = Reason {
        code: "draft.dxf.record_preserved",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-record-preserved",
    };

    /// A record to keep as foreign data was not kept because the import already holds the
    /// most rows it keeps, or the record is larger than the limits allow.
    pub const FOREIGN_OVERFLOW: Reason = Reason {
        code: "draft.dxf.foreign_overflow",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-foreign-overflow",
    };

    /// A paper space entity was not imported.
    pub const PAPER_SPACE_SKIPPED: Reason = Reason {
        code: "draft.dxf.paper_space_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-paper-space-skipped",
    };

    /// Records of the OBJECTS section (dictionaries, layouts, groups) were not imported.
    pub const OBJECTS_SKIPPED: Reason = Reason {
        code: "draft.dxf.objects_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-objects-skipped",
    };

    /// Records of a symbol table without a drafting counterpart were not imported.
    pub const TABLE_SKIPPED: Reason = Reason {
        code: "draft.dxf.table_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-table-skipped",
    };

    /// The geometry of an entity cannot be built; the entity was not imported.
    pub const GEOMETRY_INVALID: Reason = Reason {
        code: "draft.dxf.geometry_invalid",
        outcome: Outcome::Failed,
        severity: Severity::Error,
        message_id: "draft-dxf-loss-geometry-invalid",
    };

    /// Formatting codes of multi-line text were removed.
    pub const TEXT_FORMATTING: Reason = Reason {
        code: "draft.dxf.text_formatting",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-text-formatting",
    };

    /// Text was shortened or cleaned of characters the drafting model does not hold.
    pub const TEXT_ADJUSTED: Reason = Reason {
        code: "draft.dxf.text_adjusted",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-text-adjusted",
    };

    /// A value was clamped, defaulted or left out because the drafting model has no place
    /// for it; the argument `what` names it.
    pub const VALUE_ADJUSTED: Reason = Reason {
        code: "draft.dxf.value_adjusted",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-value-adjusted",
    };

    /// A coordinate along the extrusion direction was not kept.
    pub const Z_DROPPED: Reason = Reason {
        code: "draft.dxf.z_dropped",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-z-dropped",
    };

    /// Coordinates were converted from the unit of the file to the unit of the space.
    pub const UNITS_CONVERTED: Reason = Reason {
        code: "draft.dxf.units_converted",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-units-converted",
    };

    /// The file or the space declares no unit, so coordinates were not converted.
    pub const UNITS_UNITLESS: Reason = Reason {
        code: "draft.dxf.units_unitless",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-units-unitless",
    };

    /// Dimensions were imported without association to geometry.
    pub const DIMENSION_NONASSOCIATIVE: Reason = Reason {
        code: "draft.dxf.dimension_nonassociative",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-dimension-nonassociative",
    };

    /// A hatch was not imported because its boundary cannot be stored.
    pub const HATCH_DROPPED: Reason = Reason {
        code: "draft.dxf.hatch_dropped",
        outcome: Outcome::Failed,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-hatch-dropped",
    };

    /// A hatch was imported with a simpler fill than the file describes.
    pub const HATCH_APPROXIMATED: Reason = Reason {
        code: "draft.dxf.hatch_approximated",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-hatch-approximated",
    };

    /// An array of block references was expanded into single references.
    pub const ARRAY_EXPANDED: Reason = Reason {
        code: "draft.dxf.array_expanded",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-array-expanded",
    };

    /// An attribute value without a definition in its block was not imported.
    pub const ATTRIBUTE_DROPPED: Reason = Reason {
        code: "draft.dxf.attribute_dropped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-attribute-dropped",
    };

    /// A block reference was not imported because its block is missing or would nest itself.
    pub const REFERENCE_UNRESOLVED: Reason = Reason {
        code: "draft.dxf.reference_unresolved",
        outcome: Outcome::Failed,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-reference-unresolved",
    };

    /// A block reference was not imported because the chain of block references through it would
    /// be deeper than the drafting model allows.
    pub const REFERENCE_DEPTH_EXCEEDED: Reason = Reason {
        code: "draft.dxf.reference_depth_exceeded",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-reference-depth-exceeded",
    };

    /// A layer, linetype or style used but not defined in the file was created with defaults.
    pub const DEFINITION_CREATED: Reason = Reason {
        code: "draft.dxf.definition_created",
        outcome: Outcome::Mapped,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-definition-created",
    };

    /// A linetype definition was not imported because its pattern is not valid.
    pub const LINETYPE_INVALID: Reason = Reason {
        code: "draft.dxf.linetype_invalid",
        outcome: Outcome::Failed,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-linetype-invalid",
    };

    /// A definition received another name than the file gives, because of a name conflict
    /// or the length bound.
    pub const NAME_ADJUSTED: Reason = Reason {
        code: "draft.dxf.name_adjusted",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "draft-dxf-loss-name-adjusted",
    };

    /// A drafting object of a kind without a DXF counterpart was not written.
    pub const EXPORT_UNMAPPED: Reason = Reason {
        code: "draft.dxf.export_unmapped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-export-unmapped",
    };

    /// A drafting object was written with less information than it holds; the argument
    /// `what` names the difference.
    pub const EXPORT_APPROXIMATED: Reason = Reason {
        code: "draft.dxf.export_approximated",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "draft-dxf-loss-export-approximated",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        ENTITY_UNMAPPED,
        RECORD_PRESERVED,
        FOREIGN_OVERFLOW,
        PAPER_SPACE_SKIPPED,
        OBJECTS_SKIPPED,
        TABLE_SKIPPED,
        GEOMETRY_INVALID,
        TEXT_FORMATTING,
        TEXT_ADJUSTED,
        VALUE_ADJUSTED,
        Z_DROPPED,
        UNITS_CONVERTED,
        UNITS_UNITLESS,
        DIMENSION_NONASSOCIATIVE,
        HATCH_DROPPED,
        HATCH_APPROXIMATED,
        ARRAY_EXPANDED,
        ATTRIBUTE_DROPPED,
        REFERENCE_UNRESOLVED,
        REFERENCE_DEPTH_EXCEEDED,
        DEFINITION_CREATED,
        LINETYPE_INVALID,
        NAME_ADJUSTED,
        EXPORT_UNMAPPED,
        EXPORT_APPROXIMATED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons};

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-draft-dxf.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-draft-dxf.md");

    #[test]
    fn registry_has_no_problems_alone_or_with_its_dependencies() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        let combined: Vec<_> = arc_base::error::codes::ALL
            .iter()
            .chain(arc_doc::error::codes::ALL)
            .chain(arc_command::error::codes::ALL)
            .chain(arc_draft::error::codes::ALL)
            .chain(arc_io_dxf::codes::ALL)
            .chain(codes::ALL)
            .copied()
            .collect();
        assert_eq!(registry_problems(&combined), vec![]);
        assert!(codes::ALL.iter().all(|e| e.code.starts_with("draft.dxf.")));
    }

    #[test]
    fn reasons_are_unique_and_distinct_from_error_codes() {
        let mut seen = BTreeSet::new();
        for reason in reasons::ALL {
            assert!(reason.code.starts_with("draft.dxf."), "{}", reason.code);
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
        let ids: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
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
