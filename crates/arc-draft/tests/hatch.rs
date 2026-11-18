//! Hatches (CORE-DRAFT-003) and the hatch pattern library (CORE-DRAFT-020): pattern line
//! generation against analytic properties, the commands, associativity through the dependency
//! graph, the deletion rule, the package round trip and raster output.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test code"
)]

mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use arc_base::math::Point2;
use arc_base::{ObjectId, Tolerance};
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::display::{self, DisplayOptions, Lists, Primitive, Shape, entity_list, space_list};
use arc_draft::hatch::{self, Generated, Mark, Placement, generate};
use arc_draft::hatch_pattern::{PatternLine, library};
use arc_draft::types;
use arc_geom::region::{Containment, Loop2, RegionPart};
use arc_geom::{Circle2, Curve2, Polyline2, PolylineVertex, Region, Segment2};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{Fixture, curve_json};

const TOL: Tolerance = Tolerance::DEFAULT;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn polygon(points: &[(f64, f64)]) -> Curve2 {
    let vertices = points
        .iter()
        .map(|(x, y)| PolylineVertex::new(p(*x, *y), 0.0))
        .collect();
    Curve2::Polyline(Polyline2::new(vertices, true).unwrap())
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

fn loop_of(curve: Curve2) -> Loop2 {
    Loop2::new(vec![curve], &TOL).unwrap()
}

fn region(curve: Curve2) -> Region {
    Region::new(vec![RegionPart::new(loop_of(curve), vec![])]).unwrap()
}

fn ring(outer: Curve2, hole: Curve2) -> Region {
    Region::new(vec![RegionPart::new(loop_of(outer), vec![loop_of(hole)])]).unwrap()
}

fn line(angle: f64, shift: f64, spacing: f64, dashes: &[f64]) -> PatternLine {
    PatternLine {
        angle,
        origin: p(0.0, 0.0),
        shift,
        spacing,
        dashes: dashes.to_vec(),
    }
}

fn place(scale: f64) -> Placement {
    Placement {
        scale,
        angle: 0.0,
        origin: p(0.0, 0.0),
    }
}

fn run(region: &Region, lines: &[PatternLine], placement: &Placement, max: usize) -> Generated {
    generate(region, lines, placement, max, &TOL).unwrap()
}

fn ends(mark: &Mark) -> (Point2, Point2) {
    match mark {
        Mark::Segment(a, b) => (*a, *b),
        Mark::Dot(a) => (*a, *a),
    }
}

fn mid(mark: &Mark) -> Point2 {
    let (a, b) = ends(mark);
    p(0.5 * (a.x + b.x), 0.5 * (a.y + b.y))
}

/// Whether `q` lies on one of `marks`.
fn covered(marks: &[Mark], q: Point2) -> bool {
    marks.iter().any(|m| {
        let (a, b) = ends(m);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let length2 = dx * dx + dy * dy;
        let t = if length2 == 0.0 {
            0.0
        } else {
            (((q.x - a.x) * dx + (q.y - a.y) * dy) / length2).clamp(0.0, 1.0)
        };
        let (fx, fy) = (a.x + t * dx, a.y + t * dy);
        (q.x - fx).hypot(q.y - fy) < 1e-6
    })
}

fn assert_inside(region: &Region, marks: &[Mark]) {
    for m in marks {
        let (a, b) = ends(m);
        for q in [a, mid(m), b] {
            assert_ne!(
                region.classify(q, &TOL).unwrap(),
                Containment::Outside,
                "{m:?}"
            );
        }
    }
}

#[test]
fn continuous_lines_have_the_spacing_and_angle_of_the_family() {
    let r = region(square(0.0, 0.0, 10.0, 7.0));
    let family = line(30.0, 0.0, 1.5, &[]);
    let g = run(&r, &[family], &place(2.0), 10_000);
    assert!(!g.fallback && !g.marks.is_empty());
    assert_inside(&r, &g.marks);
    let (s, c) = 30.0_f64.to_radians().sin_cos();
    let mut offsets = BTreeSet::new();
    for m in &g.marks {
        let (a, b) = ends(m);
        let len = (b.x - a.x).hypot(b.y - a.y);
        // The direction is the family angle.
        assert!(((b.x - a.x) / len - c).abs() < 1e-9 && ((b.y - a.y) / len - s).abs() < 1e-9);
        // The offset from the origin along the normal is a multiple of scale * spacing.
        let n = -s * a.x + c * a.y;
        let k = n / 3.0;
        assert!((k - k.round()).abs() < 1e-9, "{k}");
        offsets.insert(k.round() as i64);
    }
    // Neighboring indices are present with no gap, so the spacing is exactly 3.
    let all: Vec<i64> = offsets.iter().copied().collect();
    assert!(all.windows(2).all(|w| w[1] - w[0] == 1), "{all:?}");
}

#[test]
fn dashes_follow_the_list_and_the_shift_staggers_the_lines() {
    let r = region(square(0.0, 0.0, 12.0, 4.0));
    // Horizontal lines 2 apart, dash 2 gap 1 dot gap 1, each line shifted by 1.
    let g = run(
        &r,
        &[line(0.0, 1.0, 2.0, &[2.0, -1.0, 0.0, -1.0])],
        &place(1.0),
        10_000,
    );
    assert_inside(&r, &g.marks);
    let on_line = |y: f64| -> Vec<&Mark> {
        g.marks
            .iter()
            .filter(|m| (mid(m).y - y).abs() < 1e-9)
            .collect()
    };
    // Line k = 1 (y = 2) starts its list at x = 1: a dash from 1 to 3, a dot at 4, then 5..7.
    let l1 = on_line(2.0);
    let has = |m: &Vec<&Mark>, a: f64, b: f64| {
        m.iter().any(|x| match x {
            Mark::Segment(s, e) => (s.x - a).abs() < 1e-9 && (e.x - b).abs() < 1e-9,
            Mark::Dot(_) => false,
        })
    };
    assert!(has(&l1, 1.0, 3.0) && has(&l1, 5.0, 7.0) && has(&l1, 9.0, 11.0));
    assert!(
        l1.iter()
            .any(|m| matches!(m, Mark::Dot(d) if (d.x - 4.0).abs() < 1e-9))
    );
    // Line k = 2 (y = 4) is on the boundary and draws nothing; line 0 likewise.
    assert!(on_line(0.0).is_empty() && on_line(4.0).is_empty());
}

#[test]
fn holes_are_left_empty_and_all_islands_shapes_work() {
    let outer = square(0.0, 0.0, 20.0, 20.0);
    let hole = Curve2::Circle(Circle2::new(p(10.0, 10.0), 5.0).unwrap());
    let r = ring(outer, hole);
    let g = run(
        &r,
        &[line(0.0, 0.0, 0.7, &[]), line(90.0, 0.0, 0.9, &[1.0, -0.5])],
        &place(1.0),
        100_000,
    );
    assert!(!g.fallback);
    assert_inside(&r, &g.marks);
    for m in &g.marks {
        let q = mid(m);
        assert!((q.x - 10.0).hypot(q.y - 10.0) > 5.0 - 1e-6);
    }
}

#[test]
fn the_phase_follows_the_origin_not_the_region() {
    let dashed = [line(25.0, 0.37, 1.1, &[1.0, -0.4, 0.0, -0.6])];
    let place = Placement {
        scale: 1.3,
        angle: 0.2,
        origin: p(0.3, -0.7),
    };
    let a = region(square(0.0, 0.0, 12.0, 9.0));
    let b = region(square(5.0, 3.0, 17.0, 12.0));
    let (ga, gb) = (
        run(&a, &dashed, &place, 100_000),
        run(&b, &dashed, &place, 100_000),
    );
    // Every dash point of one region inside the other is drawn by the other.
    let (mut shared, mut checked) = (0, 0);
    for m in &ga.marks {
        let q = mid(m);
        if b.classify(q, &TOL).unwrap() == Containment::Inside {
            checked += 1;
            if covered(&gb.marks, q) {
                shared += 1;
            }
        }
    }
    assert!(checked > 10);
    assert_eq!(shared, checked);
}

#[test]
fn a_region_far_from_the_origin_gives_the_translated_pattern() {
    let family = [line(37.0, 0.4, 1.3, &[1.0, -0.5, 0.0, -0.5])];
    let shape = [(0.0, 0.0), (9.0, 1.0), (8.0, 8.0), (4.0, 5.0), (1.0, 9.0)];
    let near = region(polygon(&shape));
    let t = 1e7;
    let moved: Vec<(f64, f64)> = shape.iter().map(|(x, y)| (x + t, y - t)).collect();
    let far = region(polygon(&moved));
    let at = |origin: Point2| Placement {
        scale: 1.0,
        angle: 0.0,
        origin,
    };
    let a = run(&near, &family, &at(p(0.0, 0.0)), 100_000);
    let b = run(&far, &family, &at(p(t, -t)), 100_000);
    assert!(!a.fallback && !b.fallback);
    assert_eq!(a.marks.len(), b.marks.len());
    for (x, y) in a.marks.iter().zip(&b.marks) {
        let ((xa, xb), (ya, yb)) = (ends(x), ends(y));
        for (u, v) in [(xa, ya), (xb, yb)] {
            assert!(
                (u.x + t - v.x).abs() < 1e-5 && (u.y - t - v.y).abs() < 1e-5,
                "{x:?} {y:?}"
            );
        }
    }
    // The same region with the origin left at zero is generated without iterating from it.
    let c = run(&far, &family, &at(p(0.0, 0.0)), 100_000);
    assert!(!c.fallback && !c.marks.is_empty());
    assert_inside(&far, &c.marks);
}

#[test]
fn the_segment_bound_falls_back_to_a_solid_fill() {
    let r = region(square(0.0, 0.0, 100.0, 100.0));
    let dense = run(&r, &[line(0.0, 0.0, 0.01, &[])], &place(1.0), 1000);
    assert!(dense.fallback && dense.marks.is_empty());
    let dashed = run(
        &r,
        &[line(0.0, 0.0, 10.0, &[0.01, -0.01])],
        &place(1.0),
        1000,
    );
    assert!(dashed.fallback);
    // Just inside the bound is generated.
    let ok = run(&r, &[line(0.0, 0.0, 10.0, &[])], &place(1.0), 1000);
    assert!(!ok.fallback && ok.marks.len() == 9);
}

#[test]
fn degenerate_parameters_and_extreme_scales_are_handled() {
    let r = region(square(0.0, 0.0, 1.0, 1.0));
    let f = [line(0.0, 0.0, 1.0, &[1.0, -1.0])];
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let e = generate(&r, &f, &place(scale), 100, &TOL).unwrap_err();
        assert_eq!(e.code(), "draft.hatch.parameter_invalid");
    }
    let bad = [line(0.0, 0.0, 0.0, &[])];
    assert_eq!(
        generate(&r, &bad, &place(1.0), 100, &TOL)
            .unwrap_err()
            .code(),
        "draft.pattern.line_invalid"
    );
    for scale in [1e-300, 1e-100, 1e100, 1e300] {
        // Either a result or a defined error, never a panic.
        let _ = generate(&r, &f, &place(scale), 1000, &TOL);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn generated_marks_lie_inside_the_region(
        angle in -360.0..360.0f64,
        shift in -3.0..3.0f64,
        spacing in prop_oneof![0.3..4.0f64, -4.0..-0.3f64],
        dashes in proptest::collection::vec(prop_oneof![0.1..2.0f64, -2.0..-0.1f64, Just(0.0)], 0..5),
        rotation in -3.0..3.0f64,
        scale in 0.5..3.0f64,
        ox in -50.0..50.0f64,
    ) {
        prop_assume!(dashes.is_empty() || dashes.iter().any(|d| *d != 0.0));
        let r = ring(
            polygon(&[(0.0, 0.0), (14.0, 1.0), (12.0, 11.0), (2.0, 9.0)]),
            Curve2::Circle(Circle2::new(p(7.0, 5.0), 2.0).unwrap()),
        );
        let family = [PatternLine { angle, origin: p(ox, 0.0), shift, spacing, dashes }];
        let placement = Placement { scale, angle: rotation, origin: p(1.0, 2.0) };
        let g = generate(&r, &family, &placement, 100_000, &TOL).unwrap();
        prop_assert!(!g.fallback);
        for m in &g.marks {
            let (a, b) = ends(m);
            for q in [a, mid(m), b] {
                prop_assert_ne!(r.classify(q, &TOL).unwrap(), Containment::Outside);
            }
        }
    }
}

#[test]
fn every_shipped_pattern_generates_inside_a_square_within_the_bound() {
    let file = library().unwrap();
    for pattern in &file.patterns {
        for (size, scale) in [(1.0, 1.0), (30.0, 1.0), (1.0, 0.1)] {
            let r = region(square(0.0, 0.0, size, size));
            let g = run(&r, &pattern.lines, &place(scale), 10_000);
            assert!(!g.fallback, "{} {size} {scale}", pattern.name);
            assert!(g.marks.len() <= 10_000);
            assert_inside(&r, &g.marks);
            if size == 30.0 {
                assert!(!g.marks.is_empty(), "{}", pattern.name);
            }
        }
    }
}

// ----- commands, associativity, deletion, round trip -----

fn s(id: ObjectId) -> String {
    id.to_string()
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
    hatch::register_evaluators(&mut evaluators, &DisplayOptions::default()).unwrap();
    (Fixture::with_evaluators(evaluators), runs)
}

fn list(f: &Fixture, id: ObjectId) -> Vec<Primitive> {
    entity_list(&f.document.snapshot(), id, &DisplayOptions::default())
        .unwrap()
        .primitives
}

fn segments(primitives: &[Primitive]) -> Vec<(Point2, Point2)> {
    primitives
        .iter()
        .filter_map(|q| match &q.shape {
            Shape::Stroke {
                curve: Curve2::Segment(seg),
                ..
            } => Some((seg.start(), seg.end())),
            _ => None,
        })
        .collect()
}

fn inline_family() -> Value {
    json!([{"angle": 0.0, "spacing": 2.0, "dashes": [1.5, -0.5]}])
}

#[test]
fn hatches_are_created_from_curves_objects_and_points_and_modified() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let outer = curve_json(&square(0.0, 0.0, 20.0, 20.0));
    let hole = curve_json(&Curve2::Circle(Circle2::new(p(10.0, 10.0), 4.0).unwrap()));
    let id = f.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "solid", "boundary": [outer, hole]}),
    );
    let snapshot = f.document.snapshot();
    let r = hatch::resolve(&snapshot, id, &DisplayOptions::default()).unwrap();
    assert!(!r.associated);
    assert_eq!(
        (r.region.parts().len(), r.region.parts()[0].holes().len()),
        (1, 1)
    );
    let fill = list(&f, id);
    assert!(matches!(&fill[..], [q] if matches!(q.shape, Shape::Region(_))));

    // The island mode changes what the hatch covers.
    f.set("core.draft.hatch.modify", &[id], "island", json!("ignore"));
    let r = hatch::resolve(&f.document.snapshot(), id, &DisplayOptions::default()).unwrap();
    assert_eq!(r.region.parts()[0].holes().len(), 0);

    // A pattern hatch with an inline pattern, then with a loaded pattern object.
    let pattern = f.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "boundary": [curve_json(&square(0.0, 0.0, 10.0, 10.0))],
               "inline": inline_family(), "scale": 1.0}),
    );
    let strokes = segments(&list(&f, pattern));
    assert!(strokes.len() > 8);
    let loaded = f
        .run(
            "core.draft.hatch_pattern.load",
            json!({"names": ["brick-bond"]}),
        )
        .unwrap();
    let brick: ObjectId = loaded["ids"][0].as_str().unwrap().parse().unwrap();
    f.set(
        "core.draft.hatch.modify",
        &[pattern],
        "pattern",
        json!(s(brick)),
    );
    f.set("core.draft.hatch.modify", &[pattern], "scale", json!(0.5));
    let bricks = segments(&list(&f, pattern));
    assert_ne!(bricks, strokes);
    assert!(bricks.len() > 8);
    assert_eq!(
        f.run("core.draft.hatch_pattern.load", json!({"names": ["nope"]}))
            .unwrap_err()
            .code(),
        "draft.pattern.unknown"
    );
    // A pattern in use cannot be deleted, an unused one can be purged with the library loaded.
    assert!(
        f.run("core.draft.entity.delete", json!({"objects": [s(brick)]}))
            .is_err()
    );
    assert!(
        f.run(
            "core.draft.hatch_pattern.load",
            json!({"names": ["brick-bond"]})
        )
        .is_err(),
        "names are unique"
    );

    // Gradient data is stored; the fill is drawn in the first color.
    let g = f.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "gradient",
               "boundary": [curve_json(&square(0.0, 0.0, 5.0, 5.0))],
               "gradient": {"kind": "radial", "angle": 0.5, "color1": [1.0, 0.0, 0.0, 1.0], "color2": [0.0, 0.0, 1.0, 1.0]}}),
    );
    let q = &list(&f, g)[0];
    assert!(matches!(q.shape, Shape::Region(_)));
    assert_eq!(q.color.as_ref().unwrap().value.rgba.r, 1.0);

    // Rejected input.
    for params in [
        json!({"owner": s(space), "boundary": [curve_json(&square(0.0, 0.0, 5.0, 5.0))]}),
        json!({"owner": s(space), "kind": "solid"}),
        json!({"owner": s(space), "kind": "solid", "boundary": [curve_json(&square(0.0, 0.0, 5.0, 5.0))], "scale": 0.0}),
        json!({"owner": s(space), "kind": "solid", "boundary": [curve_json(&Curve2::Segment(
            Segment2::new(p(0.0, 0.0), p(1.0, 0.0)).unwrap()))]}),
    ] {
        assert!(f.run("core.draft.hatch.create", params).is_err());
    }

    // Package round trip.
    support::round_trip(&f);
}

