//! Layer locks: a command that modifies, transforms, deletes or explicitly reorders an existing
//! entity on a locked layer is refused, as are moves onto and off a locked layer and the edits of
//! rich text, splines, tables, annotation scales and clip boundaries. Creating entities on a locked
//! layer is allowed, and so is any renumbering of draw order that a creation or deletion causes
//! for entities the command does not name. A refused command changes nothing, and the lock flag of
//! the layer itself stays changeable.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::Point2;
use arc_base::{Limits, ObjectId, Result};
use arc_doc::link::{self, LinkSpec, LoadMode};
use arc_doc::{Txn, Value};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Segment2};
use serde_json::{Value as Json, json};
use support::{Fixture, curve_json};

const LOCKED: &str = "draft.layer.locked";

fn code(result: Result<Json>) -> &'static str {
    result.unwrap_err().code()
}

fn exists(f: &Fixture, id: ObjectId) -> bool {
    f.document.snapshot().resolve(id).is_ok()
}

fn start_of(f: &Fixture, id: ObjectId) -> [f64; 2] {
    let snapshot = f.document.snapshot();
    let handle = snapshot.resolve(id).unwrap();
    let key = snapshot.type_key(handle).unwrap();
    let index = snapshot
        .registry()
        .get(key.as_str())
        .unwrap()
        .schema
        .fields
        .iter()
        .position(|d| d.name == "geometry")
        .unwrap();
    let value = snapshot.get(handle).unwrap().values[index].clone();
    let Some(Value::Geometry(bytes)) = value else {
        panic!("no geometry")
    };
    let Curve2::Segment(s) = Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap() else {
        panic!()
    };
    [s.start().x, s.start().y]
}

fn segment(f: &mut Fixture, space: ObjectId, layer: Option<ObjectId>) -> ObjectId {
    let geometry = curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(0.0, 0.0), Point2::new(4.0, 0.0)).unwrap(),
    ));
    let mut params = json!({"owner": space.to_string(), "geometry": geometry});
    if let Some(layer) = layer {
        params["layer"] = json!(layer.to_string());
    }
    f.create("core.draft.entity.create", params)
}

/// A drawing with a `walls` layer that holds one segment, locked, and an unlocked segment on
/// the `plain` layer.
fn locked_walls() -> (Fixture, ObjectId, ObjectId, ObjectId, ObjectId) {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let walls = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let plain = f.create("core.draft.layer.create", json!({"name": "plain"}));
    let on_walls = segment(&mut f, space, Some(walls));
    let free = segment(&mut f, space, Some(plain));
    f.set("core.draft.layer.modify", &[walls], "locked", json!(true));
    (f, space, walls, on_walls, free)
}

#[test]
fn transform_is_refused_as_a_whole_when_one_entity_is_on_a_locked_layer() {
    let (mut f, _, _, on_walls, free) = locked_walls();
    let refused = f.run(
        "core.draft.entity.transform",
        json!({"objects": [free.to_string(), on_walls.to_string()],
               "transform": {"translation": [1.0, 0.0]}}),
    );
    assert_eq!(code(refused), LOCKED);
    assert_eq!(start_of(&f, free), [0.0, 0.0]);
    assert_eq!(start_of(&f, on_walls), [0.0, 0.0]);
    let refused = f.run(
        "core.draft.entity.transform",
        json!({"objects": [on_walls.to_string()], "transform": {"translation": [1.0, 0.0]}}),
    );
    let error = refused.unwrap_err();
    assert!(format!("{:?}", error.arguments().get("layer")).contains("walls"));
}

#[test]
fn field_modify_delete_and_explicit_order_are_refused_on_a_locked_layer() {
    let (mut f, _, _, on_walls, _) = locked_walls();
    let modify = f.run(
        "core.draft.entity.modify",
        json!({"objects": [on_walls.to_string()], "field": "visible", "value": false}),
    );
    assert_eq!(code(modify), LOCKED);

    let delete = f.run(
        "core.draft.entity.delete",
        json!({"objects": [on_walls.to_string()]}),
    );
    assert_eq!(code(delete), LOCKED);
    assert!(exists(&f, on_walls));

    let order = f.run(
        "core.draft.order.front",
        json!({"objects": [on_walls.to_string()]}),
    );
    assert_eq!(code(order), LOCKED);
}

#[test]
fn creating_on_a_locked_layer_is_allowed() {
    let (mut f, space, walls, _, _) = locked_walls();
    let created = segment(&mut f, space, Some(walls));
    assert!(exists(&f, created));
    let text = f.run(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "layer": walls.to_string(), "content": "A",
               "position": [0.0, 0.0]}),
    );
    assert!(text.is_ok());
}

#[test]
fn creation_and_reorder_of_other_entities_renumber_draw_order_without_refusal() {
    let (mut f, space, _, on_walls, free) = locked_walls();
    // A new entity goes on top of the container, which renumbers the locked sibling's key.
    let top = segment(&mut f, space, None);
    assert!(exists(&f, top) && exists(&f, on_walls));
    // Reordering the unlocked entity names only that entity.
    let back = f.run(
        "core.draft.order.back",
        json!({"objects": [free.to_string()]}),
    );
    assert!(back.is_ok());
}

