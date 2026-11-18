//! Display lists through the dependency graph: nested by-block and by-layer appearance, dashes,
//! draw order and wipeouts, exact regeneration counts, graph-built lists against direct
//! evaluation, and viewport assembly with scale, units, rotation, clip and layer overrides.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test code"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::FRAC_PI_2;
use std::sync::{Arc, Mutex};

use arc_base::ObjectId;
use arc_base::math::Point2;
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::appearance::{LayerOverride, Source, resolve_in};
use arc_draft::clip::Piece;
use arc_draft::display::{
    self, DisplayList, DisplayOptions, EntityList, Lists, Primitive, Shape, space_list,
    viewport_list,
};
use arc_draft::types;
use arc_geom::{Curve2, Polyline2, PolylineVertex, Segment2};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{Fixture, curve_json, every_curve};

const EPS: f64 = 1e-9;

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap(),
    ))
}

fn polygon(points: &[(f64, f64)]) -> Value {
    let vertices = points
        .iter()
        .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
        .collect();
    curve_json(&Curve2::Polyline(Polyline2::new(vertices, true).unwrap()))
}

fn id(value: &Value, key: &str) -> ObjectId {
    value[key].as_str().unwrap().parse().unwrap()
}

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn graph_list(f: &Fixture, space: ObjectId) -> DisplayList {
    let snapshot = f.document.snapshot();
    space_list(
        &snapshot,
        Lists::Graph(f.document.graph()),
        space,
        &DisplayOptions::default(),
    )
    .unwrap()
}

fn block(f: &mut Fixture, name: &str) -> ObjectId {
    f.create("core.draft.block.define", json!({"name": name}))
}

fn insert(f: &mut Fixture, owner: ObjectId, block: ObjectId, extra: Value) -> ObjectId {
    let mut params = json!({"owner": s(owner), "block": s(block), "position": [0.0, 0.0]});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    f.create("core.draft.block.insert", params)
}

fn modify(f: &mut Fixture, objects: &[ObjectId], field: &str, value: Value) {
    f.set("core.draft.entity.modify", objects, field, value);
}

/// The end points of the drawn pieces of a stroke (the whole curve when `drawn` is `None`).
fn drawn(p: &Primitive) -> Vec<(Point2, Point2)> {
    let ends = |c: &Curve2| (c.start_point().unwrap(), c.end_point().unwrap());
    match &p.shape {
        Shape::Stroke { curve, drawn: None } => vec![ends(curve)],
        Shape::Stroke {
            drawn: Some(pieces),
            ..
        } => pieces
            .iter()
            .map(|piece| match piece {
                Piece::Curve(c) => ends(c),
                Piece::Point(q) => (*q, *q),
            })
            .collect(),
        other => panic!("not a stroke: {other:?}"),
    }
}

fn near(a: Point2, b: (f64, f64)) -> bool {
    (a.x - b.0).abs() < EPS && (a.y - b.1).abs() < EPS
}

type Ends = ((f64, f64), (f64, f64));

fn assert_pieces(p: &Primitive, expected: &[Ends]) {
    let got = drawn(p);
    assert_eq!(got.len(), expected.len(), "{got:?}");
    for ((a, b), (ea, eb)) in got.iter().zip(expected) {
        assert!(near(*a, *ea) && near(*b, *eb), "{got:?} != {expected:?}");
    }
}

