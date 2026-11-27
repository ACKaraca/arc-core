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

    /// A limit of the caller's limits profile was exceeded; the argument `limit` names it. The
    /// code and its message are the ones of `arc-io-common`, shared by every reader and writer.
    pub use arc_io_common::error::codes::LIMIT_EXCEEDED;
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

    /// A value of the model cannot be written: a number that is not finite, a negative quantity,
    /// a door or window size that is not positive, placement axes or a true north that are too short
    /// or parallel, a NUL in a text, a name repeated within a property or quantity set, or a set
    /// name repeated on one element.
    pub const IFC_WRITE_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.write_value_invalid",
        category: Category::InvalidInput,
        message_id: "io-ifc-write-value-invalid",
    };
    /// An element names a storey or host that is not in the model or of a kind the relationship
    /// does not allow, an opening has no host, or a chain of hosts returns to its start.
    pub const IFC_WRITE_REFERENCE_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.write_reference_invalid",
        category: Category::InvalidInput,
        message_id: "io-ifc-write-reference-invalid",
    };
    /// Two objects of the written file would have the same `GlobalId`.
    pub const IFC_WRITE_ID_DUPLICATE: ErrorCode = ErrorCode {
        code: "io.ifc.write_id_duplicate",
        category: Category::InvalidInput,
        message_id: "io-ifc-write-id-duplicate",
    };

    /// An edit names an instance that is not in the file, or an attribute position that its
    /// record does not have.
    pub const IFC_EDIT_TARGET_MISSING: ErrorCode = ErrorCode {
        code: "io.ifc.edit_target_missing",
        category: Category::NotFound,
        message_id: "io-ifc-edit-target-missing",
    };
    /// An edit would leave a reference to an instance that is not in the file: a new value refers
    /// to a missing instance, or the instance to remove is still referred to.
    pub const IFC_EDIT_REFERENCE_DANGLING: ErrorCode = ErrorCode {
        code: "io.ifc.edit_reference_dangling",
        category: Category::Conflict,
        message_id: "io-ifc-edit-reference-dangling",
    };
    /// An edited value cannot be written to a physical file: an instance without records, a
    /// number that is not finite, a NUL in a text, a malformed type name, enumeration or binary,
    /// nesting deeper than the writer follows, no instance name left, or a header without
    /// `FILE_DESCRIPTION`, `FILE_NAME` and `FILE_SCHEMA` in that order.
    pub const IFC_EDIT_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "io.ifc.edit_value_invalid",
        category: Category::InvalidInput,
        message_id: "io-ifc-edit-value-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        IFC_TRUNCATED,
        IFC_SYNTAX,
        IFC_STRUCTURE_INVALID,
        IFC_TEXT_INVALID,
        IFC_VALUE_INVALID,
        IFC_INSTANCE_DUPLICATE,
        IFC_ARCHIVE_ENTRY_MISSING,
        IFC_ARCHIVE_ENTRY_AMBIGUOUS,
        IFC_WRITE_VALUE_INVALID,
        IFC_WRITE_REFERENCE_INVALID,
        IFC_WRITE_ID_DUPLICATE,
        IFC_EDIT_TARGET_MISSING,
        IFC_EDIT_REFERENCE_DANGLING,
        IFC_EDIT_VALUE_INVALID,
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
    /// An element is placed a second time: a second parent, or a repeated pair in a relationship.
    pub const DUPLICATE_PLACEMENT: Reason = Reason {
        code: "io.ifc.duplicate_placement",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-duplicate-placement",
    };
    /// A relationship, property or quantity member names an instance that is missing or of a kind
    /// the views cannot use; that member is not read.
    pub const TARGET_MISSING: Reason = Reason {
        code: "io.ifc.target_missing",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-target-missing",
    };
    /// An entity the views understand but cannot map (a property of another kind, a complex
    /// property, an aggregation outside the spatial structure) is kept as generic instances.
    pub const NOT_MAPPED: Reason = Reason {
        code: "io.ifc.not_mapped",
        outcome: Outcome::Preserved,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-not-mapped",
    };
    /// A section other than `DATA` (`ANCHOR`, `REFERENCE` or `SIGNATURE`) is skipped as a whole.
    pub const SECTION_SKIPPED: Reason = Reason {
        code: "io.ifc.section_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-section-skipped",
    };
    /// An archive entry that holds resource-fork or metadata data (`__MACOSX/` or a `._` file) is
    /// not the model and is skipped.
    pub const ARCHIVE_ENTRY_SKIPPED: Reason = Reason {
        code: "io.ifc.archive_entry_skipped",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-ifc-loss-archive-entry-skipped",
    };
    /// A string holds bytes that are not UTF-8; each is read as ISO 8859-1.
    pub const TEXT_LATIN1: Reason = Reason {
        code: "io.ifc.text_latin1",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-text-latin1",
    };
    /// A string holds a raw tab, carriage return or line feed, which is kept as it is.
    pub const TEXT_CONTROL_KEPT: Reason = Reason {
        code: "io.ifc.text_control_kept",
        outcome: Outcome::Preserved,
        severity: Severity::Info,
        message_id: "io-ifc-loss-text-control-kept",
    };
    /// Bytes outside the sections are ignored: a UTF-8 byte order mark before the first keyword,
    /// and white space, NUL and 0x1A padding after `END-ISO-10303-21;`.
    pub const BYTES_IGNORED: Reason = Reason {
        code: "io.ifc.bytes_ignored",
        outcome: Outcome::Dropped,
        severity: Severity::Info,
        message_id: "io-ifc-loss-bytes-ignored",
    };
    /// Bytes after `END-ISO-10303-21;` that are not padding are reported and ignored.
    pub const TRAILING_BYTES: Reason = Reason {
        code: "io.ifc.trailing_bytes",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-trailing-bytes",
    };

    /// A representation item, placement, mapping or loop of a kind the geometry evaluation does not
    /// implement (a boolean result, a swept disk, a curve other than a polyline), or one longer than
    /// its point limit: a loop or polyline of more than 4096 points, or a face or profile whose loops
    /// together exceed 4096 points, including the bridge points of its holes. Nothing is emitted for
    /// it.
    pub const GEOMETRY_NOT_EVALUATED: Reason = Reason {
        code: "io.ifc.geometry_not_evaluated",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-geometry-not-evaluated",
    };
    /// A geometry item is degenerate (a zero or negative depth, a zero-area profile or face, a
    /// zero direction, a scale that is not positive, a hole that is not strictly inside its outer
    /// loop or in no other hole, or loops that touch or cross) or has a coordinate that is not finite
    /// after unit conversion; the item is skipped and nothing is emitted for it.
    pub const GEOMETRY_DEGENERATE: Reason = Reason {
        code: "io.ifc.geometry_degenerate",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-geometry-degenerate",
    };
    /// An index of a face, loop or polyline is not an integer in the range of its point list; the
    /// item is skipped.
    pub const INDEX_INVALID: Reason = Reason {
        code: "io.ifc.index_invalid",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-index-invalid",
    };
    /// Openings (`IfcRelVoidsElement`) are not subtracted from their elements, which are emitted
    /// solid; the report counts them once per file.
    pub const OPENING_NOT_SUBTRACTED: Reason = Reason {
        code: "io.ifc.opening_not_subtracted",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-opening-not-subtracted",
    };
    /// The unit assignment holds no usable length unit; meters and radians are assumed.
    pub const UNIT_ASSUMED: Reason = Reason {
        code: "io.ifc.unit_assumed",
        outcome: Outcome::Approximated,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-unit-assumed",
    };
    /// A geometry item is approximated: a circle is tessellated with fewer segments than its chord
    /// tolerance asks for, a face is not planar and is triangulated in the projection onto its mean
    /// plane, or an axis of a cartesian transformation operator is parallel to an axis it must be
    /// orthogonal to and is replaced by a perpendicular direction.
    pub const GEOMETRY_APPROXIMATED: Reason = Reason {
        code: "io.ifc.geometry_approximated",
        outcome: Outcome::Approximated,
        severity: Severity::Info,
        message_id: "io-ifc-loss-geometry-approximated",
    };

    /// The writer did not write a part of the model: a degenerate body (an extrusion profile or
    /// hole of fewer than three points or without area, a depth not above the linear tolerance, a
    /// direction that is too short or in the profile plane, a B-rep face of fewer than three
    /// vertices, with an index outside its vertices or with a repeated index), or an empty property
    /// or quantity set. The record kind names the entity that was not written.
    pub const NOT_WRITTEN: Reason = Reason {
        code: "io.ifc.not_written",
        outcome: Outcome::Dropped,
        severity: Severity::Warning,
        message_id: "io-ifc-loss-not-written",
    };

    /// Every reason of this crate, for the registry check.
    pub const ALL: &[Reason] = &[
        ENTITY_PRESERVED,
        REFERENCE_UNRESOLVED,
        SCHEMA_UNRECOGNIZED,
        VALUE_UNEXPECTED,
        CYCLE_CUT,
        DUPLICATE_PLACEMENT,
        TARGET_MISSING,
        NOT_MAPPED,
        SECTION_SKIPPED,
        ARCHIVE_ENTRY_SKIPPED,
        TEXT_LATIN1,
        TEXT_CONTROL_KEPT,
        BYTES_IGNORED,
        TRAILING_BYTES,
        GEOMETRY_NOT_EVALUATED,
        GEOMETRY_DEGENERATE,
        INDEX_INVALID,
        OPENING_NOT_SUBTRACTED,
        UNIT_ASSUMED,
        GEOMETRY_APPROXIMATED,
        NOT_WRITTEN,
    ];
}

