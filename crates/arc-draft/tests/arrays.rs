//! Reference arrays (CORE-DRAFT-024): item transforms of each kind, the item bound, display
//! lists and regeneration of only the array that changed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test code"
)]

mod support;

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};
use std::sync::{Arc, Mutex};

use arc_base::math::Point2;
use arc_base::{ObjectId, Tolerance};
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::array::{Kind, MAX_ITEMS, Params, items};
use arc_draft::display::{self, DisplayOptions, Lists, Shape, space_list};
use arc_draft::types;
use arc_geom::{Arc2, Curve2, Segment2};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{Fixture, curve_json};

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn params(kind: Kind) -> Params {
    Params {
        kind,
        position: Point2::new(1.0, 2.0),
        rotation: 0.0,
        rows: 1,
        columns: 1,
        row_spacing: 0.0,
        column_spacing: 0.0,
        angle: 0.0,
        center: Point2::new(0.0, 0.0),
        count: 1,
        total_angle: TAU,
        rotate_items: true,
        spacing: 0.0,
        align: true,
    }
}

fn wrap(a: f64) -> f64 {
    (a + PI).rem_euclid(TAU) - PI
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Item (row, column) lies `column * column_spacing` along the array's x axis and
    /// `row * row_spacing` along its y axis from the first item, whatever the angle.
    #[test]
    fn rectangular_items_follow_the_axes(
        rows in 1_i64..8,
        columns in 1_i64..8,
        row_spacing in -50.0_f64..50.0,
        column_spacing in -50.0_f64..50.0,
        angle in -PI..PI,
        px in -1e6_f64..1e6,
        py in -1e6_f64..1e6,
        rotation in -PI..PI,
    ) {
        let p = Params {
            rows, columns, row_spacing, column_spacing, angle, rotation,
            position: Point2::new(px, py),
            ..params(Kind::Rectangular)
        };
        let all = items(&p, None, &Tolerance::DEFAULT).unwrap();
        prop_assert_eq!(all.len() as i64, rows * columns);
        let (ax, ay) = (angle.cos(), angle.sin());
        for (k, item) in all.iter().enumerate() {
            let (r, c) = ((k as i64 / columns) as f64, (k as i64 % columns) as f64);
            let (dx, dy) = (item.position.x - px, item.position.y - py);
            let tol = 1e-9 * (1.0 + 1e6);
            prop_assert!((dx * ax + dy * ay - c * column_spacing).abs() < tol);
            prop_assert!((-dx * ay + dy * ax - r * row_spacing).abs() < tol);
            prop_assert_eq!(item.rotation, rotation);
        }
        prop_assert_eq!(all[0].position, Point2::new(px, py));
    }

    /// Polar items keep their distance from the center and turn by a constant step: a full
    /// turn divides into `count` equal steps, a partial one puts the last item at the total
    /// angle.
    #[test]
    fn polar_items_keep_their_radius_and_step(
        count in 1_i64..40,
        total in prop_oneof![Just(TAU), Just(-TAU), 0.1_f64..6.0, -6.0_f64..-0.1],
        rotate in any::<bool>(),
        px in -100.0_f64..100.0,
        py in -100.0_f64..100.0,
        cx in -100.0_f64..100.0,
        cy in -100.0_f64..100.0,
        rotation in -PI..PI,
    ) {
        prop_assume!((px - cx).hypot(py - cy) > 1e-3);
        let p = Params {
            count, total_angle: total, rotate_items: rotate, rotation,
            position: Point2::new(px, py), center: Point2::new(cx, cy),
            ..params(Kind::Polar)
        };
        let all = items(&p, None, &Tolerance::DEFAULT).unwrap();
        prop_assert_eq!(all.len() as i64, count);
        let full = total.abs() >= TAU - Tolerance::DEFAULT.angular;
        let step = if full {
            total / count as f64
        } else if count > 1 {
            total / (count - 1) as f64
        } else {
            0.0
        };
        let radius = (px - cx).hypot(py - cy);
        let start = (py - cy).atan2(px - cx);
        for (i, item) in all.iter().enumerate() {
            let (dx, dy) = (item.position.x - cx, item.position.y - cy);
            prop_assert!((dx.hypot(dy) - radius).abs() < 1e-9 * (1.0 + radius));
            let turned = wrap(dy.atan2(dx) - start - i as f64 * step);
            prop_assert!(turned.abs() < 1e-9, "item {} turned {}", i, turned);
            let expected = rotation + if rotate { i as f64 * step } else { 0.0 };
            prop_assert!((item.rotation - expected).abs() < 1e-12);
        }
        if !full && count > 1 {
            let last = all.last().unwrap();
            let turned = wrap((last.position.y - cy).atan2(last.position.x - cx) - start - total);
            prop_assert!(turned.abs() < 1e-9);
        }
    }
}

