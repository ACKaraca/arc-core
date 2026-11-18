//! The error codes owned by `arc-ui`. Fallible functions return [`arc_base::Error`] built from
//! one of these codes.

use arc_base::{Category, ErrorCode};

/// The error codes of this crate.
pub mod codes {
    use super::{Category, ErrorCode};

    /// A UI scale is outside 75 % to 300 %.
    pub const SCALE_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "ui.scale.out_of_range",
        category: Category::InvalidInput,
        message_id: "ui-scale-out-of-range",
    };
    /// An icon has an invalid name or size, or its SVG source does not parse.
    pub const ICON_INVALID: ErrorCode = ErrorCode {
        code: "ui.icon.invalid",
        category: Category::InvalidInput,
        message_id: "ui-icon-invalid",
    };
    /// An icon's SVG source exceeds the size limit.
    pub const ICON_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.icon.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-icon-too-large",
    };
    /// An icon set holds more icons than the limit.
    pub const ICON_TOO_MANY: ErrorCode = ErrorCode {
        code: "ui.icon.too_many",
        category: Category::LimitExceeded,
        message_id: "ui-icon-too-many",
    };
    /// An icon name occurs twice in one set.
    pub const ICON_DUPLICATE: ErrorCode = ErrorCode {
        code: "ui.icon.duplicate",
        category: Category::Conflict,
        message_id: "ui-icon-duplicate",
    };
    /// A menu or toolbar definition has a malformed label or command identifier.
    pub const MENU_INVALID: ErrorCode = ErrorCode {
        code: "ui.menu.invalid",
        category: Category::InvalidInput,
        message_id: "ui-menu-invalid",
    };
    /// A menu definition nests deeper than the limit.
    pub const MENU_TOO_DEEP: ErrorCode = ErrorCode {
        code: "ui.menu.too_deep",
        category: Category::LimitExceeded,
        message_id: "ui-menu-too-deep",
    };
    /// A menu or toolbar definition has more items than the limit.
    pub const MENU_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.menu.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-menu-too-large",
    };
    /// A menu item or key binding names a command that is not registered for the UI surface.
    pub const COMMAND_UNKNOWN: ErrorCode = ErrorCode {
        code: "ui.command.unknown",
        category: Category::NotFound,
        message_id: "ui-command-unknown",
    };
    /// A key binding text is not a key sequence.
    pub const KEYS_INVALID: ErrorCode = ErrorCode {
        code: "ui.keys.invalid",
        category: Category::InvalidInput,
        message_id: "ui-keys-invalid",
    };
    /// A key sequence is bound to two commands.
    pub const KEYS_CONFLICT: ErrorCode = ErrorCode {
        code: "ui.keys.conflict",
        category: Category::Conflict,
        message_id: "ui-keys-conflict",
    };
    /// A key sequence is the start of another bound sequence.
    pub const KEYS_PREFIX_CONFLICT: ErrorCode = ErrorCode {
        code: "ui.keys.prefix_conflict",
        category: Category::Conflict,
        message_id: "ui-keys-prefix-conflict",
    };
    /// A key map holds more bindings than the limit.
    pub const KEYS_TOO_MANY: ErrorCode = ErrorCode {
        code: "ui.keys.too_many",
        category: Category::LimitExceeded,
        message_id: "ui-keys-too-many",
    };

    /// Every code of this crate, for the registry check.
    pub const ALL: &[ErrorCode] = &[
        SCALE_OUT_OF_RANGE,
        ICON_INVALID,
        ICON_TOO_LARGE,
        ICON_TOO_MANY,
        ICON_DUPLICATE,
        MENU_INVALID,
        MENU_TOO_DEEP,
        MENU_TOO_LARGE,
        COMMAND_UNKNOWN,
        KEYS_INVALID,
        KEYS_CONFLICT,
        KEYS_PREFIX_CONFLICT,
        KEYS_TOO_MANY,
    ];
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use arc_base::error::registry_problems;

    use super::codes;

    const CATALOG: &str = include_str!("../../../locales/en-US/arc-ui.ftl");
    const SPECIFICATION: &str = include_str!("../../../docs/crates/arc-ui.md");

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
            assert!(entry.code.starts_with("ui."));
        }
    }
}
