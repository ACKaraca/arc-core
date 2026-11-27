# Commands and automation

Every change to a document is a command, and every surface that can change a document (menus,
command line, palette, macros, scripts, the Arc Automation Protocol, the Model Context Protocol,
headless CLIs, Hub jobs, plug-ins and the assistant panel) runs the same commands from one
registry. This document specifies `arc-command`.

## Command specification

A command is a registered value with these parts:

| Part | Content |
|---|---|
| Identifier | Stable, lowercase, dotted: `<owner>.<area>.<verb>` such as `core.edit.undo`, `core.doc.save`, `draftbuilder.draw.line`. Never reused, never renamed; a replaced command keeps its identifier as a deprecated alias |
| Label and description | Fluent message identifiers |
| Category and keywords | For the palette and documentation; keywords are localized |
| Parameters | An ordered list of named parameters with a JSON Schema (draft 2020-12) generated from the parameter type |
| Result | A JSON Schema of the result value (created ids, measurements, messages) |
| Enabled predicate | A function of the context (active document, selection, mode, permissions) returning enabled, or disabled with a reason message |
| `run` | A non-interactive function from validated parameters and context to a result, executed on the document executor inside a transaction |
| Prompt machine | Optional interactive state machine that collects parameters from user input |
| Transparency | Whether the command may run while another command's prompt machine is active |
| Undo behavior | `undoable` (default), `not_undoable` (no document change: view, query, export), or `undo_control` (undo, redo, group control) |
| Permission class | One or more of `read`, `edit`, `file_read`, `file_write`, `network`, `process`, `settings` |
| Exposure | Surfaces where the command appears: UI, command line, automation, MCP, plug-ins; the default is all |

Rules:

- `run` never opens a dialog, never waits for user input, and never depends on UI state other than
  what its parameters and context state. Given the same document state and parameters it produces
  the same result.
- Every parameter has a default or is required. Programmatic execution with only required
  parameters always behaves the same way, regardless of settings that a dialog might pre-fill.
- Parameters reference objects by `ObjectId`, never by handle, and quantities as numbers in SI base
  units or as strings with units parsed by `arc-solve` expressions (`"2400 mm"`, `"3'6\""`).
- A command changes at most one document. Cross-document operations (copy between documents) are
  two commands grouped by the caller.
- Commands that only read (queries, exports, view changes) are `not_undoable` and open no
  transaction.

### Prompt machines

A prompt machine is a finite state machine owned by the command. Each state declares:

- the prompt message, the accepted input kinds (point, distance, angle, object pick, sub-object
  pick, text, number with unit, keyword option, selection set) and the keyword options with their
  shortcut letters,
- the default value offered on Enter, if any,
- a preview function that produces transient geometry for the viewport from the parameters
  collected so far and the current pointer state,
- transitions to the next state.

The machine runs on the UI thread and reads the current snapshot; it never mutates the document.
When it completes, it emits the full parameter set and the executor runs `run`. Multi-step
commands that commit as they go (continuous drawing) emit one parameter set per step and wrap
them in a group. Escape cancels the current step, then the command; an in-progress group is rolled
back on cancel unless the command declares that completed steps are kept. The same machine is
driven by the command line, by the viewport (picks and snaps) and by the tool option bar, so a
command is equally usable by keyboard and pointer.

### Transparency

A transparent command (zoom, pan, toggle snap, query distance, set layer current) runs during
another command's prompt without cancelling it. Transparent commands may not open transactions on
the same document while the outer command holds an open group, except `not_undoable` ones.

### Guarded execution

The executor validates parameters against the schema, checks the enabled predicate and the
permission class, opens the transaction, runs `run` under an unwind boundary, validates, and
commits. Any error or escaped panic rolls back and returns an error with a stable code. A command
that exceeds its declared time budget in the UI shows progress and can be cancelled; cancellation
rolls back.

## Registry and discovery