#[test]
fn nested_references_resolve_by_block_and_by_layer_values_and_units() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let l1 = f.create(
        "core.draft.layer.create",
        json!({"name": "l1", "color": [1.0, 0.0, 0.0, 1.0], "lineweight": 0.5}),
    );
    let l2 = f.create("core.draft.layer.create", json!({"name": "l2"}));
    let b1 = block(&mut f, "b1");
    let e = f.entity(b1, seg(0.0, 0.0, 4.0, 0.0));
    modify(&mut f, &[e], "layer", json!(s(l1)));
    modify(&mut f, &[e], "color_by", json!("block"));
    let b2 = block(&mut f, "b2");
    let r1 = insert(&mut f, b2, b1, json!({}));
    modify(&mut f, &[r1], "layer", json!(s(l2)));
    modify(&mut f, &[r1], "color_by", json!("block"));
    let r2 = insert(
        &mut f,
        space,
        b2,
        json!({"position": [10.0, 0.0], "scale": [2.0, 2.0, 1.0], "rotation": FRAC_PI_2}),
    );
    modify(&mut f, &[r2], "color_by", json!("object"));
    modify(&mut f, &[r2], "color", json!([0.0, 1.0, 0.0, 1.0]));

    let list = graph_list(&f, space);
    assert!(list.failed.is_empty());
    let [p] = &list.primitives[..] else {
        panic!("{list:?}")
    };
    assert_eq!((p.source, p.path.clone()), (r2, vec![r1, e]));
    assert_eq!(p.layers, vec![l1, l2]);
    assert_pieces(p, &[((10.0, 0.0), (10.0, 8.0))]);
    let expected = resolve_in(&f.document.snapshot(), &[e, r1, r2], &BTreeMap::new()).unwrap();
    let color = p.color.clone().unwrap();
    assert_eq!(color, expected.color);
    assert_eq!(color.source, Source::Block { level: 2 });
    assert_eq!(p.lineweight.clone().unwrap(), expected.lineweight);
    assert_eq!(p.lineweight.clone().unwrap().source, Source::Layer(l1));

    // A frozen layer anywhere in the chain hides the primitive.
    f.set("core.draft.layer.modify", &[l2], "frozen", json!(true));
    assert!(graph_list(&f, space).primitives.is_empty());

    // A block in inches inserted into a space in millimeters is scaled by 25.4.
    let inches = f.create(
        "core.draft.block.define",
        json!({"name": "inch", "unit": "inch"}),
    );
    f.entity(inches, seg(0.0, 0.0, 1.0, 0.0));
    let r = insert(&mut f, space, inches, json!({}));
    let list = graph_list(&f, space);
    let p = list.primitives.iter().find(|p| p.source == r).unwrap();
    assert_pieces(p, &[((0.0, 0.0), (25.4, 0.0))]);
    // At the top level an unresolved by-block value is the default.
    assert_eq!(p.color.clone().unwrap().source, Source::Default);
}

#[test]
fn dashes_are_generated_in_the_container_and_by_block_linetypes_in_the_definition() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 1.0}, {"kind": "gap", "length": 1.0}]}),
    );
    let line = f.entity(space, seg(0.0, 0.0, 4.0, 0.0));
    modify(&mut f, &[line], "linetype_by", json!("object"));
    modify(&mut f, &[line], "linetype", json!(s(dashed)));
    let b = block(&mut f, "b");
    let inner = f.entity(b, seg(0.0, 0.0, 4.0, 0.0));
    modify(&mut f, &[inner], "linetype_by", json!("block"));
    let r = insert(
        &mut f,
        space,
        b,
        json!({"position": [0.0, 10.0], "scale": [2.0, 2.0, 1.0]}),
    );
    modify(&mut f, &[r], "linetype_by", json!("object"));
    modify(&mut f, &[r], "linetype", json!(s(dashed)));
    let list = graph_list(&f, space);
    let of = |id| list.primitives.iter().find(|p| p.source == id).unwrap();
    assert_pieces(
        of(line),
        &[((0.0, 0.0), (1.0, 0.0)), ((2.0, 0.0), (3.0, 0.0))],
    );
    // The pattern runs in the definition and scales with the reference.
    assert_pieces(
        of(r),
        &[((0.0, 10.0), (2.0, 10.0)), ((4.0, 10.0), (6.0, 10.0))],
    );
    assert_eq!(
        of(r).linetype.clone().unwrap().source,
        Source::Block { level: 1 }
    );
    // The object scale doubles the dashes.
    modify(&mut f, &[line], "linetype_scale", json!(2.0));
    let list = graph_list(&f, space);
    let p = list.primitives.iter().find(|p| p.source == line).unwrap();
    assert_pieces(p, &[((0.0, 0.0), (2.0, 0.0))]);
}

