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
    /// A field path does not resolve against an object's schema or does not reach a value of it.
    pub const PATH_INVALID: ErrorCode = ErrorCode {
        code: "doc.path.invalid",
        category: Category::InvalidInput,
        message_id: "doc-path-invalid",
    };
    /// A transaction would exceed its operation count or memory budget.
    pub const TXN_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.txn.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-txn-limit-exceeded",
    };
    /// An operation is applied to a state that does not hold its before values.
    pub const OP_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.op.mismatch",
        category: Category::Conflict,
        message_id: "doc-op-mismatch",
    };
    /// A deletion is refused by an on-delete `block` rule of a reference to the deleted object.
    pub const REFERENCE_DELETE_BLOCKED: ErrorCode = ErrorCode {
        code: "doc.reference.delete_blocked",
        category: Category::Conflict,
        message_id: "doc-reference-delete-blocked",
    };
    /// On-delete cascades would delete more objects than the limit allows.
    pub const REFERENCE_CASCADE_LIMIT: ErrorCode = ErrorCode {
        code: "doc.reference.cascade_limit",
        category: Category::LimitExceeded,
        message_id: "doc-reference-cascade-limit",
    };
    /// A reference field holds a number of references its cardinality does not allow.
    pub const REFERENCE_CARDINALITY: ErrorCode = ErrorCode {
        code: "doc.reference.cardinality",
        category: Category::InvalidInput,
        message_id: "doc-reference-cardinality",
    };
    /// An object would be the target of more than one owning reference.
    pub const REFERENCE_OWNER_CONFLICT: ErrorCode = ErrorCode {
        code: "doc.reference.owner_conflict",
        category: Category::Conflict,
        message_id: "doc-reference-owner-conflict",
    };
    /// A derived key does not follow the key grammar.
    pub const GRAPH_MALFORMED_KEY: ErrorCode = ErrorCode {
        code: "doc.graph.malformed_key",
        category: Category::InvalidInput,
        message_id: "doc-graph-malformed-key",
    };
    /// An evaluator is already registered for a derived key and type.
    pub const GRAPH_DUPLICATE_KEY: ErrorCode = ErrorCode {
        code: "doc.graph.duplicate_key",
        category: Category::Conflict,
        message_id: "doc-graph-duplicate-key",
    };
    /// No evaluator is registered for a derived key and the type of its object.
    pub const GRAPH_UNKNOWN_KEY: ErrorCode = ErrorCode {
        code: "doc.graph.unknown_key",
        category: Category::NotFound,
        message_id: "doc-graph-unknown-key",
    };
    /// A node takes part in a dependency cycle.
    pub const GRAPH_CYCLE: ErrorCode = ErrorCode {
        code: "doc.graph.cycle",
        category: Category::FailedOperation,
        message_id: "doc-graph-cycle",
    };
    /// A node read another node that failed, is blocked or is cyclic.
    pub const GRAPH_BLOCKED: ErrorCode = ErrorCode {
        code: "doc.graph.blocked",
        category: Category::FailedOperation,
        message_id: "doc-graph-blocked",
    };
    /// Nested demands of nodes exceed the depth limit of the graph.
    pub const GRAPH_DEPTH_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.graph.depth_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-graph-depth-exceeded",
    };
    /// A result was computed from a revision whose inputs changed since.
    pub const GRAPH_STALE: ErrorCode = ErrorCode {
        code: "doc.graph.stale",
        category: Category::Conflict,
        message_id: "doc-graph-stale",
    };

    /// A document contains a type of a newer major schema version than the registry knows.
    pub const LOAD_NEWER_MAJOR: ErrorCode = ErrorCode {
        code: "doc.load.newer_major",
        category: Category::Unsupported,
        message_id: "doc-load-newer-major",
    };
    /// Loading a document would exceed its record count or memory budget.
    pub const LOAD_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.load.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-load-limit-exceeded",
    };
    /// A stored object names a schema version its document does not declare.
    pub const LOAD_VERSION_UNDECLARED: ErrorCode = ErrorCode {
        code: "doc.load.version_undeclared",
        category: Category::CorruptData,
        message_id: "doc-load-version-undeclared",
    };
    /// A column of a stored object other than the payload is malformed.
    pub const LOAD_HEADER_INVALID: ErrorCode = ErrorCode {
        code: "doc.load.header_invalid",
        category: Category::CorruptData,
        message_id: "doc-load-header-invalid",
    };
    /// An object of an older major version could not be migrated to the current one.
    pub const MIGRATION_FAILED: ErrorCode = ErrorCode {
        code: "doc.migration.failed",
        category: Category::FailedOperation,
        message_id: "doc-migration-failed",
    };
    /// A preserved or quarantined object is changed.
    pub const OBJECT_NOT_MODIFIABLE: ErrorCode = ErrorCode {
        code: "doc.object.not_modifiable",
        category: Category::Conflict,
        message_id: "doc-object-not-modifiable",
    };
    /// A read-only document is changed.
    pub const DOCUMENT_READ_ONLY: ErrorCode = ErrorCode {
        code: "doc.document.read_only",
        category: Category::Conflict,
        message_id: "doc-document-read-only",
    };
    /// A type validator panicked while a loaded object was validated.
    pub const LOAD_VALIDATOR_PANICKED: ErrorCode = ErrorCode {
        code: "doc.load.validator_panicked",
        category: Category::Internal,
        message_id: "doc-load-validator-panicked",
    };
    /// A file is not an Arc package: not SQLite, another application id, or no `arc_meta`.
    pub const PACKAGE_NOT_A_PACKAGE: ErrorCode = ErrorCode {
        code: "doc.package.not_a_package",
        category: Category::InvalidInput,
        message_id: "doc-package-not-a-package",
    };
    /// A package has a newer format major version than this version reads.
    pub const PACKAGE_NEWER_FORMAT: ErrorCode = ErrorCode {
        code: "doc.package.newer_format",
        category: Category::Unsupported,
        message_id: "doc-package-newer-format",
    };
    /// A table or column of the package schema is missing, or the format version is malformed.
    pub const PACKAGE_SCHEMA_INVALID: ErrorCode = ErrorCode {
        code: "doc.package.schema_invalid",
        category: Category::CorruptData,
        message_id: "doc-package-schema-invalid",
    };
    /// A stored value has an unexpected storage class, text encoding or size.
    pub const PACKAGE_VALUE_INVALID: ErrorCode = ErrorCode {
        code: "doc.package.value_invalid",
        category: Category::CorruptData,
        message_id: "doc-package-value-invalid",
    };
    /// The payload hash of a stored object does not match its payload.
    pub const PACKAGE_HASH_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.package.hash_mismatch",
        category: Category::CorruptData,
        message_id: "doc-package-hash-mismatch",
    };
    /// Reading or writing a package, its temporary file or its lock file failed.
    pub const PACKAGE_IO_FAILED: ErrorCode = ErrorCode {
        code: "doc.package.io_failed",
        category: Category::Io,
        message_id: "doc-package-io-failed",
    };
    /// SQLite reports the package file as damaged.
    pub const PACKAGE_CORRUPT: ErrorCode = ErrorCode {
        code: "doc.package.corrupt",
        category: Category::CorruptData,
        message_id: "doc-package-corrupt",
    };
    /// The newly written temporary package failed its verification; the target is unchanged.
    pub const PACKAGE_VERIFY_FAILED: ErrorCode = ErrorCode {
        code: "doc.package.verify_failed",
        category: Category::Io,
        message_id: "doc-package-verify-failed",
    };
    /// A package to be created already exists.
    pub const PACKAGE_EXISTS: ErrorCode = ErrorCode {
        code: "doc.package.exists",
        category: Category::Conflict,
        message_id: "doc-package-exists",
    };
    /// The edit lock of a package is held by a live process or another host.
    pub const PACKAGE_LOCKED: ErrorCode = ErrorCode {
        code: "doc.package.locked",
        category: Category::Conflict,
        message_id: "doc-package-locked",
    };
    /// A preview image has an unknown kind, another media type, wrong pixel size or too many bytes.
    pub const PACKAGE_PREVIEW_INVALID: ErrorCode = ErrorCode {
        code: "doc.package.preview_invalid",
        category: Category::InvalidInput,
        message_id: "doc-package-preview-invalid",
    };
    /// A save has no last transaction for an object.
    pub const PACKAGE_LAST_TXN_MISSING: ErrorCode = ErrorCode {
        code: "doc.package.last_txn_missing",
        category: Category::InvalidInput,
        message_id: "doc-package-last-txn-missing",
    };
    /// A metadata entry of `arc_meta` is malformed.
    pub const PACKAGE_META_INVALID: ErrorCode = ErrorCode {
        code: "doc.package.meta_invalid",
        category: Category::CorruptData,
        message_id: "doc-package-meta-invalid",
    };

    /// An object flagged read-only would have its values or owner changed, or be deleted.
    pub const OBJECT_READ_ONLY: ErrorCode = ErrorCode {
        code: "doc.object.read_only",
        category: Category::Conflict,
        message_id: "doc-object-read-only",
    };
    /// A job or a step of its commit unwound on the document executor.
    pub const EXECUTOR_FAULT: ErrorCode = ErrorCode {
        code: "doc.executor.fault",
        category: Category::Internal,
        message_id: "doc-executor-fault",
    };
    /// The document executor stopped, or could not start, before the job ran.
    pub const EXECUTOR_STOPPED: ErrorCode = ErrorCode {
        code: "doc.executor.stopped",
        category: Category::Cancelled,
        message_id: "doc-executor-stopped",
    };
    /// The regeneration scheduler of the document stopped before the node settled.
    pub const SCHEDULER_STOPPED: ErrorCode = ErrorCode {
        code: "doc.scheduler.stopped",
        category: Category::Cancelled,
        message_id: "doc-scheduler-stopped",
    };
    /// An evaluator unwound while the scheduler evaluated its node.
    pub const GRAPH_EVALUATOR_PANICKED: ErrorCode = ErrorCode {
        code: "doc.graph.evaluator_panicked",
        category: Category::Internal,
        message_id: "doc-graph-evaluator-panicked",
    };

    /// A journal record is malformed, not canonical, or does not fit the type registry.
    pub const JOURNAL_RECORD_INVALID: ErrorCode = ErrorCode {
        code: "doc.journal.record_invalid",
        category: Category::CorruptData,
        message_id: "doc-journal-record-invalid",
    };
    /// A journal record exceeds a hostile-input limit.
    pub const JOURNAL_RECORD_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "doc.journal.record_limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "doc-journal-record-limit-exceeded",
    };
    /// A transaction is appended to a journal that already holds its identifier.
    pub const JOURNAL_DUPLICATE_TXN: ErrorCode = ErrorCode {
        code: "doc.journal.duplicate_txn",
        category: Category::Conflict,
        message_id: "doc-journal-duplicate-txn",
    };
    /// A transaction is appended whose parent is not the last transaction of the journal.
    pub const JOURNAL_PARENT_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.journal.parent_mismatch",
        category: Category::Conflict,
        message_id: "doc-journal-parent-mismatch",
    };
    /// An undo, redo or group rollback does not fit the current state of the document.
    pub const UNDO_STATE_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.undo.state_mismatch",
        category: Category::Internal,
        message_id: "doc-undo-state-mismatch",
    };
    /// The undo history is used out of order: a commit it did not prepare, a group or gesture
    /// that is not open, or an undo or redo inside an open group.
    pub const UNDO_SEQUENCE_INVALID: ErrorCode = ErrorCode {
        code: "doc.undo.sequence_invalid",
        category: Category::InvalidInput,
        message_id: "doc-undo-sequence-invalid",
    };

    /// Recovery protection is off: the recovery database could not be created or written.
    pub const RECOVERY_UNAVAILABLE: ErrorCode = ErrorCode {
        code: "doc.recovery.unavailable",
        category: Category::Io,
        message_id: "doc-recovery-unavailable",
    };
    /// A recovery database is damaged, of another application or of an unknown format.
    pub const RECOVERY_INVALID: ErrorCode = ErrorCode {
        code: "doc.recovery.invalid",
        category: Category::CorruptData,
        message_id: "doc-recovery-invalid",
    };
    /// A recovery database is held by a running session.
    pub const RECOVERY_LOCKED: ErrorCode = ErrorCode {
        code: "doc.recovery.locked",
        category: Category::Conflict,
        message_id: "doc-recovery-locked",
    };
    /// The base of a recovery database changed and no copy of it was kept.
    pub const RECOVERY_BASE_MISSING: ErrorCode = ErrorCode {
        code: "doc.recovery.base_missing",
        category: Category::NotFound,
        message_id: "doc-recovery-base-missing",
    };
    /// Recovery replayed a prefix of the records: the next record is damaged or does not apply.
    pub const RECOVERY_TRUNCATED: ErrorCode = ErrorCode {
        code: "doc.recovery.truncated",
        category: Category::CorruptData,
        message_id: "doc-recovery-truncated",
    };
    /// An incremental save is not allowed for this package or state; the caller saves by the
    /// full rewrite instead.
    pub const PACKAGE_INCREMENTAL_REFUSED: ErrorCode = ErrorCode {
        code: "doc.package.incremental_refused",
        category: Category::Conflict,
        message_id: "doc-package-incremental-refused",
    };
    /// No asset with the hash is stored in the package.
    pub const PACKAGE_ASSET_NOT_FOUND: ErrorCode = ErrorCode {
        code: "doc.package.asset_not_found",
        category: Category::NotFound,
        message_id: "doc-package-asset-not-found",
    };
    /// A stored asset or chunk does not match its hash, or its rows are malformed.
    pub const PACKAGE_ASSET_CORRUPT: ErrorCode = ErrorCode {
        code: "doc.package.asset_corrupt",
        category: Category::CorruptData,
        message_id: "doc-package-asset-corrupt",
    };
    /// A save unwound on the storage thread; the file holds the previous state.
    pub const PACKAGE_SAVE_PANICKED: ErrorCode = ErrorCode {
        code: "doc.package.save_panicked",
        category: Category::Internal,
        message_id: "doc-package-save-panicked",
    };
    /// A stored field descriptor is not valid JSON of the descriptor grammar, has another
    /// `descriptor_format`, exceeds a hostile-input limit or describes a schema the registry
    /// would reject.
    pub const DESCRIPTOR_INVALID: ErrorCode = ErrorCode {
        code: "doc.descriptor.invalid",
        category: Category::CorruptData,
        message_id: "doc-descriptor-invalid",
    };
    /// A foreign data row written by a transaction is malformed: its `object` is not 16
    /// identifier bytes, its `asset` not a 32-byte hash, a text column is empty, or the row's
    /// `id` differs from the one it is set at.
    pub const FOREIGN_INVALID: ErrorCode = ErrorCode {
        code: "doc.foreign.invalid",
        category: Category::InvalidInput,
        message_id: "doc-foreign-invalid",
    };
    /// A foreign data row to remove does not exist.
    pub const FOREIGN_NOT_FOUND: ErrorCode = ErrorCode {
        code: "doc.foreign.not_found",
        category: Category::NotFound,
        message_id: "doc-foreign-not-found",
    };
    /// A property set definition or instance is malformed: the descriptor of its properties does
    /// not parse, a property has a type property sets do not hold, or an instance does not fit
    /// its definition or its target.
    pub const PROPSET_INVALID: ErrorCode = ErrorCode {
        code: "doc.propset.invalid",
        category: Category::InvalidInput,
        message_id: "doc-propset-invalid",
    };
    /// An object already has an instance of a property set definition.
    pub const PROPSET_DUPLICATE: ErrorCode = ErrorCode {
        code: "doc.propset.duplicate",
        category: Category::Conflict,
        message_id: "doc-propset-duplicate",
    };
    /// A keyed list holds no part with the sub-object key.
    pub const PART_NOT_FOUND: ErrorCode = ErrorCode {
        code: "doc.part.not_found",
        category: Category::NotFound,
        message_id: "doc-part-not-found",
    };
    /// A field is not a keyed list, or its parts do not carry valid, distinct keys.
    pub const PART_INVALID: ErrorCode = ErrorCode {
        code: "doc.part.invalid",
        category: Category::InvalidInput,
        message_id: "doc-part-invalid",
    };
    /// A package has a hot rollback journal left by an interrupted save and was opened
    /// read-only, which cannot roll it back.
    pub const PACKAGE_RECOVERY_NEEDED: ErrorCode = ErrorCode {
        code: "doc.package.recovery_needed",
        category: Category::Conflict,
        message_id: "doc-package-recovery-needed",
    };
    /// A stored derived index row differs from the one recomputed from the payloads.
    pub const PACKAGE_INDEX_MISMATCH: ErrorCode = ErrorCode {
        code: "doc.package.index_mismatch",
        category: Category::CorruptData,
        message_id: "doc-package-index-mismatch",
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
        PATH_INVALID,
        TXN_LIMIT_EXCEEDED,
        OP_MISMATCH,
        REFERENCE_DELETE_BLOCKED,
        REFERENCE_CASCADE_LIMIT,
        REFERENCE_CARDINALITY,
        REFERENCE_OWNER_CONFLICT,
        LOAD_NEWER_MAJOR,
        LOAD_LIMIT_EXCEEDED,
        LOAD_VERSION_UNDECLARED,
        LOAD_HEADER_INVALID,
        MIGRATION_FAILED,
        OBJECT_NOT_MODIFIABLE,
        DOCUMENT_READ_ONLY,
        GRAPH_MALFORMED_KEY,
        GRAPH_DUPLICATE_KEY,
        GRAPH_UNKNOWN_KEY,
        GRAPH_CYCLE,
        GRAPH_BLOCKED,
        GRAPH_DEPTH_EXCEEDED,
        GRAPH_STALE,
        LOAD_VALIDATOR_PANICKED,
        OBJECT_READ_ONLY,
        EXECUTOR_FAULT,
        EXECUTOR_STOPPED,
        JOURNAL_RECORD_INVALID,
        JOURNAL_RECORD_LIMIT_EXCEEDED,
        JOURNAL_DUPLICATE_TXN,
        JOURNAL_PARENT_MISMATCH,
        UNDO_STATE_MISMATCH,
        UNDO_SEQUENCE_INVALID,
        PACKAGE_NOT_A_PACKAGE,
        PACKAGE_NEWER_FORMAT,
        PACKAGE_SCHEMA_INVALID,
        PACKAGE_VALUE_INVALID,
        PACKAGE_HASH_MISMATCH,
        PACKAGE_IO_FAILED,
        PACKAGE_CORRUPT,
        PACKAGE_VERIFY_FAILED,
        PACKAGE_EXISTS,
        PACKAGE_LOCKED,
        PACKAGE_PREVIEW_INVALID,
        PACKAGE_LAST_TXN_MISSING,
        PACKAGE_META_INVALID,
        SCHEDULER_STOPPED,
        GRAPH_EVALUATOR_PANICKED,
        RECOVERY_UNAVAILABLE,
        RECOVERY_INVALID,
        RECOVERY_LOCKED,
        RECOVERY_BASE_MISSING,
        RECOVERY_TRUNCATED,
        PACKAGE_INCREMENTAL_REFUSED,
        PACKAGE_ASSET_NOT_FOUND,
        PACKAGE_ASSET_CORRUPT,
        PACKAGE_SAVE_PANICKED,
        DESCRIPTOR_INVALID,
        PACKAGE_RECOVERY_NEEDED,
        PACKAGE_INDEX_MISMATCH,
        FOREIGN_INVALID,
        FOREIGN_NOT_FOUND,
        PROPSET_INVALID,
        PROPSET_DUPLICATE,
        PART_NOT_FOUND,
        PART_INVALID,
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
