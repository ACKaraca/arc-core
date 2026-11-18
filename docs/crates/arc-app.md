# arc-app

The application shell every Arc product is wired into on the desktop, in the browser and on tablets and phones: windows, document tabs, workspace
persistence, settings, recent files, localization, logging, crash reporting, update checks, plug-in
management and the assistant panel. The architecture is specified in
[ui-kit.md](../architecture/ui-kit.md).

## Owns

- Start-up and shutdown sequence, single-instance handling and file associations.
- Window management with winit, native menus with `muda`, native dialogs with `rfd`.
- Document tabs, open and save flows, recent files, the recovery prompt and lock-holder messages.
- The settings system: registry, scopes, files and editor.
- Localization loading (Fluent catalogs, locale selection, fallbacks).
- Logging configuration and the log viewer.
- Crash capture (out of process) and opt-in reporting; opt-in update checks.
- Plug-in discovery, trust prompts and the plug-in manager UI.
- The automation enablement switch and the local socket lifecycle.
- The asynchronous network runtime used by Hub, updates and reports.
- The assistant panel.
- The browser and mobile shells: page and application lifecycle, the file-access layer, private
  storage and quota handling, offline operation and platform availability of commands.
- The reference shell (`apps/arc-shell`) and its headless companion (`apps/arc-shell-cli`).

## Stays in products

Product registration (commands, panels, tools, extractors, object types, file types), product
default settings and workspaces, product branding, About content and installers.

## Key concepts and types

| Concept | Description |
|---|---|
| `AppBuilder` | Collects product registrations and builds the application |
| `DocumentTab` | Open document with its views and panels |
| `SettingsRegistry` | Typed settings with keys, defaults, scopes and descriptions |
| `Locale` | Active language and fallback chain |
| `CrashMonitor` | Out-of-process crash capture process |
| `UpdateFeed` | Signed release feed and verification |
| `PluginManager` | Installed packages, trust tiers, grants |
| `AssistantPanel` | Provider configuration, conversation, previews and apply |
| `FileAccess` | Per-platform open, save, share and private-storage operations on whole files |
| `PlatformProfile` | Platform kind, capabilities present, limits profile and the alternatives for missing capabilities |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | All lower crates as needed; `arc-ui`, `arc-gpu`, `arc-command`, `arc-doc`, `arc-hub-api` |
| Third-party | `winit`, `muda`, `rfd`, `tokio`, `tracing-subscriber`, `fluent-bundle`, `minidumper`, `crash-handler`, `sentry`, `reqwest`, Velopack client; `wasm-bindgen`, `web-sys` and `js-sys` (browser target only, generated glue) |

## Test strategy

End-to-end tests of the reference shell with `egui_kittest`: open, edit, save, undo, crash recovery,
read-only open on a locked file; settings scope resolution tests; locale switching tests; crash
monitor tests that crash a child process and assert a minidump and no network request; update feed
signature tests with tampered feeds; keyboard-only scenario tests. Browser and mobile: lifecycle tests
that suspend the shell at random points and assert that recovery restores every committed transaction;
file-access tests per platform with failing providers; quota-exhaustion tests asserting that caches
are evicted before documents; availability tests asserting that every command with a missing
capability is disabled with a reason.

## Window-free parts as implemented

The first part of `arc-app` has no window, renderer or network dependency: it depends on
`arc-base` and, for logic only, on `fluent-bundle`, `unic-langid`, `toml`, `tracing` and
`tracing-subscriber`. It does not query the operating system; the operating-system locale list and
the configuration directory are arguments. Fallible functions return `arc_base::Error`.

### No unsolicited network (CORE-APP-019)

The crate has no network dependency and makes no network request. Network features (updates,
reports, Hub) are added in later parts behind settings that default to off.

### Settings (CORE-APP-010)

- A `Setting` has a dotted key (one to eight segments of `[a-z][a-z0-9_]*`), a type (`Bool`,
  `Int` with an inclusive range, `Float`, `Text`, `Choice` of fixed texts), a default that satisfies
  the type, the highest scope that may supply a value, and the Fluent identifier of its
  description. Registration rejects a malformed or duplicate key and a default of the wrong type.
- Scopes are, lowest first: default, system, user, workspace, document. A setting declared at scope
  `S` accepts values in scopes up to `S`; the system scope is always accepted.
- A scope's values are a TOML table; the key `ui.language` is the key `language` in the table
  `ui`. Resolution starts at the document scope and returns the first value that is in an allowed
  scope and satisfies the type, together with its scope. Skipped values are returned as issues
  (`type_mismatch`, `scope_not_allowed`), highest scope first, and resolution continues with the next
  lower scope; with no valid value the default applies and the source is `default`.
