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
pub mod payload;
pub mod registry;
pub mod schema;

#[cfg(test)]
mod testing;

pub use descriptor::descriptor;
pub use payload::{Decoded, Encoding, Payload};
pub use registry::TypeRegistry;
pub use schema::{
    AssetHash, Bound, Cardinality, ColorSource, ColorValue, EnumVariant, ExternalRef, FieldDecl,
    Finding, GeometryBytes, GeometryEncoding, MigrateFn, Migration, OnCopy, OnDelete,
    ReferenceSpec, RelationshipRule, Schema, SchemaVersion, Severity, Strength, TypeDecl, TypeKey,
    TypeValidator, Value, ValueType,
};
