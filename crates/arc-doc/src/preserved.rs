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
//!   reference ("references kept for integrity"): it enters the reverse reference index. Its
//!   field references are not indexed, since its payload is not decoded.
//! - Its stored row is written back unchanged on every save ([`HeldObject::row`]).

use std::sync::Arc;

use arc_base::Error;

use crate::load::StoredObject;
use crate::schema::TypeKey;
use crate::snapshot::Object;

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
        }
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
