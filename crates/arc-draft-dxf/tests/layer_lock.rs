//! A drawing whose layer is locked imports completely; the layer stays locked after the import,
//! and the imported entities on it cannot be transformed (`draft.layer.locked`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::ObjectId;
use arc_doc::Value;
use arc_doc::relationship::ReferenceField;
use arc_draft::types;
use serde_json::json;
use support::{Env, bytes, drawing, layer_entry, line, on_layer};

/// The entity of the model space `space` on the layer named `name`.
fn entity_on_layer(env: &Env, space: ObjectId, name: &str) -> ObjectId {
    let snapshot = env.snapshot();
    snapshot
        .referrers(space)
        .filter(|r| r.field == ReferenceField::Owner)
        .map(|r| r.source)
        .find(|id| {
            let Ok(handle) = snapshot.resolve(*id) else {
                return false;
            };
            let Some(key) = snapshot.type_key(handle) else {
                return false;
            };
            if !types::ENTITIES.contains(&key.as_str()) {
                return false;
            }
            let values = &snapshot.get(handle).unwrap().values;
            let Some(Some(Value::Reference(layer))) = values.get(types::entity::LAYER) else {
                return false;
            };
            let layer_values = &snapshot
                .get(snapshot.resolve(*layer).unwrap())
                .unwrap()
                .values;
            matches!(
                layer_values.get(types::layer::NAME),
                Some(Some(Value::String(found))) if found == name
            )
        })
        .expect("an entity on the layer")
}

#[test]
fn a_locked_layer_imports_completely_and_its_entities_refuse_a_transform() {
    let mut env = Env::new();
    let space = env.space("model", "millimeter");
    let mut d = drawing(
        vec![
            on_layer(line([0.0; 3], [10.0, 0.0, 0.0]), "LOCKED"),
            on_layer(line([0.0; 3], [0.0, 10.0, 0.0]), "0"),
        ],
        Some(4),
    );
    // Layer flag 4 is "locked" in the DXF layer table.
    let mut locked = layer_entry("LOCKED", 1);
    locked.flags = 4;
    d.tables.layers.as_mut().unwrap().entries.push(locked);
    let path = env.write("locked.dxf", &bytes(&d));

    let outcome = env.import(&path, space).unwrap();
    assert_eq!(outcome.value["entities"], 2);

    let exported = env.exported(space);
    let layers = exported.tables.layers.unwrap();
    let entry = layers
        .entries
        .iter()
        .find(|e| e.name == "LOCKED")
        .expect("the locked layer");
    assert_ne!(
        entry.flags & 4,
        0,
        "the layer stays locked after the import"
    );

    let id = entity_on_layer(&env, space, "LOCKED");
    let refused = env.invoke(
        "core.draft.entity.transform",
        json!({"objects": [id.to_string()], "transform": {"translation": [1.0, 0.0]}}),
    );
    let Err(error) = refused else {
        panic!("the transform was not refused")
    };
    assert_eq!(error.code(), "draft.layer.locked");
}