- Products, `arc-core` crates and plug-ins register commands at start-up. Identifiers are unique per
  process; a duplicate registration fails start-up in debug builds and is reported and skipped in
  release builds.
- `command.list` and `command.describe` return every exposed command with its schema, result
  schema, permission classes, labels in the requested locale and enabled state in the given context.
- The registry exports a **command catalog** (JSON) used to generate reference documentation, the
  MCP tool list, the Python client stubs and the panic-hunt test inputs, so these cannot drift from
  the code.
- Key bindings, menu and toolbar entries and aliases refer to commands by identifier and are data
  (settings), never code.

## Command line and palette

- The **command line** accepts a command identifier, a registered alias or a localized command name,
  followed by optional inline parameters (`name=value` pairs or positional values in parameter
  order). It then drives the prompt machine for missing parameters. It keeps a history, offers
  completion for commands, keyword options, object names and units, and shows prompt messages and
  results.
- The **palette** is a fuzzy search over localized labels, keywords, identifiers and aliases of
  enabled commands, ranked by match quality and recent use. Choosing an entry starts the command as
  if typed.
- Both are provided by `arc-ui`; parsing and completion logic lives in `arc-command` so the headless
  `script` and `batch` modes use the same parser.

## Macro recording

Recording captures executed command invocations (identifier, final parameters, relevant selection
as ids) as a **macro**: a JSON Lines file, one invocation per line, plus a header line with the
protocol version and the product. Prompt-machine inputs are not recorded; their resulting
parameters are. A macro can mark parameters as **inputs** (prompted for on playback) by replacing
their values with named placeholders. Playback runs the invocations as one group, stops at the
first error and rolls the group back. Macros are the same format as `batch` input for the CLI.

## Arc Automation Protocol

The Arc Automation Protocol (AAP) is the external interface to a running application or a headless
session. It is the transport for scripts, the Python client, the MCP endpoint, the assistant panel,
Hub jobs and `arc-worker`.

### Transport

- JSON-RPC 2.0 messages, UTF-8, one message per line (newline-delimited JSON). Maximum message size
  64 MiB; larger payloads are passed as files inside the session's write roots.
- **stdio**: a headless session started with `<product>-cli serve --stdio`. Requests on stdin,
  responses and notifications on stdout, logs on stderr.
- **local socket**: a running application listens, when the user enables automation, on a Unix
  domain socket (Linux, macOS) or a named pipe (Windows) in a per-user directory accessible only by
  that user. A connecting client must present the session token written to a file in the same
  directory with user-only permissions. There is no TCP listener.

The decision is recorded in [ADR-0010](../adr/0010-automation-protocol-transport.md).

### Session

1. The client sends `arc.initialize` with its name, the protocol version range it supports and
   the permission classes it requests.
2. The server answers with the chosen protocol version, product name and version, `arc-core`
   version, the granted permission classes, read and write roots, and server capabilities.
3. Requests follow. The server processes requests for one document in order; requests for
   different documents may be processed concurrently.
4. `arc.shutdown` ends the session; a closed connection ends it as well, cancelling running
   requests (which roll back).

Granted permissions are the intersection of the request and the server's policy: a headless CLI
grants what its flags allow; a running application asks the user once per client name and
remembers the choice in settings.

### Method families

