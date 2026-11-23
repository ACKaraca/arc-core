//! The document store of the Arc suite.
//!
//! The parts that exist:
//!
//! - the value model of object fields, field paths and the declarations of object types
//!   ([`schema`]), the type registry (CORE-DOC-001, [`registry`]), JSON field descriptors
//!   (CORE-DOC-004, [`descriptor`]) and the payload codec ([`payload`]);
//! - typed copy-on-write stores, the identifier index, snapshots, working sets with savepoints
//!   and snapshot publication (CORE-DOC-002, CORE-DOC-003, CORE-DOC-017, [`snapshot`]);
//! - transactions with field-level operations and savepoints (CORE-DOC-013, CORE-DOC-014,
//!   [`txn`], [`op`]);
//! - references, relationship rules, the deletion rules and the reference check at commit and
//!   the reverse reference index (CORE-DOC-008, CORE-DOC-009, CORE-DOC-010, [`relationship`],
//!   [`integrity`], [`reverse_index`]).
//!
//! The journal, commit, the executor, migration, the dependency graph and the package build on
//! these. No function in this crate panics.

pub mod commit;
pub mod compare;
pub mod descriptor;
pub mod diagnostic;
pub mod diagnostics;
pub mod document;
pub mod ensure;
pub mod error;
pub mod events;
pub mod executor;
pub mod graph;
pub mod history;
mod index;
pub mod integrity;
pub mod journal;
pub mod link;
pub mod load;
pub mod migrate;
pub mod op;
#[cfg(feature = "package")]
pub mod package;
pub mod payload;
pub mod preserved;
pub mod propset;
pub mod record;
pub mod registry;
pub mod relationship;
pub mod reverse_index;
pub mod scheduler;
pub mod schema;
pub mod snapshot;
pub mod source_map;
mod store;
pub mod subkey;
pub mod txn;
pub mod undo;
pub mod validate;
pub mod version;

#[cfg(test)]
mod testing;

pub use commit::{CommitHooks, CommitOutcome, Committed, JournalSink, commit};
pub use compare::{ChangeKind, Comparison, FieldChange, ObjectChange, compare, compare_ops};
pub use descriptor::descriptor;
pub use diagnostic::{CollectingSink, Diagnostic, DiagnosticSink, Origin};
pub use diagnostics::{DiagnosticsList, DiagnosticsSnapshot};
pub use document::{Document, DocumentConfig, MAX_RESTORE_SCAN, StepSummary};
pub use events::{Event, EventBus, EventKind, Subscription};
pub use executor::{Completed, DryRun, Pending, SubmitOptions};
pub use graph::{
    ChangeSet, Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Graph, NodeKey,
    NodeState, NodeView, StateKind,
};
pub use history::HistoryQuery;
pub use integrity::{
    CascadeDelete, ClearedFields, DeleteReport, apply_delete_rules, check_references,
};
pub use journal::{Journal, JournalEntry};
pub use load::{
    DiagnosticKind, LoadCounts, LoadDiagnostic, LoadOptions, Loaded, LoadedRow, LoadedRows,
    StoredDiagnostic, StoredForeign, StoredObject, StoredRecord, StoredType, load, load_checked,
};
pub use op::{Op, Properties};
pub use payload::{Decoded, Encoding, Payload};
pub use preserved::{HeldKind, HeldObject, HeldReference};
pub use record::{RECORD_FORMAT, RecordHeader, TxnRecord};
pub use registry::TypeRegistry;
pub use relationship::{Reference, ReferenceField};
pub use reverse_index::{Referrer, Referrers};
pub use scheduler::{Priority, Scheduler, pool_threads};
pub use schema::{
    AssetHash, Bound, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    Finding, GeometryBytes, GeometryEncoding, MigrateFn, Migration, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Strength, TypeDecl, TypeKey,
    TypeValidator, Value, ValueType,
};
pub use schema::{FieldPath, PathSegment};
pub use snapshot::{Change, Flags, Header, Object, Savepoint, Snapshot, SnapshotCell, WorkingSet};
pub use source_map::SourceMap;
pub use store::Handle;
pub use subkey::SubKey;
pub use txn::{
    MAX_OPERATIONS, MAX_SAVEPOINTS, Txn, TxnKind, TxnLimits, TxnMeta, TxnParts, TxnSavepoint,
};
pub use undo::{MAX_MARK_NAME_BYTES, MAX_MARKS, Step, UndoLimits, UndoManager, UndoPosition};
pub use validate::DocumentValidator;
pub use version::{NewerMajor, ReadOnlyReason};
