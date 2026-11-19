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
- The protocol server: framing, sessions, method families, errors, versioning.
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
  the detail `reason` and its arguments as further details. `CommandSpec::enabled` is public
  so a user interface can show enabled states and disabled reasons without invoking.

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
- **Completion** (`complete_in`, and `complete` without a document; data only): in the first
  word, command identifiers, user aliases and localized names exposed on the command line; after
  it, `name=` for parameters not yet given and the values of a parameter with a closed list of
  values (`boolean`, `enum`); the names of live objects for a parameter that is an object
  identifier or a list of them (CORE-CMD-014); the unit symbols after a number for a quantity
  parameter. Matching is by prefix, ignoring case; at most 100 candidates. Candidates have a
  `kind` (`command`, `parameter`, `keyword`, `object`, `unit`) and a `label`.
  - An **object** candidate replaces the typed name with the object's identifier, the value the
    parameter takes, and carries the name as its label. Only objects with a name are offered, at
    most the first 100,000 of the snapshot are looked at, and a list is completed at its last
    element.
  - A **unit** candidate replaces the letters (including `°`, `²`, `³`, `_`) that end the value
    after a digit (`12m`); letters without a number before them are not a unit. All built-in unit
    symbols are offered; a prompt narrows them to the dimension it accepts. Text is split at
    whitespace, so a value with spaces is completed only inside the last word.
  - With an active prompt, `CommandLine::complete` offers its keyword options, and when there
    are none, the names of objects of the types a pick or selection prompt accepts, or the units
    of the dimension a distance, angle, number or expression prompt accepts.

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
- **Transparent commands** (CORE-CMD-012). `CommandLine::transparent(target, line)`, and
  `submit` for a line starting with `'` (`TRANSPARENT_PREFIX`) while a prompt is active, run a
  command marked transparent beside the prompt: the prompt session, its answers and the last
  command are untouched, and the run goes through the same invoker (or `drive` runner). A doubled
  prefix (`''`) at the start is an escape: the line answers the active prompt with the text after
  the first prefix, so `''abc` answers a text prompt with `'abc`. Without an active prompt the
  prefix is not special. Refused with `cmd.transparent.refused` (detail
  `reason`): `empty`, `not_transparent`, `missing_params` (a transparent command cannot prompt
  while another prompt is active), and `group_open`, when the command is undoable and the
  document has an open undo group, or controls undo. A `not_undoable` transparent command
  runs inside a group.
- **Drive.** `drive(target, step, runner)` takes one `LineStep` (start, submit, answer, back,
  repeat, transparent) like the method of the same name, except that each completed invocation goes to the
  caller's `runner` instead of `Invoker::invoke`; the runner's outcome or error is the run's.
  An application uses it to route command-line runs through its own entry point. The methods
  above are `drive` with the invoker as runner.

## Macros and batch as implemented