| Family | Methods | Notes |
|---|---|---|
| Session | `arc.initialize`, `arc.shutdown`, `arc.ping`, `arc.capabilities` | |
| Documents | `document.list`, `document.new`, `document.open`, `document.save`, `document.saveAs`, `document.close`, `document.info` | Paths are checked against read and write roots |
| Commands | `command.list`, `command.describe`, `command.validate`, `command.run`, `command.prompt`, `command.cancel` | `command.prompt` answers the prompt of the session's command line with typed text (or starts the command it names); `command.run` accepts `document`, `command`, `params`, optional `expectedRevision` and `preview: true` (runs and rolls back, returning the result and the change summary) |
| Queries | `query.objects`, `query.object`, `query.references`, `query.diagnostics`, `query.history`, `query.compare`, `query.derived` | Filters by type, owner, partition, indexed property predicates and ids; projection of fields; cursor pagination; every result names the revision it was read from |
| History | `history.undo`, `history.redo` | Steps the document's undo or redo stack; needs the `edit` permission; answers the command and label of the step it reversed |
| Events | `event.subscribe`, `event.unsubscribe`; notification `event.notify` | Document events, command progress, selection changes |
| Selection | `selection.get`, `selection.set`, `selection.add`, `selection.remove` | Session state; ids and sub-object keys |
| Views | `view.list`, `view.get`, `view.setCamera`, `view.render` | `view.render` renders a view to PNG (width, height, camera, display mode, section state) and returns it inline (base64, up to 16 MiB) or writes it to a path in the write roots |

Undo and redo are commands (`core.edit.undo`, `core.edit.redo`) and also protocol methods
(`history.undo`, `history.redo`); both change the document, so both need the `edit` permission, and
both are journaled as an interactive undo or redo. Group control (`core.edit.group_begin`,
`core.edit.group_end`) stays commands only.

Example request and response:

```json
{"jsonrpc":"2.0","id":7,"method":"command.run","params":{"document":"0190f3c2-6d1e-7a8b-9c4d-1e2f3a4b5c6d","command":"core.draft.line.create","params":{"start":[0,0],"end":[4.2,0]},"expectedRevision":118}}
{"jsonrpc":"2.0","id":7,"result":{"revision":119,"created":["0190f3c4-0a11-7c2e-8f00-5d6e7f8a9b0c"],"value":null}}
```

### Errors

JSON-RPC standard codes (`-32700` to `-32603`) are used for protocol-level failures. Application
errors use the range below and always include `data.code` (the stable `arc-base` error code),
`data.category`, `data.messageId` and `data.details`.

| Code | Meaning |
|---|---|
| -32001 | Unknown command or method in this product |
| -32002 | Parameters violate the schema; `details` lists JSON pointers and reasons |
| -32003 | Command disabled; `details.reason` gives the predicate's reason |
| -32004 | Permission class not granted |
| -32005 | Document or object not found |
| -32006 | Revision mismatch (`expectedRevision` is not current) or conflict |
| -32007 | Cancelled |
| -32008 | Limit exceeded (size, count, time) |
| -32009 | Path outside the read or write roots |
| -32010 | Command failed and was rolled back |
| -32011 | Unsupported protocol version |
| -32012 | Internal fault; the document was restored |

### Versioning

The protocol has its own `major.minor` version. Minor versions add methods, optional parameters
and result fields; clients ignore unknown result fields. Major versions may remove or change
methods; a server supports the current and the previous major for at least one suite release. JSON
Schemas of all methods are published in `schemas/automation/`. Command parameter schemas are
versioned with their commands: a breaking parameter change introduces a new command identifier.

## Model Context Protocol endpoint

Each product exposes a Model Context Protocol server (`<product>-cli mcp`, stdio) derived from the
same registry:

- **Tools** are generated from commands whose exposure includes MCP: tool name derived from the
  identifier (dots become underscores), description from the localized description, input schema
  from the parameter schema. Fixed tools add `list_commands`, `describe_command`, `query_objects`,
  `get_object`, `get_diagnostics`, `render_view` (returns an image) and `undo`.
- **Resources** expose document information, the command catalog and the most recent diagnostics.
- The endpoint runs on a headless session or, with `--attach`, forwards to a running application
  over its local socket, so an assistant can operate on what the user sees.
- Permissions: the endpoint grants `read` only unless started with `--allow-edit` (and the file
  flags below). When attached to a running application, edits run with `preview: true` first and
  are applied only after the user approves them in the application.
- No document content is sent anywhere by the endpoint itself; the MCP client decides what leaves
  the machine.

## Headless CLI conventions

Every product ships `<product>-cli` with these subcommands and flags. Products add domain
subcommands but do not change the meaning of these.