fn segment(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap())
}

#[test]
fn path_items_follow_the_curve_by_count_or_spacing() {
    let tol = Tolerance::DEFAULT;
    let line = segment(0.0, 0.0, 10.0, 0.0);
    let base = Params {
        count: 5,
        align: false,
        ..params(Kind::Path)
    };
    let all = items(&base, Some(&line), &tol).unwrap();
    let xs: Vec<f64> = all.iter().map(|i| i.position.x).collect();
    // The first item is at the array's position; the others follow the curve.
    for (x, expected) in xs.iter().zip([1.0, 3.5, 6.0, 8.5, 11.0]) {
        assert!((x - expected).abs() < 1e-9, "{xs:?}");
    }
    assert!(all.iter().all(|i| (i.position.y - 2.0).abs() < 1e-9));
    // By spacing: one item per step that fits, the end of the curve included when it is hit.
    let spaced = Params {
        spacing: 3.0,
        ..base.clone()
    };
    assert_eq!(items(&spaced, Some(&line), &tol).unwrap().len(), 4);
    let exact = Params {
        spacing: 2.5,
        ..base
    };
    assert_eq!(items(&exact, Some(&line), &tol).unwrap().len(), 5);
    // Items turn with the tangent: along a quarter circle of radius 10 (length 5π/1... 5π).
    let quarter = Curve2::Arc(Arc2::new(Point2::new(0.0, 0.0), 10.0, 0.0, FRAC_PI_2).unwrap());
    let aligned = Params {
        count: 3,
        align: true,
        position: Point2::new(0.0, 0.0),
        rotation: 0.1,
        ..params(Kind::Path)
    };
    let all = items(&aligned, Some(&quarter), &tol).unwrap();
    for (item, turn) in all.iter().zip([0.0, FRAC_PI_4, FRAC_PI_2]) {
        assert!((item.rotation - 0.1 - turn).abs() < 1e-6, "{item:?}");
    }
    let expected = [
        (0.0, 0.0),
        (10.0 * FRAC_PI_4.cos() - 10.0, 10.0 * FRAC_PI_4.sin()),
        (-10.0, 10.0),
    ];
    for (item, (x, y)) in all.iter().zip(expected) {
        assert!((item.position.x - x).abs() < 1e-6 && (item.position.y - y).abs() < 1e-6);
    }
    // A closed curve is divided evenly all the way round.
    let ring = Curve2::Circle(arc_geom::Circle2::new(Point2::new(0.0, 0.0), 1.0).unwrap());
    let four = Params {
        count: 4,
        align: false,
        position: Point2::new(0.0, 0.0),
        ..params(Kind::Path)
    };
    let all = items(&four, Some(&ring), &tol).unwrap();
    assert!((all[2].position.x + 2.0).abs() < 1e-6 && all[2].position.y.abs() < 1e-6);
    // No curve, or a curve without length, is invalid.
    assert_eq!(
        items(&four, None, &tol).unwrap_err().code(),
        "draft.array.invalid"
    );
    let ray = Curve2::Ray(
        arc_geom::Ray2::new(
            Point2::new(0.0, 0.0),
            arc_base::math::Vector2::new(1.0, 0.0),
        )
        .unwrap(),
    );
    assert_eq!(
        items(&four, Some(&ray), &tol).unwrap_err().code(),
        "draft.array.invalid"
    );
}

