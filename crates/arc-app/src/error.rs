//! The error codes owned by `arc-app`. Fallible functions return [`arc_base::Error`] built from
//! one of these codes.

use arc_base::{Category, ErrorCode};

/// The error codes of this crate.
pub mod codes {
    use super::{Category, ErrorCode};

    /// A setting key is not dotted lowercase segments.
    pub const SETTINGS_INVALID_KEY: ErrorCode = ErrorCode {
        code: "app.settings.invalid_key",
        category: Category::InvalidInput,
        message_id: "app-settings-invalid-key",
    };
    /// A setting key is registered twice.
    pub const SETTINGS_DUPLICATE_KEY: ErrorCode = ErrorCode {
        code: "app.settings.duplicate_key",
        category: Category::Conflict,
        message_id: "app-settings-duplicate-key",
    };
    /// A setting's default does not satisfy its own type.
    pub const SETTINGS_DEFAULT_MISMATCH: ErrorCode = ErrorCode {
        code: "app.settings.default_mismatch",
        category: Category::InvalidInput,
        message_id: "app-settings-default-mismatch",
    };
    /// A setting key is not registered.
    pub const SETTINGS_UNKNOWN_KEY: ErrorCode = ErrorCode {
        code: "app.settings.unknown_key",
        category: Category::NotFound,
        message_id: "app-settings-unknown-key",
    };
    /// A value does not satisfy the type of its setting.
    pub const SETTINGS_TYPE_MISMATCH: ErrorCode = ErrorCode {
        code: "app.settings.type_mismatch",
        category: Category::InvalidInput,
        message_id: "app-settings-type-mismatch",
    };
    /// A value is set in a scope above the highest scope its setting allows.
    pub const SETTINGS_SCOPE_NOT_ALLOWED: ErrorCode = ErrorCode {
        code: "app.settings.scope_not_allowed",
        category: Category::InvalidInput,
        message_id: "app-settings-scope-not-allowed",
    };
    /// A key is locked by the system scope, so a value outside that scope cannot be written.
    pub const SETTINGS_LOCKED: ErrorCode = ErrorCode {
        code: "app.settings.locked",
        category: Category::PermissionDenied,
        message_id: "app-settings-locked",
    };
    /// A value cannot be stored because a parent key holds a non-table value.
    pub const SETTINGS_PATH_CONFLICT: ErrorCode = ErrorCode {
        code: "app.settings.path_conflict",
        category: Category::Conflict,
        message_id: "app-settings-path-conflict",
    };
    /// A settings file could not be read.
    pub const FILE_READ_FAILED: ErrorCode = ErrorCode {
        code: "app.settings_file.read_failed",
        category: Category::Io,
        message_id: "app-settings-file-read-failed",
    };
    /// A settings file could not be written.
    pub const FILE_WRITE_FAILED: ErrorCode = ErrorCode {
        code: "app.settings_file.write_failed",
        category: Category::Io,
        message_id: "app-settings-file-write-failed",
    };
    /// A settings file is not valid UTF-8 TOML and was set aside.
    pub const FILE_MALFORMED: ErrorCode = ErrorCode {
        code: "app.settings_file.malformed",
        category: Category::CorruptData,
        message_id: "app-settings-file-malformed",
    };
    /// A settings file exceeds the size limit and was set aside.
    pub const FILE_TOO_LARGE: ErrorCode = ErrorCode {
        code: "app.settings_file.too_large",
        category: Category::LimitExceeded,
        message_id: "app-settings-file-too-large",
    };
    /// The system (managed) settings file exists but cannot be read.
    pub const MANAGED_READ_FAILED: ErrorCode = ErrorCode {
        code: "app.managed.read_failed",
        category: Category::Io,
        message_id: "app-managed-read-failed",
    };
    /// The system (managed) settings file is too large, not UTF-8 or not valid TOML. It is not
    /// applied and nothing is renamed.
    pub const MANAGED_INVALID: ErrorCode = ErrorCode {
        code: "app.managed.invalid",
        category: Category::CorruptData,
        message_id: "app-managed-invalid",
    };
    /// The system (managed) settings file or its folder is owned by another user or writable by
    /// group or other users, so the file is not read.
    pub const MANAGED_UNTRUSTED: ErrorCode = ErrorCode {
        code: "app.managed.untrusted",
        category: Category::PermissionDenied,
        message_id: "app-managed-untrusted",
    };
    /// A malformed settings file could not be renamed aside.
    pub const FILE_SET_ASIDE_FAILED: ErrorCode = ErrorCode {
        code: "app.settings_file.set_aside_failed",
        category: Category::Io,
        message_id: "app-settings-file-set-aside-failed",
    };
    /// A text is not a language tag.
    pub const LOCALE_INVALID_TAG: ErrorCode = ErrorCode {
        code: "app.locale.invalid_tag",
        category: Category::InvalidInput,
        message_id: "app-locale-invalid-tag",
    };
    /// A message catalog does not parse.
    pub const LOCALE_CATALOG_INVALID: ErrorCode = ErrorCode {
        code: "app.locale.catalog_invalid",
        category: Category::InvalidInput,
        message_id: "app-locale-catalog-invalid",
    };
    /// A message catalog exceeds the size limit.
    pub const LOCALE_CATALOG_TOO_LARGE: ErrorCode = ErrorCode {
        code: "app.locale.catalog_too_large",
        category: Category::LimitExceeded,
        message_id: "app-locale-catalog-too-large",
    };
    /// The fallback language has no catalog.
    pub const LOCALE_FALLBACK_MISSING: ErrorCode = ErrorCode {
        code: "app.locale.fallback_missing",
        category: Category::NotFound,
        message_id: "app-locale-fallback-missing",
    };
    /// A logging configuration value is out of range.
    pub const LOG_INVALID_CONFIG: ErrorCode = ErrorCode {
        code: "app.log.invalid_config",
        category: Category::InvalidInput,
        message_id: "app-log-invalid-config",
    };
    /// A log file could not be opened, rotated or written.
    pub const LOG_IO: ErrorCode = ErrorCode {
        code: "app.log.io",
        category: Category::Io,
        message_id: "app-log-io",
    };
    /// A recorded macro could not be written to its file.
    pub const MACRO_WRITE_FAILED: ErrorCode = ErrorCode {
        code: "app.macro.write_failed",
        category: Category::Io,
        message_id: "app-macro-write-failed",
    };
    /// The instance directory is a symbolic link, is not a directory, is open to other users or
    /// has another owner than the socket this process creates in it.
    pub const INSTANCE_DIRECTORY: ErrorCode = ErrorCode {
        code: "app.instance.directory",
        category: Category::PermissionDenied,
        message_id: "app-instance-directory",
    };
    /// The single-instance channel cannot be created, connected or read.
    pub const INSTANCE_IO: ErrorCode = ErrorCode {
        code: "app.instance.io",
        category: Category::Io,
        message_id: "app-instance-io",
    };
    /// A running instance exists but did not take the message in time.
    pub const INSTANCE_BUSY: ErrorCode = ErrorCode {
        code: "app.instance.busy",
        category: Category::Conflict,
        message_id: "app-instance-busy",
    };
    /// A file argument is not valid UTF-8 and cannot be sent to the running instance.
    pub const INSTANCE_INVALID_PATH: ErrorCode = ErrorCode {
        code: "app.instance.invalid_path",
        category: Category::InvalidInput,
        message_id: "app-instance-invalid-path",
    };
    /// Too many file arguments, or a message above the size limit.
    pub const INSTANCE_MESSAGE_TOO_LARGE: ErrorCode = ErrorCode {
        code: "app.instance.message_too_large",
        category: Category::LimitExceeded,
        message_id: "app-instance-message-too-large",
    };
    /// The automation socket cannot start its serving thread.
    pub const AUTOMATION_FAILED: ErrorCode = ErrorCode {
        code: "app.automation.failed",
        category: Category::FailedOperation,
        message_id: "app-automation-failed",
    };
    /// A client label is empty, too long or contains control characters.
    pub const AUTOMATION_CLIENT_INVALID: ErrorCode = ErrorCode {
        code: "app.automation.client_invalid",
        category: Category::InvalidInput,
        message_id: "app-automation-client-invalid",
    };
    /// A thumbnail is larger than the byte or pixel bound.
    pub const THUMBNAIL_TOO_LARGE: ErrorCode = ErrorCode {
        code: "app.thumbnail.too_large",
        category: Category::LimitExceeded,
        message_id: "app-thumbnail-too-large",
    };
    /// A thumbnail is not a PNG image.
    pub const THUMBNAIL_INVALID: ErrorCode = ErrorCode {
        code: "app.thumbnail.invalid",
        category: Category::InvalidInput,
        message_id: "app-thumbnail-invalid",
    };
    /// A thumbnail file cannot be written, read as a directory or removed.
    pub const THUMBNAIL_IO: ErrorCode = ErrorCode {
        code: "app.thumbnail.io",
        category: Category::Io,
        message_id: "app-thumbnail-io",
    };
    /// A path is not in the recent-files list.
    pub const RECENT_NOT_FOUND: ErrorCode = ErrorCode {
        code: "app.recent.not_found",
        category: Category::NotFound,
        message_id: "app-recent-not-found",
    };
    /// The list already holds the maximum number of pinned entries.
    pub const RECENT_TOO_MANY_PINNED: ErrorCode = ErrorCode {
        code: "app.recent.too_many_pinned",
        category: Category::LimitExceeded,
        message_id: "app-recent-too-many-pinned",
    };