| Subcommand | Behavior |
|---|---|
| `info <file>` | Prints format, versions, units, object counts, links and diagnostics |
| `validate <file>` | Opens and validates; exits non-zero on errors; `--rules <file>` adds model-checking rules |
| `convert <input> <output>` | Imports or exports by extension or `--format`; `--option key=value` for format options; writes a loss report beside the output or to `--loss-report` |
| `run <file> --command <id> [--params <json or @file>]` | Runs one command; `--save` saves in place, `--output <file>` saves a copy, `--create` makes the package first (inside a write root) |
| `batch <file> <macro.jsonl>` | Runs a macro or batch file as one group |
| `commands` | Prints the command catalog (`--json` for machine output) |
| `render <file> --view <id> --out <png>` | Renders a view headless |
| `inspect <file>` | Package inspection (`--json`, `--check`) |
| `upgrade <file>` | Migrates a package to the current format, keeping a backup |
| `serve --stdio` | Serves the Arc Automation Protocol |
| `mcp` | Serves the Model Context Protocol |

| Flag | Meaning |
|---|---|
| `--read-root <dir>` / `--write-root <dir>` | Repeatable; confine file access (below) |
| `--allow-edit`, `--allow-network`, `--allow-process` | Grant permission classes beyond the defaults |
| `--json` | Machine-readable output on stdout; human messages go to stderr |
| `--log-level`, `--log-format text or json` | Logging on stderr |
| `--locale <tag>` | Message language |
| `--limits <profile>` | Hostile-input limit profile (`default`, `strict`, `large-models`) |
| `--fail-on-loss` | Exit code 7 if a conversion reported losses |
| `--seed <n>` | Deterministic identifier generation |

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | Command or operation failed |
| 2 | Usage error |
| 3 | Input invalid or corrupt |
| 4 | Unsupported format or version |
| 5 | Permission or path denied |
| 6 | Limit exceeded |
| 7 | Completed with losses and `--fail-on-loss` set |
| 70 | Internal fault |

CLIs never open windows, never prompt (they fail with exit code 2 when a required parameter is
missing), perform no network access unless allowed, and produce identical output for identical
inputs and seeds.

## Confined file access

All automation surfaces (AAP sessions, MCP, CLIs, plug-ins, `arc-worker`) resolve file paths
against an explicit set of **read roots** and **write roots**:

- A path is allowed if, after resolving `.` and `..`, symbolic links and junctions, its canonical
  form lies inside a root of the required kind. Write roots are also read roots.
- Device paths, alternate data streams, paths with embedded NUL, and UNC or network paths are
  rejected unless a root itself is such a path.
- Files are opened by the host and handed to the operation; the check and the open are bound to the
  same file handle so a path swapped between check and use cannot escape.
- Defaults: a CLI grants read access to the directories of input files named on its command line
  and write access to the directories of named outputs; an AAP session or MCP endpoint grants
  nothing beyond the open document's directory (read) unless configured; plug-ins get only the
  handles the host gives them.
- External references inside documents are resolved through the same rules; a link that resolves
  outside the roots is reported as `unresolved (outside allowed roots)`.
- A shared escape-test suite in `arc-command` (symlink loops, `..` chains, case and Unicode
  normalization tricks, junctions, long paths, races) runs against every surface.

## Scripting clients

Scripting runs out of process as a client of the protocol; no interpreter is embedded.

- The official **Python client** lives in `clients/python/` and is published as a Python package.
  It starts a headless session (`serve --stdio`) or attaches to a running application, and offers
  a typed API generated from the command catalog and the protocol schemas: one method per command
  with keyword arguments matching parameters, query helpers returning plain data objects, and event
  iteration.
- Any language with JSON-RPC can use the protocol directly; the schemas are the contract.
- Scripts run with the permissions granted to their session and inside its file roots. A script
  started from an application's script panel is grouped as one undo step unless it controls groups
  itself.
