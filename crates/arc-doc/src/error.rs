//! The error codes owned by `arc-doc`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-doc.ftl`.

/// The error codes owned by `arc-doc`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// A type key is already registered.
    pub const TYPE_DUPLICATE_KEY: ErrorCode = ErrorCode {
        code: "doc.type.duplicate_key",
        category: Category::Conflict,
        message_id: "doc-type-duplicate-key",
    };
    /// A type key, a reference target key or a geometry encoding name is not a dotted key.
    pub const TYPE_MALFORMED_KEY: ErrorCode = ErrorCode {
        code: "doc.type.malformed_key",
        category: Category::InvalidInput,
        message_id: "doc-type-malformed-key",
    };
    /// A schema major version is 0, or a field was appended after the current minor version.
    pub const TYPE_VERSION_INVALID: ErrorCode = ErrorCode {
        code: "doc.type.version_invalid",
        category: Category::InvalidInput,
        message_id: "doc-type-version-invalid",
    };
    /// Two fields of one field list have the same name.
    pub const TYPE_DUPLICATE_FIELD: ErrorCode = ErrorCode {
        code: "doc.type.duplicate_field",
        category: Category::InvalidInput,
        message_id: "doc-type-duplicate-field",
    };
    /// A field declaration is malformed: name, aspect, bound, enum variants, nesting depth,
    /// empty record or appended-field rule.
    pub const TYPE_FIELD_INVALID: ErrorCode = ErrorCode {
        code: "doc.type.field_invalid",
        category: Category::InvalidInput,
        message_id: "doc-type-field-invalid",
    };
    /// A relationship rule contradicts the field that declares it.
    pub const TYPE_RELATIONSHIP_INCONSISTENT: ErrorCode = ErrorCode {
        code: "doc.type.relationship_inconsistent",
        category: Category::InvalidInput,
        message_id: "doc-type-relationship-inconsistent",
    };
    /// A default value does not match its field's type or bound.
    pub const TYPE_DEFAULT_INVALID: ErrorCode = ErrorCode {
        code: "doc.type.default_invalid",
        category: Category::InvalidInput,
        message_id: "doc-type-default-invalid",
    };
    /// The migrations do not form one chain from major version 1 to the current one.
    pub const TYPE_MIGRATION_CHAIN_INVALID: ErrorCode = ErrorCode {
        code: "doc.type.migration_chain_invalid",
        category: Category::InvalidInput,
        message_id: "doc-type-migration-chain-invalid",
    };
    /// A required field has no value.
    pub const VALUE_MISSING: ErrorCode = ErrorCode {
        code: "doc.value.missing",
        category: Category::InvalidInput,
        message_id: "doc-value-missing",
    };
    /// A value does not have its field's type, or an object has the wrong number of fields.
    pub const VALUE_TYPE_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.value.type_mismatch",
        category: Category::InvalidInput,
        message_id: "doc-value-type-mismatch",
    };
    /// A value lies outside its field's range or length bound, or is not a declared enum code.
    pub const VALUE_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "doc.value.out_of_range",
        category: Category::InvalidInput,
        message_id: "doc-value-out-of-range",
    };
    /// A floating-point value is NaN or infinite.
    pub const VALUE_NOT_FINITE: ErrorCode = ErrorCode {
        code: "doc.value.not_finite",
        category: Category::InvalidInput,
        message_id: "doc-value-not-finite",
    };
    /// A payload is malformed, not canonical, or decodes to values its declaration rejects.
    pub const PAYLOAD_INVALID: ErrorCode = ErrorCode {
        code: "doc.payload.invalid",
        category: Category::CorruptData,
        message_id: "doc-payload-invalid",
    };
    /// A payload exceeds a hostile-input limit.
    pub const PAYLOAD_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.payload.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-payload-limit-exceeded",
    };
    /// A payload uses an encoding this build cannot decode.
    pub const PAYLOAD_ENCODING_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "doc.payload.encoding_unsupported",
        category: Category::Unsupported,
        message_id: "doc-payload-encoding-unsupported",
    };
    /// An identifier names no live object in the snapshot.
    pub const OBJECT_NOT_FOUND: ErrorCode = ErrorCode {
        code: "doc.object.not_found",
        category: Category::NotFound,
        message_id: "doc-object-not-found",
    };
    /// A reference would point to a missing object or to an object of a type it does not allow.
    pub const REFERENCE_DANGLING: ErrorCode = ErrorCode {
        code: "doc.reference.dangling",
        category: Category::Conflict,
        message_id: "doc-reference-dangling",
    };

    /// A type key is not registered in the document's type registry.
    pub const TYPE_UNKNOWN: ErrorCode = ErrorCode {
        code: "doc.type.unknown",
        category: Category::NotFound,
        message_id: "doc-type-unknown",
    };
    /// An object is created with the identifier of an object live in the candidate.
    pub const OBJECT_DUPLICATE_ID: ErrorCode = ErrorCode {
        code: "doc.object.duplicate_id",
        category: Category::Conflict,
        message_id: "doc-object-duplicate-id",
    };
    /// A typed store has no slot or generation left for another object.
    pub const STORE_CAPACITY_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.store.capacity_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-store-capacity-exceeded",
    };
    /// A working set is published, but its base is no longer the published snapshot.
    pub const SNAPSHOT_STALE: ErrorCode = ErrorCode {
        code: "doc.snapshot.stale",
        category: Category::Conflict,
        message_id: "doc-snapshot-stale",
    };
    /// A savepoint belongs to another working set, or was released or invalidated.
    pub const SAVEPOINT_INVALID: ErrorCode = ErrorCode {
        code: "doc.savepoint.invalid",
        category: Category::InvalidInput,
        message_id: "doc-savepoint-invalid",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        TYPE_DUPLICATE_KEY,
        TYPE_MALFORMED_KEY,
        TYPE_VERSION_INVALID,
        TYPE_DUPLICATE_FIELD,
        TYPE_FIELD_INVALID,
        TYPE_RELATIONSHIP_INCONSISTENT,
        TYPE_DEFAULT_INVALID,
        TYPE_MIGRATION_CHAIN_INVALID,
        VALUE_MISSING,
        VALUE_TYPE_MISMATCH,
        VALUE_OUT_OF_RANGE,
        VALUE_NOT_FINITE,
        PAYLOAD_INVALID,
        PAYLOAD_LIMIT_EXCEEDED,
        PAYLOAD_ENCODING_UNSUPPORTED,
        OBJECT_NOT_FOUND,
        REFERENCE_DANGLING,
        TYPE_UNKNOWN,
        OBJECT_DUPLICATE_ID,
        STORE_CAPACITY_EXCEEDED,
        SNAPSHOT_STALE,
        SAVEPOINT_INVALID,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-doc.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-doc.md");

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
                .all(|entry| entry.code.starts_with("doc."))
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
                "{cell} is not in docs/crates/arc-doc.md"
            );
        }
    }
}