#[test]
fn picking_a_point_traces_the_region_and_follows_its_objects() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let edges = [
        ((0.0, 0.0), (20.0, 0.0)),
        ((20.0, 0.0), (20.0, 20.0)),
        ((20.0, 20.0), (0.0, 20.0)),
        ((0.0, 20.0), (0.0, 0.0)),
    ];
    for (a, b) in edges {
        f.entity(
            space,
            curve_json(&Curve2::Segment(
                Segment2::new(p(a.0, a.1), p(b.0, b.1)).unwrap(),
            )),
        );
    }
    f.entity(
        space,
        curve_json(&Curve2::Circle(Circle2::new(p(10.0, 10.0), 3.0).unwrap())),
    );
    let mut holes = |mode: &str| -> usize {
        let id = f.create(
            "core.draft.hatch.pick",
            json!({"owner": s(space), "kind": "solid", "pick": [2.0, 2.0], "island": mode}),
        );
        let r = hatch::resolve(&f.document.snapshot(), id, &DisplayOptions::default()).unwrap();
        assert!(r.associated);
        r.region.parts().iter().map(|part| part.holes().len()).sum()
    };
    assert_eq!(holes("alternating"), 1);
    assert_eq!(holes("outermost"), 1);
    assert_eq!(holes("ignore"), 0);
    // Not enclosed.
    let e = f
        .run(
            "core.draft.hatch.pick",
            json!({"owner": s(space), "kind": "solid", "pick": [50.0, 50.0]}),
        )
        .unwrap_err();
    assert_eq!(e.code(), "geom.trace.not_enclosed");
}