#[test]
fn moving_an_entity_onto_or_off_a_locked_layer_is_refused() {
    let (mut f, _, walls, on_walls, free) = locked_walls();
    let onto = f.run(
        "core.draft.entity.modify",
        json!({"objects": [free.to_string()], "field": "layer", "value": walls.to_string()}),
    );
    assert_eq!(code(onto), LOCKED);
    let off = f.run(
        "core.draft.entity.modify",
        json!({"objects": [on_walls.to_string()], "field": "layer", "value": Json::Null}),
    );
    assert_eq!(code(off), LOCKED);
}

#[test]
fn rich_text_spline_table_annotation_scale_and_clip_edits_are_refused() {
    let mut f = Fixture::drafting_with_links();
    let space = f.space("model");
    let layer = f.create("core.draft.layer.create", json!({"name": "walls"}));

    let rich = f.create(
        "core.draft.rich_text.create",
        json!({"owner": space.to_string(), "layer": layer.to_string(), "text": "A",
               "position": [0.0, 0.0]}),
    );
    let spline = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "layer": layer.to_string(),
               "fit_points": [[0.0, 0.0], [2.0, 3.0], [5.0, 1.0], [7.0, 2.0]]}),
    );
    let table = f.create(
        "core.draft.table.create",
        json!({"owner": space.to_string(), "layer": layer.to_string(), "columns": [10.0],
               "rows": [{"kind": "data", "height": 5.0}],
               "cells": [{"cell": "A1", "content": {"kind": "text", "text": "T"}}]}),
    );
    let leader = f.create(
        "core.draft.leader.create",
        json!({"owner": space.to_string(), "layer": layer.to_string(),
               "points": [[0.0, 0.0], [5.0, 5.0]], "arrow": "dot"}),
    );
    let link_id = ObjectId::from_u128(10);
    let link_spec = LinkSpec {
        document: ObjectId::from_u128(2),
        path: "linked.arc",
        hub_resource: None,
        mode: LoadMode::Full,
    };
    f.document
        .submit(move |txn: &mut Txn| link::create(txn, link_id, &link_spec))
        .wait()
        .unwrap();
    let reference = f.create(
        "core.draft.external_ref.create",
        json!({"owner": space.to_string(), "layer": layer.to_string(),
               "link": link_id.to_string(), "position": [1.0, 2.0]}),
    );

    f.set("core.draft.layer.modify", &[layer], "locked", json!(true));

    assert_eq!(
        code(f.run(
            "core.draft.rich_text.set",
            json!({"objects": [rich.to_string()], "text": "B"}),
        )),
        LOCKED
    );
    assert_eq!(
        code(f.run(
            "core.draft.spline.set",
            json!({"objects": [spline.to_string()],
                   "fit_points": [[0.0, 1.0], [2.0, 4.0], [5.0, 2.0], [7.0, 3.0]]}),
        )),
        LOCKED
    );
    assert_eq!(
        code(f.run(
            "core.draft.table.cell.set",
            json!({"table": table.to_string(), "cell": "A1",
                   "content": {"kind": "text", "text": "U"}}),
        )),
        LOCKED
    );
    assert_eq!(
        code(f.run(
            "core.draft.annotation.scales",
            json!({"objects": [leader.to_string()], "add": [2.0]}),
        )),
        LOCKED
    );
    assert_eq!(
        code(f.run(
            "core.draft.external_ref.set_clip",
            json!({"objects": [reference.to_string()], "clip": null}),
        )),
        LOCKED
    );
}

#[test]
fn the_layer_lock_flag_itself_can_change_and_unlocking_allows_the_edit() {
    let (mut f, _, walls, on_walls, _) = locked_walls();
    f.set("core.draft.layer.modify", &[walls], "locked", json!(false));
    let moved = f.run(
        "core.draft.entity.transform",
        json!({"objects": [on_walls.to_string()], "transform": {"translation": [1.0, 0.0]}}),
    );
    assert!(moved.is_ok());
    assert_eq!(start_of(&f, on_walls), [1.0, 0.0]);
}

#[test]
fn deleting_a_path_curve_clears_an_array_reference_and_is_refused_on_a_locked_layer() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let walls = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "unit", "objects": []}),
    );
    let path = segment(&mut f, space, None);
    let array = f.create(
        "core.draft.array.create",
        json!({"owner": space.to_string(), "block": block.to_string(), "position": [0.0, 0.0],
               "array": {"kind": "path", "path": path.to_string(), "count": 3}}),
    );
    // The array moves onto the layer, which is then locked. The path itself stays unlocked.
    f.run(
        "core.draft.entity.modify",
        json!({"objects": [array.to_string()], "field": "layer", "value": walls.to_string()}),
    )
    .unwrap();
    f.set("core.draft.layer.modify", &[walls], "locked", json!(true));
    let deleted = f.run(
        "core.draft.entity.delete",
        json!({"objects": [path.to_string()]}),
    );
    assert_eq!(code(deleted), LOCKED);
    assert!(exists(&f, path));
}
