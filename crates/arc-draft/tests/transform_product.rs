//! A product command that runs the entity transform in process: it checks its own prompt answer,
//! then calls `arc_draft::apply_entity_transform` inside its transaction. The checks, the refusals
//! and the undo are those of `core.draft.entity.transform`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::ObjectId;
use arc_base::math::Point2;
use arc_base::{Limits, Result};
use arc_command::{CommandSpec, Context, UndoBehavior};
use arc_doc::Value;
use arc_draft::EntityTransformParams;
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Segment2};
use serde_json::{Value as Json, json};
use support::{Fixture, curve_json};

const PRODUCT: &str = "product.test.nudge";

/// The product command: the translation must be positive, which the product asks for in its own
/// prompt, and the transform is then applied by the drafting crate in the command's transaction.
fn nudge(ctx: &mut Context<'_>, p: EntityTransformParams) -> Result<()> {
    if p.transform.translation[0] <= 0.0 {
        return Err(arc_base::Error::new(arc_draft::error::codes::FIELD_INVALID)
            .with_arg("field", "translation"));
    }
    arc_draft::apply_entity_transform(ctx, p)
}

fn fixture() -> Fixture {
    let mut f = Fixture::drafting();
    let spec = CommandSpec::new(
        PRODUCT,
        "product-test-nudge",
        "product-test-nudge-description",
        nudge,
    )
    .with_undo(UndoBehavior::Undoable);
    f.commands.register(spec).unwrap();
    f
}

fn nudge_call(f: &mut Fixture, objects: &[ObjectId], transform: Json) -> Result<Json> {
    let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
    f.run(PRODUCT, json!({"objects": ids, "transform": transform}))
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
    let Some(Value::Geometry(bytes)) = snapshot.get(handle).unwrap().values[index].clone() else {
        panic!("no geometry")
    };
    let Curve2::Segment(s) = Curve2::decode(&bytes.0, &Limits::DEFAULT).unwrap() else {
        panic!("not a segment")
    };
    [s.start().x, s.start().y]
}

fn segment(f: &mut Fixture, owner: ObjectId, x0: f64) -> ObjectId {
    let geometry = curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(x0, 0.0), Point2::new(x0 + 4.0, 0.0)).unwrap(),
    ));
    f.entity(owner, geometry)
}

#[test]
fn the_product_command_moves_entities_through_the_drafting_transform() {
    let mut f = fixture();
    let space = f.space("model");
    let a = segment(&mut f, space, 0.0);
    let b = segment(&mut f, space, 10.0);
    nudge_call(&mut f, &[a, b], json!({"translation": [2.0, 3.0]})).unwrap();
    assert_eq!(start_of(&f, a), [2.0, 3.0]);
    assert_eq!(start_of(&f, b), [12.0, 3.0]);
}

#[test]
fn the_product_check_refuses_before_anything_changes() {
    let mut f = fixture();
    let space = f.space("model");
    let a = segment(&mut f, space, 0.0);
    let refused = nudge_call(&mut f, &[a], json!({"translation": [-1.0, 0.0]}));
    assert_eq!(refused.unwrap_err().code(), "draft.field.invalid");
    assert_eq!(start_of(&f, a), [0.0, 0.0]);
}

#[test]
fn the_drafting_checks_apply_inside_the_product_command() {
    let mut f = fixture();
    let space = f.space("model");
    let walls = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let free = segment(&mut f, space, 0.0);
    let on_walls = f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "layer": walls.to_string(),
               "geometry": curve_json(&Curve2::Segment(
                   Segment2::new(Point2::new(5.0, 0.0), Point2::new(9.0, 0.0)).unwrap()))}),
    );
    f.set("core.draft.layer.modify", &[walls], "locked", json!(true));

    // A locked layer refuses the whole call, and the free entity does not move either.
    let refused = nudge_call(
        &mut f,
        &[free, on_walls],
        json!({"translation": [1.0, 0.0]}),
    );
    assert_eq!(refused.unwrap_err().code(), "draft.layer.locked");
    assert_eq!(start_of(&f, free), [0.0, 0.0]);
    assert_eq!(start_of(&f, on_walls), [5.0, 0.0]);

    // A scale that is not positive is refused by the transform's own parameter checks.
    let refused = nudge_call(
        &mut f,
        &[free],
        json!({"translation": [1.0, 0.0], "scale": 0.0}),
    );
    assert_eq!(refused.unwrap_err().code(), "draft.field.invalid");
    assert_eq!(start_of(&f, free), [0.0, 0.0]);
}

#[test]
fn one_undo_reverts_the_whole_product_command() {
    let mut f = fixture();
    let space = f.space("model");
    let a = segment(&mut f, space, 0.0);
    let b = segment(&mut f, space, 10.0);
    nudge_call(&mut f, &[a, b], json!({"translation": [2.0, 0.0]})).unwrap();
    assert!(f.document.undo().wait().unwrap().value);
    assert_eq!(start_of(&f, a), [0.0, 0.0]);
    assert_eq!(start_of(&f, b), [10.0, 0.0]);
}
