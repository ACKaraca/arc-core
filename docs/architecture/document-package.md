# Document package

The native file of every Arc product except DraftBuilder: one SQLite database with a documented
schema. This document is the format specification; anyone can read a package with standard SQLite
tools and the descriptors stored inside it.

DraftBuilder saves `.dwg` and `.dxf` directly and uses a package only for sidecar data (project
structure, toolset catalogs); the rules below apply to those sidecar packages unchanged.

## File identity

| Property | Value |
|---|---|
| Container | SQLite 3 database file, UTF-8 text encoding, 4,096-byte pages |
| `PRAGMA application_id` | `0x41524331` (ASCII `ARC1`) |
| `PRAGMA user_version` | Package format major version |
| Extension | Chosen by the product (`.archb`, `.civilb`, `.solidb`, `.meshb`, `.renderb`, `.arcrev`, `.draftb`); extensions are provisional until the first format freeze |
| Journal mode on disk | `DELETE` (rollback journal); WAL is not used for user files, so packages work on network shares and are always a single file at rest |

A file is an Arc package when the application id matches and the `arc_meta` table exists. The
format version is `major.minor`, stored in `arc_meta`. The first released format is `1.0`.

## Logical schema

All tables use the `arc_` prefix. Products add their own tables only through the `arc_ext_`
prefix and must register them in `arc_meta` (`ext_table:<name>` keys) so tools know they exist.

| Table | Purpose | Key columns |
|---|---|---|
| `arc_meta` | Key-value metadata: format version, document id, creating and last-saving application and version, units, coordinate reference, save counter, history retention, partition list | `key TEXT PRIMARY KEY`, `value` |
| `arc_type` | One row per object type and schema version present: type key, major, minor, field descriptor (JSON) | `(type_key, major, minor)` |
| `arc_object` | One row per object: id, type key, schema major and minor, owner, name, partition, flags, encoding, payload, payload hash, last modifying transaction | `id BLOB(16) PRIMARY KEY` |
| `arc_property` | Indexed fields and property-set values for queries: object, key path, typed value, unit | `(object, key)` |
| `arc_reference` | Every reference held by every object: source, field path, target, target link, rule | `(source, field, target)` |
| `arc_source_map` | Source identifiers of imported objects: object, source format, source document, source identifier | `(source_format, source_document, source_id)` |
| `arc_journal` | Retained transaction records, in commit order | `seq INTEGER PRIMARY KEY`, `txn_id` unique |
| `arc_asset` | Content-addressed assets: hash, media type, byte length, chunked flag, encoding | `hash BLOB(32) PRIMARY KEY` |
| `arc_asset_chunk` | Ordered chunk list of a chunked asset | `(asset, ordinal)` |
| `arc_chunk` | Chunk bytes, shared by all assets that contain them | `hash BLOB(32) PRIMARY KEY` |
| `arc_preview` | Thumbnail and preview images | `kind TEXT PRIMARY KEY` |
| `arc_foreign` | Preserved foreign data from imports | `id INTEGER PRIMARY KEY` |
| `arc_diagnostic` | Diagnostics present at save (failed nodes, quarantined objects, unresolved links) | `id INTEGER PRIMARY KEY` |

`arc_property`, `arc_reference` and the type and name columns of `arc_object` are **derived
indexes**: rebuilt from payloads on save and verifiable by `inspect --check`. They exist so that
standard SQL can answer "which objects of type X have property Y" or "what references object Z"
without decoding payloads, and so that partial loading can follow references.

### Example outline

```sql
CREATE TABLE arc_object (
  id          BLOB PRIMARY KEY,   -- 16-byte UUIDv7
  type_key    TEXT NOT NULL,
  schema_major INTEGER NOT NULL,
  schema_minor INTEGER NOT NULL,
  owner       BLOB,               -- 16-byte id or NULL
  name        TEXT,
  partition   TEXT,               -- product-defined load unit, e.g. a storey or a sheet
  flags       INTEGER NOT NULL,
  encoding    INTEGER NOT NULL,   -- 0 = postcard, 1 = postcard + zstd
  payload     BLOB NOT NULL,
  payload_hash BLOB NOT NULL,     -- BLAKE3 of the uncompressed payload
  last_txn    BLOB NOT NULL
) WITHOUT ROWID;
```

