//! The error codes owned by `arc-command`.
//!
//! Errors are [`arc_base::Error`] values built from the codes in [`codes`]; messages are in
//! `locales/en-US/arc-command.ftl`.

/// The error codes owned by `arc-command`.
pub mod codes {
    use arc_base::{Category, ErrorCode};

    /// No command, alias or localized name matches the typed name.
    pub const UNKNOWN: ErrorCode = ErrorCode {
        code: "cmd.unknown",
        category: Category::NotFound,
        message_id: "cmd-unknown",
    };
    /// A command specification cannot be registered: malformed identifier or message
    /// identifier, a schema outside the supported subset, or an example its schema rejects.
    pub const SPEC_INVALID: ErrorCode = ErrorCode {
        code: "cmd.spec.invalid",
        category: Category::InvalidInput,
        message_id: "cmd-spec-invalid",
    };
    /// An identifier or alias is already registered.
    pub const DUPLICATE: ErrorCode = ErrorCode {
        code: "cmd.duplicate",
        category: Category::Conflict,
        message_id: "cmd-duplicate",
    };
    /// An alias is not well formed.
    pub const ALIAS_INVALID: ErrorCode = ErrorCode {
        code: "cmd.alias.invalid",
        category: Category::InvalidInput,
        message_id: "cmd-alias-invalid",
    };
    /// A user alias to remove does not exist.
    pub const ALIAS_UNKNOWN: ErrorCode = ErrorCode {
        code: "cmd.alias.unknown",
        category: Category::NotFound,
        message_id: "cmd-alias-unknown",
    };
    /// A command was named by a deprecated alias (a warning diagnostic, not a failure).
    pub const ALIAS_DEPRECATED: ErrorCode = ErrorCode {
        code: "cmd.alias.deprecated",
        category: Category::Unsupported,
        message_id: "cmd-alias-deprecated",
    };
    /// A localized name matches more than one command.
    pub const NAME_AMBIGUOUS: ErrorCode = ErrorCode {
        code: "cmd.name.ambiguous",
        category: Category::Conflict,
        message_id: "cmd-name-ambiguous",
    };
    /// The parameter text is not JSON.
    pub const PARAMS_MALFORMED: ErrorCode = ErrorCode {
        code: "cmd.params.malformed",
        category: Category::InvalidInput,
        message_id: "cmd-params-malformed",
    };
    /// The parameters exceed a size, depth or work limit.
    pub const PARAMS_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.params.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-params-limit-exceeded",
    };
    /// The parameters violate the command's schema; the detail `violations` lists them.
    pub const PARAMS_INVALID: ErrorCode = ErrorCode {
        code: "cmd.params.invalid",
        category: Category::InvalidInput,
        message_id: "cmd-params-invalid",
    };
    /// The enabled predicate disabled the command; the argument `reason` is its message.
    pub const DISABLED: ErrorCode = ErrorCode {
        code: "cmd.disabled",
        category: Category::Conflict,
        message_id: "cmd-disabled",
    };
    /// The session lacks a permission class the command needs.
    pub const PERMISSION_DENIED: ErrorCode = ErrorCode {
        code: "cmd.permission_denied",
        category: Category::PermissionDenied,
        message_id: "cmd-permission-denied",
    };
    /// The command is not exposed on the session's surface.
    pub const NOT_EXPOSED: ErrorCode = ErrorCode {
        code: "cmd.not_exposed",
        category: Category::NotFound,
        message_id: "cmd-not-exposed",
    };
    /// The command was cancelled; its transaction was rolled back.
    pub const CANCELLED: ErrorCode = ErrorCode {
        code: "cmd.cancelled",
        category: Category::Cancelled,
        message_id: "cmd-cancelled",
    };
    /// A fault escaped the command; the document was restored.
    pub const FAULT: ErrorCode = ErrorCode {
        code: "cmd.fault",
        category: Category::Internal,
        message_id: "cmd-fault",
    };
    /// The command's result cannot be encoded or violates its result schema.
    pub const RESULT_INVALID: ErrorCode = ErrorCode {
        code: "cmd.result.invalid",
        category: Category::Internal,
        message_id: "cmd-result-invalid",
    };
    /// A command that opens no transaction asked for one.
    pub const NO_TRANSACTION: ErrorCode = ErrorCode {
        code: "cmd.no_transaction",
        category: Category::Internal,
        message_id: "cmd-no-transaction",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        UNKNOWN,
        SPEC_INVALID,
        DUPLICATE,
        ALIAS_INVALID,
        ALIAS_UNKNOWN,
        ALIAS_DEPRECATED,
        NAME_AMBIGUOUS,
        PARAMS_MALFORMED,
        PARAMS_LIMIT_EXCEEDED,
        PARAMS_INVALID,
        DISABLED,
        PERMISSION_DENIED,
        NOT_EXPOSED,
        CANCELLED,
        FAULT,
        RESULT_INVALID,
        NO_TRANSACTION,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-command.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-command.md");

    #[test]
    fn registry_has_no_problems_and_codes_are_owned() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
        assert!(codes::ALL.iter().all(|c| c.code.starts_with("cmd.")));
    }

    #[test]
    fn every_code_is_in_the_catalog_and_the_specification() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(messages.contains(entry.message_id), "{}", entry.code);
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(SPECIFICATION.contains(&cell), "{cell} is not documented");
        }
    }
}
