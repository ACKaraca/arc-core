//! Layer locks (CORE-DRAFT-016). A command that modifies, transforms, deletes or explicitly reorders
//! an existing entity on a locked layer is refused, as is a move of an entity onto or off a locked
//! layer. Creating entities on a locked layer is allowed, and so is any draw-order renumbering that a
//! command causes for entities it does not name. Each entity is judged by its own layer. The check is
//! one function shared by the writing commands; a refused command changes nothing. Changing the lock
//! flag of the layer itself is allowed, and so is every change to a layer, linetype, block or style
//! record.

use arc_base::{Error, ObjectId, Result};
use arc_doc::{Snapshot, Value};

use crate::error::codes;
use crate::types::{ENTITIES, LAYER, entity, layer};

/// Fails with `draft.layer.locked` when `id` is an entity whose layer is locked. Other objects,
/// and entities on an unlocked or absent layer, pass.
pub(crate) fn check_entity(snapshot: &Snapshot, id: ObjectId) -> Result<()> {
    let handle = snapshot.resolve(id)?;
    let is_entity = snapshot
        .type_key(handle)
        .is_some_and(|key| ENTITIES.contains(&key.as_str()));
    if !is_entity {
        return Ok(());
    }
    match snapshot.get(handle)?.values.get(entity::LAYER) {
        Some(Some(Value::Reference(layer_id))) => check_layer(snapshot, *layer_id),
        _ => Ok(()),
    }
}

/// Fails with `draft.layer.locked`, naming the layer, when the layer `layer_id` is locked.
pub(crate) fn check_layer(snapshot: &Snapshot, layer_id: ObjectId) -> Result<()> {
    // A reference that does not resolve names no locked layer; the reference rules report it.
    let Ok(handle) = snapshot.resolve(layer_id) else {
        return Ok(());
    };
    if snapshot
        .type_key(handle)
        .is_none_or(|key| key.as_str() != LAYER)
    {
        return Ok(());
    }
    let values = &snapshot.get(handle)?.values;
    if values.get(layer::LOCKED) != Some(&Some(Value::Bool(true))) {
        return Ok(());
    }
    let name = match values.get(layer::NAME) {
        Some(Some(Value::String(name))) => name.clone(),
        _ => layer_id.to_string(),
    };
    Err(Error::new(codes::LAYER_LOCKED).with_arg("layer", name))
}
