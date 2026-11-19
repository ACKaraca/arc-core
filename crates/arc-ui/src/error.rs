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

    /// A dock layout has a malformed node, reference, panel identifier, fraction or window rectangle.
    pub const DOCK_INVALID: ErrorCode = ErrorCode {
        code: "ui.dock.invalid",
        category: Category::InvalidInput,
        message_id: "ui-dock-invalid",
    };
    /// A dock layout nests splits deeper than the limit.
    pub const DOCK_TOO_DEEP: ErrorCode = ErrorCode {
        code: "ui.dock.too_deep",
        category: Category::LimitExceeded,
        message_id: "ui-dock-too-deep",
    };
    /// A dock layout has more nodes, panels or floating trees than the limit.
    pub const DOCK_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.dock.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-dock-too-large",
    };
    /// A dock operation names a panel the layout does not show.
    pub const DOCK_PANEL_UNKNOWN: ErrorCode = ErrorCode {
        code: "ui.dock.panel_unknown",
        category: Category::NotFound,
        message_id: "ui-dock-panel-unknown",
    };
    /// A dock layout would show a panel twice.
    pub const DOCK_PANEL_DUPLICATE: ErrorCode = ErrorCode {
        code: "ui.dock.panel_duplicate",
        category: Category::Conflict,
        message_id: "ui-dock-panel-duplicate",
    };
    /// A workspace has a malformed name, toolbar or tool set, or its export text does not parse.
    pub const WORKSPACE_INVALID: ErrorCode = ErrorCode {
        code: "ui.workspace.invalid",
        category: Category::InvalidInput,
        message_id: "ui-workspace-invalid",
    };
    /// No workspace, or no built-in workspace, has the name.
    pub const WORKSPACE_UNKNOWN: ErrorCode = ErrorCode {
        code: "ui.workspace.unknown",
        category: Category::NotFound,
        message_id: "ui-workspace-unknown",
    };
    /// Workspaces, their toolbars or tool sets, or an import text exceed the limit.
    pub const WORKSPACE_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.workspace.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-workspace-too-large",
    };
    /// An exported workspace has another format or a newer version.
    pub const WORKSPACE_VERSION: ErrorCode = ErrorCode {
        code: "ui.workspace.version",
        category: Category::Unsupported,
        message_id: "ui-workspace-version",
    };
    /// An entered quantity is outside the field's range.
    pub const QUANTITY_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "ui.quantity.out_of_range",
        category: Category::InvalidInput,
        message_id: "ui-quantity-out-of-range",
    };
    /// An entered number is not whole where a whole number is needed.
    pub const QUANTITY_NOT_INTEGER: ErrorCode = ErrorCode {
        code: "ui.quantity.not_integer",
        category: Category::InvalidInput,
        message_id: "ui-quantity-not-integer",
    };
    /// A property grid would compare more objects or fields than the limit.
    pub const PROPERTY_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.property.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-property-too-large",
    };
    /// Clipboard text for a table paste has more bytes or cells than the limit.
    pub const TABLE_TOO_LARGE: ErrorCode = ErrorCode {
        code: "ui.table.too_large",
        category: Category::LimitExceeded,
        message_id: "ui-table-too-large",
    };
    /// A table column index, or a table column position to move to, is not in the table.
    pub const TABLE_COLUMN_OUT_OF_RANGE: ErrorCode = ErrorCode {
        code: "ui.table.column_out_of_range",
        category: Category::InvalidInput,
        message_id: "ui-table-column-out-of-range",
    };
    /// A table would hold more sort keys than [`crate::table_model::MAX_SORT_KEYS`].
    pub const TABLE_SORT_TOO_MANY: ErrorCode = ErrorCode {
        code: "ui.table.sort_too_many",
        category: Category::LimitExceeded,
        message_id: "ui-table-sort-too-many",
    };
    /// A style picker was given more entries than the limit.
    pub const STYLE_TOO_MANY: ErrorCode = ErrorCode {
        code: "ui.style.too_many",
        category: Category::LimitExceeded,
        message_id: "ui-style-too-many",
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
        DOCK_INVALID,
        DOCK_TOO_DEEP,
        DOCK_TOO_LARGE,
        DOCK_PANEL_UNKNOWN,
        DOCK_PANEL_DUPLICATE,
        WORKSPACE_INVALID,
        WORKSPACE_UNKNOWN,
        WORKSPACE_TOO_LARGE,
        WORKSPACE_VERSION,
        QUANTITY_OUT_OF_RANGE,
        QUANTITY_NOT_INTEGER,
        PROPERTY_TOO_LARGE,
        TABLE_TOO_LARGE,
        TABLE_COLUMN_OUT_OF_RANGE,
        TABLE_SORT_TOO_MANY,
        STYLE_TOO_MANY,
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