## Payload encoding

- Object payloads, journal operations and preserved foreign records are encoded with `postcard`
  (compact, deterministic binary serialization) from the type's field list in declared order.
- Payloads larger than 4 KiB are compressed with `zstd` (level 3) and flagged in `encoding`.
- `arc_type.descriptor` holds, for each type and version present, a JSON **field descriptor**:
  ordered field names, value types, unit dimensions, optionality and enum codes. The descriptor is
  sufficient for a generic decoder to turn any payload into JSON without Arc code. The descriptor
  grammar is published as a JSON Schema in `schemas/package-descriptor.schema.json`.
- Geometry values use encodings owned and documented by their crate (`arc-geom`, `arc-mesh`,
  `arc-brep`); the descriptor names the encoding and its version.
- Encoding is deterministic: the same object value always produces the same bytes, so payload
  hashes identify identical objects across files.

The rationale is recorded in [ADR-0005](../adr/0005-payload-encoding-with-embedded-descriptors.md).

## Schema versions per object type

Every row of `arc_object` names the schema version its payload was written with. A package may
contain the same type at several versions only transiently during a save that migrates; a saved
package holds each type at exactly one version, the current one of the saving application, except
for preserved objects of unknown or newer types, which keep their original version. Migration rules
are in [document-model.md](document-model.md#schema-migration).

## Assets

Assets are large binary values referenced from fields: meshes, point blocks, images, textures,
embedded fonts, original imported files, B-rep shape data.

- An asset is identified by the BLAKE3 hash of its uncompressed bytes. Identical assets are stored
  once, however many objects use them.
- Assets up to 4 MiB are stored whole in a single chunk. Larger assets are split by content-defined
  chunking (FastCDC, minimum 256 KiB, average 1 MiB, maximum 4 MiB) so an edit to a large asset
  rewrites only the affected chunks and similar assets share chunks.
- Chunks are compressed with `zstd` unless the media type is already compressed (PNG, JPEG, LAZ,
  KTX2 with supercompression), and verified against their hash on read; a mismatch is
  `corrupt_data` for that asset only.
- Assets are reference-counted through fields and journal records at save; unreferenced assets and
  chunks are removed by `compact`, never during an ordinary save.
- An **original file** kept on import is an asset with the media type of its format and an
  `arc_meta` entry recording its name and import time.

## Thumbnails and previews

| `kind` | Content | Required |
|---|---|---|
| `thumbnail` | PNG, 256 × 256, the view the user last saved from | Yes, when a renderer is available at save |
| `preview` | PNG, longest side 1,024 | Optional |
| `sheet:<id>` | PNG of a sheet at preview resolution | Optional |

Thumbnails are rendered headless by `arc-gpu` on the storage thread after the save transaction has
been prepared. A headless save without a GPU or software adapter writes no thumbnail and records
that in `arc_meta`; it never fails the save.

## Preserved foreign data

Import keeps what the product cannot model. `arc_foreign` rows hold: source format and version,
source identifier, the owning Arc object (or none, for document-level data), a format-specific
record kind, the encoded data (format-native bytes or a structured postcard record defined by the
`arc-io-*` crate) and an optional asset. Preserved data is shown in the loss report, written back
by the same format's exporter where that format allows it, and never interpreted by other code.

## Explicit save over a recovery journal

The user's file changes only when the user saves. Between saves, committed work is protected by a
**recovery journal**:

- The recovery journal is a separate SQLite database (WAL mode) in the per-user application data
  directory: `recovery/<document id>/<session id>.arcrec`. Keeping it out of the document's folder
  makes recovery work for read-only folders and network locations.
- It records the base file fingerprint (path, size, modification time, `arc_meta` save counter) and
  every committed transaction record in order, plus assets first referenced since the last save.
- The storage thread writes records as they are committed. The durability target is that no
  transaction committed more than one second before a crash is lost.
- A successful save truncates the recovery journal to the new base. Closing a document without
  saving deletes it after the user confirms discarding changes.
- At start-up, the application lists recovery journals whose owning process is gone. If the base
  file is unchanged, recovery replays the records onto it and opens the result as unsaved changes.
  If the base changed, the recovered state opens as a new unsaved document, and the user can
  compare it with the file.
- Recovery journals older than 30 days are listed for deletion, never deleted silently.

## Atomic save

Two strategies, both atomic: after a crash or power loss at any point, the file holds either the
complete previous state or the complete new state.

| Strategy | Used when | How |
|---|---|---|
| Incremental | Saving to the same local file the document was opened from | One SQLite write transaction (rollback journal, `synchronous=FULL`) that writes changed and new objects, deletes removed ones, appends journal records, updates indexes and metadata |
| Full rewrite | Save As, Compact, a format upgrade, the first save of a recovered or imported document, or a file on a location flagged as having unreliable locking (network shares, synced folders) | Write a complete new package to a temporary file in the same directory, verify it (`PRAGMA integrity_check` on the new file, object count and hash summary), flush, then atomically replace the target |

Saving runs on the storage thread from a snapshot, so editing continues during a save. A save
reports progress and can be cancelled before the final commit or rename; cancellation leaves the
file untouched.

## File locking

Opening a package for editing takes an **edit lock**:

- A lock file `~<file name>.lock` beside the package, holding JSON with user name, host name,
  process id, application, and time, created with exclusive-create semantics and held with an
  operating-system advisory lock while the document is open.
- Another application that finds a live lock opens the package read-only and shows who holds it.
- A lock whose host is the local machine and whose process is not running is stale and is taken
  over after the user confirms. A lock from another host is never broken automatically.
- Read-only opening takes no lock and never writes to the file or its directory.
- When the package is checked out through Arc Hub, the Hub lock governs editing and the local lock
  only protects against two local processes.

## Compatibility promise

1. A package written by any released version opens in every later version. Format and schema
   migrations run in memory; the file is upgraded only when the user saves, and a format-major
   upgrade always uses the full rewrite strategy and keeps a copy of the original as
   `<name>.v<major>.bak` unless the user declines.
2. An application refuses to open a package of a newer format **major** version and does not touch
   the file or create a lock.
3. An application opens a package of a newer format **minor** version, or one containing newer
   type minor versions, read-only.
4. Preserved objects and preserved foreign data survive any number of saves by any application
   version that can open the package.

### How compatibility is tested

- `fixtures/packages/` holds, for every released format version, packages written by that release
  for each core object type and by each product's own fixtures in its repository. Fixtures are
  never regenerated; a new release adds a new directory.
- CI opens every fixture with the current code and compares a canonical JSON dump of the decoded
  document with the expected dump stored next to it (after migration).
- A refusal test opens a package with an incremented major version and checks that the file's
  bytes and modification time are unchanged and no lock file was created.
- A forward test writes a package with an extra appended field at a newer type minor version and
  checks that the current code opens it read-only and that saving a copy to a new file keeps the
  unknown field bytes unchanged.
- A crash test interrupts saves at random points (killed child process) and checks that the file
  always passes integrity checking and decodes to either the old or the new state.

## Inspecting a package

The format is meant to be read without Arc:

```text
sqlite3 model.archb "SELECT key, value FROM arc_meta;"
sqlite3 model.archb "SELECT type_key, COUNT(*) FROM arc_object GROUP BY type_key;"
sqlite3 model.archb "SELECT source, field FROM arc_reference WHERE target = x'0190...';"
```

Every product CLI provides `inspect <file>`, which prints metadata and statistics and, with
`--json`, decodes objects through the embedded descriptors. `inspect --check` verifies
SQLite integrity, payload hashes, chunk hashes, reference integrity and the derived indexes, and
reports problems without modifying the file.
