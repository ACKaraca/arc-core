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
| Third-party | `serde`, `serde_json`, `schemars`; later units add `wasmtime` (feature `wasm-plugins`) and `crossbeam-channel` |

## Test strategy

Schema tests that validate every registered command's schema and examples; the panic-hunt test
that runs every command with generated adversarial parameters and asserts no fault escapes and the
document is restored; protocol conformance tests from recorded sessions; the escape-test suite
against every surface; plug-in sandbox tests (denied imports, fuel and memory exhaustion, trap
rollback); macro record-and-playback round trips; fuzzing of the protocol message parser.

## Command model as implemented

The first part covers CORE-CMD-001 to CORE-CMD-005 and CORE-CMD-007 to CORE-CMD-010. The
modules are `spec`, `schema`, `context`, `invoke` and `registry`. Prompt machines, command-line
parsing and repeat (CORE-CMD-011, CORE-CMD-013, CORE-CMD-015) are described in the next section.

### Specification

- A command is a `CommandSpec` built from a typed function `Fn(&mut Context, P) -> Result<R>`,
  where `P: Deserialize + JsonSchema` and `R: Serialize + JsonSchema`; the registry stores it in
  erased form, and `run` is reachable only through `Invoker::invoke`.
- **Identifier**: three or more names separated by single dots, each a lowercase ASCII letter
  followed by lowercase letters, digits and underscores (`<owner>.<area>.<verb>`), at most
  128 bytes. Label, description, category and keywords are Fluent message identifiers.
- **Undo behavior**: `undoable` (the default; optionally with a coalesce key: within a gesture,
  consecutive invocations with the same key form one undo step), `not_undoable`, or
  `undo_control`. `undo_control` commands currently run like `not_undoable` ones, without a
  transaction; their commit path comes with the core undo commands.
- **Permission classes**: one or more of `read`, `edit`, `file_read`, `file_write`, `network`,
  `process`, `settings`; the default is `edit`.
- **Exposure**: a set of the surfaces `ui`, `command_line`, `automation`, `mcp`, `plugin`; the
  default is all. A command with no surface is internal: it is not in the catalog and no
  invocation reaches it.
