# arc-doc

The document store: typed objects with stable identifiers, transactions, the operation journal,
snapshots, the dependency graph and regeneration scheduler, external references, schema migration
and the single-file document package with recovery. The architecture is specified in
[document-model.md](../architecture/document-model.md) and
[document-package.md](../architecture/document-package.md).

## Owns

- The type registry, typed stores, identifier index and reverse reference index.
- Transactions, savepoints, validation hooks and the document executor.
- The operation journal, undo and redo, groups, coalescing, history, comparison and change sets.
- Snapshots and their publication.
- The dependency graph, evaluation context, scheduler, derived data cache and diagnostics.
- External reference objects and linked-document loading.
- Schema versions, migrations, preserved objects and quarantine.
- The package format, the storage thread, the recovery journal, saving, locking, assets and
  inspection (feature `package`).
- Core object types: `core.doc.link`, `core.doc.settings`, `core.doc.property_set_definition`,
  `core.doc.preserved`.

## Stays in products

Every domain object type and its validators, regeneration evaluators and migrations; partition
strategy (what a partition means for the product); the choice of history retention defaults; the
file extension.

## Key concepts and types

| Concept | Description |
|---|---|
| `Document` | Handle to an open document: executor, latest snapshot, journal, graph, storage |
| `Snapshot` | Immutable view at one revision |
| `TypeRegistry` | Type keys with schema versions, field descriptors, validators, migrations and relationship rules |
| `Store<T>` | Chunked copy-on-write store of immutable values addressed by `Handle<T>` |
| `Txn` | Open transaction with operations and savepoints |
| `TxnRecord` | Journal record (see the document model) |
| `ChangeSet` | Base state plus records, for exchange |
| `Comparison` | Per-object differences between two states |
| `NodeKey` | Dependency graph node: (object or graph id, derived key) |
| `EvalContext` | Tracks reads, provides cancellation and yield points to evaluators |
| `Diagnostic` | Severity, code, message, objects involved, origin (validator, node, load) |
| `Package` | Open SQLite package with storage thread and recovery journal |
| `AssetHash` | BLAKE3 hash of an asset |

## Dependencies

| Kind | Crates |
|---|---|
| Arc | `arc-base`, `arc-geom`, `arc-mesh` (geometry field encodings) |
| Third-party | `rusqlite` (bundled, feature `package`), `postcard`, `zstd`, `fastcdc`, `blake3`, `serde`, `rayon`, `crossbeam-channel`, `arc-swap` |

`arc-doc` does not depend on `arc-brep`; B-rep values are stored as assets with an encoding name
registered by `arc-brep`.

## Test strategy

Model-based property tests that apply random command sequences, undo and redo, and compare against
a reference model; journal inversion tests (apply record then inverse equals identity); snapshot
isolation tests under concurrent readers; scheduler tests for stale-result rejection and
cancellation; package compatibility fixtures, refusal tests, kill-during-save crash tests; fuzzing
of package and record decoding; benchmarks for commit latency, snapshot cost, load and save of the
reference large models.

## Capabilities

### Objects and stores

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-001 | Type registry | Registers object types with type key, schema version, field descriptors, validators, migrations and relationship rules; a duplicate key fails registration. | T1 | 0 |
| CORE-DOC-002 | Typed stores | Stores objects of each type in chunked copy-on-write stores; reading an object by handle costs constant time and never blocks on writers. | T1 | 0 |
| CORE-DOC-003 | Identifier lookup | Resolves an `ObjectId` to a handle in any snapshot; unknown or deleted identifiers return `doc.object.not_found`. | T1 | 0 |
| CORE-DOC-004 | Field descriptors | Generates a JSON field descriptor for each type and version from its definition, identical across runs. | T1 | 0 |
| CORE-DOC-005 | Property sets | Stores definitions of property sets as objects and their typed instances on objects; editing a definition migrates instances in the same transaction. | T1 | 1 |
| CORE-DOC-006 | Sub-object keys | Addresses object parts by `(ObjectId, local key)` with keys stable across edits that keep the part. | T1 | 1 |
| CORE-DOC-007 | Partitions | Assigns objects to product-defined partitions that can be loaded and unloaded independently. | T2 | 2 |

