# Messages owned by arc-ui: its error codes and the texts of its widgets.
# Identifiers are registered in crates/arc-ui/src/error.rs and crates/arc-ui/src/text.rs.

ui-scale-out-of-range = The UI scale { $percent } % is outside 75 % to 300 %.
ui-icon-invalid = The icon { $name } is not valid.
ui-icon-too-large = The icon { $name } is too large.
ui-icon-too-many = There are too many icons.
ui-icon-duplicate = The icon { $name } is defined twice.
ui-menu-invalid = The menu or toolbar entry { $value } is not valid.
ui-menu-too-deep = The menu is nested too deeply.
ui-menu-too-large = The menu or toolbar has too many entries.
ui-command-unknown = The command { $command } is not available.
ui-keys-invalid = The key binding { $keys } is not valid.
ui-keys-conflict = The keys { $keys } for { $command } are already bound to { $other }.
ui-keys-prefix-conflict = The keys { $keys } for { $command } overlap with { $other_keys } for { $other }.
ui-keys-too-many = There are too many key bindings.
ui-dock-invalid = The panel layout is not valid.
ui-dock-too-deep = The panel layout is nested too deeply.
ui-dock-too-large = The panel layout has too many panels.
ui-dock-panel-unknown = The panel { $panel } is not in the layout.
ui-dock-panel-duplicate = The panel { $panel } is already in the layout.
ui-workspace-invalid = The workspace is not valid.
ui-workspace-unknown = There is no workspace named { $name }.
ui-workspace-too-large = There are too many workspaces or the workspace is too large.
ui-workspace-version = The workspace was saved by a newer version or in another format.
ui-quantity-out-of-range = The value must be between { $min } and { $max }.
ui-quantity-not-integer = The value must be a whole number.
ui-property-too-large = Too many objects or fields are selected to show their properties.
ui-palette-search = Search commands
ui-palette-no-results = No matching command
ui-notification-dismiss = Dismiss
ui-severity-info = Information
ui-severity-success = Success
ui-severity-warning = Warning
ui-severity-error = Error
ui-progress-cancel = Cancel
ui-dock-separator = Panel divider
ui-dock-close = Close
ui-command-line = Command
ui-property-mixed = (mixed)
ui-property-overridden = (overridden)
ui-quantity-scrub = Drag to change the value
ui-tree-filter = Filter
ui-tree-expand = Expand
ui-tree-collapse = Collapse
ui-tree-visible = Visible
ui-tree-locked = Locked
ui-diagnostics-filter = Filter diagnostics
ui-diagnostics-none = No diagnostics
ui-viewport = { $name } viewport
ui-viewport-selection = { $count ->
    [0] No objects selected
    [one] One object selected
   *[other] { $count } objects selected
}
ui-viewport-status = { $selection }. { $prompt }
ui-view-cube = View cube
ui-view-cube-top = Top
ui-view-cube-bottom = Bottom
ui-view-cube-front = Front
ui-view-cube-back = Back
ui-view-cube-left = Left
ui-view-cube-right = Right
ui-view-cube-edge = { $first }-{ $second } edge
ui-view-cube-corner = { $first }, { $second }, { $third } corner
ui-table = Table
ui-table-sorted = { $name }, sorted { $direction }
ui-table-ascending = ascending
ui-table-descending = descending
ui-table-paste-refused = { $count ->
    [one] One cell was refused.
   *[other] { $count } cells were refused.
}
ui-table-copy-too-large = The selection has too many cells to copy.
ui-table-too-large = The text is too large to paste.
ui-style-color = Color
ui-style-layer = Layer
ui-style-linetype = Linetype
ui-style-lineweight = Lineweight
ui-style-material = Material
ui-style-filter = Filter
ui-style-true-color = True color { $hex }
ui-style-too-many = There are too many style entries.
