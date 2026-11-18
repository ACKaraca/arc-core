# 0004. SQLite package layout, recovery journal and two save strategies

- Status: Accepted

## Context

The suite decision makes a native document one SQLite file with explicit-save semantics over a
crash-safe recovery journal and atomic saves. Open points: where unsaved work lives, how saves stay
fast for large models, how the package behaves on network shares, and how concurrent editors are
excluded.

## Decision

- **Layout:** one `arc_object` table for all types with a partition column for partial loading,
  derived index tables (`arc_property`, `arc_reference`), typed metadata, retained journal,
  content-addressed assets with FastCDC chunks, previews and preserved foreign data, as specified in
  [document-package.md](../architecture/document-package.md).
- **Identity:** `application_id` `0x41524331`; format version `major.minor` in `arc_meta`;
  `user_version` holds the major.
- **Journal mode at rest:** rollback journal (`DELETE`), never WAL, so the file is always a single
  file and works on network shares.
- **Unsaved work:** a per-session recovery database in the per-user application data directory,
  written by the storage thread with a one-second durability target.
- **Saving:** incremental in-place saves inside one SQLite transaction for local files opened from
  the same path; full rewrites to a temporary file with verification and atomic replace for Save As,
  compaction, format upgrades, first saves of recovered or imported documents, and locations with
  unreliable locking.
- **Locking:** a `~<name>.lock` sidecar with owner information, created exclusively and held with an
  advisory lock; read-only opens take no lock.

## Consequences

- Saving a small change to a large model writes only changed rows.
- The user's file never contains half-finished work, and recovery works for read-only folders.
- Recovery databases accumulate if users decline recovery; the application lists old ones for
  deletion.
- Network share detection is heuristic; when unsure, the full rewrite strategy is used.

## Alternatives considered

- **One table per object type.** Natural for SQL users, but schema changes per type become package
  schema changes, and generic tools must discover tables.
- **WAL mode for user files.** Better concurrency, but produces sidecar files and fails on network
  file systems.
- **Autosave into the user's file.** Removes the recovery database but violates explicit-save
  semantics.
- **Always rewriting the whole file.** Simplest atomicity, but saves of large models become slow.
- **Recovery data beside the document.** Visible to users and fails in read-only locations.

## Reopen when

Field reports show data loss or corruption attributable to either save strategy, or incremental
saves on reference large models exceed the save-time budget.

## Amendment

The `flags` column of `arc_object` is a bit set: 1 `hidden_from_ui`, 2 `system` and 4
`read_only`, the header flags of the
[document model](../architecture/document-model.md#objects-and-typed-stores). No other bit is
defined; a row with another bit set is loaded as a quarantined object and written back
unchanged. The bits are listed in
[document-package.md](../architecture/document-package.md#example-outline); no package has been
released before this amendment, so no fixture changes.

The first implementation of the package (format 1.0) fixes what the package document left open,
without changing anything it defined; the details are in
[document-package.md](../architecture/document-package.md#format-10-schema):

- the exact statements of every table and index (column types, `NOT NULL`, the index names
  `arc_object_type_key`, `arc_reference_target` and `arc_asset_chunk_chunk`) and the columns of
  `arc_meta`, `arc_type`, `arc_property`, `arc_reference`, `arc_source_map`, `arc_journal`,
  `arc_asset`, `arc_asset_chunk`, `arc_chunk`, `arc_preview`, `arc_foreign` and
  `arc_diagnostic`;
- the pragma values (`page_size` 4,096, UTF-8, `application_id`, `user_version` = major, no
  `auto_vacuum`) and the text form of `format_version` (`1.0`);
- the `arc_meta` keys and the forms of their values, including `thumbnail` = `none` for a save
  without thumbnail;
- the contents of `arc_reference.rule` and of `arc_property` for scalar values;
- the media type, sizes and size limit of previews;
- the temporary file name, the hash summary of the verification and the platform behavior of the
  replacement;
- the lock file content (`format`, `user`, `host`, `pid`, `application`, `time_unix_ms`), the
  shared operating-system lock and the meaning of "not running" and "local machine".

No package was released before this amendment; the first compatibility fixtures are written with
it.

The first implementation of the recovery journal fixes what the package document left open, in
[document-package.md](../architecture/document-package.md#recovery-database-format-1): the
application id `ARCR`, the format version, the tables `arc_meta` and `arc_record` (with a BLAKE3
hash per record), the pragmas (WAL, `synchronous` `FULL`), the lock file and its liveness rule,
the queue size and back-pressure, the meaning of "unchanged", and the replay rules. Because a
changed base must still yield the recovered state, the session keeps a copy of its base file
beside the database; the alternative, refusing recovery when the base changed, contradicts the
decision that the recovered state opens as a new document. No recovery database was written
before this amendment.
## Amendment: assets and the in-place save

The first implementation of assets and of the incremental save (format 1.0, nothing changed in
the schema) fixes what the package document left open; the details are in
[document-package.md](../architecture/document-package.md#assets) and the
[arc-doc specification](../crates/arc-doc.md):

- an asset of up to 4 MiB is stored as a single chunk keyed by the asset hash, without chunk
  list rows; larger assets list their chunks by ordinal; `encoding` 0 stores bytes as given;
- chunk boundaries are not part of the format; FastCDC with the documented sizes is what this
  implementation uses;
- the incremental save runs in the rollback-journal mode with `synchronous = FULL` and is
  refused, falling back to the full rewrite, when the file's save counter is not the one the
  changes start from;
- opening a package for editing rolls back a hot journal left by an interrupted save; a
  read-only open does not write and fails on such a file.

## Amendment: journal, foreign data and diagnostics rows

The first implementation that writes and reads `arc_journal`, `arc_foreign` and
`arc_diagnostic` fixes what the package document left open, without changing the schema of
format 1.0; the details are in
[document-package.md](../architecture/document-package.md#format-10-schema) and the
[arc-doc specification](../crates/arc-doc.md#journal-foreign-data-and-diagnostics-in-the-package-core-doc-040):

- `arc_journal.seq` carries the save counter of the save that first stored a record in its high
  32 bits, which lets the retention `saves:<n>` work without another table;
- the text forms of `history_retention` are `none`, `saves:<n>` and `full`; an absent or
  unknown value keeps every record, so no reader drops history it does not understand;
- the stored records form one parent chain; a save keeps the newest unbroken run;
- `arc_diagnostic.origin` also takes `executor` (faults of the document executor);
- foreign data rows are written back unchanged by every save.

Packages written before this amendment have empty tables and open unchanged; the existing
fixtures stay valid and no fixture is regenerated. Older readers ignore the rows; a full
rewrite by an older reader drops them, which loses history but no document content.

