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
| `TypeDecl`, `Schema`, `FieldDecl` | Declaration of a type: key, current schema, older major schemas with migrations, validators; ordered field declarations |
| `Value`, `ValueType` | Dynamically typed field value and its declared type |
| `Payload` | Canonical binary encoding of an object's field values |
| `Object`, `Header`, `Handle` | An object (identifier, schema version, header, field values) and its in-memory address (store, slot, generation) |
| `WorkingSet`, `Savepoint`, `SnapshotCell` | Copy-on-write changes on a base snapshot and the states to roll them back to; the atomically published latest snapshot |
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
| Third-party | `rusqlite` (bundled, feature `package`), `postcard`, `zstd`, `fastcdc`, `blake3`, `serde`, `serde_json` (descriptors), `rayon`, `crossbeam-channel`, `arc-swap` |

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

### Type registry (CORE-DOC-001)

A `TypeRegistry` is an explicit value, not process-global state. The application builds one at
start, registers the core types and its own, and then shares it immutably (in an `Arc`) with every
document it opens; tests and tools may hold several registries in one process. Iteration is in
type-key order, independent of registration order.

- **Type key.** Two or more segments separated by single dots, each a lowercase ASCII letter
  followed by lowercase letters, digits and underscores; at most 128 bytes. The first segment
  names the owner (`core`, or the repository name of a product). A `TypeKey` can only be built
  from a valid key; a malformed one fails with `doc.type.malformed_key`.
- **Names.** Field names, enum variant names and aspects follow the segment grammar, at most 64
  bytes. Field names are unique within their field list.