    /// A panel, tool or extractor identifier or a file extension is malformed.
    pub const PRODUCT_INVALID_ID: ErrorCode = ErrorCode {
        code: "app.product.invalid_id",
        category: Category::InvalidInput,
        message_id: "app-product-invalid-id",
    };
    /// A panel, tool, extractor or file extension is registered twice.
    pub const PRODUCT_DUPLICATE: ErrorCode = ErrorCode {
        code: "app.product.duplicate",
        category: Category::Conflict,
        message_id: "app-product-duplicate",
    };
    /// An extractor names an object type that is not registered.
    pub const PRODUCT_UNKNOWN_TYPE: ErrorCode = ErrorCode {
        code: "app.product.unknown_type",
        category: Category::NotFound,
        message_id: "app-product-unknown-type",
    };
    /// No registered file type opens or saves the path's extension.
    pub const FILE_UNSUPPORTED_TYPE: ErrorCode = ErrorCode {
        code: "app.file.unsupported_type",
        category: Category::Unsupported,
        message_id: "app-file-unsupported-type",
    };
    /// No open tab has the identifier.
    pub const DOCUMENT_UNKNOWN: ErrorCode = ErrorCode {
        code: "app.document.unknown",
        category: Category::NotFound,
        message_id: "app-document-unknown",
    };
    /// The package is open read-only and cannot be written.
    pub const DOCUMENT_READ_ONLY: ErrorCode = ErrorCode {
        code: "app.document.read_only",
        category: Category::Conflict,
        message_id: "app-document-read-only",
    };
    /// The document has no file to save in place.
    pub const DOCUMENT_NO_PATH: ErrorCode = ErrorCode {
        code: "app.document.no_path",
        category: Category::Conflict,
        message_id: "app-document-no-path",
    };
    /// The event loop or a window could not be created.
    pub const WINDOW_FAILED: ErrorCode = ErrorCode {
        code: "app.window.failed",
        category: Category::FailedOperation,
        message_id: "app-window-failed",
    };
    /// The GPU device cannot present to the window.
    pub const WINDOW_SURFACE_FAILED: ErrorCode = ErrorCode {
        code: "app.window.surface_failed",
        category: Category::FailedOperation,
        message_id: "app-window-surface-failed",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        SETTINGS_INVALID_KEY,
        SETTINGS_DUPLICATE_KEY,
        SETTINGS_DEFAULT_MISMATCH,
        SETTINGS_UNKNOWN_KEY,
        SETTINGS_TYPE_MISMATCH,
        SETTINGS_SCOPE_NOT_ALLOWED,
        SETTINGS_PATH_CONFLICT,
        SETTINGS_LOCKED,
        MANAGED_READ_FAILED,
        MANAGED_INVALID,
        FILE_READ_FAILED,
        FILE_WRITE_FAILED,
        FILE_MALFORMED,
        FILE_TOO_LARGE,
        FILE_SET_ASIDE_FAILED,
        LOCALE_INVALID_TAG,
        LOCALE_CATALOG_INVALID,
        LOCALE_CATALOG_TOO_LARGE,
        LOCALE_FALLBACK_MISSING,
        LOG_INVALID_CONFIG,
        LOG_IO,
        THUMBNAIL_TOO_LARGE,
        THUMBNAIL_INVALID,
        THUMBNAIL_IO,
        INSTANCE_DIRECTORY,
        INSTANCE_IO,
        INSTANCE_BUSY,
        INSTANCE_INVALID_PATH,
        INSTANCE_MESSAGE_TOO_LARGE,
        AUTOMATION_FAILED,
        AUTOMATION_CLIENT_INVALID,
        RECENT_NOT_FOUND,
        RECENT_TOO_MANY_PINNED,
        PRODUCT_INVALID_ID,
        PRODUCT_DUPLICATE,
        PRODUCT_UNKNOWN_TYPE,
        FILE_UNSUPPORTED_TYPE,
        DOCUMENT_UNKNOWN,
        DOCUMENT_READ_ONLY,
        DOCUMENT_NO_PATH,
        WINDOW_FAILED,
        WINDOW_SURFACE_FAILED,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-app.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-app.md");

    #[test]
    fn registry_has_no_problems() {
        assert_eq!(registry_problems(codes::ALL), vec![]);
    }

    #[test]
    fn every_code_is_in_the_catalog_and_the_specification() {
        let messages: BTreeSet<&str> = CATALOG
            .lines()
            .filter_map(|line| line.split_once(" ="))
            .map(|(id, _)| id)
            .collect();
        for entry in codes::ALL {
            assert!(
                messages.contains(entry.message_id),
                "{} not in catalog",
                entry.code
            );
            let cell = format!("| `{}` | `{}` |", entry.code, entry.category.as_str());
            assert!(SPECIFICATION.contains(&cell), "{cell} not in specification");
            assert!(entry.code.starts_with("app."));
        }
    }
}