#[test]
fn associative_hatches_regenerate_alone_and_keep_their_boundary_when_it_is_deleted() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let a = f.entity(space, curve_json(&square(0.0, 0.0, 10.0, 10.0)));
    let b = f.entity(space, curve_json(&square(50.0, 0.0, 60.0, 10.0)));
    let make = |f: &mut Fixture, object: ObjectId| {
        f.create(
            "core.draft.hatch.create",
            json!({"owner": s(space), "objects": [s(object)], "inline": inline_family()}),
        )
    };
    let (ha, hb) = (make(&mut f, a), make(&mut f, b));
    let regenerate = |f: &mut Fixture| -> BTreeSet<ObjectId> {
        runs.lock().unwrap().clear();
        let snapshot = f.document.snapshot();
        let l = space_list(
            &snapshot,
            Lists::Graph(f.document.graph()),
            space,
            &DisplayOptions::default(),
        )
        .unwrap();
        assert!(l.failed.is_empty(), "{:?}", l.failed);
        runs.lock().unwrap().iter().copied().collect()
    };
    assert_eq!(regenerate(&mut f), BTreeSet::from([a, b, ha, hb]));
    assert!(regenerate(&mut f).is_empty());
    let before = list(&f, ha);
    f.set(
        "core.draft.entity.modify",
        &[a],
        "geometry",
        curve_json(&square(0.0, 0.0, 10.0, 20.0)),
    );
    assert_eq!(regenerate(&mut f), BTreeSet::from([a, ha]));
    let after = list(&f, ha);
    assert!(segments(&after).len() > segments(&before).len());
    // Editing the pattern of one hatch leaves the boundary object and the other hatch alone.
    f.set("core.draft.hatch.modify", &[hb], "scale", json!(2.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([hb]));

    // Deleting the boundary object keeps the last boundary and warns.
    f.run("core.draft.entity.delete", json!({"objects": [s(a)]}))
        .unwrap();
    assert_eq!(list(&f, ha), after);
    let snapshot = f.document.snapshot();
    let r = hatch::resolve(&snapshot, ha, &DisplayOptions::default()).unwrap();
    assert!(!r.associated);
    let found: Vec<_> = f
        .document
        .diagnostics_snapshot()
        .diagnostics
        .iter()
        .filter(|d| d.code() == "draft.hatch.disassociated")
        .map(|d| format!("{:?}", d.error.details().get("id")))
        .collect();
    assert_eq!(found.len(), 1);
    assert!(found[0].contains(&s(ha)));
    assert_eq!(regenerate(&mut f), BTreeSet::from([ha]));
    // Clearing the stale references makes the hatch plainly non-associative again.
    f.set(
        "core.draft.hatch.modify",
        &[ha],
        "boundary_objects",
        json!([]),
    );
    assert!(
        f.document
            .diagnostics_snapshot()
            .diagnostics
            .iter()
            .all(|d| d.code() != "draft.hatch.disassociated")
    );
}