- **Versions.** A schema version is `major.minor` with major at least 1. Each field records the
  minor version that appended it (`since_minor`); fields appear in nondecreasing `since_minor`
  order, none later than the current minor, and follow the
  [appending rule](../architecture/document-model.md#schema-migration). Fields of nested records
  are never appended (`since_minor` 0).
- **Migrations.** A type of major version N declares exactly one migration from each major
  version 1 to N − 1. A migration carries the schema of its source major version (at that major's
  last minor version), which decodes payloads of that version, and a pure function from those
  values to the values of the next major version. A migration sees decoded values only, so an
  object whose payload has trailing bytes of a newer minor version is not migrated.
- **Validators.** A type validator is a function from the field values of one object to a list
  of findings, each an error or a warning with a code and message.
- **Value types.** A record has at least one field; lists, maps and records nest at most 16
  levels. Enum variants are non-empty with unique names and codes. A geometry field names its
  encoding in the type-key grammar and an encoding version.
- **Indexing.** Only fields of the object itself can be indexed, and only when their type is not
  `opaque`, `record`, `list`, `map` or `geometry`; anything else fails with
  `doc.type.field_invalid`.
- **Bounds.** An integer range applies to `i64`, a real range (finite ends, in SI base units) to
  `f64` and `quantity`, and a length bound to `string` (bytes), `list` and `map` (entries) and
  `opaque` (bytes). The minimum is not above the maximum.
- **Relationship rules.** Cardinality `one` belongs to a required reference field, `optional` to
  an optional reference field, and `many` (with a bound of at least 1) to a field whose type is a
  list of references; a reference anywhere else is inconsistent. A field of a nested record is a
  field for this purpose. On-delete `clear` and on-copy `clear` need cardinality `optional` or
  `many`. Strength `owning` excludes on-copy `share_target`, since a target has one owner. The
  target type keys need not be registered yet; an empty list allows any type.
- **Defaults.** A default has the field's type and satisfies its bound and enum codes.

Registration checks the whole declaration and leaves the registry unchanged on failure. A repeated
key fails with `doc.type.duplicate_key`; otherwise the first defect found fails with its own code
from the table below, with the `type` argument naming the type and the `field` argument the field
as a dotted path through nested records.

### Field values

The values of one object are one optional `Value` per field in declared order; an optional field
may be absent. Floating-point values are finite everywhere, and transforms are invertible: the
linear part, divided by its largest entry magnitude, has a nonzero determinant. The field schema
check (`schema::check_values`) verifies the count, required presence, types, bounds, enum codes,
finiteness, invertibility and the `many` bound of reference lists. A wrong number of values fails
with `doc.value.type_mismatch`, its `field` argument naming the first field without a value or
the position of the first value without a field.

A `Value` takes 32 bytes; the transform, color, external reference and asset variants are boxed.

A `FieldPath` addresses a value inside an object: a field of the object, then record field names,
list indices and map keys. `FieldPath::resolve` gives the value type it addresses in a schema.
Its canonical text form writes the field name, `.name` for a record field, `[n]` for a list index
and `["key"]` for a map key with the key as a JSON string, for example `layers[2].tags[0]` or
`costs["a"]`.

| Value type | Value | Payload form (`postcard`) | JSON form |
|---|---|---|---|
| `bool` | truth value | one byte, 0 or 1 | `true` or `false` |
| `i64` | integer | zigzag varint | number |
| `f64` | finite number | 8 bytes, IEEE 754 little-endian | number |
| `string` | UTF-8 text | varint byte length, bytes | string |
| `quantity` | `Quantity` of the declared dimension | the SI value as `f64` | number (SI) |
| `enum` | variant code | varint `u32` | number (code) |
| `point2`, `point3`, `vector3` | coordinates | 2 or 3 `f64` | array of numbers |
| `transform` | invertible affine transform | 12 `f64`: the top three rows of the 4×4 matrix, column by column | array of 12 numbers, same order |
| `color` | linear RGBA and optional source | 4 `f32` little-endian, then an option of the source (varint variant 0 `indexed` with one byte, 1 `named` with a string) | `{"rgba": [...], "source": null \| {"indexed": n} \| {"named": "..."}}` |
| `reference` | `ObjectId` | 16 bytes, UUID byte order | hyphenated UUID |
| `external_reference` | link id and target id | 32 bytes | `{"link": ..., "target": ...}` |
| `asset` | `AssetHash` (BLAKE3, 32 bytes) | 32 bytes | lowercase hexadecimal |
| `geometry` | `GeometryBytes`, the owning crate's canonical bytes | varint length, bytes | lowercase hexadecimal |
| `list<T>` | elements | varint count, elements | array |
| `map<string, T>` | entries | varint count, then key and value per entry in ascending byte order of keys | object |
| `record` | nested field values | the fields like an object's fields | array, `null` for absent fields |
| `opaque` | bytes | varint length, bytes | lowercase hexadecimal |

`AssetHash` and `GeometryBytes` hold bytes only in this part: computing asset hashes belongs to the
package, and decoding geometry bytes to the crate that owns the encoding named by the field.

### Field descriptors (CORE-DOC-004)

`descriptor(decl, version)` returns the JSON descriptor of any version a declaration describes:
each minor version of the current major version and of every migration source, and `None` for a
declaration that would not register. A minor version's descriptor lists the fields present at
that minor version. The contents a descriptor carries are listed in
[document-package.md](../architecture/document-package.md#payload-encoding); this section fixes
their JSON grammar. Building a `Schema` from a descriptor (as preserved objects and package
inspection need) is not provided yet. The output is pretty-printed with
two-space indentation and no trailing newline; keys appear in the fixed order below, numbers in
their shortest round-trip form, map defaults in key order. It depends only on the declaration, so
it is byte-identical across runs, platforms and registration orders.

```json
{
  "descriptor_format": 1,
  "type_key": "core.doc.example",
  "major": 1,
  "minor": 0,
  "fields": [
    {
      "name": "height",
      "type": { "kind": "quantity", "dimension": [1, 0, 0, 0, 0, 0, 0, 0, 0] },
      "optional": false,
      "indexed": true,
      "aspect": "geometry",
      "default": 2.7,
      "bound": { "kind": "real", "min": 0.0, "max": 100.0 }
    }
  ]
}
```

- `type.kind` is the value type name of the table above. A `quantity` adds `dimension` (the nine
  exponents of length, mass, time, electric current, temperature, amount of substance, luminous
  intensity, angle and solid angle); an `enum` adds `variants` (`name`, `code`); a `geometry`
  adds `encoding` and `encoding_version`; a `list` adds `element`, a `map` adds `value` (each a
  type object); a `record` adds `fields` (field objects); a `reference` adds `targets` (type keys)
  and `rule` (`strength`, `on_delete`, `on_copy`, `cardinality`, and `max` for `many`).
- `default` is the JSON form of the default value or `null`; `bound` is `null` or an object of
  kind `int` or `real` (`min`, `max`) or `length` (`max`).

### Payload codec

The schemas passed to the codec are those of registered declarations. The codec refuses the
declarations that would make it unsafe, empty records and types nested deeper than 16 levels,
with `doc.type.field_invalid`.

`payload::encode` checks the values and writes the fields in declared order as a `postcard`
tuple: an optional field as an option (one byte 0 for absent, or 1 followed by the value), a
required field as its value, each value in the payload form of the table above. Encoding is
canonical: bit-identical values give identical bytes (`0.0` and `-0.0` differ). It takes the
same `Limits` as decoding and fails with `doc.payload.limit_exceeded` for any value that decoding
would reject under them, so that everything written can be read back.

`payload::encode_value` and `payload::decode_value` encode and decode one value of a value type
on its own, in its payload form, with the same checks and limits; this is the form of field-level
before and after values (ADR-0003). The field's bound is checked by the caller.

`payload::decode` takes the schema of the payload's major version, the minor version that wrote
it, the stored encoding and the hostile-input `Limits`:

- A payload of an older minor version holds the fields present at that version; appended fields
  get their default or stay absent.
- A payload of a newer minor version holds the known fields followed by bytes this schema does not
  know; decoding returns them unchanged beside the values, without validating them, so canonical
  form and hash identity cover the known fields only. The caller checks the written minor version
  against the versions the package declares. Any other bytes after the known fields fail with
  `doc.payload.invalid`.
- Decoded values pass the field schema check, and re-encoding them reproduces the consumed bytes;
  overlong varints, unsorted or repeated map keys and other non-canonical forms fail with
  `doc.payload.invalid`.
- `input_bytes` bounds the payload, `string_bytes` every string, map key and color name,
  `array_length` the entries of every list and map and the bytes of every geometry and opaque
  value, and `nesting_depth` the nesting of lists, maps and records; exceeding one fails with
  `doc.payload.limit_exceeded` before memory is reserved for the excess. When `memory_bytes` is
  set, the decoded values of one payload are charged against it: 32 bytes per value or absent
  field, the size of the boxed data of transforms, colors, external references and assets, and
  the length of every string, map key, color name and byte string; at most 1,024 elements of a
  list are reserved ahead of decoding them. Without `memory_bytes` only the per-collection limits
  apply. Every value of a registered type encodes to at least one byte, so a payload never
  decodes to more elements than it has bytes.
- Encoding code 1 (`postcard` with `zstd`) is reserved: this version writes code 0 only and
  rejects code 1 with `doc.payload.encoding_unsupported`.

### Stores, identifier index and snapshots (CORE-DOC-002, CORE-DOC-003, CORE-DOC-017)

An **object** (`Object`) is its `ObjectId`, the schema version of its values, a `Header` (owner,
name, partition key and the flags `hidden_from_ui`, `system` and `read_only`) and one optional
`Value` per field of its schema in declared order. Its type key is the key of the store that holds
it. Objects are dynamically typed: a store is the store of one registered type key, not a Rust
type, so products and plug-ins add types at run time through the registry. Property sets join the
object with CORE-DOC-005.

- **Stores.** A snapshot has one store per registered type key, ordered by key. A store's slots
  live in chunks of 1,024 entries; an entry is a live object (generation and reference-counted
  immutable object) or a free slot. The chunks are the leaves of a radix trie of 32-way branch
  nodes, at most five levels high, so reading by handle is a bounded walk without locks.
- **Handles.** A `Handle` is the store index, a 32-bit slot and a 32-bit generation. A freed slot
  joins a persistent last-in, first-out free list and is reused. Every create draws its
  generation from a counter of its store that is shared by every snapshot and working set
  derived from the same `Snapshot::empty` and never goes back, also not on a rollback or when a
  working set is discarded. No two objects of a document's history, in any branch, therefore
  share a slot and generation: a handle whose object was deleted, rolled back or never
  published resolves to `doc.object.not_found`, never to another object, and the store index
  keeps a handle from reaching another type's object. The counter never wraps: a store accepts
  2³² − 1 creates in its lifetime, after which a create fails with
  `doc.store.capacity_exceeded`. Generation values depend on discarded work too, so handles are
  not deterministic; nothing that is persisted or compared depends on them.
- **Identifier index.** A persistent map from `ObjectId` to handle. The identifier's 128 bits
  pass through a fixed bijective mix (xor-shifts and multiplications by odd constants); the top 6
  bits of the result select one of 64 shards, and each shard is a hash array mapped trie that
  consumes the next bits 5 at a time, with a 32-bit occupancy bitmap and one entry (leaf or child)
  per set bit. The mix is a bijection, so distinct identifiers never collide and a lookup visits
  at most 26 nodes. Removing a key collapses a node left with a single leaf into its parent.
  Unknown and deleted identifiers resolve to `doc.object.not_found`. The mix spreads generated
  identifiers but is no defense against chosen ones: identifiers chosen to share long prefixes
  after the mix (from a hostile file, for example) make lookups and writes slower and the index
  larger by a constant factor, within the same depth bound. The mix is fixed so that the crate
  stays deterministic and makes no operating-system calls.
- **Snapshots.** A `Snapshot` is one `Arc` of its roots: the registry, the type keys, the
  revision number, the number of live objects, the table of store roots and the shard table of
  the identifier index, and the root of the reverse reference index.
  Cloning a snapshot copies one pointer, and `len` is constant time. `Snapshot::empty` has
  revision 0; the revision is a label that saturates at 2⁶⁴ − 1. Iteration yields live objects in
  handle order: by type key, then by ascending slot. Dropping the last holder of a snapshot
  frees what only it holds on the dropping thread; for the last snapshot of a document of one
  million objects that takes about half a second.
- **Working sets.** A `WorkingSet` starts from a base snapshot and applies `create`,
  `replace_values`, `replace_header` and `delete`. Writes are copy-on-write: the first write
  copies the store table and the shard table, then each write copies only the chunk (1,024
  entries, objects shared by reference) and branch nodes on its slot's path and the index nodes
  on its key's path; parts already private to the working set change in place. Copying a chunk
  increments the reference count of each of its objects, so the first write to a chunk costs
  tens of microseconds at any document size. `WorkingSet::current` is the candidate snapshot,
  with revision base + 1. A create fails with `doc.object.duplicate_id` exactly when the
  identifier is live in the candidate; an identifier deleted earlier in the same working set may
  be created again, in any store, which undoes a delete or changes an object's type. The
  working set does not check values against schemas: that is commit validation.
- **Touched identifiers.** `touched` maps each changed identifier to its net change relative to
  the base: `Created` (not live in the base), `Modified` (same object, new header or values),
  `Deleted` (live in the base, not in the candidate) or `Recreated` (live in the base, deleted
  and created again, possibly of another type). An object created and deleted in the same
  working set is not listed. The before state of every entry is read from the base and the after
  state from the candidate.
- **Savepoints.** `savepoint` returns an opaque `Savepoint` that marks the candidate and the
  touched map; it costs one pointer copy and a copy of the touched map. `rollback_to` restores
  that state and keeps the savepoint, invalidating savepoints taken after it;
  `release_savepoint` drops the savepoint and every later one and keeps the changes. A savepoint
  of another working set, or one released or invalidated, fails with `doc.savepoint.invalid`
  and changes nothing. Handles obtained after a savepoint are stale after a rollback to it.
- **Publication.** A `SnapshotCell` holds the latest published snapshot in an `arc-swap` cell.
  `load` returns it in constant time without locks. `publish` takes a working set and replaces
  the published snapshot with its candidate in one compare-and-swap that succeeds only when the
  published snapshot is the working set's base itself; a working set built on another snapshot,
  including an unpublished candidate with the expected revision number or a snapshot of another
  document, fails with `doc.snapshot.stale` and publishes nothing. Old snapshots are freed when
  their last holder drops them.
- **Identifiers.** Identifiers are unique within a document at any revision. An identifier may be
  live again after it was deleted, in the same working set or a later one, when undo, redo or a
  type change re-creates the same object; a new object always gets a new identifier from the
  identifier generator.
- **Not in this layer.** Preserved objects of unregistered types and a bulk path for loading
  documents with millions of objects belong to the loading and migration unit; this layer
  stores objects of registered types created one at a time.

The `store_scale` test (ignored by default; run it in a release build) fills one store with one
million objects. On a shared four-core development machine single-object operations measured
about 55 ns per snapshot, 40 µs per replacement, 9 µs per creation and 13 µs per deletion, each
including its working set and publication, with 99.9 % of 10,000 rounds under 0.3 ms, against
the budgets of 50 µs and 2 ms in [testing.md](../testing.md); the rare rounds above 2 ms
coincided with equally long stalls of the constant-time snapshot load, that is, with the thread
being descheduled. Memory overhead is about 77 bytes per object beyond the object itself.

### Transactions and savepoints (CORE-DOC-013, CORE-DOC-014)

A `Txn` is an open transaction: a working set on a base snapshot, the ordered list of operations
that made its candidate, its limits and the metadata of its journal record. Exactly one
transaction is open per document at a time; the document executor enforces that, not `Txn`.

- **Operations.** `create` (type key, a new `ObjectId` from the caller, header and values, at the
  type's current schema version; `create_copy` also records the source identifier), `modify`
  (sets the value at a `FieldPath`: a whole field, a record field, a list element or a map
  entry), `set_properties` (name, partition and flags), `delete` and `relink` (owner). Each
  appends one `Op` naming the object by `ObjectId` and, for creates and deletes, its type key:
  `Create` and `Delete` hold the whole object, `Modify` the schema version, the path and the
  before and after values, `ModifyProperties` and `Relink` the before and after header parts.
  Every successful call appends an operation, also when it writes the value already there and
  when it changes the same field again; coalescing belongs to the journal. Values are kept
  decoded; the journal encodes them with the payload codec.
- **Property operation.** The document model defines `modify_properties` on property sets
  (CORE-DOC-005, phase 1). Until property sets exist, the operation carries the header fields
  besides the owner: name, partition and flags. Property-set values join it with CORE-DOC-005.
- **Paths.** `None` makes a field or record field absent or removes a map entry, and setting a
  missing map key adds the entry. A list changes length only as a whole: a list index must be
  within the list and cannot be set to `None`. A path that does not resolve against the object's
  schema, an index past the end of a list, a part of an absent value and `None` for a list
  element fail with `doc.path.invalid`.
- **Checks at call time.** An operation checks only what keeps the working state consistent:
  the object is live (`doc.object.not_found`), the type is registered (`doc.type.unknown`), a
  create has one value per field and its identifier is not live (`doc.object.duplicate_id`),
  the path resolves, and every given value has the addressed type, is finite and uses declared
  enum codes (fields of nested records are checked in full). Required fields, bounds of the object's own fields, reference integrity, whether
  an owner exists, the `read_only` flag and validators are checked at commit (CORE-DOC-015). A
  failed operation changes nothing.
- **Reads.** `get`, `resolve` and `snapshot` (the candidate, for iteration) see the
  transaction's changes. A snapshot loaded from the `SnapshotCell` before or during the
  transaction sees none of them.
- **Replay and inversion.** `Op::apply` applies an operation to a working set when the state
  holds its before values: a created identifier is not live, and a modified, relinked or
  deleted object is live with exactly the recorded before state (for a delete also the type and
  schema version); otherwise it fails with `doc.op.mismatch` or the working set's code and
  changes nothing. `Op::inverse` turns a create into a delete of the same object and back, and
  swaps the before and after values of the others; the inverse of a create drops the source
  identifier. Applying a transaction's operations in order to its base reproduces its
  candidate, and applying their inverses in reverse order to the candidate reproduces the base.
- **Savepoints.** `savepoint` marks the working set and the number of operations;
  `rollback_to` restores both, truncating the operation list, and the transaction stays open;
  `release` drops the savepoint and every later one and keeps the changes. Savepoints nest in
  last-in, first-out order with the rules of working-set savepoints: a savepoint of another
  transaction, or one released or invalidated by a rollback to or release of an earlier one,
  fails with `doc.savepoint.invalid` and changes nothing.
- **Limits.** `TxnLimits` holds the most operations (10 million, from the document model) and
  a memory budget, by default the `memory_bytes` of `Limits::DEFAULT`. The budget counts an
  estimate of what the operation list retains: the size of each operation and of the values,
  texts and path it holds, also for the deleted object a snapshot still shares. The copied
  chunks of the working set are not counted. An operation that would exceed either fails with
  `doc.txn.limit_exceeded` (argument `limit`: `operations` or `memory_bytes`), and the
  transaction stays usable and unchanged.
- **Finishing.** `into_parts` hands the working set, the operations and the `TxnMeta` to
  commit, which validates and publishes; `abandon` drops everything. `TxnMeta` holds the
  transaction identifier, the parent transaction (none for the first of a new document), the
  session, the author, the UTC time in milliseconds since the Unix epoch and the kind
  (`normal`, `undo_of`, `redo_of`, `merge`, `migration`, `link_reload`). The command with its
  parameters, the label, the group and the selection after are opaque canonical JSON text
  written by the command layer, the history list, grouping and the application. A transaction
  is not published without validation, so `Txn` has no publishing method.
### References, relationship rules and the reverse index (CORE-DOC-008, CORE-DOC-009, CORE-DOC-010)

- **Where references are.** A reference is a `reference` field, an element of a `list` of
  references that is the type of a field, or such a field or list inside nested records reached
  through records, lists and maps; the registry allows them nowhere else. A reference is named by
  a `ReferenceField`: the `FieldPath` of the value (a list element's path ends in its index) or
  the header `Owner`. `relationship::references` lists what an object holds, owner first, then in
  field, list and map-key order, each with its target and `ReferenceSpec`; `spec_of` finds the
  spec of one `ReferenceField`. A list may hold one target more than once; each element is its
  own reference.
- **The header owner.** The document model gives the owner no rule of its own, so it is a
  reference with the fixed spec `OWNER_SPEC`: any target type, cardinality `optional`, on-delete
  `cascade` (an object is deleted with its owner), on-copy `share_target`. Its strength is
  `associative`, because the owned object holds the reference and depends on its owner. An owner
  must be a live object like any other target. Cycles of owners are not detected.
- **Reverse reference index.** Every snapshot holds a persistent ordered set of entries
  (target, source, field), spread over 64 shards by target, each shard an AVL tree of
  reference-counted nodes. `WorkingSet::create`, `replace_values`, `replace_header` and `delete`
  update it with the difference between the object's references before and after; a rollback to
  a savepoint restores it with the rest of the candidate. A write copies the shard table once
  per working set and the nodes on the path to its entry; every other node is shared with
  earlier snapshots. `Snapshot::referrers(target)` yields the `Referrer`s (source and field) of
  any identifier, live or not, in ascending order of source and field: `O(log n)` to reach the
  first of them for `n` references in the document, then amortized constant time each.
  `Snapshot::reference_count` is the number of entries. The index records what fields hold;
  validity is the commit check's concern.
- **Deletion rules.** `integrity::apply_delete_rules(set, limits)` applies the rules to every
  object of the base the working set deleted and to every object deleted on the way. A live
  referrer whose field says `cascade`, and every object whose owner is deleted, is deleted. The
  targets of the `owning` references of a deleted object are deleted unless a live object holds
  an owning reference to them by now: this is the strength's commit-time rule. A live referrer
  whose field says `block` fails the call with `doc.reference.delete_blocked`, naming the field
  and, in details, the source and target, unless the cascades delete that referrer as well; a
  live referrer whose field says `clear` loses the reference (an optional field becomes absent,
  a list loses the element). Cascades run first with an explicit work list and visit each object
  once, so chains of any length and cycles of owning references terminate; more than
  `limits.record_count` cascade deletions fail with `doc.reference.cascade_limit`. Blocks and
  clears are decided after the cascades, so the result does not depend on the order in which
  referrers are visited. The rules follow the net change: the owning references of a deleted
  object of the base are read from the base, and an object created and deleted in the same
  working set leaves no rule to apply, so a reference to it dangles. The returned
  `DeleteReport` lists the cascade deletions in order (type key and object before) and the
  cleared objects (object before and the changed object fields; a list is named as a whole);
  the journal records them as one `delete` per deletion, then one `modify` per cleared object,
  after the transaction's own operations. Any failure restores the working set.
- **Reference check.** `integrity::check_references(set)` runs after the deletion rules. Every
  reference held by a created, modified or recreated object, owner included, must name a live
  object whose type key the field allows (an empty list allows any type); no live object may
  still refer to a deleted one; every live referrer of a recreated identifier must allow its new
  type. A violation fails with `doc.reference.dangling`, naming the field and, in details, the
  source and target. A `one` field without a reference or a `many` list longer than its bound
  fails with `doc.reference.cardinality` (the field schema check, which runs first, rejects both
  as well). An object that would be the target of two owning references fails with
  `doc.reference.owner_conflict`, since an owned target has one owner. The check costs time
  proportional to the touched objects, their references, the referrers of the targets of their
  owning references and the referrers of deleted and recreated identifiers.
- **Strengths.** `owning` has the rules above; `associative` and `informational` have no
  commit-time rule beyond their field's on-delete rule and the reference check (the dependency
  graph uses the former). Replacing an owning reference does not delete the target it no longer
  owns.
- **Copying.** `relationship::copy_references(snapshot, mapping)` plans the values of copies:
  `mapping` maps each object being copied to its copy's identifier. A reference to an object
  copied too names its copy, whatever the rule, so a copied group stays connected; any other
  reference follows its on-copy rule: `copy_target` keeps the original and lists it in
  `copy_targets`, which the caller adds to the mapping before planning again, `share_target`
  keeps it, `clear` removes it. The owner is the copy of the owner when that is copied too. The
  copy command itself (identifiers, creates, `copied_from`) belongs to an upper layer and is not
  part of phase 0 of this crate.

### Error codes

The codes `arc-doc` owns. A code is never reused for a different meaning; messages are in
`locales/en-US/arc-doc.ftl`.

| Code | Category | Raised when |
|---|---|---|
| `doc.type.duplicate_key` | `conflict` | A type key is already registered |
| `doc.type.malformed_key` | `invalid_input` | A type key, reference target key or geometry encoding name does not follow the key grammar |
| `doc.type.version_invalid` | `invalid_input` | A schema major version is 0, or a field is appended after the schema's minor version |
| `doc.type.duplicate_field` | `invalid_input` | Two fields of one field list have the same name |
| `doc.type.field_invalid` | `invalid_input` | A field name or aspect is malformed, a bound does not fit its type or is empty, enum variants are empty or repeat a name or code, a record has no field, types nest too deeply, or an appended field breaks the appending rules |
| `doc.type.relationship_inconsistent` | `invalid_input` | A relationship rule contradicts the field that declares it |
| `doc.type.default_invalid` | `invalid_input` | A default value does not have the field's type or violates its bound or enum codes |
| `doc.type.migration_chain_invalid` | `invalid_input` | The migrations are not exactly one from each major version below the current one |
| `doc.value.missing` | `invalid_input` | A required field has no value |
| `doc.value.type_mismatch` | `invalid_input` | A value does not have its field's type, or the number of values differs from the number of fields |
| `doc.value.out_of_range` | `invalid_input` | A value violates its field's range or length bound, its reference list bound, or is not a declared enum code |
| `doc.value.not_finite` | `invalid_input` | A floating-point value is NaN or infinite |
| `doc.payload.invalid` | `corrupt_data` | A payload is malformed, not canonical, followed by unexpected bytes, or decodes to values its declaration rejects |
| `doc.payload.limit_exceeded` | `limit_exceeded` | A payload exceeds a hostile-input limit |
| `doc.payload.encoding_unsupported` | `unsupported` | A payload uses an encoding this version cannot decode |
| `doc.object.not_found` | `not_found` | An identifier or handle names no live object in the snapshot |
| `doc.reference.dangling` | `conflict` | A transaction would leave a reference to a missing object or to an object of a type the reference does not allow |
| `doc.type.unknown` | `not_found` | A type key is not registered in the document's type registry |
| `doc.object.duplicate_id` | `conflict` | An object is created with the identifier of an object live in the candidate |
| `doc.store.capacity_exceeded` | `limit_exceeded` | A typed store has no slot or generation left for another object |
| `doc.snapshot.stale` | `conflict` | A working set is published, but its base is no longer the published snapshot |
| `doc.savepoint.invalid` | `invalid_input` | A savepoint belongs to another working set, or was released or invalidated |
| `doc.path.invalid` | `invalid_input` | A field path does not resolve against an object's schema or does not reach a value of the object |
| `doc.txn.limit_exceeded` | `limit_exceeded` | A transaction would exceed its operation count or memory budget |
| `doc.op.mismatch` | `conflict` | An operation is applied to a state that does not hold its before values |
| `doc.reference.delete_blocked` | `conflict` | A deletion is refused by an on-delete `block` rule of a reference to a deleted object |
| `doc.reference.cascade_limit` | `limit_exceeded` | On-delete cascades would delete more objects than `Limits::record_count` |
| `doc.reference.cardinality` | `invalid_input` | A `one` reference field has no reference, or a `many` list holds more references than its bound |
| `doc.reference.owner_conflict` | `conflict` | An object would be the target of more than one owning reference |
