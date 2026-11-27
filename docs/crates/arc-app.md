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
| Arc | All lower crates as needed; `arc-ui`, `arc-gpu` and `arc-geom` (features `window` and `render`), `arc-command`, `arc-doc`, `arc-hub-api` |
| Third-party | `image` (feature `render`, PNG encoding); `winit`, `muda`, `rfd`, `tokio`, `tracing-subscriber`, `fluent-bundle`, `minidumper`, `crash-handler`, `sentry`, `reqwest`, Velopack client; `wasm-bindgen`, `web-sys` and `js-sys` (browser target only, generated glue) |

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
`arc-base` and, for logic only, on `fluent-bundle`, `unic-langid`, `toml`, `tracing`,
`tracing-subscriber`, and `serde`, `serde_json` and `schemars` for the shell command
parameters. It does not query the operating system; the operating-system locale list and
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
  `reset` removes the value of one scope. Keys the registry does not know stay in the tables. An
  internal key (below) is refused by both with `app.settings.type_mismatch`: its owning module
  changes it.
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
- `SHELL_CATALOGS` holds the Fluent sources of the shell's messages for every shipped locale (the
  `arc-app`, `arc-ui`, `arc-command`, `arc-doc`, `arc-gpu`, `arc-base` and `arc-shell` catalogs,
  concatenated per locale; `en-US` and `tr-TR`). `shell_catalogs()` returns a `Catalogs` holding
  them. A product starts from it and adds its own catalog with `Catalogs::append(tag, source)`,
  which keeps the resources already present for the language; `Catalogs::add` replaces them.
  Where two resources define a message, the one appended last is formatted, so a product can
  override a shell text on purpose. Within one catalog a message defined twice keeps its first
  definition.
- `shell_catalogs()` fails when any shipped catalog does not parse (the tests use it).
  `shell_catalogs_with_fallback()` is the run-time form: English must parse, and a language whose
  catalog does not parse is left out and returned with its error. The reference shell reports each
  such language on standard error and its messages fall back to English.
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
  files up; the file that would exceed the count is deleted. After a completed rotation at most
  `max_files` files exist, the current one included; with one file, the file is replaced. A
  rotation interrupted by a crash can leave `<file_name>.<max_files>` (the file being dropped)
  until the next rotation removes it.
- A rotation that fails (for example, another process holds a file on Windows) undoes its
  completed moves in reverse order, so the files are as they were, appending continues in the
  current file, and rotation is tried again at the next write that exceeds the limit. The record
  that triggered the rotation is written to the current file, never dropped; the file may stay
  above the limit until a rotation succeeds.
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
  the bound is applied. Paths that are not UTF-8 are not persisted.

#### Thumbnails (CORE-APP-007)

- A thumbnail is a PNG file in a directory the caller chooses (beside the list in the user data
  directory). The file name is `<hash>.png`, where `<hash>` is the 64-bit FNV-1a hash of the path
  as bytes, written in 16 lowercase hexadecimal digits. The file never holds the path and is never
  in the settings file.
- The caller supplies the PNG bytes when a document is saved or closed; the crate does not render.
  The bytes are checked without decoding pixels: at most 64 KiB; the 8-byte signature; an `IHDR`
  chunk whose declared length is 13, with width and height at least one pixel and at most 256; and
  an `IEND` chunk as the last 12 bytes (length 0, type, fixed CRC). A 24-byte stub is refused.
- `get` returns `None` for a missing file, a file that is not a regular file (a device, a named
  pipe, a symbolic link to one), one over the byte limit, or one without a valid header; an absent
  thumbnail is not an error. The type is checked before the file is opened, because opening a
  named pipe waits for a writer; the opened file is checked again and read through a limit of one
  byte past the bound.
- On Unix the thumbnail directory is created with mode 0700 and each file with mode 0600. `put`
  writes through a temporary file and a rename.
- `remove` deletes one thumbnail. `sweep` deletes every thumbnail file whose hash no entry of the
  list produces, and every temporary file `put` writes (`.<name>.png.tmp`), so evicted, removed and
  pruned entries lose their files; the caller runs it after saving the list. A thumbnail name is
  exactly 16 lowercase hexadecimal digits and `.png`; other files are left alone. A temporary file
  of a `put` running in another process may be removed by a concurrent `sweep`; the `put` then
  fails with `app.thumbnail.io` and the previous thumbnail stays.
- The list itself is not changed by thumbnails: an entry has no thumbnail field.

### Managed settings (CORE-APP-013)