#[test]
fn parameters_and_the_item_bound_are_checked() {
    let tol = Tolerance::DEFAULT;
    let huge = Params {
        rows: 101,
        columns: 100,
        ..params(Kind::Rectangular)
    };
    assert_eq!(
        items(&huge, None, &tol).unwrap_err().code(),
        "draft.array.too_many_items"
    );
    let limit = Params {
        rows: 100,
        columns: 100,
        ..params(Kind::Rectangular)
    };
    assert_eq!(items(&limit, None, &tol).unwrap().len() as u64, MAX_ITEMS);
    for bad in [
        Params {
            rows: 0,
            ..params(Kind::Rectangular)
        },
        Params {
            count: 0,
            ..params(Kind::Polar)
        },
        Params {
            total_angle: 0.0,
            ..params(Kind::Polar)
        },
        Params {
            total_angle: f64::NAN,
            ..params(Kind::Polar)
        },
        Params {
            spacing: -1.0,
            ..params(Kind::Path)
        },
        Params {
            row_spacing: f64::INFINITY,
            ..params(Kind::Rectangular)
        },
    ] {
        assert_eq!(
            items(&bad, None, &tol).unwrap_err().code(),
            "draft.array.invalid"
        );
    }

    // Through commands and the document.
    let mut f = Fixture::new();
    let space = f.space("model");
    let line = f.entity(space, curve_json(&segment(0.0, 0.0, 1.0, 0.0)));
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "u", "objects": [s(line)]}),
    );
    let error = f
        .run(
            "core.draft.array.create",
            json!({"owner": s(space), "block": s(block),
                   "array": {"kind": "rectangular", "rows": 200, "columns": 100}}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.array.too_many_items");
    let error = f
        .run(
            "core.draft.array.create",
            json!({"owner": s(space), "block": s(block),
                   "array": {"kind": "polar", "center": [0.0, 0.0], "count": 0}}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.array.invalid");
    // A spacing that makes more items than the bound fails when the list is built.
    let path = f.entity(space, curve_json(&segment(0.0, 0.0, 10.0, 0.0)));
    let a = f.create(
        "core.draft.array.create",
        json!({"owner": s(space), "block": s(block),
               "array": {"kind": "path", "path": s(path), "count": 3}}),
    );
    f.set("core.draft.array.modify", &[a], "spacing", json!(0.0001));
    let error =
        display::entity_list(&f.document.snapshot(), a, &DisplayOptions::default()).unwrap_err();
    assert_eq!(error.code(), "draft.array.too_many_items");
    // Modifying a parameter to an invalid value is refused by the validator.
    f.set("core.draft.array.modify", &[a], "spacing", json!(0.0));
    let error = f.run(
        "core.draft.array.modify",
        json!({"objects": [s(a)], "field": "kind", "value": "rectangular"}),
    );
    assert!(error.is_ok());
    assert_eq!(
        f.run(
            "core.draft.array.modify",
            json!({"objects": [s(a)], "field": "rows", "value": 0}),
        )
        .unwrap_err()
        .code(),
        "doc.value.out_of_range"
    );
}

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

fn add(f: &mut Fixture, space: ObjectId, block: ObjectId, array: Value) -> ObjectId {
    f.create(
        "core.draft.array.create",
        json!({"owner": s(space), "block": s(block), "position": [0.0, 0.0], "array": array}),
    )
}

#[test]
fn an_array_draws_every_item_and_regenerates_alone() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let line = f.entity(space, curve_json(&segment(0.0, 0.0, 1.0, 0.0)));
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "u", "objects": [s(line)]}),
    );
    let first = add(
        &mut f,
        space,
        block,
        json!({"kind": "rectangular", "rows": 2, "columns": 3, "row_spacing": 4.0,
               "column_spacing": 5.0}),
    );
    let second = add(
        &mut f,
        space,
        block,
        json!({"kind": "polar", "center": [0.0, -20.0], "count": 4}),
    );
    let regenerate = |f: &Fixture| -> Vec<ObjectId> {
        runs.lock().unwrap().clear();
        let list = space_list(
            &f.document.snapshot(),
            Lists::Graph(f.document.graph()),
            space,
            &DisplayOptions::default(),
        )
        .unwrap();
        assert!(list.failed.is_empty(), "{:?}", list.failed);
        let mut ids = runs.lock().unwrap().clone();
        ids.sort();
        ids
    };
    // The first evaluation builds the arrays and, once, the entity of the block they insert.
    let mut all = vec![first, second, line];
    all.sort();
    assert_eq!(regenerate(&f), all);
    assert!(regenerate(&f).is_empty());
    // One stroke per item, at the item's place: row-major, x grows by the column spacing.
    let list = space_list(
        &f.document.snapshot(),
        Lists::Direct,
        space,
        &DisplayOptions::default(),
    )
    .unwrap();
    let strokes: Vec<Point2> = list
        .primitives
        .iter()
        .filter(|p| p.source == first)
        .map(|p| match &p.shape {
            Shape::Stroke { curve, .. } => curve.start_point().unwrap(),
            other => panic!("{other:?}"),
        })
        .collect();
    let places: Vec<(f64, f64)> = strokes.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(
        places,
        [
            (0.0, 0.0),
            (5.0, 0.0),
            (10.0, 0.0),
            (0.0, 4.0),
            (5.0, 4.0),
            (10.0, 4.0)
        ]
    );
    assert_eq!(
        list.primitives
            .iter()
            .filter(|p| p.source == second)
            .count(),
        4
    );
    // Editing a parameter regenerates that array only.
    f.set("core.draft.array.modify", &[first], "columns", json!(4));
    assert_eq!(regenerate(&f), [first]);
    f.set("core.draft.array.modify", &[second], "count", json!(6));
    assert_eq!(regenerate(&f), [second]);
    assert!(regenerate(&f).is_empty());
    // Editing the block's entity regenerates the entity and both arrays.
    f.set(
        "core.draft.entity.modify",
        &[line],
        "geometry",
        curve_json(&segment(0.0, 0.0, 2.0, 0.0)),
    );
    assert_eq!(regenerate(&f), all);
}