#[test]
fn wipeouts_mask_what_is_beneath_them_and_not_what_is_above() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let below = f.entity(space, seg(-5.0, 0.0, 5.0, 0.0));
    let point = f.entity(space, json!({"kind": "point", "position": [0.5, 0.5]}));
    let wipeout = f.create(
        "core.draft.wipeout.create",
        json!({"owner": s(space), "frame": true,
               "boundary": polygon(&[(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)])}),
    );
    let above = f.entity(space, seg(-5.0, 1.0, 5.0, 1.0));
    let list = graph_list(&f, space);
    let sources: Vec<_> = list.primitives.iter().map(|p| (p.source, p.part)).collect();
    // The point beneath the mask is gone; the mask and its frame follow in draw order.
    assert_eq!(
        sources,
        vec![(below, 0), (wipeout, 0), (wipeout, 1), (above, 0)]
    );
    assert!(matches!(list.primitives[1].shape, Shape::Mask(_)));
    assert_pieces(
        &list.primitives[0],
        &[((-5.0, 0.0), (-2.0, 0.0)), ((2.0, 0.0), (5.0, 0.0))],
    );
    assert_pieces(&list.primitives[3], &[((-5.0, 1.0), (5.0, 1.0))]);

    // Sent to the back, the wipeout masks nothing; brought to the front, it masks both lines.
    f.run("core.draft.order.back", json!({"objects": [s(wipeout)]}))
        .unwrap();
    let list = graph_list(&f, space);
    assert_eq!(list.primitives.len(), 5);
    assert_pieces(&list.primitives[2], &[((-5.0, 0.0), (5.0, 0.0))]);
    f.run("core.draft.order.front", json!({"objects": [s(wipeout)]}))
        .unwrap();
    let list = graph_list(&f, space);
    let of = |id| list.primitives.iter().find(|p| p.source == id).unwrap();
    assert_pieces(
        of(above),
        &[((-5.0, 1.0), (-2.0, 1.0)), ((2.0, 1.0), (5.0, 1.0))],
    );
    assert!(list.primitives.iter().all(|p| p.source != point));
    // An open boundary is refused.
    let open = json!({"owner": s(space), "boundary": seg(0.0, 0.0, 1.0, 0.0)});
    assert!(f.run("core.draft.wipeout.create", open).is_err());
}

/// A document whose display evaluators record the entity of every run.
fn counting() -> (Fixture, Arc<Mutex<Vec<ObjectId>>>) {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let mut evaluators = Evaluators::new();
    for key in types::ENTITIES {
        let inner = display::evaluator(DisplayOptions::default());
        let runs = Arc::clone(&runs);
        evaluators
            .register_object(
                TypeKey::new(*key).unwrap(),
                DerivedKey::new(display::DISPLAY).unwrap(),
                move |ctx| {
                    runs.lock().unwrap().push(ctx.object_id().unwrap());
                    inner(ctx)
                },
            )
            .unwrap();
    }
    (Fixture::with_evaluators(evaluators), runs)
}

