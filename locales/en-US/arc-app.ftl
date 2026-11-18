# Messages owned by arc-app: its error codes and the descriptions of its built-in settings.
# Identifiers are registered in crates/arc-app/src/error.rs and crates/arc-app/src/settings.rs.

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

app-setting-ui-language = The language of the user interface, or "system" to follow the operating system.
app-setting-log-level = The most detailed level written to the log files.
app-setting-log-max-file-bytes = The size at which a log file is replaced by a new one, in bytes.
app-setting-log-max-files = The number of log files kept.
app-setting-recent-max-entries = The number of unpinned files kept in the recent files list.
