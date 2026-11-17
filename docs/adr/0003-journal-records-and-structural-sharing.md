# 0003. Field-level journal records over chunked structural sharing

- Status: Accepted

## Context

The operation journal drives undo and redo, history, comparison, crash recovery and, in phase 5,
change sets for multi-user editing. Snapshots must be cheap so that readers never lock the
document. The suite decision fixes the principle (element-level operations, structural sharing, no
CRDT); the record granularity and the sharing structure are decided here.

## Decision

- A journal record is a transaction: identifier, parent, session, author, time, command and
  parameters, label, group, kind, optional selection, and an ordered list of operations.
- Operations are `create` (full value), `modify` (changed field paths with before and after values),
  `modify_properties` (changed property values with before and after), `delete` (full value before)
  and `relink` (owner before and after). Records are therefore self-inverting.
- Values in records use the package payload encoding; large values are assets referenced by hash.
- The journal is append-only. Undo and redo append inverse or repeated transactions with kinds
  `undo_of` and `redo_of`.
- Derived data is never journaled.
- Typed stores hold immutable object values in copy-on-write chunks of 1,024 slots; the identifier
  index and the reverse reference index are sharded persistent maps. A snapshot copies only roots.
- Memory for retained inverse data is bounded by a configurable budget; the undo stack drops its
  oldest steps when the budget is exceeded.

## Consequences

- Conflict detection for change sets works at field granularity, so two users editing different
  fields of the same object do not conflict.
- Records of modifications are small; creates and deletes carry full values, which large imports
  must group and budget.
- Append-only history makes recovery and synchronization simple: every state change is a forward
  record.
- Every object type must define field paths that are stable across schema minor versions.

## Alternatives considered

- **Whole-document snapshot undo.** Simple, but provides no operations for change sets or history
  queries, and memory grows with model size.
- **Object-level before and after values for modifications.** Simpler records, but coarse conflicts
  and large records for small edits to large objects.
- **Closure-based command objects with inverse functions.** Cannot be serialized, synchronized or
  replayed by other processes.
- **A general persistent collection library.** Viable, but the chunk layout tuned to typed stores
  and dense handles is small enough to own and keeps one structure for stores and snapshots.

## Reopen when

Measured undo memory or commit latency on the reference large-model fixtures exceeds the budgets in
the testing document, or multi-user editing shows field granularity is insufficient for a common
workflow.