#[test]
fn editing_one_input_regenerates_exactly_the_lists_that_depend_on_it() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let la = f.create("core.draft.layer.create", json!({"name": "a"}));
    let lb = f.create("core.draft.layer.create", json!({"name": "b"}));
    let b = block(&mut f, "b");
    let b1 = f.entity(b, seg(0.0, 0.0, 1.0, 0.0));
    let b2 = f.entity(b, seg(0.0, 1.0, 1.0, 1.0));
    modify(&mut f, &[b1], "layer", json!(s(la)));
    modify(&mut f, &[b2], "layer", json!(s(lb)));
    let r = insert(&mut f, space, b, json!({"position": [5.0, 5.0]}));
    let s1 = f.entity(space, seg(0.0, 0.0, 2.0, 0.0));
    let s2 = f.entity(space, seg(0.0, 2.0, 2.0, 2.0));
    modify(&mut f, &[s1], "layer", json!(s(la)));
    modify(&mut f, &[s2], "layer", json!(s(lb)));
    let regenerate = |f: &mut Fixture| -> BTreeSet<ObjectId> {
        runs.lock().unwrap().clear();
        let list = graph_list(f, space);
        assert!(list.failed.is_empty());
        let ran: Vec<_> = runs.lock().unwrap().clone();
        let set: BTreeSet<_> = ran.iter().copied().collect();
        assert_eq!(set.len(), ran.len(), "each list is built once");
        set
    };
    assert_eq!(regenerate(&mut f), BTreeSet::from([b1, b2, r, s1, s2]));
    assert!(regenerate(&mut f).is_empty());

    modify(&mut f, &[s1], "color_by", json!("object"));
    assert_eq!(regenerate(&mut f), BTreeSet::from([s1]));

    f.set(
        "core.draft.layer.modify",
        &[la],
        "color",
        json!([0.0, 0.0, 1.0, 1.0]),
    );
    assert_eq!(regenerate(&mut f), BTreeSet::from([s1, b1, r]));

    f.set(
        "core.draft.block.modify",
        &[b],
        "base_point",
        json!([1.0, 1.0]),
    );
    assert_eq!(regenerate(&mut f), BTreeSet::from([r]));

    modify(&mut f, &[b2], "geometry", seg(0.0, 1.0, 3.0, 1.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([b2, r]));

    let added = f.entity(b, seg(0.0, 3.0, 1.0, 3.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([added, r]));

    // A layer field that no list reads changes nothing downstream of the readers.
    f.set("core.draft.layer.modify", &[lb], "locked", json!(true));
    let ran = regenerate(&mut f);
    assert!(ran.is_subset(&BTreeSet::from([b2, s2])), "{ran:?}");
}

/// A scene with nested references, by-block and by-layer values, a dashed layer and a wipeout.
struct Scene {
    f: Fixture,
    space: ObjectId,
    layers: Vec<ObjectId>,
    blocks: Vec<ObjectId>,
    entities: Vec<ObjectId>,
    refs: Vec<ObjectId>,
    dashed: ObjectId,
}

fn scene() -> Scene {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [{"kind": "dash", "length": 0.5}, {"kind": "gap", "length": 0.25}, {"kind": "dot"}, {"kind": "gap", "length": 0.25}]}),
    );
    let l1 = f.create(
        "core.draft.layer.create",
        json!({"name": "l1", "linetype": s(dashed), "color": [1.0, 0.0, 0.0, 1.0]}),
    );
    let l2 = f.create("core.draft.layer.create", json!({"name": "l2"}));
    let curves = every_curve();
    let b1 = block(&mut f, "b1");
    let e1 = f.entity(b1, seg(0.0, 0.0, 3.0, 0.0));
    modify(&mut f, &[e1], "layer", json!(s(l1)));
    modify(&mut f, &[e1], "color_by", json!("block"));
    let e2 = f.entity(b1, curve_json(&curves[3]));
    modify(&mut f, &[e2], "layer", json!(s(l2)));
    modify(&mut f, &[e2], "linetype_by", json!("block"));
    let b2 = block(&mut f, "b2");
    let r1 = insert(&mut f, b2, b1, json!({"position": [1.0, 1.0]}));
    modify(&mut f, &[r1], "layer", json!(s(l2)));
    modify(&mut f, &[r1], "color_by", json!("block"));
    modify(&mut f, &[r1], "linetype_by", json!("block"));
    let e3 = f.entity(b2, curve_json(&curves[4]));
    let r2 = insert(
        &mut f,
        space,
        b2,
        json!({"position": [2.0, 0.0], "scale": [1.5, -1.5, 1.0], "rotation": 0.3}),
    );
    let r3 = insert(&mut f, space, b1, json!({"position": [-3.0, 2.0]}));
    let s1 = f.entity(space, curve_json(&curves[7]));
    modify(&mut f, &[s1], "layer", json!(s(l1)));
    let s2 = f.entity(space, seg(-6.0, 0.5, 6.0, 0.5));
    let w = f.create(
        "core.draft.wipeout.create",
        json!({"owner": s(space), "boundary": polygon(&[(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)])}),
    );
    let s3 = f.entity(space, curve_json(&curves[2]));
    Scene {
        f,
        space,
        layers: vec![l1, l2],
        blocks: vec![b1, b2],
        entities: vec![e1, e2, e3, r1, r2, r3, s1, s2, w, s3],
        refs: vec![r1, r2, r3],
        dashed,
    }
}

fn pick<T: Copy>(items: &[T], i: usize) -> T {
    items[i % items.len()]
}