- A `Float` accepts a TOML integer and converts it; non-finite floats are invalid. Out-of-range
  integers and texts outside a `Choice` are type mismatches.
- `set` validates key, scope, type and range and fails with a code instead of storing a bad value;
  `reset` removes the value of one scope. Keys the registry does not know stay in the tables.
- The registry `SettingsRegistry::builtin` holds `ui.language`, `log.level`, `log.max_file_bytes`,
  `log.max_files` and `recent.max_entries`, all user-scoped.

### Settings files (CORE-APP-012)

- A file is read into a TOML table, so unknown keys, including nested ones, survive load and
  save. Comments and layout are not preserved and keys are written sorted.
- A missing file yields an empty table. A file larger than 1 MiB, not valid UTF-8 or not valid
  TOML is renamed to the first free `<name>.malformed-<n>` beside it and reported with
  `app.settings_file.malformed` or `app.settings_file.too_large`; the caller applies defaults. An
  existing aside file is never overwritten. Renaming is not atomic against another process
  creating the same name.
- A save writes `.<name>.tmp` beside the target, flushes it to disk and renames it over the
  target; a failure leaves the previous file in place and removes the temporary file.

### Locale selection (CORE-APP-014)

- Catalogs are parsed once and added per language tag; a catalog with a syntax error or above
  4 MiB is rejected as a whole. A `Localizer` requires the `en-US` catalog.
- The requested tags are either the operating-system list, when the `ui.language` value is
  `system` or empty, or the single configured tag. POSIX spellings (`tr_TR.UTF-8`) are accepted;
  unparsable tags are skipped.
- Negotiation takes the requested tags in order: an exact catalog match, else a catalog with the
  same language subtag, preferring one without a region. Without a match, English is selected and
  the selection reports `matched = false`.
- The fallback chain is the selected language, its region-less sibling and `en-US`; a message
  is taken from the first catalog that defines it, so a missing message falls back per message.
  An identifier no catalog defines formats as the identifier itself.
- `set_requested` rebuilds the chain from the already parsed catalogs, so a language change
  takes effect on the next `format` call without a restart. Errors format from their message
  identifier and arguments. Bidirectional isolation marks around arguments are kept.

### Logging (CORE-APP-015)

- `subscriber` builds a `tracing` subscriber that writes text records (fields as `key=value`, no
  colors) at or above the configured level to a `RotatingWriter`. Installing it globally is the
  caller's decision. There is no log viewer in this part.
- The current file is `<file_name>`; rotation renames it to `<file_name>.1` and shifts older
  files up; the file that would exceed the count is deleted. At most `max_files` files exist,
  the current one included; with one file, the file is replaced.
- Each record is one write and is never split, so a file stays within `max_file_bytes` unless a
  single record is larger. Limits: size 4 KiB to 1 GiB, count 1 to 100. Files left over from an
  earlier, larger count are not cleaned up.
- `Redacted<T>` formats as `[redacted]` through `Debug` and `Display` without reading the value.
  Document content and credentials are logged only through it; the type system cannot stop a
  caller from formatting such a value as plain text.

### Recent files (CORE-APP-007)

- The list is ordered most recent first. `touch` moves or inserts a path at the front and keeps
  its pin. The bound applies to unpinned entries (1 to 1000, default setting 20); pinned entries
  are never evicted, at most 50. Pinned entries are listed first.
- `unpin` returns the entry to its recency position, and the bound may then evict it.
- Missing files are never checked implicitly; `prune_missing` removes every entry, pinned ones
  included, for which the caller's predicate says the file is missing, and returns the paths.
- The list is its own TOML file (`entries = [{ path, pinned }]`) written through the settings-file
  functions. On load, malformed items and duplicates are skipped, surplus pins become unpinned and
  the bound is applied. Paths that are not UTF-8 are not persisted. Thumbnails are not part of
  this model.

### Reference CLI (CORE-APP-026)

