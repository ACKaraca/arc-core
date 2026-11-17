//! The document store of the Arc suite.
//!
//! This first part provides the value model of object fields and the declarations of object
//! types ([`schema`]), the type registry (CORE-DOC-001, [`registry`]), JSON field descriptors
//! (CORE-DOC-004, [`descriptor`]) and the payload codec ([`payload`]). Stores, snapshots,
//! transactions, relationships, the journal, migration and the package build on these types.
//!
//! No function in this crate panics.

pub mod descriptor;
pub mod error;
pub mod graph;
mod index;
pub mod integrity;
pub mod load;
pub mod migrate;
pub mod op;
pub mod payload;
pub mod preserved;
pub mod registry;
pub mod relationship;
pub mod reverse_index;
pub mod schema;
pub mod snapshot;
mod store;
pub mod txn;
pub mod version;

#[cfg(test)]
mod testing;

pub use descriptor::descriptor;
pub use graph::{
    ChangeSet, Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, Graph, NodeKey,
    NodeState, NodeView, StateKind,
};
pub use integrity::{DeleteReport, apply_delete_rules, check_references};
pub use load::{
    DiagnosticKind, LoadCounts, LoadDiagnostic, LoadOptions, Loaded, LoadedRow, LoadedRows,
    StoredObject, StoredType, load,
};
pub use op::{Op, Properties};
pub use payload::{Decoded, Encoding, Payload};
pub use preserved::{HeldKind, HeldObject};
pub use registry::TypeRegistry;
pub use relationship::{Reference, ReferenceField};
pub use reverse_index::{Referrer, Referrers};
pub use schema::{
    AssetHash, Bound, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    Finding, GeometryBytes, GeometryEncoding, MigrateFn, Migration, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Strength, TypeDecl, TypeKey,
    TypeValidator, Value, ValueType,
};
pub use schema::{FieldPath, PathSegment};
pub use snapshot::{Change, Flags, Header, Object, Savepoint, Snapshot, SnapshotCell, WorkingSet};
pub use store::Handle;
pub use txn::{Txn, TxnKind, TxnLimits, TxnMeta, TxnParts, TxnSavepoint};
pub use version::{NewerMajor, ReadOnlyReason};
