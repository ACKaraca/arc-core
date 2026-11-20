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
    /// A command that commits silently wrote more than foreign data rows.
    pub const SILENT_SCOPE: ErrorCode = ErrorCode {
        code: "cmd.silent_scope",
        category: Category::Internal,
        message_id: "cmd-silent-scope",
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
    /// A relative or direct typed point, or `@` alone, has no last point to measure from.
    pub const PROMPT_NO_BASE: ErrorCode = ErrorCode {
        code: "cmd.prompt.no_base",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-no-base",
    };
    /// A direct distance has no direction: the host set none and `<angle` locked none.
    pub const PROMPT_NO_DIRECTION: ErrorCode = ErrorCode {
        code: "cmd.prompt.no_direction",
        category: Category::InvalidInput,
        message_id: "cmd-prompt-no-direction",
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
    /// The command controls undo (undo, redo, groups), which a preview cannot run and roll back.
    pub const PREVIEW_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.preview.unsupported",
        category: Category::Unsupported,
        message_id: "cmd-preview-unsupported",
    };
    /// Reserved: previews are dry runs and no longer roll back, so nothing produces it.
    pub const PREVIEW_ROLLBACK_FAILED: ErrorCode = ErrorCode {
        code: "cmd.preview.rollback_failed",
        category: Category::Internal,
        message_id: "cmd-preview-rollback-failed",
    };
    /// The cursor is not one this server issued for these filters.
    pub const QUERY_CURSOR_INVALID: ErrorCode = ErrorCode {
        code: "cmd.query.cursor_invalid",
        category: Category::InvalidInput,
        message_id: "cmd-query-cursor-invalid",
    };
    /// The document is at another revision than the one the cursor was issued for.
    pub const QUERY_CURSOR_STALE: ErrorCode = ErrorCode {
        code: "cmd.query.cursor_stale",
        category: Category::Conflict,
        message_id: "cmd-query-cursor-stale",
    };
    /// A selection keyword (`last`) names objects the session has no answer for.
    pub const SELECTION_UNAVAILABLE: ErrorCode = ErrorCode {
        code: "cmd.selection.unavailable",
        category: Category::NotFound,
        message_id: "cmd-selection-unavailable",
    };
    /// A selected identifier names no object of the document.
    pub const SELECTION_UNKNOWN: ErrorCode = ErrorCode {
        code: "cmd.selection.unknown",
        category: Category::NotFound,
        message_id: "cmd-selection-unknown",
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

    /// The local socket cannot be served because the user has not enabled automation.
    pub const SOCKET_DISABLED: ErrorCode = ErrorCode {
        code: "cmd.socket.disabled",
        category: Category::PermissionDenied,
        message_id: "cmd-socket-disabled",
    };
    /// A connection to the local socket did not present the session token.
    pub const SOCKET_UNAUTHORIZED: ErrorCode = ErrorCode {
        code: "cmd.socket.unauthorized",
        category: Category::PermissionDenied,
        message_id: "cmd-socket-unauthorized",
    };
    /// Local sockets are not available on this platform yet.
    pub const SOCKET_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.socket.unsupported",
        category: Category::Unsupported,
        message_id: "cmd-socket-unsupported",
    };
    /// The directory of the local socket is not a private directory of the user; the detail
    /// `reason` says why.
    pub const SOCKET_DIRECTORY: ErrorCode = ErrorCode {
        code: "cmd.socket.directory",
        category: Category::InvalidInput,
        message_id: "cmd-socket-directory",
    };
    /// The path of the local socket is longer than the platform allows; the arguments `length`
    /// and `limit` give the byte counts. The path itself is not reported.
    pub const SOCKET_PATH_TOO_LONG: ErrorCode = ErrorCode {
        code: "cmd.socket.path_too_long",
        category: Category::InvalidInput,
        message_id: "cmd-socket-path-too-long",
    };
    /// The system refused an operation on the local socket or its token; the detail `reason`
    /// says which.
    pub const SOCKET_IO: ErrorCode = ErrorCode {
        code: "cmd.socket.io",
        category: Category::Io,
        message_id: "cmd-socket-io",
    };
    /// No renderer is attached, so a view cannot be rendered.
    pub const RENDER_UNAVAILABLE: ErrorCode = ErrorCode {
        code: "cmd.render.unavailable",
        category: Category::Unsupported,
        message_id: "cmd-render-unavailable",
    };
    /// The requested width, height or pixel count is above the bound; the detail `limit` names it.
    pub const RENDER_SIZE_EXCEEDED: ErrorCode = ErrorCode {
        code: "cmd.render.size_exceeded",
        category: Category::LimitExceeded,
        message_id: "cmd-render-size-exceeded",
    };
    /// The image cannot be returned inline; the detail `limit` names the bound (`inline_bytes`,
    /// `message_bytes`). The file variant has no such bound beyond the file system: its message
    /// suggests it.
    pub const RENDER_TOO_LARGE: ErrorCode = ErrorCode {
        code: "cmd.render.too_large",
        category: Category::LimitExceeded,
        message_id: "cmd-render-too-large",
    };
    /// The renderer cannot draw a feature of the request; the detail `feature` names it.
    pub const RENDER_UNSUPPORTED: ErrorCode = ErrorCode {
        code: "cmd.render.unsupported",
        category: Category::Unsupported,
        message_id: "cmd-render-unsupported",
    };
    /// A transparent command cannot run now; the detail `reason` says why.
    pub const TRANSPARENT_REFUSED: ErrorCode = ErrorCode {
        code: "cmd.transparent.refused",
        category: Category::Conflict,
        message_id: "cmd-transparent-refused",
    };
    /// A macro input has no value at playback; the detail `input` names it.
    pub const MACRO_INPUT_MISSING: ErrorCode = ErrorCode {
        code: "cmd.macro.input_missing",
        category: Category::InvalidInput,
        message_id: "cmd-macro-input-missing",
    };
    /// A session ran a history operation, which a macro cannot express, so the recording is not
    /// a faithful macro; the recording is not written.
    pub const MACRO_UNRECORDABLE: ErrorCode = ErrorCode {
        code: "cmd.macro.unrecordable",
        category: Category::Unsupported,
        message_id: "cmd-macro-unrecordable",
    };
    /// The undo or redo stack has no step to step over; the detail `operation` names it.
    pub const HISTORY_EMPTY: ErrorCode = ErrorCode {
        code: "cmd.history.empty",
        category: Category::Conflict,
        message_id: "cmd-history-empty",
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
        SILENT_SCOPE,
        PROMPT_KIND,
        PROMPT_RANGE,
        PROMPT_OBJECT,
        PROMPT_UNREADABLE,
        PROMPT_NO_BASE,
        PROMPT_NO_DIRECTION,
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
        PREVIEW_ROLLBACK_FAILED,
        QUERY_CURSOR_INVALID,
        QUERY_CURSOR_STALE,
        SELECTION_UNAVAILABLE,
        SELECTION_UNKNOWN,
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
        SOCKET_DISABLED,
        SOCKET_UNAUTHORIZED,
        SOCKET_UNSUPPORTED,
        SOCKET_DIRECTORY,
        SOCKET_PATH_TOO_LONG,
        SOCKET_IO,
        RENDER_UNAVAILABLE,
        RENDER_SIZE_EXCEEDED,
        RENDER_TOO_LARGE,
        RENDER_UNSUPPORTED,
        TRANSPARENT_REFUSED,
        MACRO_INPUT_MISSING,
        MACRO_UNRECORDABLE,
        HISTORY_EMPTY,
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
