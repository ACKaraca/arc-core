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
