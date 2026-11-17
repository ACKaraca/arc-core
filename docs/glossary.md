# Glossary

Terms used across the `arc-core` documentation, one line each.

| Term | Meaning |
|---|---|
| Arc Automation Protocol (AAP) | JSON-RPC 2.0 protocol over stdio or a user-scoped local socket through which scripts, assistants, CLIs and workers drive an Arc application |
| Arc Scene | Versioned renderable scene description and session protocol between products and the renderer process |
| `arc-worker` | Helper executable that runs hostile-input parsing and crash-prone native work out of process |
| Aspect | Named group of an object's fields used as the unit of dependency tracking |
| Asset | Large binary value stored once in a package, addressed by its BLAKE3 hash |
| Baseline tag | The single `arc-core` release tag that every product of a suite release pins |
| Branch stability | Solver property that a solution does not flip orientation unless the user changed it |
| Change set | Base state plus ordered journal records, exchanged for multi-user editing |
| Chunk | Fixed-size block of 1,024 store slots shared between revisions until written; also a content-defined piece of a large asset |
| Command | Registered, typed, validated operation; the only way a document changes |
| Command catalog | JSON export of all exposed commands, used for documentation, MCP tools, the Python client and tests |
| Conflict record | Kept incoming and current values of a field changed concurrently, awaiting a resolving command |
| Derived data | Data computed from document objects by the dependency graph and never journaled |
| Derived key | Name of a kind of derived data registered with an evaluator |
| Display list | Exact drafting primitives of a space, tagged with their source objects |
| Document executor | Per-document thread that runs commands and commits transactions in order |
| Early cutoff | Stopping dirty propagation when a recomputed result equals the previous one |
| Extractor | Product function that turns changed objects into viewport render items |
| FFI crate | Workspace crate allowed to contain `unsafe` code to bind a native library |
| Field descriptor | JSON description of a type's fields stored in every package so payloads can be decoded without Arc |
| Golden-model corpus | STEP files and feature scripts with expected metrics used to gate B-rep changes |
| Group | Set of transactions that undo as one step |
| Handle | In-memory generational index into a typed store; never persisted |
| Layer table | `xtask/layers.toml`, the authoritative crate layering enforced by `cargo xtask layers` |
| Limits profile | Named set of hostile-input limits passed to readers |
| Loss report | Structured record of what an import or export mapped, approximated, preserved, dropped or failed |
| Neutral model | A format crate's typed model of its format, free of product types |
| `ObjectId` | Persistent 128-bit UUIDv7 identifier of an object |
| Partition | Product-defined load unit of a package for partial loading |
| Permission class | Category of effect a command has (`read`, `edit`, `file_read`, `file_write`, `network`, `process`, `settings`) |
| Preserved object | Object of an unknown type kept unchanged through load and save |
| Preserved record | Unmodeled foreign data kept from an import and written back on export |
| Prompt machine | Interactive state machine that collects a command's parameters from user input |
| Quarantined object | Object that failed load validation, kept read-only and listed in diagnostics |
| Read and write roots | Directories to which automation surfaces may confine file access |
| Recovery journal | Per-session database holding committed transactions since the last save |
| Revision | Session-local counter incremented by each commit |
| Second concrete user | The rule that code moves into `arc-core` only when two products need it |
| Snapshot | Immutable view of a whole document at one revision |
| Source map | Mapping from imported objects to their identifiers in the source file |
| Sub-object key | `(ObjectId, local key)` addressing a part of an object |
| Suite rules | The rule block shared verbatim by every `AGENTS.md` in the suite |
| `TopoName` | Persistent name of a B-rep face, edge or vertex |
| Transaction | Atomic, validated set of operations on one document |
| Trust tier | Host-assigned level of a plug-in package: bundled, verified, local or blocked |
| Type key | Namespaced string that names a registered object type |
| View source | Product interface supplying geometry and styles to derived views |
| WebAssembly set | Crates that must build for `wasm32-unknown-unknown` |