- **Prompt machine**: an optional `Arc<dyn PromptMachine>` (see
  [Prompt machines](#prompt-machines-command-line-and-repeat-as-implemented)).
- **Positional order**: the parameters that inline values without a name fill, in order. The
  default is the required parameters in declaration order (the order of the schema's
  `required`); optional parameters are named-only unless the command states the order with
  `with_positional`. The generated `properties` are alphabetical, so they cannot give the order.
- **Registration check**: the identifier and message identifier grammars, at least one
  permission class, both schemas within the supported subset, a parameter schema of type
  `object` with `additionalProperties: false` (an unknown parameter name is an error, not
  ignored), every example valid against the parameter schema, and a positional order of
  distinct parameter names. Failures are
  `cmd.spec.invalid` with the detail `reason`.

### Parameters and schemas

- Schemas are JSON Schema draft 2020-12 generated by `schemars`, with definitions under `$defs`.
  Property order in `properties` is alphabetical; `required` keeps declaration order.
- Validation implements the subset generated schemas use. Assertions: `type`, `enum`, `const`,
  `properties`, `required`, `additionalProperties`, `items`, `prefixItems`, `minItems`,
  `maxItems`, `minLength`, `maxLength` (counted in Unicode scalar values), `minimum`, `maximum`,
  `exclusiveMinimum`, `exclusiveMaximum`, `oneOf`, `anyOf`, `allOf`, `$ref` to
  `#/$defs/<name>`, and the boolean schemas. Annotations without effect: `$schema`, `$defs`,
  `title`, `description`, `default`, `examples`, `deprecated`, `readOnly`, `writeOnly`, `format`,
  `$comment` and keywords starting with `x-arc-`. Any other keyword fails registration.
  `integer` accepts numbers stored without fraction or exponent; numeric bounds compare as
  `f64`.
- A violation is a JSON pointer (RFC 6901) into the parameters and a kind: `type`, `required`,
  `additional_property`, `enum`, `const`, `minimum`, `maximum`, `min_length`, `max_length`,
  `min_items`, `max_items`, `one_of`, `any_of`, `false`, or `decode` (the value passed the schema
  but the parameter type rejected it). Violations are sorted by pointer and kind; at most 64 are
  reported. `cmd.params.invalid` carries them as JSON text in the detail `violations`.
- Limits: parameter text is checked for `input_bytes` and nesting depth before it is parsed;
  parsed values for `nesting_depth`, `string_bytes` (texts and keys), `array_length` (arrays and
  objects) and `record_count` (values), all from the session's `arc_base::Limits`. Validation
  stops after 1,000,000 steps or 256 nested schema levels. Exceeding any bound is
  `cmd.params.limit_exceeded` with the detail `limit`.
- **Quantities** (`QuantityParam`): a JSON number is a value in SI base units; a JSON string is
  an `arc-solve` expression with units, read with `.` as the decimal separator, in which a bare
  number takes the session's context unit of the parameter's dimension. Schema: `anyOf` number
  and string.
- **Objects** (`IdParam`): an `ObjectId` as a hyphenated UUID text; schema
  `{"type": "string", "format": "uuid"}`.

### Context, sessions and predicates

- A `Session` holds its surface, granted permission classes, limits, clock and context units.
  This layer reads no system clock: the application supplies a `Clock`.
- The `Context` of `run` gives the document state (the transaction's candidate, or the snapshot
  of a query), the transaction (`cmd.no_transaction` for commands that are not `undoable`), the
  selection, the session, identifiers and random numbers from a generator seeded with the
  invocation's seed mixed with the revision of the snapshot the command runs on (so repeated
  invocations in one document never repeat identifiers), the session clock, cancellation and
  progress, and the selection to restore afterward.
- An enabled predicate reads a `Probe` (snapshot, selection, session) and returns a `Disabled`
  reason: a message identifier with arguments. `cmd.disabled` carries the reason's identifier in
  the detail `reason` and its arguments as further details.

### Guarded execution

`Invoker::invoke` is the only entry point. It resolves the name (identifier or alias; a
deprecated alias adds a `cmd.alias.deprecated` warning to the outcome's diagnostics), then checks
in order: exposure on the session's surface (`cmd.not_exposed`), permission classes
(`cmd.permission_denied`, detail `permission`), parameter limits and schema, and the enabled
predicate. An `undoable` command runs as a job of `Document::submit`: the predicate is evaluated
on the job's base snapshot, the transaction metadata is filled (`command`: canonical JSON
`{"command": id, "params": params}` with sorted keys; `label`: `{"message_id": label}`; `group`:
the invocation's group; the session clock's time; `selection_after`: a JSON array of identifier
texts when the command set one), `run` executes, the result is encoded and checked against the
result schema, and the executor commits. Any error rolls the transaction back and keeps the
command's own code; a cancelled token (checked by `Context::checkpoint` and after `run`) gives
`cmd.cancelled`; a panic anywhere, including parameter decoding, the predicate and result
encoding, gives `cmd.fault` with the detail `panic`, and the published snapshot stays the same.
Commands that are not `undoable` run on the latest snapshot with the same checks under their own
unwind boundary. The outcome holds the result, the revision, the transaction identifier, the
coalesce key to report to the undo manager, and the diagnostics.

Progress reaches the invocation's sink only once the session clock shows 300 ms
(`PROGRESS_DELAY_MILLIS`) since the command started; fractions are clamped to `[0, 1]`.

### Registry and catalog

- Identifiers, deprecated aliases and user aliases share one namespace: registering or adding a
  name that is taken fails with `cmd.duplicate` and changes nothing; whether a duplicate stops
  start-up is the application's decision. Deprecated aliases have the identifier grammar; user
  aliases are 1 to 64 bytes of lowercase ASCII letters, digits, `_`, `.` and `-`, starting with a
  letter or digit, and are added and removed at run time.
- Resolution trims the typed name and lowercases ASCII letters, then tries the identifier, a
  deprecated alias, a user alias, and the localized names. This layer has no localization: the
  caller passes a map from identifier to localized name taken from the active catalog, compared
  case-insensitively; a name shared by several commands is `cmd.name.ambiguous`.
- Iteration is in identifier order. The catalog is a JSON object
  `{"format": "arc-command-catalog", "version": 1, "commands": [...]}` with one entry per exposed
  command in identifier order: `id`, `label`, `description`, `category`, `keywords`, `params`,
  `result`, `examples`, `permissions`, `exposure`, `undo`, `coalesce`, `transparent`, `prompt`
  (whether a prompt machine exists) and `deprecated_aliases`. Object keys are sorted, so the
  same registry always gives the same text.

## Prompt machines, command line and repeat as implemented

The modules are `prompt` (CORE-CMD-011), `line` (CORE-CMD-013) and `command_line`
(CORE-CMD-013, CORE-CMD-015).

### Prompt machines

- A `PromptMachine` is a pure function of a `PromptState`: the parameters typed inline
  (`preset`), the answers so far and the current snapshot, which it reads and never changes.
  `next` returns `Step::Ask(Prompt)` or `Step::Done(params)`; the machine has no state of its
  own, so "back" is removing the last answer. Keyword options change the path by being answers
  the machine reads. `check` may reject an answer that fits the prompt (any error, for example
  a predicate on the picked object), and `preview` returns the preview of the pending result as
  JSON data in the product's terms (the architecture's transient geometry), from the answers and
  the pointer's current input. This layer draws nothing; running a command in a rolled-back
  preview transaction is CORE-CMD-006, a later row.