/// The rules of the conformance checker ([`crate::IfcFile::conformance`]): each has a stable code,
/// the level it belongs to and the message of a finding. A finding names the instance, its entity
/// and, where one is concerned, the attribute.
pub mod rules {
    /// A conformance rule.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rule {
        /// The stable code, `io.ifc.conformance_*`.
        pub code: &'static str,
        /// The level: 1 instance structure, 2 inverse attributes and uniqueness, 3 domain and
        /// global rules.
        pub level: u8,
        /// The Fluent message of a finding; its arguments are `instance`, `entity` and `attribute`.
        pub message_id: &'static str,
    }

    /// The entity of an instance is not declared in the file's schema.
    pub const ENTITY_UNKNOWN: Rule = Rule {
        code: "io.ifc.conformance_entity_unknown",
        level: 1,
        message_id: "io-ifc-conformance-entity-unknown",
    };
    /// The entity of a simple instance is abstract.
    pub const ENTITY_ABSTRACT: Rule = Rule {
        code: "io.ifc.conformance_entity_abstract",
        level: 1,
        message_id: "io-ifc-conformance-entity-abstract",
    };
    /// The parts of a complex instance do not form a valid combination: a part's supertype is
    /// missing, two parts are alternatives of one `ONEOF`, or a part without a subtype among the
    /// parts is abstract.
    pub const COMPLEX_INVALID: Rule = Rule {
        code: "io.ifc.conformance_complex_invalid",
        level: 1,
        message_id: "io-ifc-conformance-complex-invalid",
    };
    /// The number of parameters differs from the number of explicit attributes of the entity and
    /// its supertypes (for a part of a complex instance, of the part's entity).
    pub const ATTRIBUTE_COUNT: Rule = Rule {
        code: "io.ifc.conformance_attribute_count",
        level: 1,
        message_id: "io-ifc-conformance-attribute-count",
    };
    /// `$` stands for an attribute that is not optional, or for an element of an aggregate that is
    /// not `ARRAY OF OPTIONAL`.
    pub const VALUE_MISSING: Rule = Rule {
        code: "io.ifc.conformance_value_missing",
        level: 1,
        message_id: "io-ifc-conformance-value-missing",
    };
    /// `*` stands for an attribute that is not re-declared as derived, or a derived attribute holds
    /// something else than `*`.
    pub const DERIVED_MISPLACED: Rule = Rule {
        code: "io.ifc.conformance_derived_misplaced",
        level: 1,
        message_id: "io-ifc-conformance-derived-misplaced",
    };
    /// A value is not of the kind its type declares (integer, real, number, boolean, logical,
    /// string, binary, reference, enumeration, aggregate), or a typed parameter stands where no
    /// select is declared.
    pub const VALUE_TYPE: Rule = Rule {
        code: "io.ifc.conformance_value_type",
        level: 1,
        message_id: "io-ifc-conformance-value-type",
    };
    /// An enumeration value is not an item of its enumeration.
    pub const ENUMERATION_ITEM: Rule = Rule {
        code: "io.ifc.conformance_enumeration_item",
        level: 1,
        message_id: "io-ifc-conformance-enumeration-item",
    };
    /// A value of a select is not one of its members: a typed parameter of another type, a value
    /// without its type, or a reference to an instance of an entity the select does not list.
    pub const SELECT_MEMBER: Rule = Rule {
        code: "io.ifc.conformance_select_member",
        level: 1,
        message_id: "io-ifc-conformance-select-member",
    };
    /// A reference names an instance of an entity that is neither the declared entity nor one of
    /// its subtypes.
    pub const REFERENCE_TYPE: Rule = Rule {
        code: "io.ifc.conformance_reference_type",
        level: 1,
        message_id: "io-ifc-conformance-reference-type",
    };
    /// A reference names an instance that is not in the file.
    pub const REFERENCE_DANGLING: Rule = Rule {
        code: "io.ifc.conformance_reference_dangling",
        level: 1,
        message_id: "io-ifc-conformance-reference-dangling",
    };
    /// An aggregate has fewer or more elements than its bounds allow.
    pub const AGGREGATE_SIZE: Rule = Rule {
        code: "io.ifc.conformance_aggregate_size",
        level: 1,
        message_id: "io-ifc-conformance-aggregate-size",
    };
    /// A `SET` or an aggregate declared `UNIQUE` holds an element twice.
    pub const AGGREGATE_UNIQUE: Rule = Rule {
        code: "io.ifc.conformance_aggregate_unique",
        level: 1,
        message_id: "io-ifc-conformance-aggregate-unique",
    };
    /// A string or binary value is longer than its declared width, or of another length where the
    /// width is fixed.
    pub const VALUE_WIDTH: Rule = Rule {
        code: "io.ifc.conformance_value_width",
        level: 1,
        message_id: "io-ifc-conformance-value-width",
    };
    /// The number of instances that refer to an instance through the attribute of an inverse
    /// attribute is outside the inverse attribute's bounds.
    pub const INVERSE_CARDINALITY: Rule = Rule {
        code: "io.ifc.conformance_inverse_cardinality",
        level: 2,
        message_id: "io-ifc-conformance-inverse-cardinality",
    };
    /// Two instances share the values of the attributes of a `UNIQUE` rule.
    pub const UNIQUE_RULE: Rule = Rule {
        code: "io.ifc.conformance_unique_rule",
        level: 2,
        message_id: "io-ifc-conformance-unique-rule",
    };
    /// A domain (`WHERE`) rule of the instance's entity or a supertype evaluates to `FALSE`; the
    /// attribute of the finding is `Entity.Label`.
    pub const WHERE_RULE: Rule = Rule {
        code: "io.ifc.conformance_where_rule",
        level: 3,
        message_id: "io-ifc-conformance-where-rule",
    };
    /// A domain rule of the defined type of an attribute value of the instance (or of a type it is
    /// based on) evaluates to `FALSE`; the attribute of the finding is `Type.Label`.
    pub const TYPE_RULE: Rule = Rule {
        code: "io.ifc.conformance_type_rule",
        level: 3,
        message_id: "io-ifc-conformance-type-rule",
    };
    /// A domain rule of a global rule evaluates to `FALSE` for the file; the finding has instance 0
    /// and the attribute `Rule.Label`.
    pub const GLOBAL_RULE: Rule = Rule {
        code: "io.ifc.conformance_global_rule",
        level: 3,
        message_id: "io-ifc-conformance-global-rule",
    };

    /// Every rule, in the order the checker reports them.
    pub const ALL: &[Rule] = &[
        ENTITY_UNKNOWN,
        ENTITY_ABSTRACT,
        COMPLEX_INVALID,
        ATTRIBUTE_COUNT,
        VALUE_MISSING,
        DERIVED_MISPLACED,
        VALUE_TYPE,
        ENUMERATION_ITEM,
        SELECT_MEMBER,
        REFERENCE_TYPE,
        REFERENCE_DANGLING,
        AGGREGATE_SIZE,
        AGGREGATE_UNIQUE,
        VALUE_WIDTH,
        INVERSE_CARDINALITY,
        UNIQUE_RULE,
        WHERE_RULE,
        TYPE_RULE,
        GLOBAL_RULE,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::{codes, reasons, rules};

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
        for rule in rules::ALL {
            assert!(
                rule.code.starts_with("io.ifc.conformance_"),
                "{}",
                rule.code
            );
            assert!(seen.insert(rule.code), "{} twice", rule.code);
            assert!(seen.insert(rule.message_id), "{} twice", rule.message_id);
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
            .chain(rules::ALL.iter().map(|r| r.message_id))
            .chain(crate::psets::findings::ALL.iter().map(|f| f.message_id))
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
        for rule in rules::ALL {
            let cell = format!("| `{}` | {} |", rule.code, rule.level);
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
        for finding in crate::psets::findings::ALL {
            let cell = format!("| `{}` |", finding.code);
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
    }
}
