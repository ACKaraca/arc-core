# arc-command

Commands and everything that runs them: the command specification, registry, prompt machines,
command-line parsing, macro recording, the Arc Automation Protocol endpoint, the Model Context
Protocol endpoint, the headless runner, confined file access and the WebAssembly plug-in host. The
architecture is specified in [commands-and-automation.md](../architecture/commands-and-automation.md).

## Owns

- The command type, parameter schema generation and validation, enabled predicates, permission
  classes and exposure.
- The registry, catalog export and alias tables.
- Prompt machines, input kinds and keyword options.
- Command-line and inline-parameter parsing and completion.
- The guarded executor integration with `arc-doc` transactions.
- Macros and batch files.
- The protocol server (feature `server`): framing, sessions, method families, errors, versioning.
- The MCP adapter derived from the registry.
- The headless runner and the shared CLI argument conventions.
- Read and write roots and the escape-test suite.
- The plug-in host on `wasmtime` (feature `wasm-plugins`).
- Core commands: undo, redo, group control, save, open, settings, selection and view queries.
- `apps/arc-worker`'s job protocol.

## Stays in products

Every domain command, its prompt machine and its preview; product aliases and default key bindings;
product CLI subcommands beyond the shared set.

## Key concepts and types

| Concept | Description |
|---|---|
| `CommandSpec` | Identifier, labels, parameters, result schema, predicate, run, prompt machine, transparency, undo behavior, permissions, exposure |
| `Registry` | All registered commands with aliases and catalog export |
| `Invocation` | Command identifier, parameters, document, expected revision, preview flag |
| `PromptMachine` | States with prompts, input kinds, defaults, previews and transitions |
| `Macro` | Header plus JSON Lines invocations with input placeholders |
| `Session` | Protocol session: peer, granted permissions, roots, subscriptions |
| `Roots` | Canonicalized read and write roots with the access check |
| `PluginHost` | Component instances, grants, limits and registrations |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-doc`, `arc-solve` (quantity parsing in parameters) |
| Third-party | `serde`, `serde_json`, `schemars`, `jsonschema`, `wasmtime` (feature `wasm-plugins`), `crossbeam-channel` |

## Test strategy

Schema tests that validate every registered command's schema and examples; the panic-hunt test
that runs every command with generated adversarial parameters and asserts no fault escapes and the
document is restored; protocol conformance tests from recorded sessions; the escape-test suite
against every surface; plug-in sandbox tests (denied imports, fuel and memory exhaustion, trap
rollback); macro record-and-playback round trips; fuzzing of the protocol message parser.

## Capabilities

### Command model

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-001 | Command specification | Registers commands with identifier, labels, parameter and result schemas, enabled predicate, run, optional prompt machine, transparency, undo behavior, permission classes and exposure. | T1 | 0 |
| CORE-CMD-002 | Parameter schemas | Generates JSON Schemas from parameter types and validates invocations against them, reporting each violation with a JSON pointer. | T1 | 0 |
| CORE-CMD-003 | Enabled predicates | Evaluates predicates against context and returns disabled reasons as messages; disabled commands cannot run from any surface. | T1 | 0 |
| CORE-CMD-004 | Guarded execution | Runs commands in a transaction under an unwind boundary; errors and escaped faults roll back and return stable error codes. | T1 | 0 |
| CORE-CMD-005 | Deterministic run | `run` produces the same result for the same document state, parameters and seed, and never opens dialogs or waits for input. | T1 | 0 |
| CORE-CMD-006 | Preview runs | Runs a command and rolls it back, returning its result and change summary, for automation previews and assistant approvals. | T1 | 1 |
| CORE-CMD-007 | Permission classes | Checks every invocation against the permission classes granted to its surface or session. | T1 | 0 |
| CORE-CMD-008 | Long-running commands | Reports progress for commands exceeding 300 ms and supports cancellation that rolls back. | T1 | 0 |

### Registry and interactive input

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-009 | Registry and aliases | Holds commands from all sources with unique identifiers, deprecated aliases and user aliases; resolves typed names by identifier, alias or localized name. | T1 | 0 |
| CORE-CMD-010 | Command catalog | Exports a JSON catalog of all exposed commands used for documentation, MCP tools, Python stubs and tests. | T1 | 0 |
| CORE-CMD-011 | Prompt machines | Drives command input through states that accept points, distances, angles, picks, selections, text, numbers with units and keyword options, with defaults and previews. | T1 | 0 |
| CORE-CMD-012 | Transparent commands | Runs commands marked transparent during another command's prompt without cancelling it. | T1 | 1 |
| CORE-CMD-013 | Command-line parsing | Parses command names with inline positional or named parameters and continues with the prompt machine for missing ones. | T1 | 0 |
| CORE-CMD-014 | Completion | Completes command names, keyword options, object names and units from partial input. | T2 | 1 |
| CORE-CMD-015 | Repeat last command | Exposes the last completed command and its parameters for repetition. | T1 | 0 |

### Macros and batch

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-016 | Macro recording | Records executed invocations with final parameters as JSON Lines with a header. | T1 | 1 |
| CORE-CMD-017 | Macro inputs | Replaces chosen parameter values with named placeholders that are prompted for on playback. | T2 | 1 |
| CORE-CMD-018 | Macro and batch playback | Plays macros as one group, stopping at the first error and rolling back the group. | T1 | 1 |

### Automation protocol

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-019 | Stdio sessions | Serves the protocol over stdio with newline-delimited JSON-RPC, logs on stderr only. | T1 | 0 |
| CORE-CMD-020 | Local socket sessions | Serves running applications over a user-scoped Unix socket or named pipe with token authentication, only when the user enables automation. | T1 | 1 |
| CORE-CMD-021 | Session negotiation | Negotiates protocol version and permission classes in `arc.initialize`, rejecting unsupported versions with code -32011. | T1 | 0 |
| CORE-CMD-022 | Command methods | Lists, describes, validates, runs and cancels commands, with optional expected revision and preview. | T1 | 0 |
| CORE-CMD-023 | Query methods | Answers object, reference, diagnostic, history, comparison and derived-data queries with filters, projection and cursor pagination, naming the revision read. | T1 | 1 |
| CORE-CMD-024 | Event subscriptions | Streams document events, command progress and selection changes as notifications to subscribed sessions. | T1 | 1 |
| CORE-CMD-025 | Selection methods | Gets and sets the session selection by identifiers and sub-object keys. | T1 | 1 |
| CORE-CMD-026 | View rendering | Renders a view to PNG with requested size, camera, display mode and section state, returned inline or written within write roots. | T1 | 1 |
| CORE-CMD-027 | Protocol errors | Returns standard and application error codes with stable codes, categories, message identifiers and details. | T1 | 0 |
| CORE-CMD-028 | Published schemas | Publishes JSON Schemas for every method, checked in CI against the server implementation. | T1 | 1 |

### MCP, CLI and confinement

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-029 | MCP endpoint | Serves Model Context Protocol tools generated from MCP-exposed commands plus fixed query and render tools, read-only unless edit is allowed. | T1 | 1 |
| CORE-CMD-030 | MCP attach with approval | Forwards MCP tool calls to a running application, where edits run as previews and apply only after the user approves. | T2 | 2 |
| CORE-CMD-031 | Headless runner | Provides the shared CLI subcommands, flags and exit codes so every product CLI behaves identically for shared operations. | T1 | 0 |
| CORE-CMD-032 | Read and write roots | Confines all automation file access to canonicalized roots, binding the check to the opened file handle. | T1 | 0 |
| CORE-CMD-033 | Escape-test suite | Runs symlink, junction, `..`, normalization, long-path and race cases against every surface; any escape fails CI. | T1 | 0 |
| CORE-CMD-034 | Worker job protocol | Runs isolated jobs in `arc-worker` over the protocol framing, restarting the worker after a crash and failing the job with `worker.crashed`. | T1 | 1 |

### Plug-ins and scripting

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-CMD-035 | WebAssembly plug-in host | Loads `arc:plugin` components with no ambient authority and registers their commands, node kinds, mappers and rules. | T2 | 3 |
| CORE-CMD-036 | Plug-in limits | Enforces memory, fuel, wall-clock and instance limits; a trap or limit violation ends the call and rolls back its transaction. | T2 | 3 |
| CORE-CMD-037 | Plug-in packages | Installs packages with bounded unpacking, checksum verification and atomic install, enabled per product, with host-assigned trust tiers. | T2 | 3 |
| CORE-CMD-038 | Python client | Provides the official Python client generated from the catalog and protocol schemas, able to start headless sessions or attach to running applications. | T1 | 2 |