The module `macros` covers CORE-CMD-016, CORE-CMD-017 and CORE-CMD-018 after
[Macro recording](../architecture/commands-and-automation.md#macro-recording).

### Macro format

A macro is UTF-8 JSON Lines text, one JSON object per line, each line ending with `\n`, object
keys sorted. The first line is the header; every further line is one step.

| Line | Members |
|---|---|
| Header | `format`: `"arc-macro"`; `version`: the format version, `2`; `protocol`: the automation protocol version of the recording crate (`"1.0"`); `product`: `{"name", "version"}` of the recording product |
| Step | `line`: the command line in [canonical form](#command-line-grammar); `seed`: the invocation seed (default 0); `revision`: the revision of the snapshot the command ran on (optional); `selection`: identifier texts of the selection (optional, omitted when empty); `inputs`: parameter name to input name (optional, omitted when empty; [Inputs](#inputs)) |

```json
{"format":"arc-macro","product":{"name":"arc-shell","version":"0.1.0"},"protocol":"1.0","version":2}
{"line":"shell.layer.create name=walls","revision":0,"seed":1}
```

- A step has no other members; an unknown member is `cmd.macro.malformed`. The header's
  product is informational: playback does not compare it with the running product, and a
  command the running registry does not have fails its line with `cmd.unknown`.
- **Versioning.** The reader accepts `version` 1 and 2 and reports any other version as
  `cmd.macro.version_unsupported` (detail `version`). Version 2 adds the step member
  `inputs`; a version 1 file with that member is `cmd.macro.malformed` (reason `input`), as
  its reader always read it. The writer writes version 2. A change that a version 1 reader would
  read differently, or reject (a new member, a new meaning of a member, a different line
  form), increments `version`, adds the new version to the reader while it keeps reading every
  earlier one, and adds fixtures of each version; the writer writes the newest version.
- **Batch lists.** Text whose first non-blank line does not start with `{` is a list of command
  lines, one per line; blank lines and lines starting with `#` are skipped, and each line is a
  step with seed 0 and no revision. This is the input form of `batch` besides macros.
- **Bounds** (`parse_macro`): the whole text at most `limits.input_bytes`, each line at most
  `limits.string_bytes` and at most `limits.record_count` steps (`cmd.macro.limit_exceeded`,
  detail `limit`: `input_bytes`, `line_bytes`, `line_count`); a header or step that is not
  that JSON object is `cmd.macro.malformed` with the details `line` (1-based) and `reason`
  (`header`, `step`, `empty` for a blank `line` at playback, `utf8` for a file that is not
  UTF-8). The command line of a step is further bounded by the command-line grammar (64 KiB).

### Recording

- A `Recorder` holds the header and the steps; it is shared between threads and keeps at most
  `limits.record_count` steps (more is `cmd.macro.limit_exceeded`, limit `line_count`, and the
  step is not kept). `record(registry, invocation, revision)` writes the invocation's
  parameters as their canonical line with `format_line`, its seed, its selection and the
  revision it ran on. `text()` gives the macro.
- `Server::with_recorder` attaches a recorder to the protocol server: every `command.run`
  (and every `Server::call` of `command.run`) that succeeds is recorded, with the revision of
  the document read just before the run (runs of a session are serialized). Failed, rejected
  and cancelled runs are not recorded. A step the recorder refuses is logged as the event
  `macro.record` and the run is answered as usual.
- The command specification has no property marking a command as non-deterministic or as
  interactive only, so every successful invocation is recorded, transparent and read-only
  commands included. Prompt answers are never recorded; their resulting parameters are, as the
  canonical line.

### Inputs

CORE-CMD-017. A step can declare that a parameter takes its value from a named input instead
of the recorded one.

- `MacroStep::set_input(registry, param, name)` (and `Recorder::set_input(registry, index, param,
  name)` for a recorded step) sets `inputs[param] = name`. The parameter must be in the step's
  line, and the line keeps the recorded value, which is the default a surface offers when it
  prompts. A parameter has one input; one input name may be used by many steps and parameters,
  and all of them take the one value. Input names are 1 to 64 bytes: a lowercase ASCII letter,
  then lowercase letters, digits and `_`. A step has at most 64 inputs. Anything else is
  `cmd.macro.malformed` with reason `input`, also when a macro is read.
- `Macro::inputs(registry)` lists the inputs once each in order of first use, as `MacroInput`
  (`name`, the `command` and `param` of the first use, and the recorded `default`), which is
  what a user interface needs to prompt: it builds the prompt from the command's parameter
  schema, then passes the answers on. This layer never prompts, and the headless runner has no
  flag for input values yet, so it plays only macros without inputs.
- `play_with(registry, document, session, steps, group, values)` is `play` with a map from
  input name to JSON value. Before anything changes, the first input without a value fails
  with `cmd.macro.input_missing` (argument and details `input` and `line`); `play` gives no
  values. A value replaces the parameter's recorded value and is checked by the invoker like
  any parameter. Values for names no step declares are ignored.

### Playback

- `play(registry, document, session, steps, group)` opens the undo group `group` on the
  document, then for each step parses its line with `parse_line` (no localized names) and runs
  it through `Invoker::invoke` with the session, so exposure, permissions, limits, the schema
  and the predicate are checked as for every surface. On success the group ends, forms one
  undo step, and the outcome of each step is returned in order.
- At the first failing step playback stops, the group is rolled back
  (`Document::fail_group`), and the step's error (of the line parser or the invoker, with its
  own code) is returned with the detail `line`, the step's line number. The document content
  is then as before playback; that includes the content before the failing line, which the
  failing line itself never changes.
- **Determinism.** A command's identifier stream is its seed mixed with the revision it runs on
  (see [Context](#context-sessions-and-predicates)). A recorded step replaces the recorded
  revision's mix by the current revision's, so it generates the identifiers it generated when
  recorded: the same starting package and the same macro give the same objects, identifiers
  included, and a macro recorded on one package and played on another creates objects with
  the recorded identifiers. A step without a revision (a batch list line) mixes in an
  order-independent digest of the identifiers of the objects present when playback starts,
  because a package opened again starts at revision 0 and would otherwise repeat identifiers.

## Automation protocol and file roots as implemented

The modules are `protocol` (CORE-CMD-019, CORE-CMD-021, CORE-CMD-022, CORE-CMD-027) and
`roots` (CORE-CMD-032, CORE-CMD-033). This layer serves a session over any byte stream pair;
the process that owns stdin and stdout is the [headless runner](#headless-runner-as-implemented).

### Framing

- `Server::serve` reads newline-delimited JSON-RPC 2.0 from a `BufRead` and writes to a `Write`,
  one UTF-8 message per line terminated by `\n` (a `\r` before it is ignored; a last line
  without a newline counts). Blank lines are ignored. Nothing but protocol messages is written
  to the output; diagnostics go to the caller's `Log` (`level`, a stable event name such as
  `protocol.panic`, and free-form detail), which the process maps to stderr.
- A line is at most 64 MiB (`MAX_MESSAGE_BYTES`), and at most the configured
  `max_message_bytes` and `limits.input_bytes`. A longer line is consumed to its newline
  without being kept, so memory stays bounded and the next line is read normally; it is
  answered with -32008 (`cmd.protocol.limit_exceeded`, detail `limit`: `message_bytes`) and
  id `null`.
- Nesting is counted before parsing; deeper than the session's `nesting_depth` plus 2 (the
  message and its `params`) is -32008 with `limit` `nesting_depth`. Text that is not UTF-8 or
  not JSON is -32700 (`cmd.protocol.parse`, detail `reason`: `utf8` or `json`), id `null`.
- The architecture defines one message per line, so batches are not accepted: an array is
  -32600 (`cmd.protocol.invalid_request`, `reason` `batch`) with id `null`, answered once.
- A request has `jsonrpc: "2.0"`, a string `method`, an `id` that is a string or an integer,
  and optional `params`, an object. The id is echoed as the same JSON value. A message whose
  `id` has another type is -32600 (`reason` `id`) with id `null`; a request with a wrong
  `jsonrpc` or no method is -32600 (`jsonrpc`, `method`) with its id; `params` that is an array
  is -32602 (`by_position`); another `params` type is -32600 (`params`). A message without
  `id` that is otherwise well formed is a notification: the client defines none, so it is
  logged and ignored (the log record names the method, cut to 64 characters with control
  characters replaced, under `protocol.notification_ignored`). Unknown members are ignored.
- Every request with an id gets exactly one response. Responses and notifications are written
  with sorted object keys, so identical inputs and seeds give identical bytes.

### Session

- One `serve` call is one session. Before `arc.initialize` succeeds every other method is
  -32600 (`cmd.protocol.not_initialized`); a second `arc.initialize` is -32600
  (`cmd.protocol.already_initialized`). A name that is not a protocol method is -32601
  (`cmd.protocol.method_unknown`) in any state.
- `arc.initialize` parameters: `clientName` (1 to 256 bytes), `protocolVersions`
  (`{"min": "1.0", "max": "1.0"}`, each `major.minor` with one to five digits per part and
  values up to 65535) and `permissions` (permission class names, default none). The chosen
  version is the newest of `SUPPORTED_VERSIONS` (currently `1.0`) within `[min, max]`; none is
  -32011 (`cmd.protocol.version_unsupported`) with the detail `supported`, the list of
  supported versions. Granted classes are the requested ones the host's `ceiling` contains.
  Malformed parameters are -32602 (`cmd.protocol.invalid_params`, detail `reason`).
- The result: `protocolVersion`; `server` with `name`, `version` (the product's) and
  `arcCore` (this crate's version); `permissions` (granted, sorted); `roots` with `read` and
  `write` (canonical paths; read roots only with `file_read` or `file_write` granted, write
  roots only with `file_write`); `capabilities` (`methods`, `batch: false`, `preview: true`,
  `notifications: ["event.notify"]`, `events`: the event names that can be subscribed to);
  `limits` (`messageBytes`, `nestingDepth`, `queuedRuns`, `pageSize`, `eventQueue`).
  `arc.capabilities` returns the same `capabilities`; `arc.ping` returns `{}`.
- `arc.shutdown` cancels running and waiting commands, waits until each is answered, answers
  `null` and ends the session; nothing after it is read. The end of the input stream cancels
  the same way. The documented methods of later capabilities (documents, views, and the
  methods `query.object`, `query.compare`, `query.derived`, `selection.add` and
  `selection.remove`) are -32001 (`cmd.protocol.method_unsupported`).

### Command methods

| Method | Parameters | Result |
|---|---|---|
| `command.list` | optional `document` | `commands`: the catalog entry of every command exposed on `automation`, in identifier order |
| `command.describe` | `command` (identifier or alias), optional `document` | that command's entry |
| `command.validate` | `command`, `params` (default `{}`) | `valid`, `violations` (pointer and kind, as in parameter validation) |
| `command.run` | `document`, `command`, `params` (default `{}`), optional `expectedRevision`, `preview` (default `false`), `seed` (default 0) | `command` (identifier), `revision`, `value`, `diagnostics`, `preview`, `summary`, and for a preview `baseRevision` |
| `command.cancel` | `id`, the request id of a `command.run` | `cancelled`: whether a running or waiting run was found |

- An entry is the catalog entry plus `permitted` (the granted classes cover the command's).
  The architecture lists every exposed command, so commands the session may not run are listed
  with `permitted: false` rather than hidden. With `document`, an entry also has `enabled` and,
  when disabled, `disabledReason` (`messageId`, `arguments`); a predicate that panics counts as
  disabled with `cmd-fault`. Labels are message identifiers; this layer does not localize.
- A command not exposed on `automation` is -32001 (`cmd.not_exposed`), an unknown name -32001
  (`cmd.unknown`), an unknown document -32005 (`cmd.document.unknown`). `command.validate`
  checks limits and the schema without permissions or predicates.
- `command.run` goes through `Invoker::invoke` on the session's surface `automation`, so
  exposure, permissions, limits, schema and predicate are checked as for every surface.
  `expectedRevision` is compared with the snapshot the command runs on, inside its job, so no
  other change slips in between; a mismatch is -32006 (`cmd.revision.mismatch`, details
  `expected`, `current`) and nothing runs. Previews are described in [Queries, events,
  selection and preview as implemented](#queries-events-selection-and-preview-as-implemented).
- Diagnostics have `severity` (`error` or `warning`), `code`, `category`, `messageId`,
  `arguments`, `details` and `objects`. `summary` names the objects the run created, modified
  and deleted ([Change summary](#change-summary)).
- **Concurrency**: the caller's thread reads and answers requests; one worker per session runs
  `command.run` requests one at a time in arrival order; a writer thread owns the output. Every
  other request, `command.cancel` included, is answered while a command runs. At most 64 runs
  (`MAX_QUEUED_RUNS`) are running or waiting; more is -32008 (`limit` `queued_runs`), and a run
  with the id of a pending run is -32600 (`duplicate_id`). Cancelling a waiting run answers it
  with -32007 without running it; cancelling a running one rolls it back (-32007) when it
  reaches a checkpoint. At most 256 written messages wait for the output; a client that stops
  reading holds the session back rather than growing its memory. Without threads (WebAssembly) `serve` fails with `Unsupported`.
- **Progress**: a run's progress, forwarded by the invoker only after 300 ms, is sent as the
  notification `event.notify` with `params` `{"event": "command.progress", "requestId",
  "fraction", "messageId"}` before the run's response. A session receives the progress of its
  own runs until it unsubscribes from `command.progress` ([Events](#events)).

### Errors

Every error object has `code` (below), `message` (the stable error code) and `data` with
`code`, `category`, `messageId`, `arguments` and `details`. The detail `panic` is never sent
(it is logged); `violations` is sent as an array of `{pointer, kind}`. Operating-system error
text is never sent, and path errors carry no paths. A panic while handling a message is
contained and answered with -32603 (`cmd.protocol.internal`); a panic inside a command is -32012
(`cmd.fault`).

| Protocol code | Errors |
|---|---|
| -32700 | `cmd.protocol.parse` |
| -32600 | `cmd.protocol.invalid_request`, `cmd.protocol.not_initialized`, `cmd.protocol.already_initialized` |
| -32601 | `cmd.protocol.method_unknown` |
| -32602 | `cmd.protocol.invalid_params` |
| -32603 | `cmd.protocol.internal` |
| -32001 | `cmd.unknown`, `cmd.not_exposed`, category `unsupported` |
| -32002 | category `invalid_input` |
| -32003 | `cmd.disabled` |
| -32004 | category `permission_denied` |
| -32005 | category `not_found` |
| -32006 | category `conflict` |
| -32007 | category `cancelled` |
| -32008 | category `limit_exceeded` |
| -32009 | `cmd.path.outside_roots` |
| -32010 | categories `io`, `corrupt_data`, `failed_operation` |
| -32011 | `cmd.protocol.version_unsupported` |
| -32012 | category `internal` |

A code named in the table takes precedence over its category.

### Published schemas

`schemas/automation/requests.json` and `schemas/automation/responses.json` describe the
messages of the methods above (JSON Schema draft 2020-12 with `$id`). The conformance tests
validate every request they send and every message the server writes against them.

### Read and write roots

- `Roots::new(read, write)` canonicalizes each root, which must be an existing directory
  (`cmd.roots.invalid`, detail `index`). Write roots are also read roots. A `Session` holds its
  `Roots`; the default grants nothing, so a session without roots denies all file access.
  Commands reach files only through `Context::open_read` (class `file_read` or `file_write`)
  and `Context::create_write` (class `file_write`); a missing class is
  `cmd.permission_denied`.
- `open_read(path)` opens a regular file; `create_write(path)` creates a file in an existing
  directory or truncates an existing one. The architecture lists no directory operations, so
  there are none.
- A path must be non-empty, absolute and free of NUL (`cmd.path.rejected`, `reason` `empty`,
  `relative`, `nul`) and at most `MAX_PATH_LEN` long: 4095 bytes on Linux and Android, 32767
  UTF-16 code units on Windows, 1023 bytes elsewhere (`cmd.path.too_long`, also when the system
  reports a name too long). On Windows, a component with `:` (`alternate_data_stream`) or a
  reserved device name (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM0`-`COM9`,
  `LPT0`-`LPT9` and the superscript digits 1 to 3 after `COM` and `LPT`, with any extension, ignoring case and trailing dots and spaces;
  `device_name`) is rejected, a device-namespace or non-disk verbatim path is `device`, and a
  UNC path is `network` unless a root is a UNC path. `create_write` needs a file name as the
  last component (`not_a_file_name`); a path naming something other than a regular file is
  `not_a_file`.
- The canonical form must lie inside a root of the required kind, compared component by
  component as the system returns it (`cmd.path.outside_roots`); no Unicode normalization or
  case folding is applied, so a variant is accepted only when the system resolves it to an
  object inside a root. A missing path is `cmd.path.not_found`; other system errors are
  `cmd.path.io` with `reason` `denied`, `exists` or `other`.
- The check is bound to the opened handle; data is read or written only through a handle that
  passed it, and an existing file is truncated only after the check. Per platform:
  - Unix (Linux, Android, macOS and the BSD systems): the root directory is opened, then each
    remaining component of the resolved path is opened relative to the previous directory
    handle without following a link (`openat` with `O_NOFOLLOW` and `O_DIRECTORY`), and the
    file is opened, or created with exclusive creation, relative to the last directory handle
    without following a link. A directory or file that another process swaps for a link after
    the resolution is refused (`cmd.path.outside_roots`), so neither a read nor a new file is
    redirected outside the roots. An existing name, a link among them, is resolved and opened
    like a read.
  - Windows: after the path is resolved, every directory of the resolved path from the root
    down to the parent is opened without following reparse points and without sharing delete
    access, so none can be renamed, deleted or replaced by a junction meanwhile; a reparse
    point found on that chain (a swap after resolution) is refused, and the file is opened
    without following a reparse point. A new file is created with exclusive creation inside the
    held chain; an existing name, a link among them, is resolved and opened like a read.
  - WebAssembly: there is no file system; granting a root fails and every access is
    `cmd.path.outside_roots`.
  - Everywhere: opening a named pipe that another process places at a checked path can block.
- The escape-test suite (`tests/escape.rs`) builds a tree with a canary file outside the
  roots and tries absolute and relative symbolic links out, links in, link loops, `..` chains,
  Unicode normalization and case variants, NUL, over-long paths, junctions on Windows (skipped
  with a message when the system refuses to create them) and races (a thread swapping a file
  or a directory for a link while another opens it repeatedly), asserting that the canary is
  never read, written or joined by a new file. The table of escape cases (absolute and
  dot-dot paths, links out, a dangling link, a device node, over-long and relative paths, NUL,
  a directory, and on Windows reserved names and alternate data streams) also runs through the
  stdio protocol, the Model Context Protocol endpoint and the local socket, using two
  registered file commands. Every surface must answer each case with the same error code for
  reading and for writing; the swap races run on every surface as well.

## Headless runner as implemented

The module `headless` (feature `headless`, which enables the `arc-doc` package; off in the
WebAssembly build) implements CORE-CMD-031 after the
[headless CLI conventions](../architecture/commands-and-automation.md#headless-cli-conventions).
`headless::run(args, product, stdin, stdout, stderr)` is the whole program: `args` without the
program name, a `Product` (name, version, object types, evaluators, command registry, clock and
the edit-lock owner) and the three streams; it returns an `Exit` and never panics (a contained
fault is exit 70). A product's `main` passes the process arguments and streams and returns the
code. Arguments are parsed by hand: no argument-parsing crate is in the technology baseline.

- Standard output carries JSON only: one value and a newline, sorted keys, or the protocol
  messages of `serve --stdio`, or one line per package for `batch`. Errors go to standard error as one line
  `{"error": <error object>}` with the protocol's error object
  ([Errors](#errors)); protocol log records go there as `{"level", "event", "detail"}` lines.
- The one-shot subcommands answer through `Server::call`, the synchronous form of a protocol
  request in a session granted the whole ceiling, so their output equals the result of the
  protocol method named below.
- Flags are `--name value` or `--name=value`; `--` ends the flags. Single-value flags may not
  repeat. Read and write roots and permission classes come only from flags and default to
  none: the ceiling is `read`, the classes of the `--allow-*` flags, `file_read` with any root
  and `file_write` with a write root. Package files named on the command line are opened by
  the runner itself, not through the roots, which confine the commands.

| Subcommand | Behavior | Output |
|---|---|---|
| `commands` | Lists the commands exposed on `automation` | `command.list` result |
| `describe <command>` | Describes one command (identifier or alias) | `command.describe` result |
| `validate-params <command> [--params P]` | Validates parameters without running; exit 3 when invalid | `command.validate` result |
| `run <file> --command <id> [--params P] [--seed N] [--save] [--record M]` | Opens the package, runs one command through the invoker; with `--save` opens it under the edit lock and saves the document in place after a successful run; with `--record` writes the run as a macro | `command.run` result |
| `play <file> --macro M [--save]` | Opens the package and [plays](#playback) the macro or batch list `M` on it; with `--save` saves it in place after a successful playback | `{"revision", "results": [{"line", "command", "value"}]}` |
| `batch <file>... --macro M [--save]` | Plays `M` on each package in turn, each opened, played and (with `--save`, only when its playback succeeded) saved on its own; a package that fails to open or play does not stop the others | One line per package, in order: `{"file", "result"}` with the `play` output, or `{"file", "error"}` with the error object |
| `inspect <file> [--check]` | `arc_doc::package::inspect`; with `--check` adds `findings`, exit 3 when there are any | Inspection JSON |
| `validate <file>` | `arc_doc::package::check`; exit 3 when there are findings | `{"findings": [...]}` |
| `serve --stdio [<file>...] [--save] [--record M]` | Serves the protocol on stdin and stdout; each package is a document named by its file stem (a stem twice, or one not UTF-8, is a usage error); with `--save` each is saved in place when the session ends; with `--record` the session's successful runs are written as a macro when it ends | Protocol messages |
| `mcp [<file>] [--save] [--record M]` | Serves the [Model Context Protocol](#model-context-protocol-endpoint-and-local-socket-as-implemented) on stdin and stdout for the package (a document named by its file stem), or for no document; the surface is `mcp` and `--allow-edit` lifts read-only | MCP messages |
| `help` (also `--help`) | Writes the usage text to standard error | Nothing |

A finding is the protocol's error `data` (`code`, `category`, `messageId`, `arguments`,
`details`).

| Flag | Subcommands | Meaning |
|---|---|---|
| `--read-root <dir>`, `--write-root <dir>` | all | Repeatable; the session's roots (`cmd.roots.invalid` when one is not a directory) |
| `--allow-edit`, `--allow-network`, `--allow-process` | all | Add `edit`, `network`, `process` to the ceiling |
| `--json` | all | Accepted; output is always JSON |
| `--command <id>` | `run` | The command; required |
| `--params <P>` | `run`, `validate-params` | JSON text, or `@<file>` for a file's text (at most `input_bytes`); default `{}` |
| `--seed <n>` | `run` | The invocation seed, a decimal `u64`; default 0 |
| `--save` | `run`, `serve`, `play`, `batch` | Save in place |
| `--macro <M>` | `play`, `batch` | The macro or batch list; required. Read through the read roots (`cmd.path.outside_roots` without one), at most `input_bytes` |
| `--record <M>` | `run`, `serve` | Record successful runs into the macro file `M`, created (or truncated) through the write roots before anything runs, and written when the run or session ends; a failed `run` leaves it empty. A macro names no document, so more than one package is a usage error (`record_documents`) |
| `--check` | `inspect` | Add the check's findings |
| `--stdio` | `serve` | Required; the only transport |

| Exit | Meaning |
|---|---|
| 0 | Success |
| 1 | The command or operation failed: categories `not_found`, `conflict`, `cancelled`, `io`, `failed_operation`; `batch`: at least one package failed (each failure is on its output line) |
| 2 | Usage error (`cmd.cli.usage`, detail `reason`), or parameters missing a required member (`cmd.params.invalid` with a `required` violation): a CLI never prompts |
| 3 | Input invalid or corrupt: categories `invalid_input`, `corrupt_data`; invalid parameters from `validate-params`; package findings from `inspect --check` and `validate` |
| 4 | Unsupported: category `unsupported` |
| 5 | Denied: category `permission_denied` (permission classes, `cmd.path.outside_roots`) |
| 6 | Limit exceeded: category `limit_exceeded` |
| 70 | Internal fault: category `internal`, or a fault contained by the runner |

Exit 7 (`--fail-on-loss`) belongs to conversions, which the shared set does not have yet. A
`batch` whose macro cannot be read fails before any package with that error's exit code. The
conventions name `batch <file> <macro.jsonl>` for one package; here one package is `play` and
`batch` takes several packages, so the macro is the flag `--macro`. `render` needs a renderer,
which this layer does not have; the reference CLI adds it as a product subcommand
([Reference CLI](arc-app.md#reference-cli-core-app-026)). The
conventions' `info`, `convert`, `render` and `upgrade` subcommands and the
`--log-level`, `--log-format`, `--locale`, `--limits` and `--output` flags are not provided;
they are usage errors. The architecture names parameter validation as a protocol method only and
gives `validate <file>` the package check, so parameter validation is the separate subcommand
`validate-params`.

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
| `cmd.protocol.parse` | `invalid_input` | A protocol message is not UTF-8 JSON |
| `cmd.protocol.invalid_request` | `invalid_input` | A protocol message is not a valid request; the detail `reason` says why |
| `cmd.protocol.method_unknown` | `not_found` | The protocol has no method of that name |
| `cmd.protocol.method_unsupported` | `unsupported` | The protocol method belongs to a later capability |
| `cmd.protocol.invalid_params` | `invalid_input` | The parameters of a protocol method are malformed; the detail `reason` says why |
| `cmd.protocol.internal` | `internal` | A fault escaped while handling a protocol message |
| `cmd.protocol.limit_exceeded` | `limit_exceeded` | A message or the number of queued runs exceeds a limit; the detail `limit` names it |
| `cmd.protocol.not_initialized` | `conflict` | A request other than `arc.initialize` arrived first |
| `cmd.protocol.already_initialized` | `conflict` | `arc.initialize` arrived in an initialized session |
| `cmd.protocol.version_unsupported` | `unsupported` | No supported protocol version lies in the client's range; the detail `supported` lists them |
| `cmd.document.unknown` | `not_found` | The session has no document of that identifier |
| `cmd.revision.mismatch` | `conflict` | The expected revision is not the document's current revision |
| `cmd.preview.unsupported` | `unsupported` | The command controls undo (undo, redo, groups) and cannot run as a preview |
| `cmd.preview.rollback_failed` | `internal` | Reserved: previews are dry runs and no longer roll back, so it is not produced |
| `cmd.query.cursor_invalid` | `invalid_input` | The cursor is not one this server issued for these parameters |
| `cmd.query.cursor_stale` | `conflict` | The document is at another revision than the one the cursor was issued for; the details `expected` and `current` name them |
| `cmd.selection.unknown` | `not_found` | A selected identifier names no object of the document; the detail `unknown` lists them |
| `cmd.roots.invalid` | `invalid_input` | A root to grant is not an existing directory |
| `cmd.path.rejected` | `invalid_input` | The path is not acceptable; the detail `reason` says why |
| `cmd.path.too_long` | `limit_exceeded` | The path is longer than the platform allows |
| `cmd.path.outside_roots` | `permission_denied` | The path lies outside the read or write roots |
| `cmd.path.not_found` | `not_found` | The path names nothing |
| `cmd.path.io` | `io` | The system refused the file operation; the detail `reason` says how |
| `cmd.cli.usage` | `invalid_input` | The runner's arguments do not follow its grammar; the detail `reason` says what is wrong |
| `cmd.macro.malformed` | `invalid_input` | A macro or batch line is not well formed; the details `line` and `reason` say where and why |
| `cmd.macro.version_unsupported` | `unsupported` | A macro header names a format version that is not read; the detail `version` names it |
| `cmd.macro.limit_exceeded` | `limit_exceeded` | A macro, one of its lines or a recording exceeds a limit; the detail `limit` names it |
| `cmd.macro.input_missing` | `invalid_input` | A macro input has no value at playback; the detail `input` names it |
| `cmd.transparent.refused` | `conflict` | A transparent command cannot run beside the active prompt; the detail `reason` says why |
| `cmd.socket.disabled` | `permission_denied` | The user has not enabled automation, so no socket is served |
| `cmd.socket.unauthorized` | `permission_denied` | A connection did not present the session token |
| `cmd.socket.unsupported` | `unsupported` | Local sockets are not available on this platform yet |
| `cmd.socket.directory` | `invalid_input` | The socket directory is not a private directory of the user, or its owner is not the owner of the token file; the detail `reason` says why |
| `cmd.socket.io` | `io` | The system refused an operation on the socket or its token; the detail `reason` says which |
| `cmd.render.unavailable` | `unsupported` | No renderer is attached, so a view cannot be rendered |

## Model Context Protocol endpoint and local socket as implemented

The module `protocol::mcp` (CORE-CMD-029) and `protocol::socket` (CORE-CMD-020).

### Model Context Protocol endpoint

`Server::serve_mcp(&McpConfig, input, output)` serves one session over the framing of the
automation protocol: one JSON-RPC 2.0 message per line, bounded like the protocol's messages
(`cmd.protocol.limit_exceeded` with `limit` `message_bytes` or `nesting_depth`, id `null`, and
the session goes on). It is a thin adapter: every tool call goes through `Server::call`, so
exposure, permission classes, limits, schemas and predicates are those of the automation
protocol. The server's `ServerConfig::surface` must be `mcp`, which makes
`command.list`, `command.describe` and `command.run` reach the commands exposed on that
surface; it defaults to `automation`, so nothing else changes.

- **Messages**: `initialize` (answers the client's `protocolVersion` when it is one of
  `MCP_VERSIONS`, `2025-06-18`, `2025-03-26`, `2024-11-05`, otherwise the first; capabilities
  `tools` without list-change notifications; the product as `serverInfo`), `ping`,
  `tools/list` and `tools/call`. Other requests are `cmd.protocol.method_unknown`; any
  request but `ping` before `initialize` is `cmd.protocol.not_initialized`; notifications and
  responses from the client are ignored (cancellation notifications included: requests are
  answered one at a time). Resources and prompts are not provided.
- **Tools**: the six fixed tools, then one per command exposed on `mcp` in identifier order,
  named by the identifier with dots replaced by underscores, with the parameter schema as
  `inputSchema`, the description and label (texts from `McpConfig::messages`, else the message
  identifier) and the annotation `readOnlyHint` (every permission class is `read`). A name
  taken by an earlier tool is not offered again.
  Fixed names have one underscore and identifiers give at least two, so only two commands can
  collide.

| Fixed tool | Arguments | Same as |
|---|---|---|
| `list_commands` | none | `command.list` with the document |
| `describe_command` | `command` | `command.describe` with the document |
| `query_objects` | optional `types`, `ids`, `owner`, `fields`, `limit`, `cursor` | `query.objects` |
| `get_object` | `id`, optional `fields` | the one object of `query.objects` for `id`, or `doc.object.not_found` |
| `get_diagnostics` | optional `severity`, `limit`, `cursor` | `query.diagnostics` |
| `render_view` | optional `view`, `width`, `height` | always `cmd.render.unavailable`: no renderer is attached to this layer (view rendering, CORE-CMD-026, is not provided) |

- **Results**: a call answers `content` with one text item, the JSON text of the result, and
  `isError`. A failure of the call, including `cmd.permission_denied` for an edit command, is
  `isError: true` with the protocol's error data (`code`, `category`, `messageId`,
  `arguments`, `details`) as the text, as MCP asks for errors of tools; an unknown tool or
  malformed arguments are JSON-RPC errors (-32602).
- **Read-only unless edit is allowed**: the server's ceiling decides. The headless `mcp`
  subcommand has `read` unless `--allow-edit`; a read-only session lists the edit tools and
  every call of one is refused with `cmd.permission_denied`. Tools work on the one document of
  `McpConfig::document`; a `document` argument is replaced. Edits are applied directly. Running
  edits as previews until a user approves them (CORE-CMD-030) is a later row, as are the `undo`
  tool and the `--attach` mode.

### Local socket

`LocalSocket::bind(&SocketConfig)` and `LocalSocket::serve(&Server)` implement CORE-CMD-020
after [ADR-0010](../adr/0010-automation-protocol-transport.md).

- **Enabling**: `SocketConfig::enabled` is the user's choice, made by the application. Without
  it `bind` is `cmd.socket.disabled` and nothing is created.
- **Files**: `directory` is created with mode 0700 when missing (its parent must exist); an
  existing one must be a directory, not a link, that group and others cannot access, and its
  owner must be the owner of the token file (`cmd.socket.directory`, reasons `not_directory`,
  `not_private`, `create`, `name`, `owner`). `bind` creates the socket `<name>.sock` first and
  sets it to mode 0600 before `serve` accepts anything, then writes `<name>.token`, created with
  mode 0600 and exclusive creation, and checks its owner against the directory's. A failed `bind`
  removes only the files it created: the socket it bound, or the token file it wrote. A socket
  file left by a dead process is replaced, with the token file that process left behind; one that
  accepts a connection is `cmd.socket.io` with reason `in_use`, and a token file that already
  exists is `cmd.socket.io` with reason `token`. Dropping the `LocalSocket` removes the two files
  its `bind` created. The application chooses the directory (for example below the user's
  runtime directory); this layer reads no environment.
- **Token**: 32 bytes from the operating system's random source (`/dev/urandom`), as 64
  hexadecimal digits. It is compared in constant time (equal lengths, every byte folded), and
  never logged, sent, printed by `Debug` or reported in an error.
- **Handshake**: the first line of a connection, at most 1,024 bytes before its newline, must be
  `{"jsonrpc":"2.0","id":<string or integer>,"method":"arc.authenticate","params":{"token":"<token>"}}`.
  The whole line must arrive within one deadline counted from the accept: 10 seconds by default
  (`HANDSHAKE_TIMEOUT`, set per socket in `SocketConfig::handshake_timeout`). The deadline is not
  extended by each byte that arrives. The bytes read for the handshake stop at the newline or at
  1,025 bytes, whichever comes first; the protocol session that follows is not bound by the
  deadline. It belongs to the transport, not to the protocol methods: it is not in `METHODS` and not in the
  published schemas. Success answers `{"jsonrpc":"2.0","id":<id>,"result":{}}`, after which the
  connection is an ordinary session that starts with `arc.initialize`. Anything else (a wrong,
  missing or mistyped token, another first message, an oversize or malformed line, no line in
  time) is answered with one error line, `cmd.socket.unauthorized` (-32004, id `null`, no
  details), and the connection is closed; a connection never reaches a method without it.
  `params.clientName` is optional (1 to 256 bytes); in client-table mode it is required and names
  the client whose token is compared. An unknown name is compared against a dummy token of the
  same length, so the answer takes the same time as for a known name.
- **Serving**: `serve` accepts until `SocketStop::stop` (from `stopper()`, which also wakes the
  accept), serves each authenticated connection as one session of the server on its own thread,
  allows 16 at once (`MAX_CONNECTIONS`; others are closed unanswered), of which at most 8
  (`MAX_HANDSHAKING`) wait in their handshake: a connection beyond that share is closed unanswered
  at once, so connections that never authenticate cannot hold every slot until their deadline.
  Connections that already authenticated do not count against the share. Stopping revokes and
  closes the open ones. The protocol, its limits and its framing are those of `Server::serve`
  unchanged.
- **Client table**: `LocalSocket::bind_clients` binds in client-table mode, which creates no
  `<name>.token` file. `add_client(name, ceiling)` writes `<name>@<client>.token` (owner-only,
  exclusive creation, same owner check as the session token) and returns its path. The client
  name follows the socket name rules, which do not allow `@`, so no client file can be the file
  of another socket, and it is unique in the table. The table holds at most `MAX_CLIENTS` (256)
  clients; a further `add_client` is `cmd.socket.io` with reason `clients`. A connection
  authenticated as a client runs with the intersection of that client's ceiling and the server's
  ceiling, so the `permissions` a client requests are granted only within both.
  `remove_client(name)` revokes the client's open connections, closes them, refuses its next
  handshake and removes its token file before it releases the table, so an `add_client` of the
  same name right after it succeeds; it returns whether the client existed. A revoked session
  reads no further request, starts no run it had queued and answers each such run with
  `cmd.cancelled`; a command already running is cancelled when its session's reader ends. Adding
  a client on a single-token socket is refused. A client file left by a process that died blocks
  its name until someone removes it: the directory is not swept, since that could delete the
  token of another socket with a similar name. Dropping the socket removes the client files it
  wrote.
- **Connection events**: `serve_observed(server, observer)` tells a `ConnectionObserver` when a
  connection passes its handshake (`opened`, with a connection identifier unique for the life of
  the socket and the client name the handshake carried, with control characters replaced, empty
  when none) and when it ends for any reason (`closed`). Both calls run on the connection's own
  thread and hold no lock of the socket. A refused handshake reports nothing. `serve` is
  `serve_observed` with a `NoObserver`.
- **Platforms**: Unix domain sockets on Linux and macOS. Named pipes for Windows are not
  provided: `bind` there is `cmd.socket.unsupported` (after the enabled check).

## Queries, events, selection and preview as implemented

The modules are `protocol::query`, `protocol::events` and `protocol::run` (CORE-CMD-006,
CORE-CMD-023, CORE-CMD-024, CORE-CMD-025, CORE-CMD-028). The methods belong to the methods of
the architecture's family table for protocol 1.0, which the server already named and answered
with `cmd.protocol.method_unsupported`; implementing them adds methods, optional result fields
and notification names within version 1.0, as the versioning rule of the architecture allows,
and changes no parameter or result field a 1.0 client relies on. The protocol version stays
`1.0`. `arc.capabilities` lists the methods that exist. `query.object`, `query.compare`,
`query.derived`, `selection.add` and `selection.remove` stay unsupported: comparison and
derived-data queries need the change sets and the dependency graph surfaced by later
capabilities, and the other two are single-object and incremental forms of methods provided
here.

### Change summary

Every `command.run` result has `summary`: `created`, `modified` and `deleted` (identifiers in
ascending order, at most 1000 per list), `counts` (complete) and `truncated` (a list was cut, or
an event covered more objects than it lists). It is the net effect of the run's journal
record: an object created and deleted in the same transaction appears in no list, one deleted
and created again counts as modified. A run that changed nothing has empty lists.

### Preview

`command.run` with `preview: true` runs the command as a dry run of the document
(`Invoker::dry_run`, which calls `Document::dry_run`): the command runs on the document
executor against the current snapshot and its transaction is validated exactly like a commit
(field schemas, relationship and deletion rules, type and document validators), then discarded.
The result has `preview: true`, `summary` (the net effect of the operations the commit would
have journaled, cascades included), `baseRevision` and `revision`, which are both the document's
revision, the value to pass as `expectedRevision` to the real run.

- A preview changes nothing: the snapshot is the same one, the revision, the journal, the undo
  and redo stacks and any open group are as before, and no `document.changed` event or
  diagnostic is produced. A changing preview does not clear the redo stack.
- The identifiers a command generates are seeded with the revision. Because a preview does not
  advance it, the identifiers in its summary and value are those a real run at that revision
  creates.
- `expectedRevision`, permissions, the predicate and parameter validation apply as for a run,
  and a command that fails validation or faults returns the error a real run returns. A command
  with undo behavior `undo_control` cannot be previewed: `cmd.preview.unsupported` (-32001).
  `cmd.preview.rollback_failed` is no longer produced.
- Runs and previews through one server stay mutually exclusive per document. A preview no
  longer needs it; real runs do, so that the revision recorded with a macro step and the
  selection read back from a run's own journal record are those of that run and not of another
  session's commit in between.
- A preview is not recorded in macros and does not change the selection.

### Queries

All query methods need the permission class `read` (`cmd.permission_denied` otherwise) and name
the revision they read (`revision`, the revision of the snapshot taken at the start of the
request). They take `document`, optional `limit` (the page size) and optional `cursor`.

| Method | Parameters besides the page | Result |
|---|---|---|
| `query.objects` | optional `types` (type keys; an unregistered key is `doc.type.unknown`), `ids`, `owner`, `fields` | `objects`, `nextCursor` |
| `query.references` | `id`, optional `direction` (`outgoing`, `incoming`, `both`; default `both`) | `id`, `references`, `nextCursor` |
| `query.diagnostics` | optional `severity` (`error`, `warning`) | `diagnosticsVersion`, `dropped`, `diagnostics`, `nextCursor` |
| `query.history` | none | `transactions`, `nextCursor` |

- **Filters** combine with and. `types` lists type keys, `owner` an identifier whose owned objects
  match, `ids` identifiers (an identifier named twice is visited once, one that names no object
  matches nothing). Order is by type key and then store slot, or the order of `ids`.
- **Entries**: an object has `id`, `type`, `fields` and, when present, `owner`, `name` and
  `partition`. `fields` holds every top-level field by name when `fields` is omitted, and
  otherwise the listed paths. A path is a field name, `.name` for a record field, `[index]` for
  a list element and `["key"]` for a map entry, the form `FieldPath` prints, at most 256 bytes
  and 16 steps; a malformed path is `cmd.protocol.invalid_params` with reason `fields`. A path
  that addresses no value of an object (absent, out of range, of another type) is `null`. A
  value is the canonical JSON `arc-doc` serializes a field value as (`serde_json::to_value` of
  `arc_doc::Value`, the form the package inspection prints). A reference entry has
  `direction`, `object` (the other end), `field` (the path, or `(owner)`) and `type`. A
  diagnostic has the members of a run's diagnostics. A transaction has `id`, `revision`,
  `command` and `label` (identifier and message identifier, `null` for transactions that carry
  none), `time` (milliseconds), `kind` (`normal`, `undo_of`, `redo_of`, `merge`, `migration`,
  `link_reload`), `target` (the transaction an undo or redo concerns), `group` and `counts`;
  history lists the journal oldest first.
- **Pages**: `limit` defaults to 100 and is at most `limits.pageSize`, the smaller of 1000 and
  the session's `array_length`; more is -32008 (`limit` `page_size`), 0 is
  `cmd.protocol.invalid_params`. `nextCursor` is `null` after the last page. A cursor is
  opaque; it encodes the revision (for diagnostics also the diagnostics version), the
  position and a digest of the method and its parameters other than the page. A cursor that is
  malformed or belongs to other parameters is -32002 (`cmd.query.cursor_invalid`); one
  issued at another revision is -32006 (`cmd.query.cursor_stale`, details `expected` and
  `current`), and the client starts again. Skipping to a position walks the matching objects,
  so a page costs time proportional to its position.
- `ids`, `types` and `fields` hold at most the session's `array_length` entries (-32008, `limit`
  `array_length`).
- A document's diagnostics list is the document's own (validator findings, load findings,
  failed nodes); it changes without a revision, which the diagnostics version covers.

### Events

`event.subscribe` and `event.unsubscribe` take `events` (one to three of `command.progress`,
`document.changed`, `selection.changed`) and optional `document` (omitted: every document) and
answer `subscriptions`, the session's subscriptions. Both need `read`; both are idempotent. The
notifications are `event.notify` with these `params`:

| `event` | Members | Sent |
|---|---|---|
| `command.progress` | `requestId`, `fraction`, `messageId` | for the session's own runs |
| `document.changed` | `document`, `revision`, `transaction`, `commits`, `changes` (a change summary) | after a commit of the document, whoever made it |
| `selection.changed` | `document`, `revision`, `source` (`client` or `command`), `objects` | when the session's selection of the document changes |
| `events.dropped` | `dropped` | before the next notification, when `dropped` notifications were lost |

- A session receives `command.progress` for its own runs from the start, so clients that never
  subscribe are unaffected; unsubscribing stops it. A run's events, progress included, are
  written before its response. A newer progress notification of a run replaces an unsent one
  of the same run.
- Document events come from the document's event bus. A subscription sees commits after
  it was made. Events that the bus coalesced (more than 64 unread) arrive as one event with
  `commits` above 1 and the union of the changes, and a change set larger than the bus holds
  is `truncated`. A subscription ignores the other bus events (regeneration, diagnostics,
  saves, link states).
- Each session queues at most `limits.eventQueue` (256) notifications for its client. When
  the client reads slower than events arrive, the oldest queued notification is dropped, the
  loss is counted and announced with `events.dropped` ahead of the notifications that follow;
  the document is never held back. Responses are not subject to this queue.

### Selection

`selection.get` (`document`) and `selection.set` (`document`, `objects`: entries of `id` and
optional `subObjects`, a list of keys) answer `document`, `revision` and `objects`; both need
`read`. The selection belongs to the session and the document; it is ordered, repeated
identifiers merge their keys, and `selection.set` replaces it (an empty list clears it).

- Every identifier must name an object of the latest revision; otherwise nothing changes and
  the answer is -32005 (`cmd.selection.unknown`, argument `count`, detail `unknown`: the
  identifiers, at most 1000). At most 10000 objects, 1024 keys of 256 bytes each per object
  (-32008, `limit` `selection` or `sub_objects`). Keys are opaque to this layer.
- Commands the session runs receive the identifiers of the selection that still name objects,
  as their `selection` (`Context::selection`). `selection.get` and runs drop identifiers whose
  object was deleted.
- After a command that recorded a selection to restore after it (`set_selection_after`), the
  session's selection of that document is that selection, without keys, and a `selection.changed`
  event with `source` `command` is sent. Previews do not change the selection.

### Schema coverage

`schemas/automation/requests.json` and `schemas/automation/responses.json` define `params` and
`result` of every method in `METHODS` under the method's name; the conformance test fails when
a method lacks either, or is not exercised by the recorded session, and validates every message
of that session against them, notifications included.

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