- A `Prompt` has a key (unique on one path; the parameter name for declarative machines), a
  message identifier with arguments, the accepted inputs (never empty) and an optional default
  answer. Accepted inputs and their constraints (bounds inclusive, in SI base units):

| `Accept` | Answer (`PromptInput`) | JSON value |
|---|---|---|
| `Point { dims }` (2D, 3D or both) | `Point { x, y, z }` in meters | `[x, y]` or `[x, y, z]` |
| `Distance { min, max }` | `Distance(meters)` | number |
| `Angle { min, max }` | `Angle(radians)` | number |
| `Pick { types }` | `Pick(ObjectId)` of a live object whose type key is listed (any when empty) | identifier text |
| `Selection { min, max, types }` | `Selection(ids)`: distinct live objects of the listed types | array of identifier texts |
| `Text { min_chars, max_chars }` | `Text` | string |
| `Number { dimension, min, max, integer }` | `Number(Quantity)` of that dimension | number in SI units; a JSON integer when `integer` |
| `Expression` | `Expression(text)` that `arc-solve` parses | string, evaluated by the command |
| `Keyword { options }` | `Keyword(id)` | the option's value (its identifier by default) |

- Keyword options have an identifier (1 to 64 bytes, no whitespace, unique ignoring ASCII case),
  a label message identifier, an optional shortcut letter (unique ignoring ASCII case) and the
  parameter value they stand for. A prompt that accepts nothing or has malformed options ends
  the session with `cmd.prompt.invalid`.
- `PromptSequence` is the declarative machine: one `PromptStep` per parameter, asked in order,
  skipping parameters given inline; a step may add a check. `CommandSpec::with_prompts` gives
  every step without a default the parameter's schema `default` when an accepted input can
  represent it (the inverse of the JSON mapping above); otherwise the step has no default.
- A command without a machine gets `PromptSequence::from_schema`: one step for each required
  parameter, in `required` order, with the message `cmd-prompt-param` (argument `name`). The
  schema kind decides the input: a quantity (`anyOf` number and string) takes an `Expression`,
  `number` a dimensionless `Number`, `integer` an integer `Number`, `boolean` the keywords
  `true` (`y`) and `false` (`n`), a closed list of values (`enum`, or `oneOf` of `const`)
  keywords with those values (label `cmd-keyword-value`, to which the surface passes the option
  identifier as `value`), a `uuid` string a `Pick`, an array of `uuid` strings a
  `Selection`, an array of numbers or quantities a `Point`, everything else `Text`.

### Prompt sessions

- A `PromptSession` drives one machine for one command and holds the inline parameters, the
  answers and the current prompt. It is transient UI state and is not serialized. Every call of
  the machine runs under an unwind boundary; a panic or an error of the machine ends the session
  (`Feedback::Failed`, a panic as `cmd.fault`).
- `answer(input)`: the input must fit one accepted kind and its constraints and pass the
  machine's check. A rejection returns `Feedback::Rejected` with the error, whose message
  identifier is the message, and the session stays at the same prompt: `cmd.prompt.kind`,
  `cmd.prompt.range` (detail `reason`: `not_finite`, `minimum`, `maximum`, `dimensions`,
  `count`, `duplicate`, `length` or `integer`), `cmd.prompt.object` (`not_found` or `type`),
  `cmd.prompt.limit_exceeded` (text over the session's `string_bytes`, a selection over its
  `array_length`, or more than 1,024 answers), or the expression parser's error.
