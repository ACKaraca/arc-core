# 0002. UUIDv7 persistent identifiers and generational handles

- Status: Accepted

## Context

The suite document model requires stable 128-bit identifiers for references, IFC GlobalId mapping,
issues, change sets and persistent links, and dense in-memory handles for speed. The identifier
form, the handle form and the rules for copying and importing must be fixed before any document is
written, because they appear in every file and protocol message.

## Decision

- `ObjectId` is a UUID version 7: 48-bit millisecond timestamp, random bits, version and variant.
  Identifiers are generated locally without coordination, sort roughly by creation time (which
  keeps SQLite primary-key inserts local), and never repeat.
- `Handle<T>` is a 64-bit generational index (32-bit slot, 32-bit generation) into one typed store.
  Handles are never persisted, never sent over a protocol, and never used across documents.
- The identifier-to-handle index is a sharded persistent hash map (64 shards) shared structurally
  between revisions.
- Copies get new identifiers and record `copied_from` in the journal. Imports get new identifiers
  and record source identifiers in the source map; re-imports resolve through the source map.
- A seeded generator produces deterministic identifiers for tests and `--seed` runs; the random part
  is drawn from the seed and the timestamp part from a logical clock.
- Sub-object keys `(ObjectId, u32)` address parts of objects; local keys are owned by the type.

## Consequences

- Identifiers can be created offline on any machine, which multi-user editing requires.
- Documents leak creation time at millisecond resolution through identifiers; this is accepted and
  documented.
- Every lookup by identifier costs a hash probe; hot paths keep handles for the duration of a
  snapshot.

## Alternatives considered

- **UUID version 4.** Random order fragments SQLite indexes and gives no locality.
- **64-bit sequential identifiers per document.** Compact, but collide when documents are merged or
  objects are created concurrently by several users.
- **Using source identifiers (IFC GlobalId, DWG handles) directly.** Formats have incompatible
  identifier spaces and uniqueness guarantees; mapping keeps the document model independent.
- **Persisting handles.** Fast to load but ties files to in-memory layout and breaks on any store
  change.

## Reopen when

A measured workload shows identifier lookups above 10 % of regeneration time that handle caching
cannot remove, or a standard the suite must follow requires a different identifier form.
