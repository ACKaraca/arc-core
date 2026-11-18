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

    /// The answer is not a kind of input the prompt accepts, or not one of its keyword options.
    pub const PROMPT_KIND: ErrorCode = ErrorCode {
        code: "cmd.prompt.kind",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-kind",
    };
    /// The answer is outside the prompt's constraints; the detail `reason` says which.
    pub const PROMPT_RANGE: ErrorCode = ErrorCode {
        code: "cmd.prompt.range",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-range",
    };
    /// A picked or selected object does not exist or has a type the prompt does not accept.
    pub const PROMPT_OBJECT: ErrorCode = ErrorCode {
        code: "cmd.prompt.object",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-object",
    };
    /// Typed text cannot be read as any input the prompt accepts.
    pub const PROMPT_UNREADABLE: ErrorCode = ErrorCode {
        code: "cmd.prompt.unreadable",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-unreadable",
    };
    /// Enter was given to a prompt without a default.
    pub const PROMPT_NO_DEFAULT: ErrorCode = ErrorCode {
        code: "cmd.prompt.no_default",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-no-default",
    };
    /// A prompt machine produced a prompt that accepts nothing or has malformed keyword options.
    pub const PROMPT_INVALID: ErrorCode = ErrorCode {
        code: "cmd.prompt.invalid",
        category: Category::Internal,
        message_id: "cmd-prompt-invalid",
    };
    /// An answer or the number of answers exceeds a limit.
    pub const PROMPT_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.prompt.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-prompt-limit-exceeded",
    };
    /// Input was given when no prompt is active.
    pub const PROMPT_ENDED: ErrorCode = ErrorCode {
        code: "cmd.prompt.ended",
        category: Category::Conflict,
        message_id: "cmd-prompt-ended",
    };
    /// The command line does not follow the grammar; the detail `reason` says where.
    pub const LINE_SYNTAX: ErrorCode = ErrorCode {
        code: "cmd.line.syntax",
        category: Category::InvalidInput,
        message_id: "cmd-line-syntax",
    };
    /// The command line exceeds a length, count or nesting limit.
    pub const LINE_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.line.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-line-limit-exceeded",
    };
    /// A named inline parameter does not exist.
    pub const LINE_UNKNOWN_PARAM: ErrorCode = ErrorCode {
        code: "cmd.line.unknown_param",
        category: Category::InvalidInput,
        message_id: "cmd-line-unknown-param",
    };
    /// An inline parameter is given twice.
    pub const LINE_DUPLICATE_PARAM: ErrorCode = ErrorCode {
        code: "cmd.line.duplicate_param",
        category: Category::InvalidInput,
        message_id: "cmd-line-duplicate-param",
    };
    /// More positional values than positional parameters.
    pub const LINE_TOO_MANY_VALUES: ErrorCode = ErrorCode {
        code: "cmd.line.too_many_values",
        category: Category::InvalidInput,
        message_id: "cmd-line-too-many-values",
    };
    /// An inline value does not fit its parameter.
    pub const LINE_VALUE: ErrorCode = ErrorCode {
        code: "cmd.line.value",
        category: Category::InvalidInput,
        message_id: "cmd-line-value",
    };
    /// There is no completed command to repeat.
    pub const REPEAT_NONE: ErrorCode = ErrorCode {
        code: "cmd.repeat.none",
        category: Category::NotFound,
        message_id: "cmd-repeat-none",
    };

    /// A protocol message is not UTF-8 JSON.
    pub const PROTOCOL_PARSE: ErrorCode = ErrorCode {
        code: "cmd.protocol.parse",
        category: Category::InvalidInput,
        message_id: "cmd-protocol-parse",
    };
    /// A protocol message is not a valid request; the detail `reason` says why.
    pub const PROTOCOL_INVALID_REQUEST: ErrorCode = ErrorCode {
        code: "cmd.protocol.invalid_request",
        category: Category::InvalidInput,
        message_id: "cmd-protocol-invalid-request",
    };
    /// The protocol has no method of that name.
    pub const PROTOCOL_METHOD_UNKNOWN: ErrorCode = ErrorCode {
        code: "cmd.protocol.method_unknown",
        category: Category::NotFound,
        message_id: "cmd-protocol-method-unknown",
    };
    /// The protocol method exists but this server does not provide it.
    pub const PROTOCOL_METHOD_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.protocol.method_unsupported",
        category: Category::Unsupported,
        message_id: "cmd-protocol-method-unsupported",
    };
    /// The parameters of a protocol method are malformed; the detail `reason` says why.
    pub const PROTOCOL_INVALID_PARAMS: ErrorCode = ErrorCode {
        code: "cmd.protocol.invalid_params",
        category: Category::InvalidInput,
        message_id: "cmd-protocol-invalid-params",
    };
    /// A fault escaped while handling a protocol message.
    pub const PROTOCOL_INTERNAL: ErrorCode = ErrorCode {
        code: "cmd.protocol.internal",
        category: Category::Internal,
        message_id: "cmd-protocol-internal",
    };
    /// A protocol message or the number of queued runs exceeds a limit; the detail `limit` names it.
    pub const PROTOCOL_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.protocol.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-protocol-limit-exceeded",
    };
    /// A request other than `arc.initialize` arrived before the session was initialized.
    pub const PROTOCOL_NOT_INITIALIZED: ErrorCode = ErrorCode {
        code: "cmd.protocol.not_initialized",
        category: Category::Conflict,
        message_id: "cmd-protocol-not-initialized",
    };
    /// `arc.initialize` arrived in an initialized session.
    pub const PROTOCOL_ALREADY_INITIALIZED: ErrorCode = ErrorCode {
        code: "cmd.protocol.already_initialized",
        category: Category::Conflict,
        message_id: "cmd-protocol-already-initialized",
    };
    /// No protocol version the server supports lies in the client's range.
    pub const PROTOCOL_VERSION_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.protocol.version_unsupported",
        category: Category::Unsupported,
        message_id: "cmd-protocol-version-unsupported",
    };
    /// The session has no document of that identifier.
    pub const DOCUMENT_UNKNOWN: ErrorCode = ErrorCode {
        code: "cmd.document.unknown",
        category: Category::NotFound,
        message_id: "cmd-document-unknown",
    };
    /// The expected revision is not the document's current revision.
    pub const REVISION_MISMATCH: ErrorCode = ErrorCode {
        code: "cmd.revision.mismatch",
        category: Category::Conflict,
        message_id: "cmd-revision-mismatch",
    };
    /// Preview runs are not provided by this implementation.
    pub const PREVIEW_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.preview.unsupported",
        category: Category::Unsupported,
        message_id: "cmd-preview-unsupported",
    };
    /// A root to grant is not an existing directory.
    pub const ROOTS_INVALID: ErrorCode = ErrorCode {
        code: "cmd.roots.invalid",
        category: Category::InvalidInput,
        message_id: "cmd-roots-invalid",
    };
    /// The path is not acceptable; the detail `reason` says why.
    pub const PATH_REJECTED: ErrorCode = ErrorCode {
        code: "cmd.path.rejected",
        category: Category::InvalidInput,
        message_id: "cmd-path-rejected",
    };
    /// The path is longer than the platform allows.
    pub const PATH_TOO_LONG: ErrorCode = ErrorCode {
        code: "cmd.path.too_long",
        category: Category::LimitExceeded,
        message_id: "cmd-path-too-long",
    };
    /// The path lies outside the read or write roots of the session.
    pub const PATH_OUTSIDE_ROOTS: ErrorCode = ErrorCode {
        code: "cmd.path.outside_roots",
        category: Category::PermissionDenied,
        message_id: "cmd-path-outside-roots",
    };
    /// The path names nothing.
    pub const PATH_NOT_FOUND: ErrorCode = ErrorCode {
        code: "cmd.path.not_found",
        category: Category::NotFound,
        message_id: "cmd-path-not-found",
    };
    /// The operating system refused the file operation; the detail `reason` says how.
    pub const PATH_IO: ErrorCode = ErrorCode {
        code: "cmd.path.io",
        category: Category::Io,
        message_id: "cmd-path-io",
    };
    /// The arguments of the headless runner do not follow its grammar; the detail `reason`
    /// says what is wrong.
    pub const CLI_USAGE: ErrorCode = ErrorCode {
        code: "cmd.cli.usage",
        category: Category::InvalidInput,
        message_id: "cmd-cli-usage",
    };
    /// A macro or batch text is not well formed; the details `line` and `reason` say where
    /// and why.
    pub const MACRO_MALFORMED: ErrorCode = ErrorCode {
        code: "cmd.macro.malformed",
        category: Category::InvalidInput,
        message_id: "cmd-macro-malformed",
    };
    /// A macro header names a format version this crate does not read; the detail `version`
    /// names it.
    pub const MACRO_VERSION_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.macro.version_unsupported",
        category: Category::Unsupported,
        message_id: "cmd-macro-version-unsupported",
    };
    /// A macro text, one of its lines, or a recording exceeds a limit; the detail `limit`
    /// names it.
    pub const MACRO_LIMIT_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.macro.limit_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-macro-limit-exceeded",
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
        PROMPT_KIND,
        PROMPT_RANGE,
        PROMPT_OBJECT,
        PROMPT_UNREADABLE,
        PROMPT_NO_DEFAULT,
        PROMPT_INVALID,
        PROMPT_LIMIT_EXCEEDED,
        PROMPT_ENDED,
        LINE_SYNTAX,
        LINE_LIMIT_EXCEEDED,
        LINE_UNKNOWN_PARAM,
        LINE_DUPLICATE_PARAM,
        LINE_TOO_MANY_VALUES,
        LINE_VALUE,
        REPEAT_NONE,
        PROTOCOL_PARSE,
        PROTOCOL_INVALID_REQUEST,
        PROTOCOL_METHOD_UNKNOWN,
        PROTOCOL_METHOD_UNSUPPORTED,
        PROTOCOL_INVALID_PARAMS,
        PROTOCOL_INTERNAL,
        PROTOCOL_LIMIT_EXCEEDED,
        PROTOCOL_NOT_INITIALIZED,
        PROTOCOL_ALREADY_INITIALIZED,
        PROTOCOL_VERSION_UNSUPPORTED,
        DOCUMENT_UNKNOWN,
        REVISION_MISMATCH,
        PREVIEW_UNSUPPORTED,
        ROOTS_INVALID,
        PATH_REJECTED,
        PATH_TOO_LONG,
        PATH_OUTSIDE_ROOTS,
        PATH_NOT_FOUND,
        PATH_IO,
        CLI_USAGE,
        MACRO_MALFORMED,
        MACRO_VERSION_UNSUPPORTED,
        MACRO_LIMIT_EXCEEDED,
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