### References

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-008 | Relationship rules | Enforces strength, on-delete (cascade, clear, block), on-copy and cardinality rules for every reference field at commit. | T1 | 0 |
| CORE-DOC-009 | No dangling references | Rejects any transaction that would leave a reference to a missing or wrongly typed object with `doc.reference.dangling`. | T1 | 0 |
| CORE-DOC-010 | Reverse reference index | Answers "what references this object" for any object in constant time per result, maintained incrementally. | T1 | 0 |
| CORE-DOC-011 | External references | Links other documents through link objects with relative paths; linked documents load read-only and their objects are addressable as `(link, ObjectId)`. | T1 | 1 |
| CORE-DOC-012 | Unresolved and outdated links | Keeps unresolved links and their references intact and reports them; detects changed linked files and reloads them only through an undoable command. | T1 | 1 |

### Transactions and executor

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-013 | Transactions | Applies create, modify, property, delete and relink operations inside a transaction; reads inside see uncommitted changes; other readers see none until commit. | T1 | 0 |
| CORE-DOC-014 | Savepoints | Rolls a transaction back to a savepoint without abandoning it. | T1 | 0 |
| CORE-DOC-015 | Commit validation | Runs field, reference, type and document validation in order; any error rolls back everything; warnings become diagnostics. | T1 | 0 |
| CORE-DOC-016 | Document executor | Runs transactions for one document strictly in order on its own thread; callers submit work and receive results without blocking the UI thread. | T1 | 0 |
| CORE-DOC-017 | Snapshot publication | Publishes an immutable snapshot atomically on each commit; taking a snapshot costs constant time regardless of document size. | T1 | 0 |
| CORE-DOC-018 | Fault rollback | Restores the last committed snapshot when a transaction body fails or unwinds, leaving no partial change. | T1 | 0 |

### Journal, undo and history

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-019 | Journal records | Appends one self-inverting record per committed transaction with field-level before and after values. | T1 | 0 |
| CORE-DOC-020 | Undo and redo | Undoes and redoes by appending inverse or repeated transactions; undo then redo returns a state whose comparison with the original is empty. | T1 | 0 |
| CORE-DOC-021 | Groups | Nests transactions in groups that undo as one step; a failed group owner rolls back the whole group. | T1 | 0 |
| CORE-DOC-022 | Coalescing | Merges consecutive transactions with the same coalesce key within one gesture into one undo step. | T1 | 0 |
| CORE-DOC-023 | Undo limits | Bounds the undo stack by step count and retained-memory budget, dropping oldest steps first while keeping journal records. | T1 | 0 |
| CORE-DOC-024 | Undo conflict detection | Refuses to undo a transaction whose fields were changed afterward by a merge, link reload or change set, with `doc.undo.conflict`. | T1 | 2 |
| CORE-DOC-025 | History queries | Lists records filtered by object, type, author and time range, and the records that touched a given object. | T2 | 1 |
| CORE-DOC-026 | State comparison | Compares two states (revisions, files, change sets) per object with changed field paths and both values, skipping shared chunks; output has a canonical JSON form. | T1 | 1 |
| CORE-DOC-027 | Change sets | Exports records since a base as a change set and applies change sets with rebasing of commuting operations. | T1 | 5 |
| CORE-DOC-028 | Conflict records | Keeps both values of conflicting fields in conflict records instead of discarding either, until a resolving command runs. | T1 | 5 |