fn apply(sc: &mut Scene, (op, i, v): (u8, usize, f64)) {
    let e = pick(&sc.entities, i);
    let f = &mut sc.f;
    let gray = json!([v.abs() / 3.0, 0.5, 0.25, 1.0]);
    match op {
        0 => {
            modify(f, &[e], "color_by", json!("object"));
            modify(f, &[e], "color", gray);
        }
        1 => modify(
            f,
            &[e],
            "color_by",
            json!(if v > 0.0 { "block" } else { "layer" }),
        ),
        2 => {
            modify(
                f,
                &[e],
                "linetype_by",
                json!(if v > 0.0 { "object" } else { "layer" }),
            );
            modify(f, &[e], "linetype", json!(s(sc.dashed)));
        }
        3 => f.set(
            "core.draft.layer.modify",
            &[pick(&sc.layers, i)],
            "color",
            gray,
        ),
        4 => f.set(
            "core.draft.block.modify",
            &[pick(&sc.blocks, i)],
            "base_point",
            json!([v, 0.0]),
        ),
        5 => {
            f.run("core.draft.order.front", json!({"objects": [s(e)]}))
                .unwrap();
        }
        6 => f.set(
            "core.draft.layer.modify",
            &[pick(&sc.layers, i)],
            if i % 2 == 0 { "frozen" } else { "on" },
            json!(v > 0.0),
        ),
        7 => modify(f, &[pick(&sc.refs, i)], "rotation", json!(v)),
        _ => modify(f, &[e], "linetype_scale", json!(v.abs() + 0.1)),
    }
}

fn graph_entity(f: &Fixture, id: ObjectId) -> EntityList {
    let view = f
        .document
        .graph()
        .evaluate_now(&display::node(id).unwrap())
        .unwrap();
    view.result
        .unwrap()
        .downcast_ref::<EntityList>()
        .unwrap()
        .clone()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

    #[test]
    fn graph_built_lists_equal_direct_evaluation_after_edits(
        ops in prop::collection::vec((0_u8..9, 0_usize..64, -3.0_f64..3.0), 1..8),
    ) {
        let mut sc = scene();
        let options = DisplayOptions::default();
        // Lists are cached before the edits, so a stale node would show.
        graph_list(&sc.f, sc.space);
        for op in ops {
            apply(&mut sc, op);
            let snapshot = sc.f.document.snapshot();
            for &e in &sc.entities {
                let direct = display::entity_list(&snapshot, e, &options).unwrap();
                prop_assert_eq!(graph_entity(&sc.f, e), direct);
            }
            let direct = space_list(&snapshot, Lists::Direct, sc.space, &options).unwrap();
            prop_assert!(direct.failed.is_empty(), "{:?}", direct.failed);
            prop_assert_eq!(graph_list(&sc.f, sc.space), direct);
        }
    }
}

