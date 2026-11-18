# Messages owned by arc-app: its error codes, document banners and the descriptions of its
# built-in settings.
# Identifiers are registered in crates/arc-app/src/error.rs, application.rs and settings.rs.

app-settings-invalid-key = The setting key { $key } is not valid.
app-settings-duplicate-key = The setting { $key } is already registered.
app-settings-default-mismatch = The default value of the setting { $key } does not match its type.
app-settings-unknown-key = The setting { $key } does not exist.
app-settings-type-mismatch = The value of the setting { $key } is not valid for its type.
app-settings-scope-not-allowed = The setting { $key } cannot be set at the { $scope } level.
app-settings-path-conflict = The setting { $key } cannot be stored because another value occupies its place in the file.
app-settings-file-read-failed = The settings file { $path } could not be read.
app-settings-file-write-failed = The settings file { $path } could not be saved.
app-settings-file-malformed = The settings file { $path } is damaged. It was renamed and the defaults are used.
app-settings-file-too-large = The settings file { $path } is too large. It was renamed and the defaults are used.
app-settings-file-set-aside-failed = The damaged settings file { $path } could not be renamed.
app-locale-invalid-tag = The text { $tag } is not a language tag.
app-locale-catalog-invalid = The message catalog for { $tag } contains errors.
app-locale-catalog-too-large = The message catalog for { $tag } is too large.
app-locale-fallback-missing = The catalog for the fallback language { $tag } is missing.
app-log-invalid-config = The logging setting { $field } is not valid.
app-log-io = The log file could not be written.
app-recent-not-found = The file { $path } is not in the recent files.
app-recent-too-many-pinned = No more files can be pinned.
app-product-invalid-id = The identifier { $id } is not valid.
app-product-duplicate = { $id } is already registered.
app-product-unknown-type = The extractor { $id } names the object type { $type }, which is not registered.
app-file-unsupported-type = The file { $path } is not of a type this application can use.
app-document-unknown = The document is not open.
app-document-read-only = The file { $path } is open read-only and cannot be saved.
app-document-no-path = The document has not been saved yet. Choose a file to save it to.

app-banner-locked = { $user } on { $host } is editing this file with { $application }. It is open read-only.
app-banner-locked-unknown = Another application is editing this file. It is open read-only.
app-banner-stale-lock = This file was left locked by { $user } on { $host } with { $application }, which is no longer running. It is open read-only until you take over the lock.
app-banner-newer-format = This file was saved in format { $version }, which is newer than this application supports. It is open read-only.
app-banner-recovered-copy = The file changed after these changes were made. They were recovered onto the earlier version as a new document.
app-banner-take-over = Take over the lock

app-setting-ui-language = The language of the user interface, or "system" to follow the operating system.
app-setting-log-level = The most detailed level written to the log files.
app-setting-log-max-file-bytes = The size at which a log file is replaced by a new one, in bytes.
app-setting-log-max-files = The number of log files kept.
app-setting-recent-max-entries = The number of unpinned files kept in the recent files list.
app-setting-ui-theme = The color theme: light, dark, high_contrast_light or high_contrast_dark.
app-setting-ui-density = The spacing of the user interface: compact, standard or comfortable.
app-setting-ui-scale = The size of the user interface in percent, from 75 to 300.

app-settings-search = Search settings
app-settings-reset = Reset

app-log-empty = No log records yet.

app-recovery-title = Unsaved changes from an earlier session
app-recovery-entry = { $name }: { $records } unsaved changes
app-recovery-untitled = Untitled document
app-recovery-unreadable = These changes cannot be read.
app-recovery-recover = Recover
app-recovery-compare = Compare
app-recovery-discard = Discard
app-recovery-later = Decide later
app-recovery-compare-base = Saved file: { $path }
app-recovery-compare-no-base = The document was never saved.
app-recovery-compare-unchanged = The saved file is unchanged since these changes were made.
app-recovery-compare-changed = The saved file changed or is missing; the changes would be recovered as a new document.
app-recovery-compare-objects = The saved file holds { $objects } objects.
app-recovery-compare-records = { $records } changes would be replayed.

app-command-file-new = New
app-command-file-new-description = Opens a new, empty document.
app-command-file-open = Open…
app-command-file-open-description = Opens a document from a file.
app-command-file-save = Save
app-command-file-save-description = Saves the active document to its file.
app-command-file-save-as = Save As…
app-command-file-save-as-description = Saves the active document to another file.
app-command-file-close = Close
app-command-file-close-description = Closes the active document.
app-command-quit = Quit
app-command-quit-description = Ends the application after asking about unsaved changes.
app-command-undo = Undo
app-command-undo-description = Undoes the last change of the active document.
app-command-redo = Redo
app-command-redo-description = Redoes the last undone change of the active document.
app-command-select = Select
app-command-select-description = Changes the selection.
app-command-select-clear = Clear Selection
app-command-select-clear-description = Clears the selection.

app-dialog-unsaved = This document has unsaved changes. Save them before closing?
app-window-failed = The application window could not be opened.
app-window-surface-failed = The graphics device cannot draw into the application window.

app-menu-file = File
app-menu-edit = Edit