### Dependency graph and regeneration

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-029 | Derived keys and evaluators | Registers derived keys with pure evaluators; evaluator reads are recorded automatically as dependencies. | T1 | 0 |
| CORE-DOC-030 | Dirty propagation with early cutoff | Marks dependents of changed object aspects dirty and stops propagation where a recomputed result hash is unchanged. | T1 | 0 |
| CORE-DOC-031 | Prioritized scheduling | Evaluates dirty nodes on the worker pool in blocking, interactive, visible and background priority order, in parallel where independent. | T1 | 0 |
| CORE-DOC-032 | Cancellation and stale rejection | Cancels running jobs whose inputs change and applies a result only if none of its inputs changed after the revision it read. | T1 | 0 |
| CORE-DOC-033 | Synchronous ensure | Lets a running command block until listed nodes are clean for its uncommitted state, discarding those results on rollback. | T1 | 1 |
| CORE-DOC-034 | Failure states | Tracks clean, dirty, running, failed, blocked and cyclic node states, keeps the last good result for display, and clears failures when evaluation succeeds. | T1 | 0 |
| CORE-DOC-035 | Diagnostics | Maintains a document diagnostics list from validators, failed nodes, quarantined objects and unresolved links, with change events. | T1 | 0 |

### Events and migration

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-036 | Document events | Emits committed, regenerated, diagnostics, saved, link-state and conflict events through bounded channels, coalescing for slow subscribers. | T1 | 0 |
| CORE-DOC-037 | Schema migration | Migrates payloads of older schema versions in memory on load through chained migrations; files change only on save. | T1 | 0 |
| CORE-DOC-038 | Newer-version handling | Opens documents containing newer type minor versions read-only with unknown bytes preserved, and refuses newer major versions without modifying the file. | T1 | 0 |
| CORE-DOC-039 | Preserved and quarantined objects | Loads objects of unregistered types as preserved objects and invalid objects as quarantined read-only objects, writing both back unchanged. | T1 | 0 |

### Package and storage

| ID | Capability | Behavior | Tier | Phase |
|---|---|---|---|---|
| CORE-DOC-040 | Package format | Writes and reads the documented SQLite package schema with application id, format version, objects, descriptors, indexes, journal, assets, previews and preserved data. | T1 | 0 |
| CORE-DOC-041 | Recovery journal | Writes committed transactions to a per-session recovery database so that no transaction committed more than one second before a crash is lost. | T1 | 0 |
| CORE-DOC-042 | Crash recovery | Lists orphaned recovery journals at start-up and replays them onto an unchanged base, or opens the recovered state as a new document when the base changed. | T1 | 0 |
| CORE-DOC-043 | Atomic incremental save | Saves local files in place within one SQLite transaction writing only changed rows; an interrupted save leaves the previous state intact. | T1 | 0 |
| CORE-DOC-044 | Atomic full rewrite | Writes a complete verified package to a temporary file and replaces the target atomically for Save As, compaction, upgrades and unreliable locations. | T1 | 0 |
| CORE-DOC-045 | Background save | Saves from a snapshot on the storage thread with progress and cancellation while editing continues. | T1 | 0 |
| CORE-DOC-046 | Edit locks | Takes a sidecar edit lock on open for editing; other processes open read-only and show the holder; stale local locks are taken over only after confirmation. | T1 | 0 |
| CORE-DOC-047 | Content-addressed assets | Stores large values once by BLAKE3 hash, chunked with FastCDC above 4 MiB, verifying hashes on read. | T1 | 0 |
| CORE-DOC-048 | Partial loading | Loads selected partitions and objects on demand, following references to load required targets. | T2 | 2 |
| CORE-DOC-049 | Compaction | Removes unreferenced assets, chunks and history beyond the retention setting, reporting reclaimed size. | T2 | 1 |
| CORE-DOC-050 | Inspection and check | Dumps metadata, statistics and decoded objects as JSON using embedded descriptors, and verifies integrity, hashes, references and indexes without writing. | T1 | 0 |
| CORE-DOC-051 | Previews | Stores thumbnail and preview images supplied by the application at save, and saves without them when none are available. | T1 | 0 |
| CORE-DOC-052 | Source map | Records source identifiers of imported objects and resolves re-imports to existing objects. | T1 | 1 |