#[test]
fn a_path_array_follows_its_curve_entity_and_a_stored_curve() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let line = f.entity(space, curve_json(&segment(0.0, 0.0, 1.0, 0.0)));
    let block = f.create(
        "core.draft.block.define",
        json!({"name": "u", "objects": [s(line)]}),
    );
    let path = f.entity(space, curve_json(&segment(0.0, 10.0, 20.0, 10.0)));
    let by_entity = add(
        &mut f,
        space,
        block,
        json!({"kind": "path", "path": s(path), "count": 3, "align": false}),
    );
    let by_curve = add(
        &mut f,
        space,
        block,
        json!({"kind": "path", "curve": curve_json(&segment(0.0, 0.0, 0.0, 8.0)), "spacing": 4.0}),
    );
    let starts = |id: ObjectId| -> Vec<(f64, f64)> {
        display::entity_list(&f.document.snapshot(), id, &DisplayOptions::default())
            .unwrap()
            .primitives
            .iter()
            .map(|p| match &p.shape {
                Shape::Stroke { curve, .. } => {
                    let q = curve.start_point().unwrap();
                    ((q.x * 1e6).round() / 1e6, (q.y * 1e6).round() / 1e6)
                }
                other => panic!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(starts(by_entity), [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0)]);
    // Aligned items turn with the curve (a quarter turn along a vertical path); the block
    // line is rotated about its base point, so its start stays at the item position.
    assert_eq!(starts(by_curve), [(0.0, 0.0), (0.0, 4.0), (0.0, 8.0)]);
    // Deleting the path entity clears the reference; with no stored curve the list fails.
    f.run("core.draft.entity.delete", json!({"objects": [s(path)]}))
        .unwrap();
    let error = display::entity_list(
        &f.document.snapshot(),
        by_entity,
        &DisplayOptions::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "draft.array.invalid");
}