#[test]
fn moving_the_boundary_does_not_shift_the_pattern() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let id = f.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "boundary": [curve_json(&square(0.0, 0.0, 10.0, 10.0))],
               "inline": [{"angle": 20.0, "spacing": 1.7, "shift": 0.3, "dashes": [1.0, -0.6]}],
               "origin": [0.4, 0.1]}),
    );
    let first = segments(&list(&f, id));
    f.set(
        "core.draft.hatch.modify",
        &[id],
        "loops",
        json!([curve_json(&square(4.0, 3.0, 14.0, 13.0))]),
    );
    let moved = segments(&list(&f, id));
    let as_marks = |v: &[(Point2, Point2)]| -> Vec<Mark> {
        v.iter().map(|(a, b)| Mark::Segment(*a, *b)).collect()
    };
    let moved_marks = as_marks(&moved);
    let (mut checked, mut shared) = (0, 0);
    for (a, b) in &first {
        let q = p(0.5 * (a.x + b.x), 0.5 * (a.y + b.y));
        if (4.5..9.5).contains(&q.x) && (3.5..9.5).contains(&q.y) {
            checked += 1;
            shared += usize::from(covered(&moved_marks, q));
        }
    }
    assert!(checked > 5);
    assert_eq!(checked, shared);
}

#[cfg(feature = "raster")]
#[test]
fn a_solid_hatch_with_a_hole_fills_the_ring_and_leaves_the_hole_in_paper_color() {
    use arc_draft::plot::{Offset, Paper, PlotArea, PlotOptions, PlotScale, model_stream};
    use arc_draft::raster::{RasterOptions, render};

    let mut f = Fixture::new();
    let space = f.space("model");
    let id = f.create(
        "core.draft.hatch.create",
        json!({"owner": s(space), "kind": "solid", "boundary": [
            curve_json(&square(0.0, 0.0, 40.0, 40.0)),
            curve_json(&Curve2::Circle(Circle2::new(p(20.0, 20.0), 8.0).unwrap()))]}),
    );
    support::pen(&mut f, id, support::BLACK, 0.25, None);
    let paper = Paper {
        width: 60.0,
        height: 60.0,
        margins: [0.0; 4],
    };
    let options = PlotOptions {
        area: PlotArea::Window {
            min: p(0.0, 0.0),
            max: p(40.0, 40.0),
        },
        scale: PlotScale::Ratio(1.0),
        offset: Offset::Origin(p(10.0, 10.0)),
        ..PlotOptions::default()
    };
    let snapshot = f.document.snapshot();
    let stream = model_stream(
        &snapshot,
        Lists::Direct,
        space,
        &paper,
        &options,
        &DisplayOptions::default(),
    )
    .unwrap();
    let image = render(&stream, &RasterOptions::default()).unwrap();
    let scale = 300.0 / 25.4;
    let at = |x: f64, y: f64| {
        let px = (x * scale) as u32;
        let py = image.height - 1 - (y * scale) as u32;
        image.pixel(px, py).unwrap()
    };
    // In the ring (page 15, 15 is model 5, 5): ink. At the center of the hole and outside: paper.
    assert_eq!(at(15.0, 15.0)[0], 0);
    assert_eq!(at(30.0, 30.0), [255, 255, 255, 255]);
    assert_eq!(at(5.0, 5.0), [255, 255, 255, 255]);
    assert_eq!(at(30.0 + 7.0, 30.0)[0], 255);
    assert_eq!(at(30.0 + 9.0, 30.0)[0], 0);
    assert_eq!(image.skipped, 0);
}