- `accept_default()` answers with the default (Enter; `cmd.prompt.no_default` without one),
  `back()` removes the last answer and asks again, `cancel()` ends the session. Input after the
  session ended is `cmd.prompt.ended`.
- When the machine is done, its parameters are merged over the inline ones and validated
  against the parameter schema with the session's limits. A violation rejects the last answer
  (it is removed and its prompt asked again with `cmd.params.invalid`); with no answer to
  remove the session fails. Valid parameters give `Feedback::Done(Invocation)`, which runs only
  through `Invoker::invoke`, the same path as a programmatic call, so prompts never bypass
  validation, permissions or predicates.
- **Typed answers** (`answer_text`): the text is trimmed; empty text accepts the default. A
  keyword option is matched first by identifier or shortcut letter, ignoring ASCII case. Then
  each accepted kind is tried in order: a point is two or three comma-separated components
  (commas inside parentheses do not separate), each a length read like a distance; a distance,
  an angle and a number are read by `arc_solve::parse_quantity` with `.` as the decimal
  separator and the session's context unit of the dimension for bare numbers (so
  `1200 mm + 3 in` is a distance, and an angle prompt needs an angle context unit for bare
  numbers); an expression is kept as text; a pick is an identifier text; a selection is
  identifier texts separated by commas or whitespace; a text is the text. The architecture
  defines no relative or polar coordinate entry, so only absolute `x,y[,z]` is read. Text no kind
  reads is `cmd.prompt.unreadable`; text a kind reads but rejects reports that kind's error.

### Command-line grammar

A line is at most 64 KiB. Whitespace is Unicode whitespace.

```ebnf
line       = ws , [ name , { ws1 , argument } , ws ] ;
name       = quoted | name-char , { name-char } ;    (* name-char: any character but whitespace *)
argument   = [ param-name , "=" ] , value ;           (* no whitespace around "=" *)
param-name = ( letter | "_" ) , { letter | digit | "_" } ;      (* ASCII *)
value      = quoted | list | json | bare ;            (* followed by whitespace, the end, or, in a list, "," or "]" *)
quoted     = '"' , { char - ( '"' | "\" ) | escape } , '"' ;
escape     = "\" , ( '"' | "\" | "n" | "t" ) ;
list       = "[" , ws , [ value , { ws , "," , ws , value } , ws ] , "]" ;
json       = "$" , json-value ;                       (* RFC 8259, no leading whitespace *)
bare       = bare-part , { bare-part } ;              (* does not start with '"', "[", "]" or "$" *)
bare-part  = group | char - ( whitespace | '"' | "[" | "]" | "(" | ")" ) ;   (* in a list also not "," *)
group      = "(" , { group | char - ( "(" | ")" ) } , ")" ;
```

- An inline value may contain whitespace only inside quotes or parentheses:
  `length="1200 mm + 3 in"` or `length=(1200 mm + 3 in)`. Text with `"` such as `3'6"` must be
  quoted with escapes. Lists nest at most 32 deep, parentheses 128 deep; a line has at most
  1,024 arguments and a list at most 1,024 elements (`cmd.line.limit_exceeded`, detail
  `limit`).
- The name is resolved by `Registry::resolve` with the caller's localized names (`cmd.unknown`,
  `cmd.name.ambiguous`; a localized name with spaces is quoted). Positional values fill the
  positional order and must precede named ones (`cmd.line.syntax` with reason
  `positional_after_named`); `cmd.line.too_many_values`, `cmd.line.unknown_param` and
  `cmd.line.duplicate_param` report the rest.
- Values convert by the parameter's schema kind: `$` JSON is taken as is; a quantity keeps the
  text as a string (an expression with units evaluated by the command); `number` and `integer`
  take a JSON number literal; `boolean` takes `true` or `false`; a closed list takes one of its
  values; strings take the text; an array takes a list, or bare text split at commas outside
  parentheses (`0,0`), each element converted by the item kind. A value that does not fit, or
  inline parameters that violate the schema without its `required` list, are `cmd.line.value`.
- Every error from the line carries the byte span of the offending text in the details `start`
  and `end`; name errors carry the span of the name.
- **Canonical form** (`format_line`): the command identifier, then every parameter as
  `name=value` in name order. A value is written bare when that reads back to the same JSON,
  else quoted, else as `$` and compact JSON, so parsing the canonical line gives the same command
  and parameters. Macros record this form.
