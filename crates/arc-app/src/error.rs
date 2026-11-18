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

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        SETTINGS_INVALID_KEY,
        SETTINGS_DUPLICATE_KEY,
        SETTINGS_DEFAULT_MISMATCH,
        SETTINGS_UNKNOWN_KEY,
        SETTINGS_TYPE_MISMATCH,
        SETTINGS_SCOPE_NOT_ALLOWED,
        SETTINGS_PATH_CONFLICT,
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
        RECENT_NOT_FOUND,
        RECENT_TOO_MANY_PINNED,
        PRODUCT_INVALID_ID,
        PRODUCT_DUPLICATE,
        PRODUCT_UNKNOWN_TYPE,
        FILE_UNSUPPORTED_TYPE,
        DOCUMENT_UNKNOWN,
        DOCUMENT_READ_ONLY,
        DOCUMENT_NO_PATH,
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