`apps/arc-shell-cli` is one package with a library and a binary. The library holds the sample
document type set the reference shell uses: `shell.layer` (`name`), `shell.point` (`position`)
and `shell.line` (`start`, `end`), each version 1.0, where points and lines have an optional
`layer` reference to a layer (associative, cleared when the layer is deleted). They are
registered through `TypeRegistry::register`. The sample commands `shell.layer.create`,
`shell.point.create`, `shell.line.create`, `shell.object.move` (adds an offset to the points of
a point or line; other types are unchanged) and `shell.object.delete` (applies the deletion
rules) are undoable, need the class `edit` and are registered through `Registry::register`;
every change is a transaction of the invoker. `product(clock, owner)` bundles them for
`arc_command::headless::run`, and the binary's `main` only passes the process arguments, the
standard streams, the system clock and the edit-lock owner (user and host from the environment)
and returns the runner's exit code. The subcommands, flags and exit codes are those of the
[headless runner](arc-command.md#headless-runner-as-implemented). The layer table forbids a
dependency on an application crate, so the windowed shell cannot depend on this package as it
stands; reusing the sample set there needs a library crate with its own class.

### Error codes

The codes `arc-app` owns; messages are in `locales/en-US/arc-app.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `app.settings.invalid_key` | `invalid_input` | A setting key is not one to eight dotted lowercase segments |
| `app.settings.duplicate_key` | `conflict` | A setting key is registered twice |
| `app.settings.default_mismatch` | `invalid_input` | A setting's default does not satisfy its type |
| `app.settings.unknown_key` | `not_found` | A key is not registered |
| `app.settings.type_mismatch` | `invalid_input` | A value does not satisfy its setting's type or range (also reported as an issue by resolution) |
| `app.settings.scope_not_allowed` | `invalid_input` | A value is in a scope above the setting's highest scope, or in the default scope (also reported as an issue) |
| `app.settings.path_conflict` | `conflict` | A parent key of a setting holds a non-table value, or the key holds a table |
| `app.settings_file.read_failed` | `io` | An existing settings file cannot be read |
| `app.settings_file.write_failed` | `io` | A settings file cannot be written |
| `app.settings_file.malformed` | `corrupt_data` | A settings file is not UTF-8 TOML; it was set aside |
| `app.settings_file.too_large` | `limit_exceeded` | A settings file exceeds 1 MiB; it was set aside |
| `app.settings_file.set_aside_failed` | `io` | A malformed settings file cannot be renamed |
| `app.locale.invalid_tag` | `invalid_input` | A catalog is added under a text that is not a language tag |
| `app.locale.catalog_invalid` | `invalid_input` | A catalog has a syntax error |
| `app.locale.catalog_too_large` | `limit_exceeded` | A catalog exceeds 4 MiB |
| `app.locale.fallback_missing` | `not_found` | There is no `en-US` catalog |
| `app.log.invalid_config` | `invalid_input` | A logging limit or file name is out of range |
| `app.log.io` | `io` | A log directory or file cannot be opened |
| `app.recent.not_found` | `not_found` | A path is not in the recent-files list |
| `app.recent.too_many_pinned` | `limit_exceeded` | A fifty-first entry is pinned |

## Capabilities

### Shell and documents

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-001 | Product registration | Builds an application from product registrations of commands, panels, tools, extractors, object types and file types without product code in `arc-app`. | T1 | 0 |
| CORE-APP-002 | Windows and multi-monitor | Opens main and secondary windows, restores sizes and positions per monitor configuration and handles per-monitor scale changes. | T1 | 0 |
| CORE-APP-003 | Document tabs | Holds several open documents in tabs, each with its own views, undo history and dirty state. | T1 | 0 |
| CORE-APP-004 | Open and save flows | Opens files by dialog, drag and drop, command line argument and file association; saves, saves as and closes with unsaved-change confirmation. | T1 | 0 |
| CORE-APP-005 | Read-only and lock messages | Opens locked or newer-format documents read-only with a banner naming the lock holder or the writing version. | T1 | 0 |
| CORE-APP-006 | Recovery prompt | Lists recoverable sessions at start-up and lets the user recover, compare or discard each. | T1 | 0 |
| CORE-APP-007 | Recent files | Keeps a recent-files list with thumbnails and pinned entries, removing entries whose files no longer exist on request. | T1 | 0 |
| CORE-APP-008 | Single instance | Routes files opened while the application runs to the existing instance as new tabs. | T2 | 1 |
| CORE-APP-009 | Native menus and dialogs | Uses native menu bars where the platform has them and native file dialogs, both driven by command identifiers. | T1 | 0 |

### Settings, localization and logging

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-010 | Typed settings | Registers settings with key, type, default, scope and description; values resolve through default, system, user, workspace and document scopes. | T1 | 0 |
| CORE-APP-011 | Settings editor | Shows every setting with its effective value and source, with search and reset per setting. | T1 | 0 |
| CORE-APP-012 | Settings files | Stores user settings as TOML, preserves unknown keys, and sets aside malformed files with a notification. | T1 | 0 |
| CORE-APP-013 | Managed settings | Applies system-scope settings and locks from an administrator file that user settings cannot override. | T2 | 1 |
| CORE-APP-014 | Locale selection | Selects the UI language from settings or the operating system, switching without restart, with English fallback. | T1 | 0 |
| CORE-APP-015 | Structured logging | Writes structured logs to rotating files with a configurable level and a log viewer panel; logs never contain document content or credentials. | T1 | 0 |

### Reliability and updates

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-016 | Crash capture | Captures a minidump and the log tail from a separate monitor process when the application crashes, stored locally. | T1 | 1 |
| CORE-APP-017 | Opt-in crash reports | Shows the stored report on next start and sends it to the configured endpoint only after the user consents; off by default. | T1 | 1 |
| CORE-APP-018 | Opt-in update checks | When enabled, fetches and verifies a signed release feed and offers updates installed through Velopack; no request is made when disabled. | T1 | 1 |
| CORE-APP-019 | No unsolicited network | Makes no network request at start-up or during use unless the user enabled a feature that requires one. | T1 | 0 |

### Automation, plug-ins and assistant

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-020 | Automation switch | Enables and disables the local automation socket from settings, shows connected clients and their permissions, and remembers per-client grants. | T1 | 1 |
| CORE-APP-021 | Script panel | Runs Python scripts through the official client against the current document as one undo step, showing output and errors. | T2 | 2 |
| CORE-APP-022 | Plug-in manager | Lists installed plug-ins with trust tier, permissions and status; enables, disables, grants, revokes and removes them. | T2 | 3 |
| CORE-APP-023 | Assistant panel | Connects to a user-configured compatible model endpoint (including local models), proposes commands as previews, and applies them only after explicit approval; nothing is sent without a user action. | T3 | 5 |
| CORE-APP-024 | Hub connection panel | Signs in to a configured Hub, shows connection state, and opens Hub files into document tabs. | T1 | 2 |

### Reference shell

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-025 | Reference shell application | Opens, edits, saves and undoes a document of sample object types with a viewport, property grid, outliner and command line, exercising every phase 0 crate. | T1 | 0 |
| CORE-APP-026 | Reference headless CLI | Provides the shared CLI subcommands for the sample document type, used in CI as the protocol and CLI conformance target. | T1 | 0 |

### Browser, tablet and phone

The behavior is specified in [ui-kit.md](../architecture/ui-kit.md#browser-tablet-and-phone-targets)
and the [platform targets](../architecture/overview.md#platform-targets).

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-APP-027 | Browser shell | Hosts the application in a page canvas with page-hide and unload handling that flushes the recovery journal and warns about unsaved changes; the generated glue contains no application logic. | T3 | 5 |
| CORE-APP-028 | Mobile shell lifecycle | Handles suspend, resume, termination, memory warnings, orientation and tablet multi-window changes without losing committed work. | T3 | 5 |
| CORE-APP-029 | File-access layer | Opens, saves, shares and imports whole files through browser file handles or upload and download, the system document picker with persisted permissions, or native dialogs, using the full-rewrite save strategy where in-place writes are unavailable. | T3 | 5 |
| CORE-APP-030 | Private storage and quota | Keeps the recovery journal, settings, thumbnails and caches in private storage, shows usage against the quota, requests persistence on a user action and evicts caches before anything else. | T3 | 5 |
| CORE-APP-031 | Offline browser build | Installs the browser build for offline use from cached assets, offers a new build only after unsaved work is saved, and resumes Hub synchronization when a connection returns. | T3 | 5 |
| CORE-APP-032 | Remote worker jobs | Submits an operation that is unavailable on the device (kernel operations, PDF rasterization, 3DM import, licensed bridges) to a Hub worker after explicit user consent, shows progress and applies the result as a document change or asset. | T3 | 5 |
| CORE-APP-033 | Platform availability | Lists commands whose required capability is missing as disabled with a reason naming the alternative, from the platform profile and the command catalog. | T3 | 5 |
| CORE-APP-034 | Store-managed updates | Disables update checks where the platform store delivers updates and keeps opt-in crash reports through an in-process handler where no monitor process can run. | T3 | 5 |
| CORE-APP-035 | Web and mobile sign-in | Signs in to a Hub with the authorization code flow and PKCE through the platform web-authentication session or a page redirect, keeping tokens in the platform secure store or, in the browser, in memory for the page session. | T3 | 5 |
| CORE-APP-036 | Hub-launched browser sessions | Opens a document chosen in a Hub's web client in the browser build of the owning product, with check-out and locks governed by the Hub. | T3 | 5 |