- **Completion** (`complete`, data only): in the first word, command identifiers, user aliases and
  localized names exposed on the command line; after it, `name=` for parameters not yet given and
  the values of a parameter with a closed list of values (`boolean`, `enum`); with an active
  prompt, its keyword options. Matching is by prefix, ignoring case; at most 100 candidates.

### Command line and repeat

- `CommandLine` holds the active prompt session and the last completed command. `submit(line)`:
  with an active prompt the line answers it; otherwise a complete line runs at once through
  `Invoker::invoke`, and a line with missing required parameters starts the command's machine
  (or the derived one) with the inline parameters as `preset`. `start` begins a command from a
  menu or tool with given parameters; `answer`, `back` and `cancel` drive the active prompt.
- The empty line accepts the active prompt's default; with no active command it repeats the
  last command (Enter), or does nothing when there is none.
- **Repeat** (`repeat_last`): the last completed command is the last invocation that succeeded
  through the command line, or that the application reported with `record`; transparent
  commands are not recorded. The architecture does not say that repeat prompts again, so repeat
  runs the same command with the same final parameters, again through `Invoker::invoke`;
  `last()` exposes the command and parameters for a surface that prefers to re-enter the prompts.
  `cmd.repeat.none` when nothing completed yet.

### Error codes

| Code | Category | Meaning |
|---|---|---|
| `cmd.unknown` | `not_found` | No command, alias or localized name matches |
| `cmd.spec.invalid` | `invalid_input` | The specification fails the registration check |
| `cmd.duplicate` | `conflict` | The identifier or alias is taken |
| `cmd.alias.invalid` | `invalid_input` | The alias is not well formed |
| `cmd.alias.unknown` | `not_found` | The user alias to remove does not exist |
| `cmd.alias.deprecated` | `unsupported` | Warning: the name is a deprecated alias |
| `cmd.name.ambiguous` | `conflict` | The localized name matches several commands |
| `cmd.params.malformed` | `invalid_input` | The parameter text is not JSON |
| `cmd.params.limit_exceeded` | `limit_exceeded` | The parameters exceed a limit |
| `cmd.params.invalid` | `invalid_input` | The parameters violate the schema |
| `cmd.disabled` | `conflict` | The enabled predicate disabled the command |
| `cmd.permission_denied` | `permission_denied` | A permission class is not granted |
| `cmd.not_exposed` | `not_found` | The command is not exposed on the surface |
| `cmd.cancelled` | `cancelled` | Cancelled; the transaction was rolled back |
| `cmd.fault` | `internal` | A fault escaped; the document was restored |
| `cmd.result.invalid` | `internal` | The result cannot be encoded or violates its schema |
| `cmd.no_transaction` | `internal` | A command without a transaction asked for one |
| `cmd.prompt.kind` | `invalid_input` | The answer is not a kind of input the prompt accepts, or not one of its keyword options |
| `cmd.prompt.range` | `invalid_input` | The answer is outside the prompt's constraints; the detail `reason` says which |
| `cmd.prompt.object` | `invalid_input` | A picked or selected object does not exist or has a type the prompt does not accept |
| `cmd.prompt.unreadable` | `invalid_input` | Typed text cannot be read as any input the prompt accepts |
| `cmd.prompt.no_default` | `invalid_input` | Enter was given to a prompt without a default |
| `cmd.prompt.invalid` | `internal` | A prompt machine produced a prompt that accepts nothing or has malformed keyword options |
| `cmd.prompt.limit_exceeded` | `limit_exceeded` | An answer or the number of answers exceeds a limit |
| `cmd.prompt.ended` | `conflict` | Input was given when no prompt is active |
| `cmd.line.syntax` | `invalid_input` | The command line does not follow the grammar; the detail `reason` says where |
| `cmd.line.limit_exceeded` | `limit_exceeded` | The command line exceeds a length, count or nesting limit |
| `cmd.line.unknown_param` | `invalid_input` | A named inline parameter does not exist |
| `cmd.line.duplicate_param` | `invalid_input` | An inline parameter is given twice |
| `cmd.line.too_many_values` | `invalid_input` | More positional values than positional parameters |
| `cmd.line.value` | `invalid_input` | An inline value does not fit its parameter |
| `cmd.repeat.none` | `not_found` | There is no completed command to repeat |

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
