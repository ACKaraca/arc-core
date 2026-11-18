//! Preserved and quarantined objects (CORE-DOC-039): stored objects a document keeps without
//! being able to use them.
//!
//! A **preserved** object is a stored object of a type the registry does not know (or, in the
//! read-only view of a document with a newer major version, of that newer version). A
//! **quarantined** object is a stored object that is invalid: its payload does not decode or
//! validate, a column is malformed, or its migration or a validator failed or panicked. Both
//! are **held objects**: the snapshot keeps them beside the typed stores, fixed at load.
//!
//! - A held object has its identifier, resolves through the identifier index
//!   ([`crate::Snapshot::resolve`], [`crate::Snapshot::get`] and [`crate::Snapshot::type_key`]
//!   give its object without field values and the type key of its row), and is listed by
//!   [`crate::Snapshot::held`] and counted by [`crate::Snapshot::held_len`].
//! - It cannot change and survives every save: every write of a working set to it fails with
//!   `doc.object.not_modifiable` (argument `id`, detail `kind`), also the cascade of deleting its
//!   owner, which is therefore refused. Working sets, savepoints and publication share the held
//!   objects unchanged.
//! - It takes part in reference integrity as a target: a reference to it is not dangling, and the
//!   type check of the reference uses the type key of its row. Its own header owner is a
//!   reference ("references kept for integrity"): it enters the reverse reference index. The
//!   field references of a preserved object of an unregistered type are decoded with the
//!   descriptor its document stores for its type and version ([`HeldObject::references`]) and
//!   enter the index too, with the relationship rules of that descriptor; its payload is never
//!   rewritten. Other held objects hold their owner only.
//! - Its stored row is written back unchanged on every save ([`HeldObject::row`]).

use std::sync::Arc;

use arc_base::{Error, ObjectId};

use crate::error::codes;
use crate::load::StoredObject;
use crate::relationship::ReferenceField;
use crate::schema::{ReferenceSpec, TypeKey};
use crate::snapshot::Object;

/// A field reference of a preserved object, decoded with the stored descriptor of its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldReference {
    /// The field path that holds it.
    pub field: ReferenceField,
    /// The target identifier.
    pub target: ObjectId,
    /// The allowed target types and the relationship rule the descriptor states.
    pub spec: ReferenceSpec,
}

/// Why a stored object is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeldKind {
    /// The type is not registered, or is of a newer major version opened in the read-only view.
    Preserved,
    /// The object is invalid.
    Quarantined,
}

/// A preserved or quarantined object: its stored row, unchanged, and what the document shows of
/// it.
#[derive(Debug)]
pub struct HeldObject {
    kind: HeldKind,
    type_key: Option<TypeKey>,
    object: Arc<Object>,
    row: StoredObject,
    reason: Error,
    references: Vec<HeldReference>,
}

impl HeldObject {
    pub(crate) fn new(
        kind: HeldKind,
        type_key: Option<TypeKey>,
        object: Object,
        row: StoredObject,
        reason: Error,
    ) -> Self {
        Self {
            kind,
            type_key,
            object: Arc::new(object),
            row,
            reason,
            references: Vec::new(),
        }
    }

    /// The object with the field references decoded from its payload.
    pub(crate) fn with_references(mut self, references: Vec<HeldReference>) -> Self {
        self.references = references;
        self
    }

    /// The field references decoded with the stored descriptor of its type, in field order;
    /// empty for a quarantined object and for a preserved object whose descriptor or payload
    /// does not decode. The header owner is not among them.
    #[must_use]
    pub fn references(&self) -> &[HeldReference] {
        &self.references
    }

    /// The error of a write to this object: `doc.object.not_modifiable` (argument `id`, detail
    /// `kind`).
    pub(crate) fn not_modifiable(&self) -> Error {
        let kind = match self.kind {
            HeldKind::Preserved => "preserved",
            HeldKind::Quarantined => "quarantined",
        };
        Error::new(codes::OBJECT_NOT_MODIFIABLE)
            .with_arg("id", self.object.id.to_string())
            .with_detail("kind", kind)
    }

    /// Whether the object is preserved or quarantined.
    #[must_use]
    pub fn kind(&self) -> HeldKind {
        self.kind
    }

    /// The type key of the stored row, or `None` when it is malformed.
    #[must_use]
    pub fn type_key(&self) -> Option<&TypeKey> {
        self.type_key.as_ref()
    }

    /// The object: identifier, the stored schema version (0.0 when malformed), the header as far
    /// as its columns are valid, and no field values.
    #[must_use]
    pub fn object(&self) -> &Arc<Object> {
        &self.object
    }

    /// The stored row exactly as it was loaded; a save writes it back unchanged.
    #[must_use]
    pub fn row(&self) -> &StoredObject {
        &self.row
    }

    /// Why the object is held: `doc.type.unknown` or `doc.load.newer_major` for a preserved
    /// object, the failure that quarantined it otherwise.
    #[must_use]
    pub fn reason(&self) -> &Error {
        &self.reason
    }
}