- The administrator file is `managed-settings.toml` in a fixed system location: `/etc/arc-shell/`
  on Linux, `/Library/Application Support/arc-shell/` on macOS and `C:\ProgramData\arc-shell\` on
  Windows. The reference shell reads it at start; no environment variable or argument changes the
  path, because a user-chosen file could lock nothing. The Windows path is fixed to the system
  drive; it will move to the known-folder API later.
- On Unix, before the file is read, `apply_system` requires the file to be a regular file (a
  symbolic link is refused) owned by uid 0, in a folder owned by uid 0, and neither writable by
  group or other users. Otherwise the file is refused with `app.managed.untrusted` and nothing is
  read. A missing file passes.
- On Windows, before the file is read, `apply_system` requires the file to be a regular file (a
  link is not accepted) and both the file and its folder to be owned by an administrative principal:
  the built-in Administrators group, Local System or the TrustedInstaller service
  (`arc_winsec::owner_is_administrative`, [ADR-0017](../adr/0017-windows-security-ffi-crate.md)).
  Any user can create folders and files under `C:\ProgramData`, so a file that fails the check, or
  whose check fails, is ignored as if absent: nothing is read, no key is locked, the start is not
  refused, and a warning with the reason (`not_regular`, `not_administrative` or `check_failed`) is
  logged and returned in `Managed::ignored_file`, which the reference shell prints to standard error
  (the log subscriber does not exist yet at that point). Consequences: a folder created by a standard
  user before deployment makes the file ignored, so an administrator must create the folder; a file
  whose owner is the creating account rather than the Administrators group (the default-owner policy
  "object creator") is ignored; an administrative file that is momentarily unreadable is ignored, not
  fatal. A file that passes the check and is then unusable stops the start like on the other
  platforms. The check examines the owner only, not the access control list: an account that can
  write to an administratively owned folder or file despite its owner defeats it. A deployment guide
  that requires non-default folder permissions needs the list inspected too.
- Its top-level values are the system scope. The table `managed` holds the lock list,
  `managed.locked`, an array of setting keys. The table is reserved: no setting key starts with
  `managed`.
- A locked key takes its value from the system scope only. A value in the user, workspace or
  document scope is skipped with an issue of kind `locked`, which resolution reports like the other
  issues. A locked key without a system value takes its default.
- `set` and `reset` in a scope other than the system scope fail with `app.settings.locked`, which
  names the key. The settings editor row carries `locked`, and the windowed editor disables the
  row's edit controls.
- Each apply replaces the whole lock set and the system scope together: a key removed from the
  file is unlocked. The new state is computed first and installed only when the system table is
  accepted, so a refused file leaves the previous locks and values in force.
- Lock-list entries that are not registered keys, that are not text, or a `locked` value that is
  not an array are ignored and logged with `tracing` at warning level. Only the key is logged.
- The file is read, never written or renamed. A missing file applies nothing. A file that exists
  but cannot be read, is too large, not UTF-8, not TOML or refused by the Unix ownership check stops
  the reference shell from starting: it prints the localized error and exits with a non-zero code.
  Every such error is fatal (`managed::is_fatal`).
- Decisions: fail closed. A damaged administrator file, and on Unix an untrusted one, stops the start, so a user
  cannot run without the locks it sets; the rejected alternative applied nothing and started, which
  lets anyone who can damage the file remove the locks. On Windows an untrusted file is ignored instead, because any user can plant one under `C:\ProgramData`: refusing the start would let any user block every other user, and a planted file cannot be told from an administrator's by its content. The rejected alternative left Windows unchecked. A reset in the user scope of a locked key
  is refused too; the rejected alternative allowed it, since it only removes a value that is
  ignored anyway, but the rule is one check for all writes. A malformed file is not set aside,
  unlike user files; renaming inside a system directory needs rights the application does not have.
- Known gap: the reference shell logs the ignored entries before the log subscriber exists, so
  those records are not written. Logging is installed when the event loop starts.

### Run directory

The single-instance channel and the automation socket live in a short run directory, because a
Unix socket path holds at most 103 bytes (`SOCKET_PATH_LIMIT` of `arc-command`). `run_dir::choose`
(Unix) picks it from the candidates that the caller reads (`RunInputs`); it reads no environment
variable. The order, first match wins:

1. `<runtime>/<app>`, when the runtime directory (`XDG_RUNTIME_DIR`) is set, absolute and not
   empty, and the folder is a directory that is not a symbolic link, owned by the current user, with
   mode 0700. A missing folder is created with mode 0700. A folder that fails a check is not used,
   and the next candidate is tried.
2. The per-user data directory, when every socket path below it fits the limit.
3. `<temporary>/<app>-<uid>`, created with mode 0700 when missing. An existing folder must pass the
   same checks as in step 1 (`app.instance.directory` with reason `not_directory`, `owner` or
   `not_private`, otherwise `app.instance.io` with reason `create`). A folder whose socket paths do
   not fit is `app.instance.path_too_long`, and the call fails.

The user id is the owner of a probe file the call creates in the parent folder and removes again,
so no libc call is needed. The chosen folder holds the sockets' own folders `instance` and
`automation`, which the socket code creates with its own private-directory checks; those checks
apply to whatever is chosen. The reference shell passes the choice to `SingleInstance::acquire`
(`<run>/instance`) and to the automation switch (`<run>/automation`). When no folder is chosen, the
shell starts as the only instance with automation unavailable and reports the reason once on
standard error. Windows uses the data directory.

### Single instance (CORE-APP-008)

- `SingleInstance::acquire(dir, args)` returns `Acquisition::Primary(instance)` or
  `Acquisition::Forwarded`. On Unix the channel is `instance.sock` in `dir`, a Unix domain socket
  (on Windows see below). `dir` is
  created with mode 0700 when missing. An existing `dir` is refused (`app.instance.directory`) when
  it is a symbolic link, is not a directory, has any group or other permission, or when the socket
  this process creates in it has another owner.
- The socket path `dir/instance.sock` must fit the platform limit (103 bytes on Unix). `acquire`
  checks it first, before `dir` is created, the lock is taken or any connection is made, and fails
  with `app.instance.path_too_long`. It does not fall back to "no running instance": the caller
  decides. The reference shell then starts without single-instance forwarding.
- A start first connects and writes one line, `{"paths":[...]}`, of absolute paths, then closes its
  write side. A running instance that accepts the line makes the start `Forwarded`; the caller
  exits. Each argument is made absolute with `std::path::absolute` before it is sent. An argument
  that is not UTF-8 is an error.
- No listener, or a socket file that refuses connections, is a stale socket: it is removed only
  after the connection failed, and the start becomes the instance. A socket file that is another
  kind of file is left alone and the start fails with `app.instance.io`. A running instance that
  accepts the connection but does not take the write within the 500 ms deadline gives
  `app.instance.busy`.
- `acquire` decides and binds under an exclusive lock on `instance.lock` in the same directory
  (created with mode 0600, released when `acquire` returns). Two starts that find the same stale
  socket therefore yield one instance and one forwarding start.
- `SingleInstance::poll(now)` never blocks. It takes the clock value `now` from the caller and
  returns the paths of messages complete at that time. Accepted connections are non-blocking and
  keep their bytes between calls; a message completes when its newline arrives, so a line split
  across writes is delivered after the last one. Limits, all documented in the code:
  - at most 8 connections are accepted per call, and at most 16 wait for their line at a time; the
    oldest waiting connection is dropped beyond that;
  - a connection is dropped when 500 ms have passed on the clock since it was accepted, when its
    line exceeds 64 KiB without a newline, when it is closed before a newline, or when the line is
    not a valid message (not JSON, `paths` not an array, more than 64 paths);
  - a call returns at most 64 paths, and the paths returned in any one second of the clock (a
    window that starts at the first call that returns a path after the previous window) are at
    most 256; paths beyond a bound are dropped and logged without their content.
  Relative entries inside an accepted message are skipped.
- The receiving side opens each path with `Application::open`, so the application's own checks
  apply. The reference shell polls every 250 ms while its listener exists; a forwarded file
  therefore opens within a quarter of a second.
- Windows: the channel is the named pipe `\\.\pipe\arc-instance-<hash>` (the name scheme of
  `arc-command`'s pipe naming, with the instance directory in the hash), with the owner-only
  security descriptor `D:P(A;;GA;;;OW)`. Its first instance is created exclusively; a start that
  finds the name taken forwards instead. The running instance proves itself before a start sends
  its paths: it writes a random secret to `instance.secret` in the directory when it starts
  listening, and sends the secret as one line when a client connects. The client reads that line
  within 2 s, compares it with the file (a file that is empty or not a full 64-digit secret proves
  nothing), and only then sends `{"paths":[...]}`, which it writes within 2 s. The instance answers
  `ok` once the message is queued for `poll`; the client waits for that line within 2 s. The
  connection itself is waited for within 500 ms. If the proof does not arrive, does not match, the
  write does not finish, or the `ok` does not arrive within its deadline, the start is not a
  forward: it returns `app.instance.busy` and runs as an independent instance. This holds even when
  the primary is slow to acknowledge, and the paths are never sent to a server that failed the
  proof. A start that gets `busy` after its paths were written cannot know whether the primary
  opened them; the paths may be opened once by the primary and once by the start. Each connection has one reader thread. At most 16 readers are
  within the deadline and at most 64 have passed it without a message; a connection beyond those
  is closed unread. The message bounds of Unix apply. `poll` never blocks. The 500 ms deadline is
  not enforced on the reader threads: a silent connection keeps its reader until its peer writes or
  closes. The instance directory is protected by the user profile's access control only; a
  directory outside the profile exposes the secret to other users. The Unix wire format is
  unchanged: the proof and the acknowledgment exist on Windows only.
- A platform with neither channel: `acquire` returns `Primary` and `poll` returns nothing, so the
  application starts normally.
- Decisions: a start with no file arguments also forwards and exits, so a second launch focuses
  the running instance's open flow; the rejected alternative starts a second instance without
  files. Paths are made absolute without resolving symbolic links, which needs no file to exist;
  the rejected alternative canonicalizes, which fails for a path that was removed and resolves
  links the user did not name. Polling is a timer; the rejected alternative is a wake-up from a
  listener thread, which needs a cross-thread queue into the single-threaded event loop.
- Shortcut: `std` offers no connect timeout for Unix sockets, so connecting to a listener whose
  backlog is full can wait. Upgrade when a non-blocking connect is available.
- Shortcut: the lock is a blocking `File::lock`; a holder that stalls delays other starts. The
  holder's work is bounded by the 500 ms forwarding deadline. Upgrade with a try-lock and a retry
  limit if that ever shows.

### Automation switch (CORE-APP-020)

- The setting `automation.enabled` is a truth value, off by default, settable in the user scope. A
  locked value (CORE-APP-013) refuses writes, so a system that locks it off keeps it off.
- The setting `automation.grants` is registered with the internal setting type: the system scope
  can lock it (a locked key refuses `grant` and `revoke` with `app.settings.locked`), but it is not
  resolved or edited through the settings store and is hidden from the settings editor.
- While the system scope locks `automation.grants`, the effective grants are exactly the system
  scope's value of the key; the user scope's grants are ignored, and with no system value there are
  none. An administrator can therefore forbid the user's grants and define the grants in the same
  lock. Without a lock the effective grants are the user scope's.
- `Automation::sync(store, dir, product, renderer)` applies the effective value. On: binds a
  client-table socket (`LocalSocket::bind_clients`) named `automation` in `dir` and serves the
  product's commands on a thread the module owns, with one client per effective grant. A socket
  that already listens is reconciled with the effective grants: a client whose grant is no longer
  effective, or changed, is removed (its connections close), and a grant without a client is added.
  Off: stops that thread, which closes the connections and removes the socket and the token files.
  `start` and `stop` do the same without the setting; dropping an `Automation` stops it. The
  reference shells call `sync` from their event loop when the effective value changes, with the
  directory `automation` in the user data directory and the product's renderer.
- Grants are stored in the user settings file as `automation.grants`, an array of tables
  `{ client = "<label>", permissions = ["read", ...] }`. A label is 1 to 64 bytes with no control
  characters; `grant` replaces an earlier grant of the label, and an empty permission set or
  `revoke` removes it. Malformed entries and repeated labels are skipped on read.
- Each stored grant is a client of the table. The application chooses the client's identifier
  (`client-<n>`, unique for the life of the switch) and sends it as `clientName` in the handshake; the
  client's token is the content of the file `automation@client-<n>.token` in the socket directory,
  which `Automation::clients` reports as `token_path`. The label, the identifier and the permissions
  are shown; the token is never shown, logged or put in an error. The client is listed before the
  socket admits it, so a connection that authenticates at once is listed; a client the socket
  refuses is removed from the list again, and its grant stays in the settings.
- `Automation::grant` and `Automation::revoke` change the settings and the running socket at once,
  with no restart. `grant` on a label that has a client removes that client first, which closes its
  connections and refuses its old token, then adds a client with the new permissions and a new token
  (replacement). `revoke` removes the client, closes its connections and removes its token file; its
  next request is not answered and a new connection with its old token is `cmd.socket.unauthorized`.
  Other clients are not affected.
- The socket's server ceiling is every permission class, so a client's grant is the ceiling that
  applies (the connection runs with the intersection, as the socket specification says). Rejected:
  the union of all grants as one ceiling, which the single-token socket used; it lets one client's
  permissions reach another client's connections.
- The connection model: the socket's `ConnectionObserver` records each connection that passes its
  handshake (its identifier, the client's label, its client identifier and permissions) and forgets
  it when it closes. The model is one mutex-protected snapshot that the observer updates on the
  socket's threads and `connections()` and `clients()` copy on the UI thread; the observer never
  calls the socket. `subscribe()` returns a channel that gets a message after each change; messages
  coalesce, so a reader re-reads the snapshot each time. A test blocks on that channel.
- A serving error (the listener fails) clears the running state: the socket handle is released, the
  files are removed when the serving thread ends, the model forgets the clients and connections,
  and `failure()` reports `app.automation.failed`. A failed `start` (for example `cmd.socket.directory`
  when the directory is not private) records its code as the failure and leaves no files. `sync`
  with the switch on starts again after either, since `start` joins the ended thread first.
- The served documents: `sync` serves none, so document commands answer with an unknown-document
  error until the shell publishes its open documents to the serving thread. `start` takes the
  documents, which the tests use. Server-side log records go to `tracing` by event name only, never
  by detail, since some details are client labels.
- Not provided yet: the panel that shows the clients and connections in the window (the model is
  readable; the window does not draw it), and grants from the window (the `grant` and `revoke`
  methods exist, the editor does not call them).

### View rendering (CORE-CMD-026, feature `render`)

- The module `render` (feature `render`, on with `window`) renders the default view of a snapshot
  headless. A product draws either through `Extract`, a function from a snapshot object to render
  items (lines, curves, meshes), or through `Drawing`, a function from a snapshot to one
  `arc_gpu::Extraction` per object (items with their geometries, such as the fills and outline
  glyphs of `arc_gpu::draft::space_extractions`). The extractions are applied to a scene, the view
  is zoomed to what they draw and shaded on white through `arc-gpu` on the adapter its selection
  rules choose, and encoded as PNG. `render::render` (per-object) and `render::render_drawing`
  return the PNG, the number of objects drawn and the adapter (`name`, `software`, `fallback`).
- The frame covers the exact bounds of curves: a curve's bounding box from `arc-geom` (an arc's
  bulge included), the points of line items and the bounding sphere of the mesh objects. The view
  looks down −z, so the frame uses the planar extent only: the center is the middle of the planar
  box that holds both, and the radius reaches the farthest box corner and the far side of the
  sphere. A sphere alone frames itself with its own radius. Rays and lines frame nothing.
- `SnapshotRenderer::new(extract)` and `SnapshotRenderer::with_drawing(drawing)` implement
  `arc_command::ViewRenderer`. The first takes the per-object function, the second the drawing. It
  accepts the view `default` and the display mode `shaded`, and refuses a camera and section planes
  with `cmd.render.unsupported` (detail `feature`), since nothing draws them yet.
- The product attaches its renderer with `ProductBuilder::view_renderer`; the windowed shell
  attaches `SnapshotRenderer` over its extractor, so the automation socket answers `view.render`.
  `arc-shell-cli` attaches it to the `serve` and `mcp` subcommands. The renderer runs on the
  thread of the session that asked.
- The product gives the keyword `last` its meaning with `ProductBuilder::last_objects`. The hook
  reaches every session the application serves: the default session, the shell's sessions, the
  command line and the automation socket (see [arc-command](arc-command.md#context-sessions-and-predicates)).
- `SnapshotRenderer` keeps one GPU device: the first render that needs it creates it, and later
  renders reuse it. A failed creation is not kept, so the next render tries again. The renderer
  takes its lock before the extraction, so renders run one at a time and a session waits its turn.
  An extraction error is returned to the client as it is (`Extract` returns a `Result`), and no
  device is created for it. The reference shell's extractor returns its errors this way too.
- Feature `render` adds the dependencies `arc-gpu`, `arc-geom` and `image` (PNG encoding, the
  workspace entry with the `png` feature only). `arc-app` is layer 6, above `arc-gpu` (layer 5).

### Reference CLI (CORE-APP-026)

`apps/arc-shell-cli` is one package with a library and a binary. The sample document type set
the reference shells use lives in the library crate `crates/arc-shell-sample` (class `layer(4)`,
[ADR-0001](../adr/0001-enforced-layering.md#amendment-sample-set-of-the-reference-shell)), which
the CLI's library re-exports unchanged: `shell.layer` (`name`), `shell.point` (`position`)
and `shell.line` (`start`, `end`), each version 1.0, where points and lines have an optional
`layer` reference to a layer (associative, cleared when the layer is deleted). They are
registered through `TypeRegistry::register`. The sample commands `shell.layer.create`,
`shell.point.create`, `shell.line.create`, `shell.object.move` (adds an offset to the points of
a point or line; other types are unchanged), `shell.object.set_field` (sets `name` of layers,
`position` of points or `start` and `end` of lines on the listed `objects` from the canonical
JSON `value`, the shape of the property grid's edit intent; any other field or value is
`cmd.params.invalid`) and `shell.object.delete` (applies the deletion rules) are undoable, need the class `edit` and are registered through `Registry::register`;
every change is a transaction of the invoker. `product(clock, owner)` bundles them for
`arc_command::headless::run`, and the binary's `main` only passes the process arguments, the
standard streams, the system clock and the edit-lock owner (user and host from the environment)
and returns the runner's exit code. The subcommands, flags and exit codes are those of the
[headless runner](arc-command.md#headless-runner-as-implemented), plus the product subcommand
`render`; `arc_shell_cli::run` dispatches `render` and passes every other subcommand to the
runner.

`render <file> --output <png> [--view <name>] [--width <n>] [--height <n>]` (also
`--read-root`, `--write-root`, `--json`) renders the package headless:

- The package is opened read-only. `render::extract_sample` turns each point into a dot and
  each line into a segment, 3 px wide, color sRGB (40, 40, 40); layers draw nothing. The
  windowed shell has its own copy of this extractor in `apps/arc-shell`, since one application
  cannot depend on the other and `arc-shell-sample` (layer 4) may not depend on `arc-gpu`
  (layer 5).
- The view is the default view: orthographic, looking down −z with +y up, zoomed with
  `arc_gpu::navigation::zoom_to_sphere` to the sphere around the box of all drawn points,
  enlarged by 10 %; with nothing drawn it is centered on the origin, 10 m tall. The sample set
  defines no named views, so `--view` accepts only `default`; another name is `cmd.cli.usage`
  with reason `unknown_view`.
- The image is `--width` × `--height` pixels (default 800 × 600, decimal `u32`), drawn by
  `arc_app::render` (see [View rendering](#view-rendering-core-cmd-026-feature-render)) on the
  adapter its selection rules choose, which falls back to a software adapter when nothing better
  exists. The PNG (8-bit RGBA) is written through `Roots::create_write`, so `--output` must lie
  inside a write root (`cmd.path.outside_roots`, exit 5, without one).
- Standard output is one JSON object: `view`, `width`, `height`, `objects` (the number of
  objects drawn) and `adapter` (`name`, `software`, `fallback`). Errors are reported like the
  runner's, with the same exit codes; `gpu.*` errors map by their categories.

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
| `app.settings.locked` | `permission_denied` | A write or reset outside the system scope names a key that the system scope locks |
| `app.managed.read_failed` | `io` | The system settings file exists but cannot be read; the application does not start |
| `app.managed.untrusted` | `permission_denied` | On Unix, the system settings file or its folder is not owned by uid 0, is not a regular file, or is writable by group or other users; the application does not start |
| `app.automation.failed` | `failed_operation` | The automation socket's serving thread cannot be started, or it ends with an error (recorded as the failure) |
| `app.automation.client_invalid` | `invalid_input` | A client label is empty, longer than 64 bytes or contains control characters |
| `app.managed.invalid` | `corrupt_data` | The system settings file is larger than 1 MiB, not UTF-8 or not valid TOML; the application does not start |
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
| `app.macro.write_failed` | `io` | A recorded macro cannot be written to the chosen file |
| `app.recent.not_found` | `not_found` | A path is not in the recent-files list |
| `app.recent.too_many_pinned` | `limit_exceeded` | A fifty-first entry is pinned |
| `app.thumbnail.too_large` | `limit_exceeded` | A thumbnail exceeds 64 KiB or 256 pixels on a side |
| `app.thumbnail.invalid` | `invalid_input` | A thumbnail is not a PNG image: no signature, no 13-byte IHDR chunk or no IEND chunk at the end |
| `app.thumbnail.io` | `io` | A thumbnail file cannot be written, read as a directory or removed |
| `app.instance.directory` | `permission_denied` | The instance directory is a symbolic link, not a directory, open to other users, or owned by another user than the socket |
| `app.instance.io` | `io` | The single-instance socket cannot be created, connected, configured or read |
| `app.instance.busy` | `conflict` | A running instance exists but did not take the message, prove itself or acknowledge it in time, or failed the proof; the start runs as an independent instance |
| `app.instance.invalid_path` | `invalid_input` | A file argument is not valid UTF-8 and cannot be sent |
| `app.instance.message_too_large` | `limit_exceeded` | More than 64 file arguments, or a message above 64 KiB |
| `app.instance.path_too_long` | `invalid_input` | The socket path of the single-instance channel is longer than the platform allows (103 bytes on Unix); the arguments `length` and `limit` give the byte counts, and the path is not reported. Nothing is created or connected to |
| `app.product.invalid_id` | `invalid_input` | A panel, tool or extractor identifier is not a dotted key, or a file extension is malformed or can neither be opened nor saved |
| `app.product.duplicate` | `conflict` | A panel, tool, extractor or file extension is registered twice |
| `app.product.unknown_type` | `not_found` | An extractor names an object type that is not registered |
| `app.document.held` | `conflict` | `save_as` names a file another tab of the application holds |
| `app.file.unsupported_type` | `unsupported` | No registered file type with the needed capability matches a path's extension |
| `app.document.unknown` | `not_found` | No open tab has the identifier |
| `app.document.read_only` | `conflict` | A save targets a package that is open read-only |
| `app.document.no_path` | `conflict` | A document never saved is saved in place; it needs save as |
| `app.window.failed` | `failed_operation` | The event loop or the main window cannot be created |
| `app.window.surface_failed` | `failed_operation` | The GPU device cannot present to the window (no surface, no surface format) |

## Application model as implemented

The application model has no window either. It depends on `arc-doc` (feature `package`) and
`arc-command` (feature `headless`), takes the data directory, limits, identifier generators and
the recent-files list as arguments (`AppConfig`), and answers no user decision itself: the caller
passes the answers. Fallible functions return `arc_base::Error` with the codes in the
[table above](#error-codes) or those of the crates below.

### Product registration (CORE-APP-001)

- `ProductBuilder::new(name, version)` collects a product's registrations: object types and
  evaluators (`types_mut`, `with_types`, `evaluators_mut`) and commands (`commands_mut`,
  `with_commands`) are the `arc-doc` and `arc-command` registries themselves, with their own
  checks; settings (`setting`, beside the built-in ones of `SettingsRegistry::builtin`), file
  types, panels, tools and extractors are added here. `build(clock, owner)` returns a `Product`.
- A `Product` contains the `arc_command::headless::Product` (name, version, types, evaluators,
  commands, clock and edit-lock owner) and returns it from `headless()`, so the headless runner
  and the application run the same registration; the application-only parts are kept beside it.
- A `FileType` is an extension (1 to 16 lowercase ASCII letters and digits, matched
  case-insensitively), the description's message identifier and whether it opens and saves.
  Every file type is a document package unless the product registers its own format for the
  extension (see [Native formats](#native-formats-and-openers)); the extension routes a path to
  the product either way.
- Panels and tools are `Entry`s (a dotted-key identifier, unique per kind, the label's message
  identifier and a `Factory`); an `Extractor` has an identifier, the object type keys it handles
  (each must be registered when the product is built) and a `Factory`. `Factory::create` returns
  `Box<dyn Any + Send>`, which the windowed shell downcasts to the type it registered; any
  `Fn() -> T` closure is a factory. This crate never calls a factory.
- Tools are registered (`ProductBuilder::tool`, `Product::tools`) and kept, but no surface
  instantiates or renders them yet: the shell does not read `Product::tools`. Tools are not a
  working surface; a product that needs its own tool surface uses an interaction controller
  (below).
- `ProductBuilder::path_dialog(command, PathDialog)` makes the shell ask for a file when the
  command starts without `path` (see the menus and dialogs section).
- `ProductBuilder::interaction(factory)` (feature `window`) registers the product's interaction
  controller: a factory that creates one `Interaction` per tab (see
  [Interaction controller](#interaction-controller-extension)).

### Document tabs (CORE-APP-003)

- `Application::new(product, config)` holds the product, a `SettingsStore` over the product's
  settings, the recent files and the tabs in display order. A `Tab` has a `TabId` that is never
  reused, not even by another `Application` of the process (one counter), its document (with
  its own undo history), the package when it has a file, the
  recovery journal, the banners and the view states (`views`, `views_mut`; opaque
  `Box<dyn Any + Send>` values the shell keeps).
- A tab is dirty when its document's revision differs from the revision of its saved state and
  the document is not at the saved state's place in its undo history (`Document::undo_position`).
  Both are read before a save, so a commit that races the save leaves the tab dirty, never falsely
  clean. Undoing back to the saved state is a new revision but the saved place, so the tab is clean
  again; a redo forms a new undo step, so redoing past the save leaves it dirty. A new document is
  clean until edited; a recovered document is dirty until saved.
- A silent commit (`Context::commit_silently`) counts as a modification: it is a committed
  transaction, so the revision moves and a clean package tab becomes dirty. The rows it adds are
  document content: a package closed without saving them would lose them, so the close prompt is
  correct even though the user made no edit. Hosts get no flag that tells such a commit from an
  edit (only the declared undo behavior).
- The title is the file name, or none for a document never saved, which the shell names. `activate`, `move_tab` (the position is clamped) and `close` manage the
  order; the tab opened last becomes active, and closing the active tab activates the next one,
  else the previous one.
- `close(tab, ask)` closes a clean tab at once. For a dirty tab `ask` is called and its answer
  decides: `Save` saves first and keeps the tab open with the error when the save fails
  (`app.document.no_path` for a document never saved); `Discard` deletes the recovery journal;
  `Cancel` keeps it open. Closing releases the edit lock; a recovery journal without records is
  deleted.
- `invoke(tab, session, invocation)` runs a product command on the tab's document; on a
  read-only tab the `edit` class is removed from the session, so editing commands fail with
  `cmd.permission_denied`. `undo(tab)` and `redo(tab)` step the document's undo history and
  wait for the step; they return false when there was nothing to step. `undo_many(tab, count)` and
  `redo_many(tab, count)` step several at once in one transaction ([`Document::undo_many`]).

### Open and save flows (CORE-APP-004)

- `open(path)` is the one entry for a dialog, drag and drop, a command-line argument and a file
  association. A path already open activates its tab. Otherwise the extension must be a file type
  that opens (`app.file.unsupported_type`), the package is opened with `Package::open` for
  editing under the product's lock owner and loaded with `Package::open_document`; a document
  open for editing gets a recovery journal (the package's `document_id`, or a generated one, and
  a fresh session identifier for each journal) whose failure to start is a banner, not an error.
  The path moves to the front of the recent files. On an error nothing is opened.
- `new_document()` opens a document without a file, protected by a journal without base. When
  the product named a new-document command (`ProductBuilder::new_document_command`), the command
  runs on the new document as its starting content with `Invocation::initial`: the transaction is
  recorded in the journal (so the document saves, and recovers with its content) but commits as
  bookkeeping, journal kind `merge`, so it is no undo step and the tab starts clean. It is accepted
  only on a document at revision 0 (`cmd.silent_scope` otherwise). `new_document_in(language)`
  also gives the command the language tag as the session setting `app.language`, so the
  starting content is named in the interface language.
- `save(tab)` saves in place with `Package::save_document`, passing the recovery journal, so the
  journal holds nothing to replay afterwards. A document never saved fails with
  `app.document.no_path`, a read-only package with `app.document.read_only`. A
  `doc.recovery.unavailable` error after the file was written counts as saved and becomes a
  banner. Any other failure keeps the document open and dirty and returns the error.
- `save_as(tab, target)` requires a file type that saves. A missing target is created with the
  current package's metadata (or a new document identifier) and the document is written to it by
  the full rewrite; an existing target is opened for editing and overwritten (asking for the
  overwrite is the dialog's task), or refused with `app.document.read_only` when it opens
  read-only. On success the old package is dropped, which releases its lock, the tab belongs to
  the target, its read-only banners are cleared and a recovery journal starts when it had none.
  On failure a target this call created is removed and the tab is unchanged.
- `take_over(tab)` takes over a stale edit lock after the user confirmed it
  (`Package::take_over`) and starts a recovery journal.

### Native formats and openers

- `ProductBuilder::opener(extension, NativeFormat)` registers a product's own format for one
  extension. The extension must be well formed and unique (`app.product.invalid_id`,
  `app.product.duplicate`) and must not be the package extension declared with
  `ProductBuilder::package_extension`. `ProductBuilder::build` fails with `app.product.invalid_id`
  for an opener whose extension has no declared file type.
- `NativeFormat::new(open, save)`: `open(path, config, session)` reads the file and returns an
  `arc_doc::Document` (the application passes the document configuration: identifiers, session
  and limits). `save(document, path, session)` writes a document to a file. What the callbacks
  receive is in [Native session](#native-session).
- A save creates an empty temporary file beside the target before `save` runs: exclusively, with
  `Roots::create_new`, inside the session's write roots, under a name
  `.<name>.<process id>.<number>.tmp`. A name taken by a stale file is tried with the next number,
  at most eight names, then the save fails with `cmd.path.io` (reason `exists`). `save` receives the
  path of that existing, empty file, not the target, and writes it by its own means. On success the
  application renames the file over the target with `Roots::replace_file`; on failure it removes
  the temporary file and the target stays as it was. A relative target is resolved against the
  current directory first.
- The temporary file takes the permissions of an existing target before `save` runs: the mode bits on
  Unix, the read-only attribute on Windows. They are set on the handle of the exclusive create, before
  any content is written, so the saved file keeps the target's mode (a 0600 file is still 0600 after
  a save). The owner, the group, access control lists and extended attributes are not carried over,
  and a target that does not exist gets the default permissions. A read-only target is refused before
  anything is created, with `cmd.path.io` (reason `denied`): the rename would replace it silently, and
  `save` does not run.
  session they are given, so the helper has no reason to be public.
- `open(path)` with a registered extension calls the product's `open`; the package reader is the
  default for every other extension. The file must still be a declared file type that opens. A
  path already open in a tab activates it: two paths name the same file when their canonical
  forms are equal (a path that cannot be canonicalized is compared as given).
- A native document is clean when opened and has no package, no edit lock and no recovery
  journal. It is not protected against a crash: the application shows the banner
  `app-document-unprotected` on it and on any document converted to the native format. A native
  file is not locked against another process; the product owns the file's integrity.
- `save(tab)` of a native tab requires that the file type allows saving (`app.file.unsupported_type`
  otherwise), then calls the product's writer, and the tab is clean. A failed save keeps the tab
  dirty and leaves the file as it was.
- `save_as(tab, target)` with a target that has a registered opener: a native tab is written by
  the product's writer and moves to the target; a new document that was never saved is written
  the same way and becomes a native tab, its recovery journal discarded (the file holds its
  changes). A package document is refused with `app.file.unsupported_type` and nothing is
  written. A native tab saved to a package target is refused with the same error.
- `save_as` refuses a target that another tab holds (`app.document.held`), and writes nothing.

### Native session

- The session the callbacks receive is the application's session (`Application::set_session`;
  default: the user-interface surface, the `read`, `edit`, `file_read` and `file_write` classes, and
  the product's clock) with its read roots replaced: the read root is the directory subtree that
  contains the file. For `save` the write root is that same directory subtree, and the read root
  too. Surface, permissions, limits and clock come from the application's session; roots never do.
  The application has no roots policy of its own, so the file's directory is the policy.
- `set_session` is therefore a trust point for the surface, the permission classes, the limits and
  the clock of product commands run by the native callbacks. It is not a trust point for roots.
- `set_session` keeps the product's `last` hook: the session given takes the product's hook in place
  of its own. The session's settings are its own; the shell's user-interface and command-line
  sessions do not share settings with each other (only the hook).
- A relative path is resolved against the current directory before the directory is taken. The
  directory must exist (`cmd.roots.invalid` otherwise); a path without a parent is
  `app.file.unsupported_type`.
- A command run through `arc_command::Invoker` with the session can reach files only inside those
  subtrees, through the same checks as any other file access.
- The application's own writer helper is not exported: products reach files through the session
  they are given, so the helper has no reason to be public.

### Read-only and lock banners (CORE-APP-005)

A `Banner` is a message identifier and named arguments. A package that opens read-only shows:

| Cause | Message | Arguments |
|---|---|---|
| Live lock with a readable holder | `app-banner-locked` | `user`, `host`, `application` |
| Live lock that cannot be read | `app-banner-locked-unknown` | none |
| Stale lock of this host | `app-banner-stale-lock` | `user`, `host`, `application` |
| Newer format minor version | `app-banner-newer-format` | `version` (`major.minor`) |

A package of a newer format major version is not opened at all: `open` fails with
`doc.package.newer_format` (arguments `version` and `current`), as the package reader refuses it
without touching the file or creating a lock. Errors that do not stop an open or a save
(`doc.recovery.unavailable`, `doc.recovery.truncated`) are banners with the error's message
identifier and arguments.

### Recovery prompt (CORE-APP-006)

- `recoverable()` lists the recovery databases under `<data_dir>/recovery` that no running
  session holds (`arc_doc::package::recovery::scan`): each `Orphan` names its document, base
  file, record count and times. The application's own open documents hold their journals and are
  not listed.
- `recovery_offers()` is `recoverable()` for the start-up prompt: a readable orphan with no
  records (for example an application killed right after opening a document) has nothing to
  recover, since its base file, if any, is the saved file itself, so it is deleted instead of
  listed. An unreadable orphan is kept and offered for discard.
- `compare(orphan)` changes nothing and replays nothing. It reports the base path, whether the
  base still has the recorded fingerprint, the number of objects in the base file as it is now
  (from `package::inspect`; none without a base or when it cannot be inspected) and the number
  of recorded transactions that recovery would replay. A per-object difference needs the replay
  and is not provided.
- `recover(orphan)` replays the orphan into a new, dirty, active tab and deletes the orphan. With
  an unchanged base the tab belongs to the base file and takes its edit lock (or opens read-only
  with a lock banner); with a changed base (banner `app-banner-recovered-copy`) or no base the
  tab has no path and is saved with `save_as`. A replay that stopped early adds the banner of its
  `doc.recovery.truncated` error. On an error the orphan is kept.
- `discard_recovery(orphan)` deletes the database after the user chose to discard it
  (`doc.recovery.locked` when a running session holds it).

### Settings editor (CORE-APP-011)

- `settings_editor::rows(store)` lists every registered setting in key order with its effective
  value, source scope, highest allowed scope, description message identifier and skipped
  values.
- `search(store, localizer, query)` keeps the rows whose key or localized description contains
  the trimmed query, ignoring case; a blank query keeps every row.
- `set(store, scope, key, value, file)` and `reset(store, scope, key, file)` go through
  `SettingsStore::set` and `reset` and then write the scope's table to `file` when one is given,
  with the settings-file save; a reset that removed nothing writes nothing. A failed write
  returns `app.settings_file.write_failed` and leaves the value changed in memory.

The integration test `application` builds the product from the sample set and covers the
registrations, independent dirty state and undo of two documents, the close decision table, open,
save, save as and reopen, a failed save, a second instance on a locked package, the newer-major
fixture, recovery after an application was dropped with unsaved changes, and the settings editor.

## Windowed shell as implemented

### Window state (CORE-APP-002)

`window_state` is a model without a window and is always built.

- A monitor is its position and size in physical pixels and its scale factor; scale factors
  outside 0.25 to 16 or not finite count as 1. The configuration key lists each monitor with a
  nonzero size as `WxH+X+Y@S` (scale with three decimals), sorted and joined by `;`, so it does
  not depend on the order the platform lists monitors in; at most 16 monitors count.
- `WindowStates::save(monitors, id, placement)` stores a window's position, inner size (physical
  pixels) and maximized state under the current key; the key becomes the most recent one. At
  most 32 configurations and 16 windows per configuration are kept, the oldest dropped first.
  Window identifiers are 1 to 64 bytes of lowercase letters, digits, `.`, `-` and `_` starting
  with a letter; malformed identifiers, coordinates beyond 2^20 and zero sizes are ignored.
- `restore(monitors, primary, id)` takes the placement saved for the current configuration,
  else the most recent one of the window from any configuration, and clamps it: the window
  moves onto the monitor it overlaps most, entirely inside it, or is centered on the primary
  monitor when it overlaps none; sizes shrink to the monitor and grow to 160 pixels where the
  monitor allows. Without a saved placement or a usable monitor the caller's default applies.
- `rescale(size, old, new)` keeps the logical size when a window's scale factor changes.
- The states are a TOML table (`configurations = [{ key, windows = [{ id, x, y, width, height,
  maximized }] }]`) loaded and saved through the settings-file functions; malformed entries,
  duplicates and entries beyond the limits are skipped on load.

### Shell commands (CORE-APP-009)

`commands::register` adds the commands the shell carries out itself, exposed on the UI and
command-line surfaces only (so automation never reaches a dialog); none opens a transaction:
`app.file.new`, `app.file.open` and `app.file.save_as` (optional `path`; without one the
platform shows a file dialog), `app.file.save` (a document never saved asks for a file),
`app.file.close` (a dirty document asks whether to save), `app.shell.quit`, `app.edit.undo`,
`app.edit.redo`, `app.select.set` (`objects`, `mode` `replace`, `add` or `toggle`, and the
viewport's `gesture`), `app.select.clear`, `app.macro.record_start` and `app.macro.record_stop`
(see the frame composition). Invoked through `arc_command::Invoker` they change
nothing. The labels are in `locales/en-US/arc-app.ftl`. `shell::default_menu` is a File and an
Edit menu of these commands.

### Interaction controller (extension, feature `window`)

Module `shell::interaction`. A product that runs its own command line, prompts and viewport input
registers a factory; each tab's view then owns one `Interaction`, and the shell routes to it while
it is registered. Without a registration the shell behaves as before.

- `Interaction` (`Send`) has five methods, called on the shell's frame: `pointer(cx, &PointerEvent)
  -> bool` for each viewport pointer event before the widget handles it (`true` consumes it),
  `dynamic(cx, &DynamicEvent)` for dynamic input edits, `frame(cx)` once per viewport frame before `output` is read (default no-op; it refreshes what the controller derives from the document and selection, and must not change the document), `line(cx, LineAction) -> Result<()>` for
  every action of the command panel (a submitted line or option, Escape as `Cancel`, `Back` or
  `ClearInput`, undo and redo as `Invoke` or `CancelThen`, and repeat), and `output(&self) ->
  InteractionOutput` for what the shell shows. `closing(cx)` has a default no-op. The shell calls
  it once when a tab closes through the shell (the close command, the tab's close dialog, or
  quitting), after the close is decided and any save it makes has succeeded, and before the tab
  leaves the application. The tab's document is still open then, and `cx.document()` works. The
  controller is dropped afterwards, so `closing` is the place to end state it left open, such as
  an undo group it began. A cancelled or failed close does not call it. The hook must not commit
  a change to the document: it runs after any save, so the change is not in the saved file. On the
  save path the tab's recovery journal is closed, not discarded, and a change committed in the hook
  leaves a record that keeps the journal for recovery on the next start. Closing a tab with
  `Application::close` without the shell drops the controller without this call.
- `InteractionOutput`: `status` (a `LineStatus`: active, answers, selection, repeatable), `prompt`
  (a `Prompt`, which labels the command panel), `options` (a `PromptControls` for the tool option
  bar: keyword buttons and parameter fields), `history` (lines shown above the field, at most 500,
  most recent last), `history_first` (the sequence number of `history[0]`, default 0: a controller that
  drops its oldest lines raises it by the number dropped, so the lines the shell keeps among them stay
  in place) and `reports_results` (default `false`: the shell adds `Done: <command>` for each
  undoable command the controller ran through `InteractionContext::invoke`, and `*Cancel*` for Escape
  at a prompt, including one the controller's `output` gives that the history has not recorded yet,
  and before the line of a command started from a menu, a key, a button or a typed command when it
  ends the prompt (the prompt is gone or another one asks afterwards; claimed commands and
  selection changes excepted); `true` when the controller writes those lines itself), `overlays` (`ProductOverlay`s), `dynamic` (a `DynamicOutput`: a prompt message
  and `DynamicField`s, shown beside the pointer) and `selection` (`Option<Vec<ObjectId>>`: the
  objects of the controller's own that the viewport highlights and counts; `None`, the default,
  shows the tab's selection, which follows a click, Escape or a command at once, and the shell then
  also sets `status.has_selection` from it, so Escape on a selection clears it) and `pointer_icon`
  (`Option<egui::CursorIcon>`, default `None`: the viewport keeps the crosshair; a value hides the
  crosshair over the viewport and sets that mouse pointer, for example a resize arrow over a handle).
- `InteractionContext` gives what a controller needs: `document()` (the tab's `arc_doc::Document`),
  `invoke(Invocation)` (a document command, seeing the tab's selection when it names none, through `Application::invoke` in the shell's command
  session), `session()`, `read_only()`, `selection()` (the tab's shell selection), `document_path()` (the file of the tab's document, `None`
  before its first save), `camera()` (the
  tab's camera, which is the view the controller's input is in), `language()` (the interface language
  tag, also the session setting `app.language` of `session()`), `decimal_separator()` (`,` or `.`
  for that language), `ask(Intent)` and `tab()`. The foundation has no
  space concept, so the camera stands for the active space.
- A product controller must honor `read_only()`. A read-only tab is a package open without write
  access (`Application::is_read_only_tab`, the rule `Application::invoke` also applies). On such a
  tab the session of `session()` has no `edit` permission, so `invoke` refuses editing commands,
  but a controller that changes `document()` itself, without `invoke`, is not stopped by that. It
  must check `read_only()` before any such change and must not change the document when it is
  true. A closed tab counts as read-only.
- While a controller is registered: the viewport gets the controller's overlays (after the
  product's own `ShellConfig::overlays`, so level labels stay), dynamic panel and highlight, and no
  prompt of the shell's own line (the widget does not answer prompts itself); the command panel
  draws the controller's prompt, history and option bar and sends its lines and options to `line`;
  dynamic events go to `dynamic`. The shell's own command line does not prompt.
- Shell commands are the shell's, whoever controls the line, unless the controller claims them
  with `Interaction::handles(command) -> bool`. The default claims nothing; a controller with its own
  undo, such as one that steps back one answer, claims `app.edit.undo`, `app.edit.redo` and
  `app.select.clear`. A submitted line (the first word, with its parameters) or an
  `Invoke` or `CancelThen` intent naming an unclaimed shell command never reaches `line`: the shell
  cancels a running tool first (`LineAction::Cancel`),
  except for save and save as, and carries the command out. Every other command a menu, a key binding (`Action::Start`) or a toolbar button
  starts reaches `line` as `LineAction::Invoke(intent)` with its parameters, so a menu item and a
  toolbar button start a tool the same way (the commands of panels, such as the property grid's
  edit and a click's selection command, still run on the shell's command line); a controller maps
  `Invoke` of a command that needs prompts to the start of its own prompts.
- Keys over the viewport. `Interaction::key(cx, &KeyInput) -> KeyReply` (default `Pass`) is offered, on
  the frame and after the shell's key bindings, the Tab key, key presses with the command key that
  no binding took (the platform's copy, cut and paste events arrive as `C`, `X` and `V` with the
  command key) and typed text, while the pointer is over the viewport or the viewport has the focus
  and no text field has it. `Consumed` removes the event from the frame, `Text(s)` replaces typed
  text (what a controller held back, for the command line), `Pass` leaves it: typing then starts the
  command line as before. `KeyInput` and `KeyReply` are `non_exhaustive`.
- Command-line text. `Interaction::input_changed(cx, text) -> Option<LineAction>` (default `None`) is
  offered the text typed in the command line every frame while it is not empty; an action it returns
  is sent to `line` and the shell clears the input, so a controller can complete a shortcut as it
  is typed. Typing over the viewport reaches the command line as before (the controller passes
  it). Tab over the viewport is offered to `key` even while a text field has the focus. Escape on an
  empty command line gives the focus to the viewport. An Enter or Escape that another text field gave
  up in the frame is not the command line's (`CommandLineView::defer_keys`).
- Context menu. `Interaction::context_menu(&self, cx) -> Vec<ContextEntry>` (default empty) is asked
  every frame while a menu opened by a secondary click over the viewport is shown; an empty list
  opens no menu, so a controller without entries keeps handling the secondary button itself. A `ContextEntry` has a message identifier, a `LineAction` (Enter is
  `Submit(String::new())`, Cancel is `Cancel`, a command is `Invoke(intent)`), `enabled` and a
  separator flag, and the constructors `new`, `command`, `separator` and `disabled`. Choosing an
  entry sends its action to `line`.
- Display state and banner. `InteractionOutput::hidden` (default empty) lists objects the viewport
  does not draw and does not pick; `isolated` (default `None`) lists the only objects it draws and
  picks. The shell hands them to the renderer as display overrides (`arc-gpu`) and filters the hits
  of its own picking by them. `banner` (default `None`) is a text, already localized, shown in a
  frame around the viewport with a label at its top left corner while a temporary display state is
  active. None of this is saved. Framing everything (a zoom-extents request, the first framing of a
  document) frames only the objects the display state shows.
- Camera requests. `Interaction::take_view` (default `None`) returns a `ViewRequest`, taken once
  per frame: `Extents`, `Previous` (the camera before the last request), `Window { a, b }`,
  `Scale(factor)`, `Plan { a, b, z }` and `Camera(frame)` (the enum is non-exhaustive). `Plan` sets an orthographic camera
  looking down −z with +y up, framed on the rectangle between the corners at height `z` with a 5 %
  margin, and locks the orientation: the viewport then takes pans and zooms but no rotation, and
  the view cube is not shown. `Camera(frame)` sets a camera the controller read from
  `InteractionContext::camera` earlier and releases the lock, so a product returns to the camera
  it left.
- Extraction on request. `InteractionContext::ask_extraction(objects)` (at most 10 000 per call,
  from `frame`, `line` and `key`) queues the objects for extraction from the tab's current snapshot
  on the extraction workers, as a commit that changed them would. A product uses it when what an
  object draws depends on state outside the document (a drawing shown only while a view is open,
  filtered by a temporary display state). While extraction is queued or running the shell counts
  as busy, so frames keep coming and a result is drawn when it arrives without further input.
- A hover pick (the pre-selection) waits at most 40 ms for the render thread and keeps at most one
  pick queued: while the render thread applies a large extraction result, the pointer highlights
  nothing instead of holding up the frame. A click, a rubber band and a pick of the product wait
  as before.
- `InteractionContext` also has `setting(key) -> Option<SettingValue>` (the effective value),
  `ask_setting(key, Option<SettingValue>)` (set in the user scope, or remove from it, after the call;
  at most 16 per call), `ask_panel(panel)` (show a panel, or bring it to the front of its group)
  and `ask_clipboard_text(text)` (put at most 4096 bytes on the operating system clipboard when the
  frame ends; a controller that keeps its own clipboard does this on copy, because the platform
  reports a paste shortcut only while the system clipboard holds something).
- `PointerKind::DoubleClick` reaches `pointer` after the second click of a double click (see
  [arc-ui](arc-ui.md)).
- `InteractionOutput::inline_arguments` (default `false`): the command line ends the input at a
  space like Enter (one-letter commands, keyword answers); `true` keeps the space as text so an idle
  line takes inline arguments such as `app.file.open path=<file>`.
- `InteractionContext::ask(intent)` queues an intent (at most 16 per call) from any method of the
  controller; after the call returns (after the viewport frame for `pointer` and `dynamic`) the
  shell dispatches each (`Action::Dispatch`): a shell command is carried out, any other command starts on the
  shell's own command line, which runs it when its parameters are complete and does not prompt
  while a controller is registered.
- `ShellConfig::work_plane` (`WorkPlaneProbe`, a function of the application, the active tab and
  its selection returning `Option<arc_ui::WorkPlane>`) sets the plane that point input of the active tab's
  viewport lands on: the click that answers a point prompt without a controller, and the `model` of
  every `PointerEvent` a controller receives (see `ProductInput::work_plane` in
  [arc-ui](arc-ui.md)). `None` keeps the plane through the camera target.
- Decisions with the rejected alternatives:
  - One controller per tab, created when the tab's view is created. Rejected: one for the product,
    which would share a prompt between tabs.
  - Output as a value per frame, not a list of calls. Rejected: the controller painting its own
    overlays, which would give it the egui context and the widget's internals.
  - The controller runs commands through `InteractionContext::invoke`, which needs no mutable
    application: the shell hands out `&Application`, as the panels already receive it.
  - A shell command goes to the shell even when a controller is registered. Rejected: leaving it
    to the controller, which cannot run `app.file.*` or clear the selection.
  - The controller's session carries `app.language`; the foundation's parsers read `.` as the
    decimal separator whatever the language, so a product that accepts a decimal comma converts it
    itself where the prompt cannot be a point (a point's coordinates are separated by `,`).
- Verified through a real window with a product controller: Tab, typed text and the paste shortcut
  over the viewport, the context menu, double click and the banner. Not verified here: the option
  bar's layout with a long option list.

### Frame composition (CORE-APP-025, feature `window`)

The feature `window` adds `arc-ui` (feature `viewport`), `arc-gpu`, `arc-geom`, `winit`,
`egui-winit`, `egui-wgpu`, `rfd` and, on macOS only, `muda`. `Shell::frame(app, ctx, input)`
needs no window:

- Each frame drops input events with non-finite positions, deltas or factors, or with
  magnitudes above 10^6 points (egui sums wheel deltas, which overflows from finite input),
  resolves the key bindings and takes the native menu's intents, draws the egui menu bar (unless a native menu
  bar shows the menu), the toolbar, the document tab strip and the dock with the product's panels, then carries out what the panels
  asked for, in order.
- A key is handled once. The keymap takes the key presses its bindings match (a completed
  binding or a stroke of a pending chord) out of the frame's events before any widget sees them,
  so a binding wins over a widget's own handling of the same key: a product that binds Ctrl+Z
  and a command line that also handles Ctrl+Z run one undo. While a text field has the focus,
  strokes without Ctrl, Alt or Command are the field's and no binding takes them (a product can
  bind Delete without a rename deleting objects). The command line handles Ctrl+Z, Ctrl+Y and
  Ctrl+Shift+Z whenever no text field has the focus, so a product needs no binding for them.
- Command tabs (`ShellConfig::command_tabs`) replace the flat toolbar strip when given. A row of
  tab labels (real, keyboard-reachable tabs, each padded 12 points either side, highlighted on
  hover) sits under the menu bar: the current tab has an
  accent underline, a double click on it folds the panel away or back (kept per window), and
  picking another tab while folded expands it. Below the row a fixed-height panel lays the
  current tab's groups out left to right in a horizontal scroll area, without wrapping: each
  group draws its items, its label centered underneath in the muted text color, and a thin
  separator divides groups. `ToolItem::IconButton` is large (a 32 point icon above a label of up
  to two lines, 64 points wide) or small (a 16 point icon beside the label; consecutive small
  buttons stack three to a column); `Choice` and `ZoomExtents` draw as in the flat strip; `ZoomExtentsIcon` is the zoom-extents
  request drawn like an `IconButton` (large or small). A
  contextual tab (`CommandTab::context`, a probe of the application and the active document
  tab) is shown only while the probe is true, in the accent color; it becomes current when it
  appears and the tab chosen before returns when it goes. Labels and hints are message
  identifiers like `ToolItem::Button`, and a click submits the item's `line`.
- `ShellConfig::quick_access` items draw as icon-only buttons in the menu bar row after the
  menus, with the label and hint as the tooltip. `ShellConfig::status_text` returns strings
  drawn right-aligned in the status bar, separated by dividers, after the toggles.
- Product icons (`ShellConfig::icons`, name and SVG source) join the starter set in one atlas,
  built at the scale's pixel edge and rebuilt when the scale changes. A name that is unknown, or
  an atlas that failed to build (logged once), shows the label as text. Products check their set
  in a test with `shell::check_icons`. Every icon button exposes its label as its accessible name
  and as the tooltip.
- The tab strip below the toolbar has one tab per open document: its name (the file name, or
  "Untitled document", numbered when several), a `*` while it has unsaved changes, a click to
  activate it and a close button (`ShellAction::ActivateTab`, `ShellAction::CloseTab`; closing asks
  about unsaved changes like `app.file.close`). The window title carries the same mark.
- The command panel keeps the input row at the bottom of the panel whatever its height, and the
  history above it scrolls to the newest line, which is at the bottom. The shell keeps the last 500
  lines of its own per tab, placed among the controller's lines by sequence number: a typed command
  on an idle line as `Command: <text>`, a command started by a menu, key or button as `Command: <name>`
  (the command's label), each prompt the command line asks as a line that an answer typed to it
  completes (`Specify next point: 5,4`), `*Cancel*` for Escape at a prompt, `Done: <name>` when an
  undoable command finished, and errors. A controller's own history lines come with them in order.
  Lines can be selected and copied with Ctrl+C. File actions add a line (`Saved <name>`, `Opened <name>`, a new
  document, a closed document); a dialog that returned no file says `No file was chosen` and how
  to continue by typing a path; a close or quit that is not carried out because the question was
  answered with Cancel, or could not be shown, says so.
- `ShellConfig::initial_view` (`InitialView { toward_camera }`) sets the direction a new tab's
  viewport looks from (model coordinates, z up; straight down without one); the camera is framed
  on the scene once it is known. `ShellConfig::overlays` (`OverlayProbe`) is called each frame
  with the application, the active tab and the language tag, and returns
  `arc_ui::ProductOverlay`s the viewport paints over the scene in model coordinates, such as
  `Label`s that face the screen; labels that would cover each other are stepped down in the
  order given. `PanelContext::overlays` hands them to the viewport panel.
- The selection a command sets with `Context::set_selection` (`Outcome::selection`) becomes the
  tab's selection after a command-line step, so a command can select what it created or a
  product can map a click (`ViewportPanel::commands`, a tree's `TreeCommands`) to the objects it
  stands for by naming its own selection command.
- The sessions the shell runs commands in use the product's clock, so journal records carry the
  real time, and hold the session setting `app.language` (`commands::LANGUAGE_SETTING`), the
  language tag in effect.
- Panels are product `Entry`s whose factory returns `Box<dyn Panel>` (`shell::panel`); a
  factory of another type is reported with `app.product.invalid_id` and left out. The standard
  panels are `ViewportPanel`, `PropertiesPanel` (with the product's edit command),
  `OutlinerPanel` (every object, flat, labelled by its `name` text field or type and
  identifier), `CommandLinePanel`, `DiagnosticsPanelView`, `SettingsPanel` and `LogPanel`.
  Extractors are product `Extractor`s whose factory returns an `arc_gpu` extractor
  (`shell::extractor`). An extractor sees the snapshot, so it may read other objects, but the shell
  extracts only the objects a commit changed: `ProductBuilder::extraction_dependents(f)` registers
  `f(snapshot, changes) -> Vec<ObjectId>`, called after each commit with listed changes, whose
  objects are extracted too (the walls of a storey when its level changes, for example). A commit
  of "everything" extracts everything and does not call it.
- `SettingsPanel` (CORE-APP-011) lists every setting with a search text over keys and
  localized descriptions. A setting whose highest allowed scope is the user scope or above is
  edited there: a check box for a truth value, a choice list for a choice, otherwise a text field
  committed when it loses focus (text that does not parse as the type is reported with
  `app.settings.type_mismatch`); a value the user scope supplies has a reset button. Edits are
  `Action::Setting`s; the shell carries them out with `settings_editor::set` and `reset` in the
  user scope, writing the user settings file.
- `LogPanel` (CORE-APP-015) shows the records of the platform's `LogRing`, newest at the bottom.
- Every tab has a view: selection, camera (initially at `ShellConfig::initial_view`, else looking
  down the z axis at the origin; orthographic, framed on the scene), the viewport widget, an `arc_gpu::Viewport` created on the first
  frame that shows the tab when the shell has a GPU, an extraction driver with two workers, a
  subscription to the document's events, and its command line, property grid, tree and
  diagnostics states. Committed events submit their changes to extraction, and finished results
  go to the viewport; deleted objects leave the selection. `settle` waits for extraction and
  the viewports, for tests and the first frame; frames never wait.
- Viewport reactions go to the viewport (size, camera, highlight, rubber-band overlay); picks run
  on the viewport's render thread through `Viewport::with_scene`, waiting at most 5 s.
- An intent naming a shell command is carried out by the shell. Any other intent starts the
  command through the active tab's command line (`CommandLine::drive`), which prompts for
  missing parameters; every completed invocation of a product command goes to
  `Application::invoke` with the UI session (read, edit, file read and file write), and
  `invocations()` lists the commands that did. Each command-line step takes the next seed of a
  sequence from `ShellConfig::seed`, so commands generate new identifiers also in a reopened
  document whose revisions repeat earlier ones; the reference shell seeds it from the clock and
  the process. Errors are kept in `errors()` (at most 64).
- `FrameInput::language` and `PanelContext::language` carry the active language tag (the
  localizer's language, for example `tr`). The diagnostics panel's `language` and the outliner's
  filter (`arc_ui::text::matches_filter`) fold case by it, so in Turkish `İ` matches `i` and `I`
  matches `ı`. The shell has no command palette panel; a product's palette sets
  `Palette::language` from the same value.
- The viewport panel draws `arc_ui::view_cube::ViewCube` over the viewport widget. A returned
  `ViewIntent` becomes the camera through `ViewIntent::frame_from` and
  `arc_gpu::navigation::interpolate`; the shell has no animation tick, so the end frame
  (`t = 1`) is applied at once.
- `app.macro.record_start` attaches an `arc_command::macros::Recorder` (the product's name and
  version, default limits; starting again discards an unsaved recording). Every product command
  that completes through `Application::invoke` in the command-line step is recorded with the
  revision it ran on, its seed and selection; shell commands and failed runs are not. A full
  recorder drops further steps silently. `app.macro.record_stop` ends the recording and asks
  for a file with `Request::SaveMacro(text)`, which `platform::carry_out` writes after a save
  dialog (`macro.jsonl` suggested; `app.macro.write_failed` when the write fails); without a
  recording it does nothing.
- The frame returns the egui output, the `Request`s for the platform (`OpenDialog`,
  `SaveDialog(tab)`, `AskClose(tab)`, `Quit`, `SaveMacro(text)`, `PathDialog`), whether more frames are on their way and whether
  a setting changed.

### Start-up, prompts and settings in the shell (CORE-APP-004, 005, 006, 010, 011, 012, 015)

- `Shell::start(app, files)` runs once before the first frame. With `UserFiles` (the user
  settings file and the recent-files file) it loads the settings file into the user scope
  (`settings_file::load`; a malformed or oversized file is set aside and its error shown as a
  warning notification) and keeps both paths for writing. It applies the appearance settings and
  lists `Application::recovery_offers()` in the recovery prompt (sessions without records are
  discarded, not listed); a failure to list is an error
  notification.
- Notifications (`arc_ui::Notifications`) are drawn at the top of the main window.
- The recovery prompt is a modal over the main window while sessions are listed. Each entry
  shows its file name (or "untitled") and record count, with recover, compare and discard; an
  entry that cannot be read offers discard only. Compare shows what `Application::compare`
  reports (base path, whether the base is unchanged, its object count, the records to replay)
  and changes nothing. Recover and discard call `Application::recover` and
  `discard_recovery` and remove the entry; "decide later" closes the prompt and leaves the
  sessions for the next start. Nothing is recovered or discarded without one of these choices.
- The active tab's banners are drawn in a strip above the dock, formatted through the catalog
  with their arguments; the stale-lock banner has a take-over button that calls
  `Application::take_over`. Banners of other tabs show when their tab is active.
- `shell::prefs::register_settings` registers `ui.theme` (`light`, `dark`,
  `high_contrast_light`, `high_contrast_dark`), `ui.density` (`compact`, `standard`,
  `comfortable`) and `ui.scale` (75 to 300 percent), each up to the user scope. A value of one of
  them set in any scope overrides the appearance of `ShellConfig`; the shell applies it at start
  and after each settings edit, without a restart. `ui.language` is applied by the localizer's
  owner with `prefs::apply_language` (at start and when a frame reports `settings_changed`),
  so labels change from the next frame on.
- Files dropped on a window (egui's `dropped_files`, at most 64 per frame) open through
  `Application::open`; a refused file is an error in `errors()`. After every frame and after the
  platform carried out a request, `Shell::save_recent` writes the recent files when they changed
  since the last write; the event loop also writes them at quit.
- `shell::install_logging(app, dir)` installs the process-wide subscriber from `log.level`,
  `log.max_file_bytes` and `log.max_files` (read once; a change applies at the next start) with
  the file `shell.log` under `dir`, and returns the `LogRing` the subscriber also fills:
  `logging::subscriber_with_ring` keeps the latest 1000 records, each cut to 4096 bytes. The ring
  holds the same formatted text as the files, so a value logged as `Redacted` is `[redacted]`
  there too; the shell logs dropped file paths only that way.

### Floating dock trees as windows (CORE-APP-002, CORE-UI-001)

- `Shell::windows()` models one secondary window per floating tree of the layout: its
  identifier (`float.` and the tree's first panel in lowercase, or `float-<n>` when that is not
  a valid window identifier or repeats), its rectangle and its panels. `Shell::frame_window(app,
  ctx, input, id)` composes that window in its own egui context: its tree with the same panels,
  actions and requests as the main window, without menu bar, notifications, banners or prompt.
  Changes its dock makes are written back into the layout; a tree whose last panel closes is
  removed, and moving a panel from it into the main tree is dropped. `dock_window(id)` docks a
  window's panels back into the main tree. `layout()` and `layout_mut()` expose the layout.

### Menus, dialogs and the event loop (CORE-APP-002, CORE-APP-009)

- `FileDialogs` (open, save, the save-discard-cancel question) and `NativeMenu` (install the
  resolved entries, poll activated command identifiers) are traits; `carry_out(app, dialogs,
  request)` drives `Application::open`, `save_as` and `close` from them. Closing a dirty
  document never saved asks for a file after the answer to save; quitting closes every tab in
  order and stops at the first one the user keeps.
- A product command registered with `ProductBuilder::path_dialog(command, PathDialog)` (mode open
  or save, title message, filters of a label message and extensions, default name where
  `{document}` is the active document's name without extension) opens that file dialog when it
  starts without a `path`, whether from a menu entry, a toolbar button, a typed line or a
  controller's `ask`. The frame returns `Request::PathDialog { tab, dialog, then }`;
  `carry_out` shows it through `FileDialogs::pick_path` (the dialog implementation of the open and
  save dialogs) and runs `then` with the chosen file as `path`, through the tab's controller when
  it has one (`LineAction::Invoke`). A dialog that returns nothing says `No file was chosen`.
  `ProductBuilder::build` fails with `app.product.invalid_id` for a dialog naming an unregistered
  command.
- `FileDialogs::can_ask` tells whether the question about unsaved changes can be shown natively.
  When it cannot, `carry_out` returns `Done::Asking` for `AskClose` and `Quit`, and the shell shows
  a modal in its own window (the document's name, the text of the question, Save, Discard and
  Cancel; Escape cancels). Cancel keeps the document, Discard closes it, Save saves and closes it
  (a document never saved asks for a file first and is asked about again if it is still unsaved);
  after a quit, the next dirty tab is asked in turn. `NativeDialogs::can_ask` is true on Windows
  and macOS, where `rfd` shows the question with the system library, and on Linux only when
  `zenity` is on the search path.
- `NativeDialogs` uses `rfd`: the desktop portal on Linux (where `rfd` asks the question
  through `zenity`), the system dialogs on Windows and macOS; the question's answers map to
  save (yes), discard (no) and cancel (anything else). `native_menu()` is a `muda` menu bar on macOS, item identifiers being command
  identifiers; on Windows and Linux there is none and the shell draws the egui menu bar.
- `event_loop::run` starts the shell (`Shell::start` with `Startup::files`, the log subscriber
  under `Startup::log_dir`, the language from `Startup::os_locales`), opens the main window with
  `winit`, restores its placement from the window-state file, presents through a surface of the
  `arc-gpu` device (`Gpu::instance`, `Gpu::adapter`; a non-sRGB surface format), copies each new
  viewport frame into an `Rgba8Unorm` texture registered with the `egui-wgpu` renderer, saves the
  placement on moves and resizes, keeps the logical size on scale changes and writes the file on
  quit. After each frame it opens a window for every floating tree without one (placement from
  the window-state file under the window's identifier, else the tree's rectangle) and closes
  the windows of trees that are gone; each secondary window has its own surface, egui state,
  renderer and textures, and closing it docks its panels back. Dropped files reach the frame
  through `egui-winit`. The text font is
  loaded at start from a short list of system font files (DejaVu Sans, Noto Sans or Liberation
  Sans on Linux, Segoe UI or Arial on Windows, Helvetica or Arial on macOS), at most 32 MiB.

### What runs where

- Tested headless: the window-state model with a property test; the shell commands; menus
  through a fake native menu and the egui menu bar; the open, save, close and quit flows
  through fake dialogs; the log ring's bounds and redaction; and, in `apps/arc-shell`, a
  scripted session on the software adapter (create a layer, a point and a line on the command
  line, select the point by a click in the viewport, set its x coordinate to `1200 mm + 3 in` in
  the property grid, undo, save through the save dialog, close and reopen through the open
  dialog, with every edit in `invocations()`), a panic-hunt property test of hostile event
  streams, command lines and intents, and frames that drive: the recovery prompt after a dropped
  application (compare, recover, discard); the lock banner of a second instance and the
  stale-lock banner with take-over; settings edits written to the file, effective at once
  (scale, and a language switch that changes a panel title) and after a restart, and reset; a
  malformed settings file set aside with a notification; a dropped file opened, the recent
  files written and the log viewer showing the record with its path redacted; and a floating
  tree composed as a second window and docked back.
- Compiled and linted but never run here: the `winit` event loop with its secondary windows,
  their placement and `DroppedFile` events, the window surfaces and their presentation, the
  `egui-wgpu` renderer and texture registration, `NativeDialogs` and the `muda` menu bar (built
  on macOS only), and font loading from the system.

### Owner decisions

- No native menu bar on Windows: `muda` attaches one only through an `unsafe` function, which
  the workspace lints forbid outside the FFI crates. Windows shows the egui menu bar.
- No bundled font: egui's fonts are under a license outside the allowlist. Without one of the
  listed system fonts, text shows as boxes; a font under an allowed license, or a font
  lookup crate, would remove this.
- The egui clipboard integration and Wayland window title text are off: each brings dependency
  trees not reviewed for this change.
- The AccessKit platform adapter (`egui-winit` feature `accesskit`, screen readers) is off. It
  adds `accesskit_winit` 0.34.2 with about 50 crates (`accesskit_unix`, `atspi`, `zbus` 5,
  `zvariant` 5, `async-io` and others); their licenses pass `cargo deny`, but the advisories
  check fails: RUSTSEC-2026-0370, -0371 and -0372 mark `zvariant`, `zbus_names` and
  `zvariant_derive` unmaintained. The AccessKit tree itself is built in every frame and is what
  the headless tests read.
- The window surface is created from the `arc-gpu` device's instance, which has no display
  handle, so presenting works on Vulkan, Metal and DirectX 12 and not on the OpenGL fallback.

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
