//! Purge of unused styles, layers, linetypes and block definitions (CORE-DRAFT-021).
//!
//! An object is *unused* when no live object refers to it through a field: the reverse index
//! lists no referrer other than objects it owns. Ownership is not use: a block definition
//! that only its own entities point to is unused. [`unused`] lists them; the command
//! `core.draft.purge` deletes a selection in one transaction and refuses with
//! `draft.purge.in_use` when one of them is in use in the transaction's state.

use arc_base::ObjectId;
use arc_doc::Snapshot;
use arc_doc::relationship::ReferenceField;

use crate::style::STYLES;
use crate::types::{BLOCK, LAYER, LINETYPE};

/// The types purge considers.
#[must_use]
pub fn purgeable() -> Vec<&'static str> {
    let mut keys = vec![LAYER, LINETYPE, BLOCK];
    keys.extend_from_slice(STYLES);
    keys
}

/// Whether a live object refers to `id` through a field.
#[must_use]
pub fn in_use(snapshot: &Snapshot, id: ObjectId) -> bool {
    snapshot
        .referrers(id)
        .any(|r| r.field != ReferenceField::Owner && snapshot.resolve(r.source).is_ok())
}

/// The unused objects of the purgeable types, in ascending identifier order.
#[must_use]
pub fn unused(snapshot: &Snapshot) -> Vec<ObjectId> {
    let mut out: Vec<ObjectId> = purgeable()
        .into_iter()
        .filter_map(|key| snapshot.iter_type(key).ok())
        .flat_map(|objects| objects.map(|(_, o)| o.id).collect::<Vec<_>>())
        .filter(|id| !in_use(snapshot, *id))
        .collect();
    out.sort();
    out
}
