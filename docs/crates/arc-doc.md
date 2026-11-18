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
- Core object types: `core.doc.link`, `core.doc.settings`, `core.doc.property_set_definition`.
  Preserved and quarantined objects are not objects of a core type: they are held beside the
  typed stores and keep the type key of their stored row (see
  [loading](#loading-migration-and-held-objects-core-doc-037-core-doc-038-core-doc-039)).

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
  the identifier index, the root of the reverse reference index, the held objects and the
  reasons the document is read-only.
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
- **Not in this layer.** Preserved and quarantined objects and the bulk load path are specified
  with [loading](#loading-migration-and-held-objects-core-doc-037-core-doc-038-core-doc-039).

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
  element fail with `doc.path.invalid`. `None` for a required field of a nested record fails
  with `doc.value.missing`, since nested records are checked in full.
- **Checks at call time.** An operation checks only what keeps the working state consistent:
  the object is live (`doc.object.not_found`), the type is registered (`doc.type.unknown`), a
  create has one value per field and its identifier is not live (`doc.object.duplicate_id`),
  the path resolves, and every given value has the addressed type, is finite and uses declared
  enum codes (fields of nested records are checked in full, and a list of references within its
  `many` bound, `doc.value.out_of_range`). Required fields, bounds of the object's own fields,
  reference integrity, whether an owner exists, the `read_only` flag and validators are checked
  at commit (CORE-DOC-015). A failed operation changes nothing.
- **Cost of a modify.** An object's values are one vector: a modify copies all of them with
  one value written, so its time and memory grow with the object's values however small the
  change, and a created object is shared by the operation and the store. Large data belongs in
  assets.
- **Reads.** `get`, `resolve` and `snapshot` (the candidate, for iteration) see the
  transaction's changes. A snapshot loaded from the `SnapshotCell` before or during the
  transaction sees none of them.
- **Replay and inversion.** `Op::apply` applies an operation to a working set when the state
  holds its before values: a created identifier is not live, and a modified, relinked or
  deleted object is live with exactly the recorded before state (for a delete also the type and
  schema version), values compared bit for bit (`Value::bits_eq`: `0.0` and `-0.0` differ, as
  their payloads do); otherwise it fails with `doc.op.mismatch` or the working set's code and
  changes nothing. Operations replayed from a journal are hostile input, so `apply` also checks
  what it writes like the transaction's operations do: a create needs a registered type and
  schema version and one value of the right type per field, and a written value must have the
  addressed type, be finite and use declared enum codes. `Op::inverse` turns a create into a delete of the same object and back, and
  swaps the before and after values of the others; the inverse of a create drops the source
  identifier. Applying a transaction's operations in order to its base reproduces its
  candidate, and applying their inverses in reverse order to the candidate reproduces the base.
- **Savepoints.** `savepoint` marks the working set, the number of operations and the memory
  estimate; `rollback_to` restores all three, truncating the operation list, and the
  transaction stays open; `release` drops the savepoint and every later one and keeps the
  changes. Savepoints nest in last-in, first-out order with the rules of working-set
  savepoints: a savepoint of another transaction, or one released or invalidated by a rollback
  to or release of an earlier one, fails with `doc.savepoint.invalid` and changes nothing.
- **Limits.** `TxnLimits` holds the most operations (`MAX_OPERATIONS`, 10 million, from the
  document model), the most savepoints held at a time (`MAX_SAVEPOINTS`, 1,024) and a memory
  budget, by default the `memory_bytes` of `Limits::DEFAULT`. The budget counts an estimate of
  what the transaction retains: the size of each operation and of the values, texts and path it
  holds, also for the deleted object a snapshot still shares; 64 bytes per entry of the touched
  map each savepoint copies; and each object version a live savepoint keeps alive after an
  operation replaced it (a version current at an older savepoint is current at every later one
  until it is replaced, so the newest savepoint decides). A released savepoint stays charged.
  The copied chunks and index nodes of the working set are not counted. An operation or
  savepoint that would exceed a limit fails with `doc.txn.limit_exceeded` (argument `limit`:
  `operations`, `savepoints` or `memory_bytes`), and the transaction stays usable and
  unchanged.
- **Deletion rules.** `apply_delete_rules(limits)` applies the relationship rules to the
  transaction's deletions (see the next section) and appends what they change as operations:
  one `Delete` per cascade deletion in order, then one `Modify` per cleared field, with the
  same count and memory charges as the transaction's own operations, so that the operation list
  still replays to the candidate and inverts to the base. Owning references are read from the
  state each object had when the transaction deleted it. Any failure restores the working set,
  the operations and the estimate; the transaction's savepoints stay valid. Commit runs it
  before the reference check.
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
  validity is the commit check's concern. A replacement within one major schema version
  updates the entries of the owner, when it changed, and of the fields whose values differ, so
  its cost follows the changed fields, not every reference of the object; a list that changes
  length renumbers its later elements' entries. Each entry takes about 130 bytes (a tree node
  and its field path).
- **Deletion rules.** `Txn::apply_delete_rules` inside a transaction, and
  `integrity::apply_delete_rules(set, limits)` on a working set without one, apply the rules to
  every object of the base the working set deleted, to every object of the base it recreated
  with another type key, and to every object deleted on the way. A live referrer whose field
  says `cascade`, and every object whose owner is deleted, is deleted. The targets of the
  `owning` references of a deleted object are deleted unless a live object holds an owning
  reference to them by now: this is the strength's commit-time rule. The owning references
  are read from the state the object had when it was deleted: from the transaction's `Delete`
  operations of its identifier, except one that a create of the same type key follows (a
  replacement keeps what the object owned), or, for a bare working set, which keeps no
  intermediate state, from the base. A live referrer whose field says `block` fails the call with
  `doc.reference.delete_blocked`, naming the field and, in details, the source and target,
  unless the cascades delete that referrer as well; a live referrer whose field says `clear`
  loses the reference (an optional field becomes absent, a list loses the element). Cascades
  run first with an explicit work list and visit each object once, so chains of any length and
  cycles of owning references terminate; more than `limits.record_count` cascade deletions fail
  with `doc.reference.cascade_limit`. Blocks and clears are decided after the cascades, so the
  result does not depend on the order in which referrers are visited. An object created and
  deleted in the same working set leaves no rule to apply, so a reference to it dangles.
- **Type changes.** An object recreated with another type key is a delete followed by a create
  for the block, owning and header-owner rules: the targets the old object owned are deleted
  unless a live object owns them, and the references to it that the base holds and the working
  set kept (entries of the base's reverse index still present in the candidate) refuse with
  `block` or, as header owners, take their objects along. Other references to it stay and must
  allow the new type at the reference check. A bare working set compares the type keys of the
  base and the candidate; a transaction counts a type change whenever one of the identifier's
  deletes contributes a state, so the result does not depend on how often the rules run. An object recreated with the same type key keeps
  its referrers, and the targets the old object owned stay, as when an owning reference is
  replaced.
- **Report.** The `DeleteReport` lists the cascade deletions in order (type key and object
  before) and the cleared objects (object before and after and the changed object fields; a
  list is named as a whole). `DeleteReport::operations` gives the operations that reproduce it:
  one `Delete` per deletion, then one `Modify` per changed field of each cleared object, which
  the transaction appends after its own operations. Any failure restores the working set and
  keeps savepoints taken before the call valid. The rules cost time proportional to the
  deleted objects, the references they hold, their referrers and the referrers of the objects
  they own.
- **Reference check.** `integrity::check_references(set)` runs after the deletion rules. Every
  reference held by a created, modified or recreated object, owner included, must name a live
  object whose type key the field allows (an empty list allows any type); no live object may
  still refer to a deleted one; every live referrer of a recreated identifier must allow its new
  type. A violation fails with `doc.reference.dangling`, naming the field and, in details, the
  source and target. A `one` field without a reference or a `many` list longer than its bound
  fails with `doc.reference.cardinality` (the field schema check, which runs first, rejects both
  as well). An object that would be the target of two owning references fails with
  `doc.reference.owner_conflict`, since an owned target has one owner; the details name the
  source, the target and the other owning reference (`owner`, `owner_field`), which may be
  another element of the same list. Only owning references the working set added are checked
  for conflicts, since the base passed the check: a reference counts as added unless the base
  state of the same identifier holds it at the same field path. The check costs time
  proportional to the touched objects, their references, the referrers of the targets of the
  owning references they added and the referrers of deleted and recreated identifiers.
- **Strengths.** `owning` has the rules above; `associative` and `informational` have no
  commit-time rule beyond their field's on-delete rule and the reference check (the dependency
  graph uses the former). Replacing an owning reference does not delete the target it no longer
  owns.
- **Copying.** The on-copy rule is not checked at commit: it is planned by
  `relationship::copy_references(snapshot, mapping)`, which computes the values of copies:
  `mapping` maps each object being copied to its copy's identifier. A reference to an object
  copied too names its copy, whatever the rule, so a copied group stays connected; any other
  reference follows its on-copy rule: `copy_target` keeps the original and lists it in
  `copy_targets`, which the caller adds to the mapping before planning again, `share_target`
  keeps it, `clear` removes it. The owner is the copy of the owner when that is copied too. The
  copy command itself (identifiers, creates, `copied_from`) belongs to an upper layer and is not
  part of phase 0 of this crate.

### Loading, migration and held objects (CORE-DOC-037, CORE-DOC-038, CORE-DOC-039)

`load(registry, types, rows, options)` builds the snapshot of a stored document. The package
reads its tables into neutral rows: a `StoredType` per `arc_type` row (type key, major, minor,
descriptor text) and a `StoredObject` per `arc_object` row with every column in its stored form
(identifier bytes, type key, schema major and minor, owner bytes, name, partition, flags,
encoding, payload, payload hash, last transaction); the payload is still encoded. The `flags`
bits are 1 `hidden_from_ui`, 2 `system` and 4 `read_only`. `StoredObject::of` makes the row of
an object at the version of a schema. Loading changes no input, writes nothing and appends no
journal record: migrated values exist in memory only, and the file changes when a save writes
them. A transaction of kind `migration` is reserved for an explicit command that rewrites stored
data; loading writes none.

- **Version policy.** For each type row of a registered type: the current version and older
  minor versions of the current major decode, appended fields taking their default or staying
  absent; older major versions migrate; a newer minor version of the current major decodes the
  known fields and keeps the trailing bytes, and makes the document read-only
  (`ReadOnlyReason::NewerMinor`, naming the type and the writing version); a newer major
  version fails the load with `doc.load.newer_major` (arguments `type`, `version`, `current`)
  before any object row is read, so nothing is created and the input is untouched. With
  `NewerMajor::ReadOnlyView`, the explicit choice the document model offers, the document opens
  read-only (`ReadOnlyReason::NewerMajor`) and the objects of that version are preserved. A type
  row whose version numbers are out of range is ignored. An object row must name a version its
  document declares; otherwise it is quarantined with `doc.load.version_undeclared`. A newer
  minor version of an older major version (newer than the source schema of its migration) is
  not migrated, since a migration would drop its trailing bytes: its objects are quarantined
  (`doc.migration.failed`, reason `trailing bytes`) and the document stays writable, because
  their rows are written back unchanged.
- **Read-only documents.** The reasons travel with the snapshot (`Snapshot::read_only`) and with
  every snapshot derived from it, so neither the executor nor the save path can lose them. Every
  write of a working set on a read-only snapshot fails with `doc.document.read_only`, whose
  arguments name the first reason's type and writing version.
- **Migration.** `migrate::decode_current` decodes a payload of an older major version with the
  source schema of that version, then applies the migrations one major version at a time; each
  result must pass the field schema check of the next schema. A payload with trailing bytes of a
  newer minor version of the old major, a failing migration function and a result that fails the
  check fail with `doc.migration.failed` (arguments `type`, `from`; details `step`, the major
  version of the failing step, and `reason`: `trailing bytes`, `no migration`, `migration
  failed`, `panicked` or `invalid result`); decoding failures keep their codes. The object is
  then quarantined. Each step looks its migration up in the declaration, so an object of major
  version 1 of a type at major version `N` costs O(`N`²) comparisons; `N` comes from the
  registry, not from the file.
- **Load validation.** Decoded values pass the field schema check in the decoder; the type
  validators run on every decoded object. An error finding quarantines the object with that
  finding; a warning becomes a diagnostic.
- **Panics of registered functions.** A migration or validator that panics is a defect of its
  type. The load contains the panic with `catch_unwind` and quarantines the one object: a
  migration with `doc.migration.failed` (reason `panicked`), a validator with
  `doc.load.validator_panicked` (argument `type`). Registered functions must not rely on this:
  on targets where panics abort instead of unwinding, the process ends.
- **Held objects.** A row of an unregistered type is **preserved** (reason `doc.type.unknown`),
  as is an object of a newer major version in the read-only view (reason `doc.load.newer_major`).
  A row with a malformed column (`doc.load.header_invalid`, argument `column`: `type_key`,
  `schema_major`, `schema_minor`, `owner`, `flags`, `name` or `partition` beyond
  `string_bytes`), an unsupported encoding, an undeclared version, a payload that does not
  decode, a failed migration or a validator error is **quarantined**. Both are `HeldObject`s:
  the row unchanged, the type key of the row (`None` when it is malformed), the reason, and an
  object with the identifier, the stored version (0.0 when malformed), the header with each
  valid column (owner, name, partition and flags are parsed independently; an invalid one takes
  its default and the first invalid one is the reason) and no field values. The snapshot holds them beside the typed stores, fixed
  at load: they resolve through the identifier index, `get` and `type_key` answer for them,
  `held` lists them in load order and `held_len` counts them, while `iter` and `len` cover the
  typed stores only. Every write of a working set to a held object fails with
  `doc.object.not_modifiable`; working sets, savepoints and publication share them unchanged. A
  reference to a held object is not dangling, and its type check uses the row's type key. Of a
  held object's own references only the header owner is kept for integrity and indexed; its
  field references stay in its undecoded payload, since building a schema from a stored
  descriptor is not provided yet. Preserved and quarantined objects survive every save, so
  nothing deletes them: deleting the owner of a held object, or the source of an owning
  reference to one, fails with `doc.object.not_modifiable`, because the cascade would delete
  it; the argument `id` names the held object and the detail `kind` says `preserved` or
  `quarantined`.
- **Rows without a usable identifier.** A row whose identifier is not 16 bytes or is nil
  (`doc.load.header_invalid`, column `id`), and a later row with the identifier of an earlier
  one (`doc.object.duplicate_id`; the first row wins), is kept in
  `LoadedRows::unaddressable` and written back unchanged; it is not part of the snapshot. A
  package cannot hold a duplicate, since the identifier is the primary key of `arc_object`; a
  save to another target writes unaddressable rows after the loaded ones, so the loaded row of
  an identifier still comes first.
- **Limits.** `record_count` bounds the type rows and the object rows. The load charges what
  the loaded document retains against `memory_bytes`: every type row 256 bytes, its descriptor
  and twice its key; every object row 256 bytes, its byte lengths and 32 bytes per payload byte
  (an upper estimate of the values it can decode to); a held or unaddressable row 2,048 bytes
  more and four times its type key; a loaded object 64 bytes and its `last_txn` value when that
  value is new; and an original row kept in a read-only document 768 bytes more and its
  trailing bytes. The charges come from the `load_scale` measurement below, so the estimate
  stays above the measured memory. Exceeding either limit fails the whole load with
  `doc.load.limit_exceeded` (argument `limit`) instead of quarantining rows one by one.
  Per-payload limits apply through the payload codec and quarantine the one object;
  `string_bytes` also bounds type keys, names and partitions, and a longer column quarantines
  the row. The loader does not enforce `wall_clock_seconds`; the caller does.
- **Reference integrity.** A dangling reference does not fail the load. After all rows, every
  reference of every loaded and held object that names no loaded object, or an object of a type
  its field does not allow, is reported; the objects stay as they are. Commit validation
  rejects such references only when a transaction touches their source.
- **Bulk load.** The snapshot is built without working sets: each object goes straight into its
  store, the identifier index and the reverse reference index, on nodes private to the build,
  so every invariant of those structures holds as after the same creates through a working
  set. The result has revision 0 and its own generation counters.
- **Result.** `Loaded` holds the snapshot; the diagnostics (`LoadDiagnostic`: kind `Preserved`,
  `Quarantined`, `Unaddressable`, `Warning` or `Dangling`, the object, and the error), in row
  order followed by the dangling references in handle order; the type rows as read
  (`Loaded::types`); the side table `LoadedRows`; and `LoadCounts`.
- **Side table.** `LoadedRows` keeps what a save cannot derive from the snapshot: the
  unaddressable rows; the stored `last_txn` of every object loaded into the typed stores
  (`LoadedRows::last_txn`), interned as a sorted entry of identifier and index per object (20
  bytes) and each distinct value once; and, only in a read-only document, the original row of
  each loaded object whose stored row is not the encoding of its loaded values (an older version
  or a newer minor version, with the trailing bytes beside it; `LoadedRows::get`). A writable
  document keeps no original rows: a save re-encodes those objects anyway. The held objects
  carry their own rows.
- **Write-back contract.** A save of a writable document writes:
  - the row of every held object (`HeldObject::row`), preserved or quarantined, of any type,
    unchanged, and then every unaddressable row unchanged;
  - every object of the typed stores encoded at the current version of its type, migrated and
    older-minor objects included, so the saved package holds each registered type at exactly
    one version; its `last_txn` is the transaction that last changed it since load, which the
    save knows from the journal, or `LoadedRows::last_txn` for an object no transaction
    changed; the package derives `payload_hash` from the payload bytes it writes;
  - the current type row of every registered type it writes objects of, with the descriptor of
    the registry, and the stored type row (from `Loaded::types`) of every row it writes
    unchanged.

  A copy of a read-only document writes every row as stored: the held and unaddressable rows,
  the original row (`LoadedRows::get`) of every object that has one, and every other object
  encoded at the current version with `LoadedRows::last_txn`, which reproduces its stored row;
  and every type row as read. The original payload of an object with trailing bytes equals the
  encoding of its values followed by those bytes. The `load_writeback` test checks the contract
  differentially: random documents saved by it come back row for row, and load again to the
  same document.

The `load_scale` test (ignored by default; run it in a release build) loads one million objects
of one integer field. On a shared four-core development machine it took about 1.6 s, decoding
and validation included; building the same snapshot from decoded objects took about 1.4 s through
the bulk path against 1.6 to 1.7 s through working-set creates. Insertion into the identifier index
takes about half of that time.
### Dependency graph (CORE-DOC-029, CORE-DOC-030, CORE-DOC-034)

The graph core lives in `arc_doc::graph`; the worker pool, priorities, cancellation of running
jobs (CORE-DOC-031, CORE-DOC-032), `ensure` (CORE-DOC-033) and the diagnostics list
(CORE-DOC-035) build on it.

- **Nodes and evaluators.** A `NodeKey` is a `Subject` (an `ObjectId`, or a graph-local `u64`
  chosen by the product) and a `DerivedKey` in the key grammar (`core.geometry.solid`), failing
  with `doc.graph.malformed_key` otherwise. `Evaluators` is an explicit value like the type
  registry: one evaluator per derived key and object type key, and one per derived key for
  graph-local nodes; a second one fails with `doc.graph.duplicate_key`. An evaluator is a pure
  function from an `EvalContext` to a `DerivedValue`, an `Arc` of a type implementing `Derived`:
  a 128-bit content hash and downcast access. `content_hash` hashes any `Hash` value with two
  fixed-key SipHash-1-3 passes; it is stable within one process run, which suffices because
  results are never stored, journaled or part of a snapshot. Distinct results are assumed not to
  collide in 128 bits; contents chosen to collide can at worst leave a dependent showing a result
  of the previous content.
- **Recorded reads.** The context reads the snapshot of the job's revision and records each
  read: `exists` and `type_key` depend on the object's existence, `header` on its header, `field`
  on the aspect of the object field (the aspect declared on the field of the object, also for
  values inside its nested records), `referrers` on the set of references to the target, and
  `node` on another node. A read of an object that is not live, or of a field its type lacks,
  depends on the object's existence, so the node reruns when the object appears. Dependencies are
  replaced by the reads of each applied evaluation.
- **Change sets.** `ChangeSet::from_commit(set, ops)` derives the changed inputs of a commit
  from the working set's touched map and the operation list, which must include the operations
  the deletion rules added: created, deleted and recreated objects change as a whole; a modified
  object changes the aspect of the object field each `modify` path starts with, and its header
  for each property or relink operation (a modified object without operations changes as a
  whole); the referrers of a target change when a touched object gains or loses a reference to
  it, owner included. The commit calls `Graph::apply(changes, snapshot)` with the published
  snapshot.
- **Dirty propagation and early cutoff.** `apply` marks the readers of the changed inputs dirty
  and, with an explicit work list, every dependent of a node that becomes dirty: every dependent
  of a dirty node is dirty, so a node is clean exactly when its result is valid for the current
  revision. The nodes of created, deleted and recreated objects are removed and their dependents
  become dirty; nodes are otherwise kept, so memory is bounded by the nodes demanded for live
  objects and graph-local identifiers. Recomputation is lazy. Each node keeps a stamp that
  changes only when its settled outcome changes (another result hash, another error, another
  list of blocking or cycle nodes) and records the stamp of every node it read. A dirty node
  whose recorded object inputs did not change first brings the nodes it read up to date; when
  their stamps are unchanged, it becomes clean without running its evaluator. Marking costs time
  proportional to the readers of the changed inputs and the nodes that become dirty; bringing a
  node up to date visits each node it depends on at most once per change.
- **Jobs and the stale-result rule.** `Graph::begin(node)` captures the current snapshot and
  marks the node running; `Job::evaluate` runs without any graph lock; `Graph::finish(job,
  outcome)` applies the outcome under the graph's mutex, returning `Applied` or `Stale`. An
  outcome is applied only when the node is still dirty, no object input the job read changed in a
  change set applied after the job began (the graph keeps the change sets applied since the
  oldest running job began), and every node it read is still clean with the stamp it read;
  otherwise it is discarded and the node stays dirty. A cancelled evaluation (`base.cancelled`,
  from `EvalContext::yield_point`) is discarded as well, and a job dropped unfinished leaves its
  node dirty. The graph is `Send + Sync`. A node demanded from inside an evaluation is evaluated
  inline on the same thread, on the demanding job's snapshot, through the same begin and finish;
  no thread ever waits for another, so demands cannot deadlock, and two threads may evaluate one
  node at once, the second result being discarded. `evaluate_now(node)` loops begin, evaluate
  and finish until the node is settled, at most 16 times (`doc.graph.stale`); `evaluate_all_dirty`
  does so for every node dirty when called.
- **States.** A node is `clean`, `dirty`, `running` (dirty with a job), `failed` (the evaluator
  returned an error, kept as an `arc_base::Error`), `blocked` (it read a failed, cyclic or
  blocked node, whatever its evaluator returned; it lists the failed and cyclic nodes upstream)
  or `cyclic`. Demanding a node that is being evaluated on the same thread closes a cycle: every
  node from the demanded one to the demanding one becomes cyclic with the members listed, and
  `node` returns `doc.graph.cycle`. Inline demands nest at most `DEFAULT_DEPTH_LIMIT` (256) deep
  per thread, configurable per graph; the demand beyond it fails with `doc.graph.depth_exceeded`,
  so a long chain evaluated from its far end fails where one evaluated from its near end does
  not. The last good result and the revision it was computed from stay visible in every state; a
  later successful evaluation clears a failure. `Graph::node` returns a node's view,
  `non_clean` the views of all nodes that are not clean in time proportional to them, and
  `drain_transitions` the state changes since the previous drain, one per node.
of one integer field. On a shared four-core development machine it took about 1.6 to 1.7 s,
decoding and validation included; building the same snapshot from decoded objects took about
1.4 s through the bulk path against 1.6 to 1.7 s through working-set creates. Insertion into the
identifier index takes about half of that time; a constructor that builds the index in bulk is
left for later. Its `memory_of_*` tests measure the resident memory per loaded object of one
integer field, side table included, and check that the `memory_bytes` charge stays above it:
about 350 bytes for an object at the current version or migrated in a writable document, about
1,050 bytes for a migrated object of a read-only document, and about 1,900 bytes for a held
row.

### Commit, executor and events (CORE-DOC-015, CORE-DOC-016, CORE-DOC-018, CORE-DOC-036)

- **Commit.** `commit(txn, hooks)` validates a finished transaction in the order of the
  [document model](../architecture/document-model.md#transactions): (1) the field schema check
  (`check_values` with bounds and required fields) of every created, modified or recreated
  object, the error gaining the details `object` and `type`; (2) `Txn::apply_delete_rules`, whose
  cascades and clears become operations, then the read-only flag of objects, then
  `check_references`; (3) the type validators of those objects, in identifier order; (4) the
  document validators in registration order. A validator finding of severity error fails the
  commit with the validator's own error (details `origin`: `type_validator` or
  `document_validator`, and `object` for a type validator); warnings become `Diagnostic`s. Every
  stage looks only at the touched identifiers and their referrers, so validation cost follows
  the change, not the document. Then the candidate is published with `SnapshotCell::publish`,
  and in this order the `JournalSink` records a `Committed` (metadata, final operation list,
  snapshots before and after), the graph applies `ChangeSet::from_commit`, the
  `DiagnosticSink` receives the warnings and the `committed` event is emitted. Any failure
  before publication leaves the published snapshot untouched and drops the transaction. A step
  after publication that unwinds does not undo the publication: it becomes a
  `doc.executor.fault` diagnostic and the remaining steps run.
- **Transactions without operations** publish nothing: no revision, record or event; the result
  names the unchanged snapshot. A transaction with operations on a read-only document fails with
  `doc.document.read_only`.
- **Read-only objects.** An object whose header in the base has the `read_only` flag keeps its
  values, owner and existence: modifying a field, relinking or deleting it, directly or through
  a cascade or a cleared reference, fails with `doc.object.read_only`. Its name, partition and
  flags may change, so a command clears the flag in a transaction of its own before editing it.
- **Document validators** are functions of the base snapshot, the candidate snapshot and the
  touched map, returning findings; they are added with `Document::add_validator` and run at every
  later commit.
- **Diagnostics.** A `Diagnostic` is a severity, an `arc_base::Error` (stable code, message
  identifier, arguments, details), the objects involved and an origin: `Validator`, `Node`,
  `Load` (with the load diagnostic kind) or `Executor` (faults). Load diagnostics convert with
  severity error for quarantined and unaddressable rows and warning otherwise. A
  `DiagnosticSink` receives each commit's warnings and faults; `CollectingSink` keeps a bounded
  number. The maintained list is CORE-DOC-035.
- **Executor.** Each `Document` runs one thread that executes submitted jobs strictly in
  submission order. A job is a closure that receives a new `Txn` on the latest snapshot (with
  the next transaction identifier of the document's generator and the previous published
  transaction as parent) and returns a value; the executor commits the transaction and sends
  the outcome to the `Pending` handle, which the caller waits on, polls or drops. Submission
  never waits for a job (the queue is unbounded; `std::sync::mpsc`). A job that returns an
  error, unwinds or fails its commit leaves the published snapshot identical (the same pointer,
  `Snapshot::ptr_eq`) to the previous one; an unwind is caught on the executor thread and
  reported as `doc.executor.fault` (category `internal`, detail `panic`) to the caller and as a
  diagnostic to the sink, and the thread runs the next job. Read-only queries use
  `Document::snapshot` on any thread and never go through the executor. Dropping the document or
  `Document::close` stops the thread after the running job; queued and later jobs fail with
  `doc.executor.stopped`. On `wasm32`, which has no threads, the executor runs each job inline in
  `submit` with the same order and rollback; a job submitted from a running job runs after it.
- **Events.** The `EventBus` delivers the six events of the document model: `Committed`
  (revision, transaction, kind, changed identifiers by type key and change kind, the number of
  commits covered and the published snapshot), `Regenerated` (revision, nodes that became clean
  and that failed), `DiagnosticsChanged` (added and removed), `Saved` (path, format version,
  revision), `LinkStateChanged` (new state per link) and `ConflictRecorded` (record identifiers).
  Commit emits `Committed`; the other units call `emit`. Each subscription has a bounded queue
  (64 events by default); when it is full, later events go to an overflow of one event per kind
  that merges events of the same kind: the latest revision, transaction, snapshot, save and link
  states, and the union of changed identifiers, nodes and records, which becomes "everything"
  beyond 65,536 identifiers; diagnostics lists beyond 1,024 entries are dropped with an
  `overflow` flag. Events arrive in emission order, the overflow after the queue. Emitting never
  waits for a subscriber; a dropped subscription is removed at the next emission. Handlers
  receive immutable snapshots and change the document only by submitting jobs.

The `commit_scale` test (ignored by default; run it in a release build) fills one store with one
million objects and measures submit-to-result of 10,000 single-field modifications through the
executor, including validation, publication, the graph change set and the event. On a shared
four-core development machine it measured a mean of about 76 µs and a 99th percentile of about
0.17 ms against the 2 ms budget of [testing.md](../testing.md); single rounds of up to 10 ms
occurred when the thread was descheduled.

### Journal, undo and redo (CORE-DOC-019 to CORE-DOC-023)

- **Records.** A `TxnRecord` (`arc_doc::record`) holds the fields of the
  [record shape](../architecture/document-model.md#record-shape) and the final operation list of
  a committed transaction, including the operations the deletion rules added, together with the
  type key of each operation's object: that of its create or delete, of an earlier create of the
  same identifier in the list, or of the object in the snapshot the transaction was applied on.
  `from_commit(meta, ops, before)` builds it; an operation on an object none of them knows fails
  with `doc.journal.record_invalid`. `encode(registry, limits)` writes the canonical, versioned
  layout of [journal-record.md](../architecture/journal-record.md), with values in the payload
  encoding at the schema version each operation names, which must be the registered schema of
  that major version. `decode(bytes, registry, limits)` treats the bytes as hostile and decodes
  the structure and then the values against the registry in one pass, since every value is
  length-prefixed; it re-encodes the result and refuses any other bytes. Limits fail with
  `doc.journal.record_limit_exceeded`, everything else with `doc.journal.record_invalid`, with
  the payload codec's error as the source. The decoder has a `cargo-fuzz` target,
  `crates/arc-doc/fuzz` (`record`).
- **Journal.** A `Journal` is the append-only list of the records of one session.
  `append(meta, ops, before, after)` is the commit hook: it builds the record, encodes it and
  appends it in constant time besides encoding, failing with `doc.journal.duplicate_txn` for an
  identifier it holds and `doc.journal.parent_mismatch` when the journal is not empty and the
  parent is not its last transaction; a failure appends nothing. Entries keep the decoded record,
  its encoding (for the package) and the revision it published; `get` finds an entry by
  transaction identifier in constant time, `iter` lists them in commit order and `ancestors`
  follows the parent chain while the parents are in the journal. Persisting and retaining
  records across saves belongs to the package.
- **Undo and redo.** An `UndoManager` holds the undo and redo stacks as steps, each the
  identifiers of the transactions it contains. It does not commit: `undo`, `redo` and
  `fail_group` return an ordinary `Txn` on the given current snapshot, which the caller commits
  like any transaction (deletion rules, validation, publication, journal) and reports with
  `committed(journal, txn, coalesce_key)`; the stacks change only then, so a refused commit
  changes nothing, and the manager does not depend on the commit path. Undo applies the inverses
  of the top step's operations in reverse order, kind `undo_of(last transaction of the step)`,
  and moves the step to the redo stack. Redo applies the step's original operations again
  (source identifiers of copies included), kind `redo_of(last transaction of the step)`; the
  redo transaction forms a new undo step, whose undo inverts it again, so the undo of an undo is
  a redo. A reported `normal` or `link_reload` transaction clears the redo stack; `merge` and
  `migration` transactions leave both stacks unchanged. The operations are checked strictly
  against the current state first (`Op::apply` on a scratch working set): a state that does not
  hold the step's after values fails with `doc.undo.state_mismatch`, an internal error in
  phase 0, where nothing but the session's commands writes the document; refusing such an undo
  as a conflict is CORE-DOC-024. Reporting a transaction that is not in the journal, or an undo
  or redo that is not of the step on top of its stack, fails with `doc.undo.sequence_invalid`.
- **Groups.** `begin_group(id)` opens a group, nested in the open ones; `group()` is the
  identifier of the outermost, which every transaction committed while it is open must carry in
  its metadata (`doc.undo.sequence_invalid` otherwise). The transactions of the outermost group
  form one step when `end_group` closes it; an empty group forms none. `fail_group` closes the
  innermost group because its owner failed: it returns one transaction that applies the inverses
  of the operations of every transaction committed since that group began, in reverse order, of
  kind `undo_of(last member)`, the kind of the model that restores an earlier state. The members
  leave the open step and the rollback joins no step, so a failed outermost group leaves the
  stacks as they were before it began, except for the redo stack its members cleared. Undo and
  redo inside an open group fail with `doc.undo.sequence_invalid`; a step back within the open
  group belongs to the command's prompt machine.
- **Coalescing.** The record shape holds no coalesce key or gesture, so they are the command
  layer's: `begin_gesture` and `end_gesture` bracket a gesture (a drag of a grip, a slider
  scrub), and `committed` takes the transaction's coalesce key. A transaction joins the top
  undo step when a gesture is open, it has a key, and the top step was formed in the same
  gesture with the same key; after an undo, no transaction joins an older step. The journal
  keeps every record; a step's inverse is the composition of its members' inverses in reverse
  order.
- **Limits.** `UndoLimits` bounds the undo stack by step count (`DEFAULT_UNDO_STEPS`, 1,000)
  and by the estimated memory of the steps' records (`DEFAULT_UNDO_MEMORY_BYTES`, 1 GiB). A
  record's estimate is the length of its encoding plus the size of one in-memory operation per
  operation; objects of creates and deletes are often shared with snapshots, so their encoded
  size stands in for them. After every change of the undo stack, and when the limits change,
  the oldest steps are dropped while the stack exceeds either limit, a single step larger than
  the budget included; their records stay in the journal. The redo stack holds only steps that
  were on the undo stack.
- **Comparison.** `undo::differences(a, b)` lists the identifiers of the objects that differ
  between two snapshots (live in one only, or with another type key, schema version, header or
  values, compared bit for bit), walking both and skipping objects shared by pointer; an empty
  list is the empty comparison the undo tests check. The comparison API with changed paths and
  both values is CORE-DOC-026 (phase 1).

The `undo_steps` test `undo_of_a_thousand_modifications_stays_within_budget` (ignored by
default; run it in a release build) undoes a step of 1,000 single-field modifications of 1,000
objects, including the deletion rules, the reference check, publication, the journal record and
the report to the manager; the worst of 20 rounds measured 3.3 ms on a shared four-core
development machine, against the budget of 50 ms in [testing.md](../testing.md).
### Package format, full rewrite, edit locks and previews (CORE-DOC-040, CORE-DOC-044, CORE-DOC-046, CORE-DOC-051)

The module `package` exists with the feature `package`, which is off by default and in the
WebAssembly build; `rusqlite` (bundled SQLite) and `blake3` are its optional dependencies. The
file format, including the exact schema of format 1.0, the `arc_meta` keys, the lock file and
the full-rewrite procedure, is specified in
[document-package.md](../architecture/document-package.md#format-10-schema). No `rusqlite` type
appears in a public signature. `Package` keeps no database connection open between calls, so a
save may replace the file it was opened from on every platform.

- **Open.** `Package::open(path, mode, limits)` reads the 100-byte header with plain file reads:
  not SQLite or another application id is `doc.package.not_a_package`; a `user_version` above
  the supported major is `doc.package.newer_format` (arguments `version`, `current`) before
  SQLite opens the file and before any lock, so the file, its modification time and its
  directory stay unchanged. SQLite then opens the file read-only with `SQLITE_LIMIT_LENGTH` set
  to `input_bytes`; `arc_meta` must exist (`doc.package.not_a_package`), `format_version` must
  parse as `major.minor` with the major of `user_version` and at least 1, and every documented
  table must have its columns (`doc.package.schema_invalid`, argument `table`). Format 1.0 is the
  first: there is no older format to migrate. A newer minor version opens read-only
  (`ReadOnlyCause::NewerFormat`) without a lock. `Mode::Edit(owner)` takes the edit lock;
  `Mode::ReadOnly` touches nothing. `Package::create` writes an empty package by the full
  rewrite (`doc.package.exists` when the path exists) and opens it for editing.
- **Metadata.** `Metadata` holds the documented `arc_meta` keys as fields and every other key
  unchanged in `other`; an interpreted key with a value of the wrong storage class or form is
  `doc.package.meta_invalid` (argument `key`). Document settings and tolerances are not
  `arc_meta` keys: they belong to the object type `core.doc.settings`.
- **Load.** `Package::load(registry, options)` streams the `arc_type` and `arc_object` rows in
  key order into `load`, so `record_count` and `memory_bytes` bound the rows as they are read.
  A value of an unexpected storage class or invalid UTF-8 fails the load with
  `doc.package.value_invalid` (arguments `table`, `column`); a value longer than `input_bytes`
  with `doc.load.limit_exceeded` (`input_bytes`). The `payload_hash` of each row of encoding 0
  is compared with the BLAKE3 hash of its payload; a mismatch adds a diagnostic of kind
  `Warning` with `doc.package.hash_mismatch` (argument `id`, the identifier bytes in hex) and
  counts as a warning. The row itself loads as its other columns allow: quarantining it needs a
  reason hook in `load`, which this unit does not add. Rows of other encodings are not hashed.
- **Full rewrite.** `save_as(target, save, progress)` builds the rows by the write-back
  contract above, with `Save::touched` mapping each object changed since load to the bytes of
  its last transaction (`doc.package.last_txn_missing` for an object with neither that nor a
  loaded `last_txn`; `Save::loaded` is `None` for a document never loaded), writes the next
  `save_counter` and `thumbnail` = `none` when no thumbnail is supplied, and returns the
  metadata written. `write(target, contents, progress)` writes given rows as they are, with the
  derived indexes of an optional snapshot. Both run the steps of `SaveStep` in order, calling
  `progress` before each; an error from it cancels the save. Rows are written in key order and
  the file holds no time stamps, so equal contents give byte-identical files with the same SQLite
  library version; a different SQLite version may lay out pages differently. Two rows with one
  identifier fail with `doc.object.duplicate_id` before anything is written.
- **Edit locks.** `LockOwner` (user, host, application, time) comes from the application: the
  crate asks the operating system only for its process id. A lock of another opener makes the
  package read-only with `ReadOnlyCause::Locked(holder)`, or `StaleLock(holder)` when the
  holder's host is the owner's and no process holds the file's operating-system lock.
  `Package::take_over(owner)`, the application's call after the user confirmed, checks that the
  lock is still stale and still names the same holder, replaces it and makes the package
  editable; otherwise `doc.package.locked`. Dropping the package releases the lock and removes
  the lock file. Two take-overs of the same stale lock confirmed at the same instant can race:
  one may remove the lock the other has just created.
- **Previews.** `Preview` is kind, media type, pixel size and PNG bytes; each is checked at save
  and at read as the format requires (`doc.package.preview_invalid`, argument `kind`, detail
  `reason`). Previews are read with `Package::previews`; a save without previews leaves
  `arc_preview` empty.
- **Not yet.** Incremental and background saves, assets and chunks, the journal table, the
  recovery journal, source maps, preserved foreign data, stored diagnostics and inspection come
  with later units; their tables exist and stay empty. A newer-minor package saved as a copy
  keeps the rows this version reads, not tables it does not know. Temporary files left by a
  killed save are not cleaned up.

The `package_kill` test kills a child process saving 400 objects at random points 50 times in
the default suite (1,000 times in the ignored `kill_during_save_1000`) and checks that the
target always opens and equals the old or the new state; `package_roundtrip` checks the
write-back contract through the file against an in-memory load.

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
| `doc.txn.limit_exceeded` | `limit_exceeded` | A transaction would exceed its operation count, savepoint count or memory budget |
| `doc.op.mismatch` | `conflict` | An operation is applied to a state that does not hold its before values |
| `doc.reference.delete_blocked` | `conflict` | A deletion is refused by an on-delete `block` rule of a reference to a deleted object |
| `doc.reference.cascade_limit` | `limit_exceeded` | On-delete cascades would delete more objects than `Limits::record_count` |
| `doc.reference.cardinality` | `invalid_input` | A `one` reference field has no reference, or a `many` list holds more references than its bound |
| `doc.reference.owner_conflict` | `conflict` | An object would be the target of more than one owning reference |
| `doc.load.newer_major` | `unsupported` | A document contains a type of a newer major schema version than the registry, and the read-only view was not chosen |
| `doc.load.limit_exceeded` | `limit_exceeded` | Loading would exceed the record count or the memory budget of its limits |
| `doc.load.version_undeclared` | `corrupt_data` | A stored object names a schema version its document does not declare |
| `doc.load.header_invalid` | `corrupt_data` | A column of a stored object other than the payload is malformed |
| `doc.migration.failed` | `failed_operation` | An object of an older major version cannot be migrated: trailing bytes, a failing or panicking migration or an invalid result |
| `doc.object.not_modifiable` | `conflict` | A preserved or quarantined object would be changed |
| `doc.document.read_only` | `conflict` | A read-only document would be changed |
| `doc.graph.malformed_key` | `invalid_input` | A derived key does not follow the key grammar |
| `doc.graph.duplicate_key` | `conflict` | An evaluator is already registered for a derived key and object type, or for a graph-local derived key |
| `doc.graph.unknown_key` | `not_found` | No evaluator computes a derived key for the type of its object |
| `doc.graph.cycle` | `failed_operation` | A node demands a node that is being evaluated on the same thread |
| `doc.graph.blocked` | `failed_operation` | A node reads a node that is failed, blocked or cyclic |
| `doc.graph.depth_exceeded` | `limit_exceeded` | Nested demands on one thread exceed the depth limit of the graph |
| `doc.graph.stale` | `conflict` | A result was computed from a revision whose inputs changed since, or keeps being superseded |
| `doc.load.validator_panicked` | `internal` | A type validator panicked while a loaded object was validated |
| `doc.object.read_only` | `conflict` | An object flagged read-only would have its values or owner changed, or be deleted |
| `doc.executor.fault` | `internal` | A job, or a step of its commit, unwound on the document executor |
| `doc.executor.stopped` | `cancelled` | The document executor stopped, or could not start, before the job ran |
| `doc.journal.record_invalid` | `corrupt_data` | A journal record is truncated, malformed, not canonical or of another format version, names an unregistered type or schema version, holds values its types reject, or an operation's object has no known type |
| `doc.journal.record_limit_exceeded` | `limit_exceeded` | A journal record exceeds a hostile-input limit |
| `doc.journal.duplicate_txn` | `conflict` | A transaction is appended to a journal that holds its identifier |
| `doc.journal.parent_mismatch` | `conflict` | A transaction is appended whose parent is not the journal's last transaction |
| `doc.undo.state_mismatch` | `internal` | An undo, redo or group rollback does not fit the current state of the document |
| `doc.undo.sequence_invalid` | `invalid_input` | The undo history is used out of order: a commit it did not prepare, a group that is not open, a group member without the group's identifier, or an undo or redo inside an open group |
| `doc.package.not_a_package` | `invalid_input` | A file is not SQLite, has another application id or has no `arc_meta` table |
| `doc.package.newer_format` | `unsupported` | A package has a newer format major version; the file is not touched |
| `doc.package.schema_invalid` | `corrupt_data` | A documented table or column is missing, the format version is malformed or disagrees with `user_version`, or the text encoding is not UTF-8 |
| `doc.package.value_invalid` | `corrupt_data` | A stored value has an unexpected storage class or text encoding |
| `doc.package.hash_mismatch` | `corrupt_data` | The `payload_hash` of a stored object is not the BLAKE3 hash of its payload |
| `doc.package.io_failed` | `io` | Reading or writing a package, its temporary file or its lock file failed |
| `doc.package.corrupt` | `corrupt_data` | SQLite reports the package file as damaged |
| `doc.package.verify_failed` | `io` | A newly written temporary package failed its verification; the target is unchanged |
| `doc.package.exists` | `conflict` | A package to be created already exists |
| `doc.package.locked` | `conflict` | A lock to be taken over is live, belongs to another host or was taken in the meantime |
| `doc.package.preview_invalid` | `invalid_input` | A preview has an unknown or repeated kind, another media type, a wrong pixel size or too many bytes |
| `doc.package.last_txn_missing` | `invalid_input` | A save knows no last transaction for an object |
| `doc.package.meta_invalid` | `corrupt_data` | An `arc_meta` entry the package interprets is malformed |
