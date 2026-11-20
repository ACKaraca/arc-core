//! The exports a product command uses inside its own transaction: entity creation, spline field
//! values, placing on top of the draw order and the layer lock checks. Each test registers a
//! product command that calls one export, as the product would, and runs it through the invoker.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_base::{ObjectId, Tolerance};
use arc_command::{CommandSpec, Context, IdParam, Permission, UndoBehavior};
use arc_doc::{FieldPath, GeometryBytes, Value};
use arc_draft::commands::{create_entity, place_on_top};
use arc_draft::lock::{check_entity, check_layer};
use arc_draft::spline;
use arc_draft::types::{self, entity};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Nurbs2, Segment2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use support::{Fixture, curve_json};

/// Registers a product command of `id` with the parameters `P`.
fn product<P>(
    f: &mut Fixture,
    id: &'static str,
    run: impl Fn(&mut Context<'_>, P) -> arc_base::Result<Option<ObjectId>> + Send + Sync + 'static,
) where
    P: serde::de::DeserializeOwned + JsonSchema,
{
    let spec = CommandSpec::new(
        id,
        "product-command",
        "product-command-description",
        move |ctx, p: P| {
            run(ctx, p).map(|id| serde_json::json!({ "id": id.map(|i| i.to_string()) }))
        },
    )
    .with_undo(UndoBehavior::Undoable)
    .with_permissions([Permission::Edit]);
    f.commands.register(spec).unwrap();
}

fn segment(a: Point2, b: Point2) -> Curve2 {
    Curve2::Segment(Segment2::new(a, b).unwrap())
}

fn geometry(curve: &Curve2) -> Value {
    Value::Geometry(GeometryBytes(curve.encode()))
}

fn draw_order(f: &Fixture, id: ObjectId) -> i64 {
    match f.values(id).get(entity::DRAW_ORDER) {
        Some(Some(Value::I64(key))) => *key,
        other => panic!("no draw order: {other:?}"),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Draw {
    owner: IdParam,
    layer: Option<IdParam>,
    start: [f64; 2],
    end: [f64; 2],
}

#[test]
fn create_entity_places_a_new_entity_on_top_of_its_owner() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let first = f.line(space);
    let walls = f.create("core.draft.layer.create", json!({"name": "walls"}));
    product(&mut f, "test.product.draw", |ctx, p: Draw| {
        let curve = segment(Point2::from(p.start), Point2::from(p.end));
        let created = create_entity(
            ctx,
            types::LINE,
            p.owner.0,
            p.layer,
            vec![(entity::KIND, geometry(&curve))],
        )?;
        Ok(Some(created.id.0))
    });
    let id: ObjectId = f
        .run(
            "test.product.draw",
            json!({"owner": space.to_string(), "layer": walls.to_string(),
                   "start": [0.0, 0.0], "end": [5.0, 0.0]}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    let snapshot = f.document.snapshot();
    let handle = snapshot.resolve(id).unwrap();
    assert_eq!(snapshot.type_key(handle).unwrap().as_str(), types::LINE);
    assert_eq!(snapshot.get(handle).unwrap().header.owner, Some(space));
    assert!(draw_order(&f, id) > draw_order(&f, first));
    assert_eq!(f.values(id)[entity::LAYER], Some(Value::Reference(walls)));
    assert_eq!(
        f.values(id)[entity::KIND],
        Some(geometry(&segment(
            Point2::new(0.0, 0.0),
            Point2::new(5.0, 0.0)
        )))
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Spline {
    owner: IdParam,
    points: Vec<[f64; 2]>,
}

#[test]
fn spline_values_create_a_spline_as_the_foundation_command_does() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    product(&mut f, "test.product.spline", |ctx, p: Spline| {
        let fit: Vec<Point2> = p.points.iter().map(|[x, y]| Point2::new(*x, *y)).collect();
        let curve = Nurbs2::interpolate(&fit, 3, &Tolerance::DEFAULT)?;
        let created = create_entity(
            ctx,
            types::SPLINE,
            p.owner.0,
            None,
            spline::values(&curve, &fit),
        )?;
        Ok(Some(created.id.0))
    });
    let points = json!([[0.0, 0.0], [1.0, 2.0], [3.0, 1.0], [4.0, 3.0]]);
    let id: ObjectId = f
        .run(
            "test.product.spline",
            json!({"owner": space.to_string(), "points": points}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

    // The foundation command with the same fit points stores the same field values.
    let reference = f.create(
        "core.draft.spline.create",
        json!({"owner": space.to_string(), "fit_points": points}),
    );
    let (ours, theirs) = (f.values(id), f.values(reference));
    assert_eq!(ours[entity::KIND], theirs[entity::KIND]);
    assert_eq!(
        ours[entity::SPLINE_FIT_POINTS],
        theirs[entity::SPLINE_FIT_POINTS]
    );
    assert!(matches!(
        ours[entity::SPLINE_FIT_POINTS],
        Some(Value::List(_))
    ));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Front {
    objects: Vec<IdParam>,
}

#[test]
fn place_on_top_moves_the_named_entities_above_the_others() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let a = f.line(space);
    let b = f.line(space);
    let c = f.line(space);
    product(&mut f, "test.product.front", |ctx, p: Front| {
        let ids: Vec<ObjectId> = p.objects.iter().map(|i| i.0).collect();
        place_on_top(ctx, &ids)?;
        Ok(None)
    });
    f.run("test.product.front", json!({"objects": [a.to_string()]}))
        .unwrap();
    assert!(draw_order(&f, a) > draw_order(&f, b));
    assert!(draw_order(&f, a) > draw_order(&f, c));
    assert!(draw_order(&f, c) > draw_order(&f, b));
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Edit {
    objects: Vec<IdParam>,
    layer: IdParam,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Move {
    objects: Vec<IdParam>,
    layer: IdParam,
}

/// A document with a free and a locked layer, a line on each, and the locked layer's id.
fn locked_fixture() -> (Fixture, ObjectId, ObjectId, ObjectId, ObjectId) {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let free = f.create("core.draft.layer.create", json!({"name": "free"}));
    let locked = f.create("core.draft.layer.create", json!({"name": "walls"}));
    f.set("core.draft.layer.modify", &[locked], "locked", json!(true));
    let on_locked = f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "layer": locked.to_string(),
               "geometry": curve_json(&segment(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)))}),
    );
    let on_free = f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "layer": free.to_string(),
               "geometry": curve_json(&segment(Point2::new(0.0, 1.0), Point2::new(1.0, 1.0)))}),
    );
    (f, free, locked, on_free, on_locked)
}

#[test]
fn check_entity_refuses_a_change_to_an_entity_on_a_locked_layer() {
    let (mut f, free, _locked, on_free, on_locked) = locked_fixture();
    product(&mut f, "test.product.touch", |ctx, p: Edit| {
        let txn = ctx.txn()?;
        for IdParam(id) in &p.objects {
            check_entity(txn.snapshot(), *id)?;
        }
        for IdParam(id) in p.objects {
            txn.modify(
                id,
                FieldPath::new("layer"),
                Some(Value::Reference(p.layer.0)),
            )?;
        }
        Ok(None)
    });
    let before = f.document.snapshot().revision();
    let error = f
        .run(
            "test.product.touch",
            json!({"objects": [on_locked.to_string()], "layer": free.to_string()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.layer.locked");
    // A batch with one locked entity changes nothing, including the other entity.
    let error = f
        .run(
            "test.product.touch",
            json!({"objects": [on_free.to_string(), on_locked.to_string()], "layer": free.to_string()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.layer.locked");
    assert_eq!(f.document.snapshot().revision(), before);
    // An entity on an unlocked layer is changed.
    f.run(
        "test.product.touch",
        json!({"objects": [on_free.to_string()], "layer": free.to_string()}),
    )
    .unwrap();
    assert_eq!(
        f.values(on_free)[entity::LAYER],
        Some(Value::Reference(free))
    );
}

#[test]
fn check_layer_refuses_a_move_onto_a_locked_layer() {
    let (mut f, free, locked, on_free, _on_locked) = locked_fixture();
    product(&mut f, "test.product.move", |ctx, p: Move| {
        let txn = ctx.txn()?;
        check_layer(txn.snapshot(), p.layer.0)?;
        for IdParam(id) in p.objects {
            txn.modify(
                id,
                FieldPath::new("layer"),
                Some(Value::Reference(p.layer.0)),
            )?;
        }
        Ok(None)
    });
    let before = f.document.snapshot().revision();
    let error = f
        .run(
            "test.product.move",
            json!({"objects": [on_free.to_string()], "layer": locked.to_string()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.layer.locked");
    assert_eq!(f.document.snapshot().revision(), before);
    assert_eq!(
        f.values(on_free)[entity::LAYER],
        Some(Value::Reference(free))
    );
    f.run(
        "test.product.move",
        json!({"objects": [on_free.to_string()], "layer": free.to_string()}),
    )
    .unwrap();
}