#[test]
fn a_viewport_maps_scales_rotates_clips_and_overrides_layers() {
    let mut f = Fixture::drafting();
    let model = f.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "meter"}),
    );
    let red = f.create(
        "core.draft.layer.create",
        json!({"name": "red", "color": [1.0, 0.0, 0.0, 1.0]}),
    );
    let hidden = f.create("core.draft.layer.create", json!({"name": "hidden"}));
    let short = f.entity(model, seg(1.0, 0.0, 3.0, 0.0));
    let long = f.entity(model, seg(2.0, -10.0, 2.0, 10.0));
    let frozen = f.entity(model, seg(2.0, 0.0, 2.5, 0.0));
    modify(&mut f, &[short], "layer", json!(s(red)));
    modify(&mut f, &[frozen], "layer", json!(s(hidden)));
    let created = f
        .run(
            "core.draft.layout.create",
            json!({"name": "sheet", "paper_width": 210.0, "paper_height": 297.0, "orientation": "landscape"}),
        )
        .unwrap();
    let sheet = id(&created, "space");
    // 1:100 from meters to millimeters: one model meter is 10 sheet millimeters.
    let vp = f.create(
        "core.draft.viewport.create",
        json!({"layout": s(sheet), "model": s(model), "center": [100.0, 100.0],
               "width": 100.0, "height": 60.0, "view_center": [2.0, 0.0],
               "scale": 0.01, "rotation": FRAC_PI_2}),
    );
    let blue = [0.0, 0.0, 1.0, 1.0];
    f.set(
        "core.draft.viewport.modify",
        &[vp],
        "layer_overrides",
        json!([
            [s(red), false, blue, null, 0.7, null],
            [s(hidden), true, null, null, null, null]
        ]),
    );
    let snapshot = f.document.snapshot();
    let list = viewport_list(
        &snapshot,
        Lists::Graph(f.document.graph()),
        vp,
        &DisplayOptions::default(),
    )
    .unwrap();
    assert!(list.failed.is_empty());
    let of = |id| list.primitives.iter().find(|p| p.source == id);
    assert!(of(frozen).is_none(), "frozen in the viewport");
    let p = of(short).unwrap();
    assert_pieces(p, &[((100.0, 90.0), (100.0, 110.0))]);
    let color = p.color.clone().unwrap();
    let overrides = BTreeMap::from([(
        red,
        LayerOverride {
            color: Some(arc_doc::ColorValue {
                rgba: arc_base::Color::new(0.0, 0.0, 1.0, 1.0),
                source: None,
            }),
            lineweight: Some(0.7),
            ..LayerOverride::default()
        },
    )]);
    let expected = resolve_in(&snapshot, &[short], &overrides).unwrap();
    assert_eq!(color, expected.color);
    assert_eq!(color.source, Source::ViewportOverride(red));
    assert_eq!(p.lineweight.clone().unwrap(), expected.lineweight);
    // The long line crosses the sheet horizontally and is cut to the rectangle x in [50, 150].
    assert_pieces(of(long).unwrap(), &[((150.0, 100.0), (50.0, 100.0))]);
    // The space's own list keeps the frozen-in-viewport entity and the layer's color.
    let model_list = graph_list(&f, model);
    assert_eq!(model_list.primitives.len(), 3);

    // An arbitrary clip: a triangle whose slanted side cuts the horizontal line at x = 120.
    f.set(
        "core.draft.viewport.modify",
        &[vp],
        "clip",
        polygon(&[(60.0, 80.0), (140.0, 80.0), (60.0, 160.0)]),
    );
    let snapshot = f.document.snapshot();
    let list = viewport_list(
        &snapshot,
        Lists::Graph(f.document.graph()),
        vp,
        &DisplayOptions::default(),
    )
    .unwrap();
    let p = list.primitives.iter().find(|p| p.source == long).unwrap();
    assert_pieces(p, &[((120.0, 100.0), (60.0, 100.0))]);
    assert!(list.clip.is_some());

    // A locked viewport keeps its view.
    f.set("core.draft.viewport.modify", &[vp], "locked", json!(true));
    let refused = f.run(
        "core.draft.viewport.modify",
        json!({"objects": [s(vp)], "field": "scale", "value": 0.02}),
    );
    assert_eq!(refused.unwrap_err().code(), "draft.viewport.locked");
}

#[test]
fn lists_beyond_the_nesting_and_size_bounds_fail_and_are_left_out() {
    let options = DisplayOptions {
        max_nesting: 2,
        max_primitives: 1,
        ..DisplayOptions::default()
    };
    let mut evaluators = Evaluators::new();
    display::register_evaluators(&mut evaluators, options).unwrap();
    let mut f = Fixture::with_evaluators(evaluators);
    let space = f.space("model");
    let b1 = block(&mut f, "b1");
    f.entity(b1, seg(0.0, 0.0, 1.0, 0.0));
    let b2 = block(&mut f, "b2");
    insert(&mut f, b2, b1, json!({}));
    let b3 = block(&mut f, "b3");
    insert(&mut f, b3, b2, json!({}));
    let deep = insert(&mut f, space, b3, json!({}));
    let two = block(&mut f, "two");
    f.entity(two, seg(0.0, 0.0, 1.0, 0.0));
    f.entity(two, seg(0.0, 1.0, 1.0, 1.0));
    let large = insert(&mut f, space, two, json!({}));
    let fine = insert(&mut f, space, b2, json!({}));
    let snapshot = f.document.snapshot();
    let code = |id| {
        display::entity_list(&snapshot, id, &options)
            .unwrap_err()
            .code()
    };
    assert_eq!(code(deep), "draft.block.nesting_too_deep");
    assert_eq!(code(large), "draft.display.too_large");
    let graph = f.document.graph();
    let state = graph.evaluate_now(&display::node(deep).unwrap()).unwrap();
    assert_eq!(state.state.kind(), arc_doc::graph::StateKind::Failed);
    for lists in [Lists::Graph(graph), Lists::Direct] {
        let list = space_list(&snapshot, lists, space, &options).unwrap();
        let mut failed = vec![deep, large];
        failed.sort();
        assert_eq!(list.failed, failed);
        assert_eq!(list.primitives.len(), 1);
        assert_eq!(list.primitives[0].source, fine);
    }
}
