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
  flags       INTEGER NOT NULL,   -- 1 hidden_from_ui, 2 system, 4 read_only
  encoding    INTEGER NOT NULL,   -- 0 = postcard, 1 = postcard + zstd
  payload     BLOB NOT NULL,
  payload_hash BLOB NOT NULL,     -- BLAKE3 of the uncompressed payload
  last_txn    BLOB NOT NULL
) WITHOUT ROWID;
```

### Format 1.0 schema

A package of format 1.0 is created by exactly these statements, in this order; `sqlite_master`
holds them verbatim (a test compares it with this block). SQLite adds one automatic index for
`arc_journal.txn_id`. A reader checks that every table exists with at least these columns;
tables and appended columns of a newer minor version and `arc_ext_` tables are allowed.

```sql
CREATE TABLE arc_meta (
  key TEXT PRIMARY KEY NOT NULL,
  value
) WITHOUT ROWID;
CREATE TABLE arc_type (
  type_key TEXT NOT NULL,
  major INTEGER NOT NULL,
  minor INTEGER NOT NULL,
  descriptor TEXT NOT NULL,
  PRIMARY KEY (type_key, major, minor)
) WITHOUT ROWID;
CREATE TABLE arc_object (
  id BLOB PRIMARY KEY NOT NULL,
  type_key TEXT NOT NULL,
  schema_major INTEGER NOT NULL,
  schema_minor INTEGER NOT NULL,
  owner BLOB,
  name TEXT,
  partition TEXT,
  flags INTEGER NOT NULL,
  encoding INTEGER NOT NULL,
  payload BLOB NOT NULL,
  payload_hash BLOB NOT NULL,
  last_txn BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE arc_property (
  object BLOB NOT NULL,
  key TEXT NOT NULL,
  value,
  unit TEXT,
  PRIMARY KEY (object, key)
) WITHOUT ROWID;
CREATE TABLE arc_reference (
  source BLOB NOT NULL,
  field TEXT NOT NULL,
  target BLOB NOT NULL,
  target_link BLOB,
  rule TEXT NOT NULL,
  PRIMARY KEY (source, field, target)
) WITHOUT ROWID;
CREATE TABLE arc_source_map (
  object BLOB NOT NULL,
  source_format TEXT NOT NULL,
  source_document TEXT NOT NULL,
  source_id TEXT NOT NULL,
  PRIMARY KEY (source_format, source_document, source_id)
) WITHOUT ROWID;
CREATE TABLE arc_journal (
  seq INTEGER PRIMARY KEY,
  txn_id BLOB NOT NULL UNIQUE,
  record BLOB NOT NULL
);
CREATE TABLE arc_asset (
  hash BLOB PRIMARY KEY NOT NULL,
  media_type TEXT NOT NULL,
  byte_length INTEGER NOT NULL,
  chunked INTEGER NOT NULL,
  encoding INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE arc_asset_chunk (
  asset BLOB NOT NULL,
  ordinal INTEGER NOT NULL,
  chunk BLOB NOT NULL,
  PRIMARY KEY (asset, ordinal)
) WITHOUT ROWID;
CREATE TABLE arc_chunk (
  hash BLOB PRIMARY KEY NOT NULL,
  encoding INTEGER NOT NULL,
  bytes BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE arc_preview (
  kind TEXT PRIMARY KEY NOT NULL,
  media_type TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  bytes BLOB NOT NULL
) WITHOUT ROWID;
CREATE TABLE arc_foreign (
  id INTEGER PRIMARY KEY,
  source_format TEXT NOT NULL,
  source_version TEXT,
  source_id TEXT,
  object BLOB,
  record_kind TEXT NOT NULL,
  data BLOB NOT NULL,
  asset BLOB
);
CREATE TABLE arc_diagnostic (
  id INTEGER PRIMARY KEY,
  severity TEXT NOT NULL,
  code TEXT NOT NULL,
  object BLOB,
  origin TEXT NOT NULL,
  arguments TEXT NOT NULL
);
CREATE INDEX arc_object_type_key ON arc_object (type_key);
CREATE INDEX arc_reference_target ON arc_reference (target);
CREATE INDEX arc_asset_chunk_chunk ON arc_asset_chunk (chunk);
```

- **Header and pragmas.** `page_size` 4,096, `encoding` UTF-8, `application_id` 1,095,910,193
  (`0x41524331`), `user_version` the format major version, no `auto_vacuum`. The journal mode at
  rest is `DELETE`, SQLite's default, which is not stored in the file; a reader opens the file
  read-only, with `trusted_schema` off and `cell_size_check` on.
- **`arc_meta`.** Keys and values: `format_version` (text `1.0`), `document_id` (text, the
  hyphenated lowercase UUID), `created_by` and `saved_by` (text, application and version),
  `units`, `coordinate_reference` and `history_retention` (text), `save_counter` (integer, the
  number of saves; each save writes the next one), `partitions` (text, a JSON array of strings;
  absent when empty) and `thumbnail` (text `none` when the last save wrote no thumbnail; absent
  otherwise). Absent keys mean "not set". Other keys are kept unchanged by every save.
- **Derived indexes.** `arc_reference` holds one row per reference of every object: `field` is
  the field path, `(owner)` for the header owner; `rule` is a JSON object with `strength`,
  `on_delete` and `on_copy`; `target_link` is NULL for references within the document. Of a
  preserved or quarantined object only the owner is indexed. `arc_property` holds one row per
  indexed field with a scalar value (truth value and enum code as integer, integer, real,
  text; a quantity as real in SI base units with `unit` the JSON array of its nine dimension
  exponents); `key` is the field name.
- **`arc_preview`.** `media_type` is `image/png`; `width` and `height` are the image's size in
  pixels and equal its `IHDR` header; `thumbnail` is 256 × 256, `preview` has a longest side of
  1,024 and `sheet:<id>` (an identifier of 1 to 256 bytes without control characters) a longest
  side of at most 1,024; each image is at most 16 MiB.
- **`arc_diagnostic`.** `severity` (`error`, `warning`), `code` (a stable error code), `object`
  (16 identifier bytes or NULL), `origin` (`validator`, `node`, `load`) and `arguments` (a JSON
  object).

## Payload encoding

- Object payloads, journal operations and preserved foreign records are encoded with `postcard`
  (compact, deterministic binary serialization) from the type's field list in declared order.
- Payloads larger than 4 KiB are compressed with `zstd` (level 3) and flagged in `encoding`.
- `arc_type.descriptor` holds, for each type and version present, a JSON **field descriptor**
  listing the fields in order, each with name, value type, unit dimension (for quantities), the
  variant names and codes (for enums), the encoding name and version (for geometry), the
  relationship rule (for references), default, valid range or length bound, optionality, indexing
  and aspect. The descriptor is sufficient for a generic decoder to turn any payload into JSON
  without Arc code. The descriptor grammar is specified in
  [arc-doc](../crates/arc-doc.md#field-descriptors-core-doc-004) and published as the JSON
  Schema `schemas/package-descriptor.schema.json` (`descriptor_format` 1).
- Geometry values use encodings owned and documented by their crate (`arc-geom`, `arc-mesh`,
  `arc-brep`); the descriptor names the encoding and its version.
- Encoding is deterministic: bit-identical object values always produce the same bytes, so
  payload hashes identify identical objects across files.

The rationale is recorded in [ADR-0005](../adr/0005-payload-encoding-with-embedded-descriptors.md).

## Schema versions per object type

Every row of `arc_object` names the schema version its payload was written with. A package may
contain the same type at several versions only transiently during a save that migrates; a saved
package holds each type at exactly one version, the current one of the saving application, except
for preserved objects of unknown or newer types and quarantined objects, whose rows are written
back unchanged with their original version. Migration rules are in
[document-model.md](document-model.md#schema-migration).

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

In format 1.0 an asset of up to 4 MiB is one `arc_chunk` row whose `hash` is the asset hash;
its `arc_asset.chunked` is 0 and it has no `arc_asset_chunk` rows. A larger asset has `chunked`
1 and one `arc_asset_chunk` row per chunk, `ordinal` counting from 0, `chunk` the chunk hash; a
chunk holds at most 4 MiB. `encoding` 0 in `arc_asset` and `arc_chunk` means the bytes as given
(no compression). The chunk boundaries are not part of the format: a reader concatenates the
listed chunks and never recomputes boundaries, so packages chunked by any algorithm read back
the same. The references counted at compaction are the asset values of the decoded objects and,
for rows a reader does not decode (preserved, quarantined and unaddressable rows), any stored
asset hash whose 32 bytes occur in the payload.

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

### Recovery database format 1

- **File and identity.** `<session id>` is the hyphenated lowercase session identifier and
  `<document id>` the document's `arc_meta` `document_id` (a document never saved generates one).
  `PRAGMA application_id` is 1,095,914,322 (`0x41524352`, ASCII `ARCR`), `user_version` is 1,
  the journal mode is WAL and `synchronous` is `FULL`, so every group commit is on disk when it
  returns. The tables:

  ```sql
  CREATE TABLE arc_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value
  ) WITHOUT ROWID;
  CREATE TABLE arc_record (
    seq INTEGER PRIMARY KEY,
    txn_id BLOB NOT NULL UNIQUE,
    time_unix_ms INTEGER NOT NULL,
    record BLOB NOT NULL,
    hash BLOB NOT NULL
  );
  ```

  `arc_meta` keys: `format_version` (text `1`), `document_id` and `session_id` (text),
  `created_unix_ms` (integer), and for a document with a file `base_path` (text, the path the
  application opened), `base_size`, `base_modified_unix_ns` and `base_save_counter` (integers)
  and `base_copy` (text, a file name in the same directory). `arc_record` holds one row per
  committed transaction in commit order: its identifier (16 bytes), the commit time, the
  [journal record](journal-record.md) and the 32-byte BLAKE3 hash of the record bytes, which
  detects damage SQLite does not.
- **Liveness.** Beside the database, `<session id>.lock` is held with an exclusive
  operating-system lock while the session runs. A recovery journal is orphaned when another
  process can take that lock or the lock file does not exist, the rule of the edit lock.
- **Base copy.** When the session has a base file, a copy of it is kept beside the database
  (`<session id>-<n>.arcbase`, written to a temporary file, flushed and renamed), so that the
  recovered state can be built when the file has changed. A save replaces the copy with one of
  the saved file. Without a copy, a changed base cannot be recovered and the journal is kept.
- **Writing.** The commit hands each record to the storage thread through a queue of 1,024
  records; the thread writes them in group commits, at the latest one second after the first
  record of a batch and whenever the queue runs empty. A full queue makes the next commit wait
  (back-pressure) instead of dropping records. A failure of the database (disk full, no
  permission) never fails a commit: recovery protection turns off and a diagnostic says so; the
  next successful save turns it on again.
- **Save and close.** A save deletes the records up to the saved state's last transaction and
  records the new base fingerprint and copy. A clean close deletes the database, its copy and
  its lock when it holds no record; otherwise they stay until the user confirms discarding.
- **Unchanged.** The base is unchanged when the file at `base_path` has the recorded size and
  modification time and opens as a package with the recorded save counter.
- **Replay.** Records are replayed in `seq` order through the ordinary commit path (operations
  applied strictly, validation on), keeping their identifiers and metadata. A record whose hash
  does not match, that does not decode, that does not name the previous record as its parent or
  that fails to apply or validate ends the replay: the longest valid prefix is recovered and the
  rest is reported. When the base changed, the records are replayed onto the copy and the
  result opens as a new unsaved document. The recovered document's journal holds the replayed
  records; its undo history starts empty, because coalesce keys and gestures are not part of
  records. Recovery starts a new session with the same base and the replayed records and then
  deletes the orphan, so a crash after recovery recovers again.

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

The full rewrite in detail: the temporary file is `~<file name>.<process id>-<n>.tmp` beside the
target, created exclusively. The summary is the BLAKE3 hash over every `arc_object` row in
identifier order (length and bytes of the identifier, length and bytes of `payload_hash`, the
BLAKE3 hash of `payload`), compared with the rows the save meant to write, together with the
object and type row counts and the header. The temporary file is flushed (`fsync` or its
platform equivalent) and renamed over the target; on Unix the directory is flushed afterwards. A
failure or cancellation before the rename removes the temporary file and leaves the target
unchanged; a process killed during a save can leave a temporary file behind, never a changed
target. On Windows the rename replaces an existing target as the platform's file replacement
does: atomic on a local NTFS volume, and refused while another process keeps the target open
without delete sharing. `payload_hash` is the 32-byte BLAKE3 hash of the payload as written for
encoding 0.

## File locking

Opening a package for editing takes an **edit lock**:

- A lock file `~<file name>.lock` beside the package, holding JSON with user name, host name,
  process id, application, and time, created with exclusive-create semantics and held with an
  operating-system advisory lock while the document is open.
- Another application that finds a live lock opens the package read-only and shows who holds it.
- A lock whose host is the local machine and whose process is not running is stale and is taken
  over after the user confirms. A lock from another host is never broken automatically.
- Read-only opening takes no lock and never writes to the file or its directory.
- The lock file of format 1 holds one JSON object: `format` (1), `user`, `host`, `pid`,
  `application` and `time_unix_ms` (milliseconds since the Unix epoch). The holder writes and
  flushes it, then holds a shared operating-system lock on it, so other processes can still read
  it. A process is "not running" when another process can take an exclusive lock on the file;
  the host is "the local machine" when `host` equals the opening application's host name. A lock
  file that cannot be read or parsed counts as live. Closing the package releases the lock and
  removes the file.
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
reports problems without modifying the file. Both build on `arc-doc`'s `inspect` and `check`,
specified in [arc-doc](../crates/arc-doc.md#inspection-check-and-saving-a-document-core-doc-050):
they need no type registry, so a derived index row set matches when it is either everything
recomputed from the payload or, as a save writes for a row whose type it did not know, the
owner row alone. A file with a hot rollback journal is refused until an open for editing rolls
the journal back.
