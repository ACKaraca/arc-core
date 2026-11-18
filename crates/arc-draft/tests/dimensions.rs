//! Dimensions, chains, leaders, multi-leaders and annotation scaling: analytic geometry of
//! every dimension kind, measurement formatting against a reference formatter, associativity
//! through the dependency graph, chains against a spacing model, leader geometry, viewports of
//! different annotation scales and the package round trip.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    reason = "test code"
)]

mod support;

use std::collections::BTreeSet;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
use std::sync::{Arc, Mutex};

use arc_base::ObjectId;
use arc_base::Tolerance;
use arc_base::math::Point2;
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::annotation::{Arrow, Drawn, Part};
use arc_draft::dimension::{
    self, Definition, Kind, Resolution, Sizes, field, geometry, measure, resolved_node,
};
use arc_draft::display::{
    self, DisplayOptions, Lists, Primitive, Shape, entity_list, space_list, viewport_list,
};
use arc_draft::format::{NumberFormat, format_length};
use arc_draft::leader::{Attachment, frame_geometry, leader_geometry, multileader_geometry};
use arc_draft::types;
use arc_geom::{Curve2, Segment2};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{Fixture, curve_json};

const EPS: f64 = 1e-9;
const TOL: Tolerance = Tolerance::DEFAULT;

fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

fn close(a: Point2, b: Point2) -> bool {
    (a - b).norm() < EPS
}

fn sizes() -> Sizes {
    Sizes {
        text_height: 2.5,
        gap: 0.5,
        arrow: Arrow::ClosedFilled,
        arrow_size: 2.0,
        extension_offset: 1.0,
        extension_beyond: 1.5,
        above: false,
    }
}

fn def(kind: Kind, points: &[Point2], line: Point2) -> Definition {
    Definition {
        kind,
        points: points.to_vec(),
        line_position: line,
        text_position: None,
        rotation: 0.0,
    }
}

fn ends(c: &Curve2) -> (Point2, Point2) {
    (c.start_point().unwrap(), c.end_point().unwrap())
}

fn strokes(d: &Drawn, part: Part) -> Vec<(Point2, Point2)> {
    d.strokes
        .iter()
        .filter(|(q, _)| *q == part)
        .map(|(_, c)| ends(c))
        .collect()
}

fn has_segment(list: &[(Point2, Point2)], a: Point2, b: Point2) -> bool {
    list.iter()
        .any(|(x, y)| (close(*x, a) && close(*y, b)) || (close(*x, b) && close(*y, a)))
}

fn tips(d: &Drawn) -> Vec<Point2> {
    d.fills.iter().map(|(_, f)| f.vertices()[0].point).collect()
}

#[test]
fn linear_dimensions_have_analytic_extension_lines_arrows_and_text() {
    let s = sizes();
    let d0 = def(Kind::Linear, &[p(0.0, 0.0), p(30.0, 10.0)], p(5.0, 20.0));
    assert_eq!(measure(&d0, &TOL).unwrap(), 30.0);
    let g = geometry(&d0, &s, 6.0, &TOL).unwrap();
    let ext = strokes(&g, Part::Extension);
    assert!(has_segment(&ext, p(0.0, 1.0), p(0.0, 21.5)), "{ext:?}");
    assert!(has_segment(&ext, p(30.0, 11.0), p(30.0, 21.5)));
    let line = strokes(&g, Part::Line);
    // Broken around the centered text: half of 6 plus the gap.
    assert!(has_segment(&line, p(0.0, 20.0), p(11.5, 20.0)), "{line:?}");
    assert!(has_segment(&line, p(18.5, 20.0), p(30.0, 20.0)));
    assert_eq!(tips(&g), [p(0.0, 20.0), p(30.0, 20.0)]);
    // The arrow at the start points to it from the inside.
    assert!(close(
        g.fills[0].1.vertices()[1].point,
        p(2.0, 20.0 - 2.0 / 6.0)
    ));
    let text = g.text.unwrap();
    assert!(close(text.center, p(15.0, 20.0)) && text.rotation == 0.0);

    // Text above the line and the whole line drawn.
    let above = geometry(&d0, &Sizes { above: true, ..s }, 6.0, &TOL).unwrap();
    assert!(close(
        above.text.unwrap().center,
        p(15.0, 20.0 + 0.5 + 1.25)
    ));
    assert!(has_segment(
        &strokes(&above, Part::Line),
        p(0.0, 20.0),
        p(30.0, 20.0)
    ));

    // Vertical: rotation π/2 measures y.
    let v = Definition {
        rotation: FRAC_PI_2,
        ..d0.clone()
    };
    assert!((measure(&v, &TOL).unwrap() - 10.0).abs() < EPS);
    let g = geometry(&v, &s, 6.0, &TOL).unwrap();
    let t = tips(&g);
    assert!(
        close(t[0], p(5.0, 0.0)) && close(t[1], p(5.0, 10.0)),
        "{t:?}"
    );
    assert!((g.text.unwrap().rotation - FRAC_PI_2).abs() < EPS);

    // A moved text keeps the arrows and draws a leader to it.
    let moved = Definition {
        text_position: Some(p(15.0, 40.0)),
        ..d0
    };
    let g = geometry(&moved, &s, 6.0, &TOL).unwrap();
    assert_eq!(g.text.unwrap().center, p(15.0, 40.0));
    assert!(has_segment(
        &strokes(&g, Part::Line),
        p(15.0, 20.0),
        p(15.0, 36.5)
    ));
}

#[test]
fn aligned_dimensions_too_short_for_their_text_put_arrows_and_text_outside() {
    let d = def(Kind::Aligned, &[p(0.0, 0.0), p(3.0, 4.0)], p(-8.0, 6.0));
    assert!((measure(&d, &TOL).unwrap() - 5.0).abs() < EPS);
    let g = geometry(&d, &sizes(), 6.0, &TOL).unwrap();
    let (a, b) = (p(-8.0, 6.0), p(-5.0, 10.0));
    let t = tips(&g);
    assert!(close(t[0], a) && close(t[1], b), "{t:?}");
    let e = (b - a) / 5.0;
    // Outside: the arrow at `a` points along the line toward `a` from beyond it.
    let base = g.fills[0].1.vertices()[1].point;
    assert!(((base - (a - e * 2.0)).dot(&e)).abs() < EPS);
    assert!(has_segment(
        &strokes(&g, Part::Line),
        a - e * 4.0,
        b + e * 4.0
    ));
    assert!(close(g.text.unwrap().center, b + e * (4.0 + 3.5)));
    assert!(has_segment(
        &strokes(&g, Part::Extension),
        p(-0.8, 0.6),
        a + (a - p(0.0, 0.0)) * 0.15
    ));
    assert!(
        measure(
            &def(Kind::Aligned, &[p(1.0, 1.0), p(1.0, 1.0)], p(0.0, 0.0)),
            &TOL
        )
        .is_err()
    );
}

#[test]
fn angular_dimensions_take_the_sector_of_the_line_position() {
    let s = sizes();
    let three = def(
        Kind::AngularPoints,
        &[p(0.0, 0.0), p(10.0, 0.0), p(0.0, 10.0)],
        p(5.0, 5.0),
    );
    assert!((measure(&three, &TOL).unwrap() - FRAC_PI_2).abs() < EPS);
    let r = 50_f64.sqrt();
    let g = geometry(&three, &s, 3.0, &TOL).unwrap();
    let t = tips(&g);
    assert!(close(t[0], p(r, 0.0)) && close(t[1], p(0.0, r)), "{t:?}");
    let ext = strokes(&g, Part::Extension);
    assert!(
        has_segment(&ext, p(11.0, 0.0), p(r + 1.5, 0.0)) == (r > 10.0),
        "{ext:?}"
    );
    let text = g.text.unwrap();
    assert!(close(
        text.center,
        p(r * FRAC_PI_4.cos(), r * FRAC_PI_4.sin())
    ));
    assert!((text.rotation + FRAC_PI_4).abs() < EPS);
    // The other side: the reflex angle.
    let reflex = Definition {
        line_position: p(-5.0, -5.0),
        ..three
    };
    assert!((measure(&reflex, &TOL).unwrap() - 1.5 * PI).abs() < EPS);

    // Two lines meeting at the origin; the extension lines reach from the nearer ends.
    let lines = def(
        Kind::AngularLines,
        &[p(0.0, 0.0), p(10.0, 0.0), p(0.0, 5.0), p(0.0, 10.0)],
        p(9.0, 12.0),
    );
    assert!((measure(&lines, &TOL).unwrap() - FRAC_PI_2).abs() < EPS);
    let g = geometry(&lines, &s, 3.0, &TOL).unwrap();
    let ext = strokes(&g, Part::Extension);
    assert!(has_segment(&ext, p(11.0, 0.0), p(16.5, 0.0)), "{ext:?}");
    assert!(has_segment(&ext, p(0.0, 11.0), p(0.0, 16.5)));
    let t = tips(&g);
    assert!(close(t[0], p(15.0, 0.0)) && close(t[1], p(0.0, 15.0)));
    let opposite = Definition {
        line_position: p(-3.0, -4.0),
        ..lines.clone()
    };
    let g = geometry(&opposite, &s, 3.0, &TOL).unwrap();
    assert!(has_segment(
        &strokes(&g, Part::Extension),
        p(-1.0, 0.0),
        p(-6.5, 0.0)
    ));
    let parallel = def(
        Kind::AngularLines,
        &[p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0), p(1.0, 1.0)],
        p(1.0, 1.0),
    );
    assert!(measure(&parallel, &TOL).is_err());

    // An arc's angle and length, counterclockwise from start to end.
    let arc = def(
        Kind::AngularArc,
        &[p(0.0, 0.0), p(10.0, 0.0), p(0.0, 10.0)],
        p(20.0, 20.0),
    );
    assert!((measure(&arc, &TOL).unwrap() - FRAC_PI_2).abs() < EPS);
    let length = Definition {
        kind: Kind::ArcLength,
        ..arc.clone()
    };
    assert!((measure(&length, &TOL).unwrap() - 5.0 * PI).abs() < EPS);
    let g = geometry(&length, &s, 3.0, &TOL).unwrap();
    let r = 800_f64.sqrt();
    assert!(has_segment(
        &strokes(&g, Part::Extension),
        p(11.0, 0.0),
        p(r + 1.5, 0.0)
    ));
}

#[test]
fn radial_diameter_and_ordinate_dimensions() {
    let s = sizes();
    let inside = def(Kind::Radial, &[p(0.0, 0.0), p(10.0, 0.0)], p(5.0, 0.0));
    assert_eq!(measure(&inside, &TOL).unwrap(), 10.0);
    let g = geometry(&inside, &s, 6.0, &TOL).unwrap();
    assert_eq!(tips(&g), [p(10.0, 0.0)]);
    assert_eq!(g.fills[0].1.vertices()[1].point.x, 8.0);
    let line = strokes(&g, Part::Line);
    assert!(
        has_segment(&line, p(0.0, 0.0), p(1.5, 0.0))
            && has_segment(&line, p(8.5, 0.0), p(10.0, 0.0))
    );
    assert_eq!(g.text.unwrap().center, p(5.0, 0.0));
    let outside = def(Kind::Radial, &[p(0.0, 0.0), p(10.0, 0.0)], p(20.0, 3.0));
    let g = geometry(&outside, &s, 6.0, &TOL).unwrap();
    assert_eq!(g.fills[0].1.vertices()[1].point.x, 12.0);
    assert!(has_segment(
        &strokes(&g, Part::Line),
        p(10.0, 0.0),
        p(16.5, 0.0)
    ));
    assert_eq!(g.text.unwrap().center, p(20.0, 0.0));

    let dia = def(Kind::Diameter, &[p(0.0, 0.0), p(10.0, 0.0)], p(0.0, 0.0));
    assert_eq!(measure(&dia, &TOL).unwrap(), 20.0);
    let g = geometry(&dia, &s, 6.0, &TOL).unwrap();
    assert_eq!(tips(&g), [p(-10.0, 0.0), p(10.0, 0.0)]);
    assert_eq!(g.text.unwrap().center, p(0.0, 0.0));
    assert!(
        measure(
            &def(Kind::Radial, &[p(0.0, 0.0), p(0.0, 0.0)], p(1.0, 0.0)),
            &TOL
        )
        .is_err()
    );

    let x = def(Kind::OrdinateX, &[p(0.0, 0.0), p(10.0, 5.0)], p(10.0, 20.0));
    assert_eq!(measure(&x, &TOL).unwrap(), 10.0);
    let g = geometry(&x, &s, 6.0, &TOL).unwrap();
    assert_eq!(strokes(&g, Part::Line), [(p(10.0, 6.0), p(10.0, 20.0))]);
    let t = g.text.unwrap();
    assert!(close(t.center, p(10.0, 23.5)) && (t.rotation - FRAC_PI_2).abs() < EPS);
    let y = def(Kind::OrdinateY, &[p(0.0, 0.0), p(10.0, 5.0)], p(25.0, 5.0));
    assert_eq!(measure(&y, &TOL).unwrap(), 5.0);
    let g = geometry(&y, &s, 6.0, &TOL).unwrap();
    assert_eq!(strokes(&g, Part::Line), [(p(11.0, 5.0), p(25.0, 5.0))]);
    assert!(close(g.text.unwrap().center, p(28.5, 5.0)));
    assert!(g.fills.is_empty());
}

/// Decimal digits of `n / 10^precision` written from the integer: the reference formatter.
fn reference(n: i64, precision: u32, lead: bool, trail: bool, sep: char) -> String {
    let p = precision as usize;
    let mut digits = n.unsigned_abs().to_string();
    while digits.len() < p + 1 {
        digits.insert(0, '0');
    }
    let (int, frac) = digits.split_at(digits.len() - p);
    let mut frac = frac.to_owned();
    if trail {
        while frac.ends_with('0') {
            frac.pop();
        }
    }
    let int = if lead && int == "0" && !frac.is_empty() {
        ""
    } else {
        int
    };
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    out.push_str(int);
    if !frac.is_empty() {
        out.push(sep);
        out.push_str(&frac);
    }
    out
}

proptest! {
    #[test]
    fn decimal_formatting_matches_the_reference(
        n in -1_000_000_000_000_i64..1_000_000_000_000,
        precision in 0_u32..=8,
        lead in any::<bool>(),
        trail in any::<bool>(),
        comma in any::<bool>(),
        suffix in any::<bool>(),
    ) {
        let sep = if comma { ',' } else { '.' };
        let format = NumberFormat {
            precision,
            suppress_leading: lead,
            suppress_trailing: trail,
            separator: sep,
            suffix: suffix.then(|| "mm".to_owned()),
            ..NumberFormat::default()
        };
        let value = n as f64 / 10_f64.powi(precision as i32);
        let mut expected = reference(n, precision, lead, trail, sep);
        if suffix {
            expected.push_str(" mm");
        }
        prop_assert_eq!(format_length(value, &format, 1.0), expected);
    }

    #[test]
    fn formatting_and_geometry_survive_extreme_magnitudes(
        x in prop_oneof![-1e300..1e300_f64, -1e-300..1e-300_f64, Just(0.0)],
        y in prop_oneof![-1e300..1e300_f64, -1e-300..1e-300_f64],
        kind in 0_u32..10,
        precision in 0_u32..=8,
        unit_format in 0_u32..5,
    ) {
        let format = NumberFormat {
            precision,
            unit_format: arc_draft::format::UnitFormat::of(unit_format),
            ..NumberFormat::default()
        };
        prop_assert!(!format_length(x, &format, 1.0).is_empty());
        let kind = Kind::of(kind).unwrap();
        let points: Vec<Point2> = (0..kind.points())
            .map(|i| p(x * (i as f64 + 1.0), y * i as f64))
            .collect();
        let d = def(kind, &points, p(y, x));
        // Either works or fails with an error; never panics.
        let _ = measure(&d, &TOL);
        let _ = geometry(&d, &sizes(), 3.0, &TOL);
    }
}

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn seg(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    curve_json(&Curve2::Segment(
        Segment2::new(p(x0, y0), p(x1, y1)).unwrap(),
    ))
}

fn assoc(object: ObjectId, key: &str) -> Value {
    json!({"object": s(object), "key": key})
}

fn aligned(f: &mut Fixture, owner: ObjectId, points: Value, line: [f64; 2]) -> ObjectId {
    f.create(
        "core.draft.dimension.create",
        json!({"owner": s(owner), "kind": "aligned", "points": points, "line_position": line}),
    )
}

/// A document whose display evaluators record every entity they build.
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
    dimension::register_evaluators(&mut evaluators, &DisplayOptions::default()).unwrap();
    arc_draft::style::register_evaluators(&mut evaluators).unwrap();
    (Fixture::with_evaluators(evaluators), runs)
}

fn measurement(f: &Fixture, dim: ObjectId) -> f64 {
    let view = f
        .document
        .graph()
        .evaluate_now(&resolved_node(dim).unwrap())
        .unwrap();
    view.result
        .unwrap()
        .downcast_ref::<Resolution>()
        .unwrap()
        .measurement
}

fn modify(f: &mut Fixture, objects: &[ObjectId], field: &str, value: Value) {
    f.set("core.draft.entity.modify", objects, field, value);
}

fn of(list: &[Primitive], id: ObjectId, part: Part) -> Vec<Primitive> {
    list.iter()
        .filter(|q| q.source == id && q.part == part as u32)
        .cloned()
        .collect()
}

#[test]
fn associative_dimensions_follow_their_geometry_through_the_graph() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let a = f.entity(space, seg(0.0, 0.0, 10.0, 0.0));
    let b = f.entity(space, seg(0.0, 20.0, 30.0, 20.0));
    let dim_a = aligned(
        &mut f,
        space,
        json!([assoc(a, "start"), assoc(a, "end")]),
        [0.0, -5.0],
    );
    let dim_b = aligned(
        &mut f,
        space,
        json!([assoc(b, "start"), assoc(b, "end")]),
        [0.0, 25.0],
    );
    let block = f.create("core.draft.block.define", json!({"name": "part"}));
    let member = f.entity(block, seg(0.0, 0.0, 4.0, 0.0));
    let r = f.create(
        "core.draft.block.insert",
        json!({"owner": s(space), "block": s(block), "position": [100.0, 0.0], "scale": [2.0, 2.0, 1.0]}),
    );
    let path = |key: &str| json!({"object": s(r), "path": [s(member)], "key": key});
    let dim_c = aligned(
        &mut f,
        space,
        json!([path("start"), path("end")]),
        [100.0, -5.0],
    );
    let regenerate = |f: &mut Fixture| -> BTreeSet<ObjectId> {
        runs.lock().unwrap().clear();
        let snapshot = f.document.snapshot();
        let list = space_list(
            &snapshot,
            Lists::Graph(f.document.graph()),
            space,
            &DisplayOptions::default(),
        )
        .unwrap();
        assert!(list.failed.is_empty(), "{:?}", list.failed);
        runs.lock().unwrap().iter().copied().collect()
    };
    assert_eq!(
        regenerate(&mut f),
        BTreeSet::from([a, b, dim_a, dim_b, member, r, dim_c])
    );
    assert!(regenerate(&mut f).is_empty());
    assert_eq!(measurement(&f, dim_a), 10.0);
    assert_eq!(measurement(&f, dim_c), 8.0);

    // Stretch, move and scale the referenced line: only its dimension regenerates.
    modify(&mut f, &[a], "geometry", seg(0.0, 0.0, 20.0, 0.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([a, dim_a]));
    assert_eq!(measurement(&f, dim_a), 20.0);
    modify(&mut f, &[a], "geometry", seg(5.0, 5.0, 25.0, 5.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([a, dim_a]));
    let snapshot = f.document.snapshot();
    let list = entity_list(&snapshot, dim_a, &DisplayOptions::default()).unwrap();
    let arrows: Vec<Point2> = of(&list.primitives, dim_a, Part::Arrow)
        .iter()
        .map(|q| match &q.shape {
            Shape::Fill(f) => f.vertices()[0].point,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(arrows, [p(5.0, -5.0), p(25.0, -5.0)]);
    modify(&mut f, &[a], "geometry", seg(10.0, 10.0, 50.0, 10.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([a, dim_a]));
    assert_eq!(measurement(&f, dim_a), 40.0);

    // Editing the block member moves the dimension that reaches it through the reference.
    modify(&mut f, &[member], "geometry", seg(0.0, 0.0, 5.0, 0.0));
    assert_eq!(regenerate(&mut f), BTreeSet::from([member, r, dim_c]));
    assert_eq!(measurement(&f, dim_c), 10.0);

    // Deleting the line leaves the dimension with its last geometry and a diagnostic.
    let before = entity_list(&f.document.snapshot(), dim_a, &DisplayOptions::default()).unwrap();
    f.run("core.draft.entity.delete", json!({"objects": [s(a)]}))
        .unwrap();
    let after = entity_list(&f.document.snapshot(), dim_a, &DisplayOptions::default()).unwrap();
    assert_eq!(before, after);
    assert_eq!(measurement(&f, dim_a), 40.0);
    let points = dimension::points_of(&f.values(dim_a));
    assert!(
        points
            .iter()
            .all(|dp| dp.object.is_none() && dp.is_disassociated())
    );
    let diagnostics = f.document.diagnostics_snapshot();
    let found: Vec<_> = diagnostics
        .diagnostics
        .iter()
        .filter(|d| d.code() == "draft.dimension.disassociated")
        .collect();
    assert_eq!(found.len(), 1);
    assert!(format!("{:?}", found[0].error.details().get("id")).contains(&s(dim_a)));
    assert_eq!(regenerate(&mut f), BTreeSet::from([dim_a]));
    // A wrong key is refused at creation.
    assert!(
        f.run(
            "core.draft.dimension.create",
            json!({"owner": s(space), "kind": "radial", "points": [assoc(b, "center"), assoc(b, "end")],
                   "line_position": [0.0, 0.0]})
        )
        .is_err()
    );
}

#[test]
fn chains_keep_their_spacing_when_members_change() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let linear = |f: &mut Fixture, x: f64, y: f64| {
        f.create(
            "core.draft.dimension.create",
            json!({"owner": s(space), "kind": "linear",
                   "points": [{"position": [0.0, 0.0]}, {"position": [x, 0.0]}],
                   "line_position": [0.0, y]}),
        )
    };
    let dims: Vec<ObjectId> = (1..=4)
        .map(|i| linear(&mut f, 10.0 * f64::from(i), -3.0 * f64::from(i)))
        .collect();
    let y = |f: &Fixture, d: ObjectId| match &f.values(d)[field::LINE_POSITION] {
        Some(arc_doc::Value::Point2(q)) => q.y,
        other => panic!("{other:?}"),
    };
    let created = f
        .run(
            "core.draft.chain.create",
            json!({"kind": "baseline", "members": [s(dims[0]), s(dims[1]), s(dims[2])], "spacing": 7.0}),
        )
        .unwrap();
    let chain: ObjectId = created["id"].as_str().unwrap().parse().unwrap();
    // The model: the first member's offset, then one spacing farther per member.
    let check = |f: &Fixture, members: &[ObjectId]| {
        for (i, d) in members.iter().enumerate() {
            assert!((y(f, *d) - (-3.0 - 7.0 * i as f64)).abs() < EPS, "{i}");
        }
    };
    check(&f, &dims[..3]);
    f.run(
        "core.draft.chain.add",
        json!({"chain": s(chain), "dimension": s(dims[3])}),
    )
    .unwrap();
    check(&f, &dims);
    f.run(
        "core.draft.chain.remove",
        json!({"chain": s(chain), "dimension": s(dims[1])}),
    )
    .unwrap();
    check(&f, &[dims[0], dims[2], dims[3]]);
    f.run(
        "core.draft.chain.respace",
        json!({"chain": s(chain), "spacing": 2.0}),
    )
    .unwrap();
    assert!((y(&f, dims[3]) - (-7.0)).abs() < EPS);
    // A member twice, or a dimension that is not linear, is refused.
    assert!(
        f.run(
            "core.draft.chain.add",
            json!({"chain": s(chain), "dimension": s(dims[0])})
        )
        .is_err()
    );

    let continued = f
        .run(
            "core.draft.chain.create",
            json!({"kind": "continued", "members": [s(dims[1]), s(dims[3])]}),
        )
        .unwrap();
    let _ = continued;
    assert_eq!(y(&f, dims[3]), y(&f, dims[1]));
    support::round_trip(&f);
}

#[test]
fn leaders_put_arrow_tips_landings_and_content_where_they_belong() {
    let g = leader_geometry(
        &[p(0.0, 0.0), p(10.0, 10.0), p(20.0, 10.0)],
        Arrow::ClosedFilled,
        2.5,
        2.5,
    )
    .unwrap();
    assert_eq!(tips(&g), [p(0.0, 0.0)]);
    assert_eq!(strokes(&g, Part::Landing), [(p(20.0, 10.0), p(22.5, 10.0))]);
    let back = leader_geometry(&[p(0.0, 0.0), p(-10.0, 10.0)], Arrow::Open, 2.5, 1.0).unwrap();
    assert_eq!(
        strokes(&back, Part::Landing),
        [(p(-10.0, 10.0), p(-11.0, 10.0))]
    );
    assert!(leader_geometry(&[p(1.0, 1.0), p(1.0, 1.0)], Arrow::Dot, 1.0, 0.0).is_err());

    let m = multileader_geometry(
        &[vec![p(0.0, 0.0)], vec![p(0.0, 10.0), p(5.0, 10.0)]],
        p(20.0, 5.0),
        1.0,
        2.5,
        0.5,
        Arrow::ClosedFilled,
        2.0,
    )
    .unwrap();
    assert_eq!(m.anchor, p(20.5, 5.0));
    assert_eq!(
        strokes(&m.drawn, Part::Landing),
        [(p(17.5, 5.0), p(20.0, 5.0))]
    );
    let lines = strokes(&m.drawn, Part::Line);
    assert!(lines.iter().all(|(_, e)| *e == p(17.5, 5.0)));
    assert_eq!(tips(&m.drawn), [p(0.0, 0.0), p(0.0, 10.0)]);
    let (boxes, _) =
        frame_geometry(&[vec![1.0]], 2.0, 0.5, m.anchor, 1.0, Attachment::Middle).unwrap();
    let bb = boxes[0].bounding_box();
    assert!(close(bb.min(), p(20.5, 3.5)) && close(bb.max(), p(22.5, 6.5)));

    // Through the display list: text content starts past the landing, centered on it.
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let ml = f.create(
        "core.draft.multileader.create",
        json!({"owner": s(space), "leaders": [[[0.0, 0.0]], [[0.0, 10.0]]], "position": [20.0, 5.0],
               "text": "AB"}),
    );
    let tol_ml = f.create(
        "core.draft.multileader.create",
        json!({"owner": s(space), "leaders": [[[0.0, -20.0]]], "position": [20.0, -20.0],
               "direction": "left", "content": "tolerance", "tolerance": [["0.1", "A"]], "attachment": "top"}),
    );
    let leader = f.create(
        "core.draft.leader.create",
        json!({"owner": s(space), "points": [[0.0, 0.0], [5.0, 5.0]], "arrow": "dot"}),
    );
    let snapshot = f.document.snapshot();
    let list = space_list(
        &snapshot,
        Lists::Graph(f.document.graph()),
        space,
        &DisplayOptions::default(),
    )
    .unwrap();
    assert!(list.failed.is_empty());
    let bounds = |prims: &[Primitive]| {
        let mut b: Option<arc_geom::Aabb2> = None;
        for q in prims {
            if let Shape::Stroke { curve, .. } = &q.shape {
                let c = curve.bounding_box().unwrap();
                b = Some(b.map_or(c, |x| x.union(&c)));
            }
        }
        b.unwrap()
    };
    let text = bounds(&of(&list.primitives, ml, Part::Text));
    assert!(
        text.min().x >= 20.625 - 1e-6 && text.min().x < 21.5,
        "{text:?}"
    );
    assert!(text.min().y < 5.0 && text.max().y > 5.0);
    let frame = bounds(&of(&list.primitives, tol_ml, Part::Frame));
    assert!((frame.max().x - (20.0 - 0.625)).abs() < 1e-9 && (frame.max().y + 20.0).abs() < 1e-9);
    assert!(!of(&list.primitives, tol_ml, Part::Text).is_empty());
    assert_eq!(of(&list.primitives, leader, Part::Arrow).len(), 1);
    support::round_trip(&f);
}

#[test]
fn viewports_show_the_representation_of_their_annotation_scale_at_paper_size() {
    let mut f = Fixture::drafting();
    let model = f.space("model");
    let dim = f.create(
        "core.draft.dimension.create",
        json!({"owner": s(model), "kind": "linear",
               "points": [{"position": [0.0, 0.0]}, {"position": [4000.0, 0.0]}],
               "line_position": [0.0, -500.0], "annotative": true, "scales": [0.01, 0.02]}),
    );
    // The 1:50 representation sits elsewhere.
    f.set(
        "core.draft.entity.modify",
        &[dim],
        "scales",
        json!([[0.01, null, null], [0.02, [0.0, -800.0], null]]),
    );
    let created = f
        .run(
            "core.draft.layout.create",
            json!({"name": "sheet", "paper_width": 420.0, "paper_height": 297.0}),
        )
        .unwrap();
    let sheet: ObjectId = created["space"].as_str().unwrap().parse().unwrap();
    let viewport = |f: &mut Fixture, scale: f64, annotation: f64| {
        let vp = f.create(
            "core.draft.viewport.create",
            json!({"layout": s(sheet), "model": s(model), "center": [200.0, 150.0], "width": 400.0,
                   "height": 280.0, "view_center": [2000.0, 0.0], "scale": scale}),
        );
        f.set(
            "core.draft.viewport.modify",
            &[vp],
            "annotation_scale",
            json!(annotation),
        );
        vp
    };
    let vps = [
        viewport(&mut f, 0.01, 0.01),
        viewport(&mut f, 0.02, 0.02),
        viewport(&mut f, 0.01, 0.05),
    ];
    let snapshot = f.document.snapshot();
    let options = DisplayOptions::default();
    for (i, vp) in vps.iter().enumerate() {
        let list =
            viewport_list(&snapshot, Lists::Graph(f.document.graph()), *vp, &options).unwrap();
        let mine: Vec<_> = list.primitives.iter().filter(|q| q.source == dim).collect();
        if i == 2 {
            assert!(mine.is_empty());
            continue;
        }
        let scale = [0.01, 0.02][i];
        assert!(mine.iter().all(|q| q.annotation_scale == Some(scale)));
        // The arrowhead is 2.5 paper millimeters long.
        for q in mine.iter().filter(|q| q.part == Part::Arrow as u32) {
            let Shape::Fill(region) = &q.shape else {
                panic!()
            };
            let v = region.vertices();
            let base = Point2::from((v[1].point.coords + v[2].point.coords) / 2.0);
            assert!(((v[0].point - base).norm() - 2.5).abs() < 1e-6);
        }
        let text: Vec<Primitive> = mine
            .iter()
            .filter(|q| q.part == Part::Text as u32)
            .map(|q| (*q).clone())
            .collect();
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for q in &text {
            let Shape::Stroke { curve, .. } = &q.shape else {
                panic!()
            };
            let b = curve.bounding_box().unwrap();
            lo = lo.min(b.min().y);
            hi = hi.max(b.max().y);
        }
        assert!((hi - lo - 2.5).abs() < 0.2, "{}", hi - lo);
        // The dimension line: at the representation's position on the sheet.
        let line_y = [150.0 - 5.0, 150.0 - 16.0][i];
        let ext = mine.iter().filter(|q| q.part == Part::Arrow as u32).count();
        assert_eq!(ext, 2);
        let tip = mine
            .iter()
            .find_map(|q| match &q.shape {
                Shape::Fill(r) if q.part == Part::Arrow as u32 => Some(r.vertices()[0].point),
                _ => None,
            })
            .unwrap();
        assert!((tip.y - line_y).abs() < 1e-6, "{tip:?}");
    }
    // Model space shows the representation of the current annotation scale.
    let at = |scale: f64| {
        let options = DisplayOptions {
            annotation_scale: scale,
            ..DisplayOptions::default()
        };
        space_list(&snapshot, Lists::Direct, model, &options)
            .unwrap()
            .primitives
            .len()
    };
    assert!(at(0.01) > 0 && at(0.02) > 0);
    assert_eq!(at(1.0), 0);
    // Adding a scale keeps the existing representations.
    f.run(
        "core.draft.annotation.scales",
        json!({"objects": [s(dim)], "add": [0.05, 0.02]}),
    )
    .unwrap();
    let reps = arc_draft::annotation::representations(f.values(dim)[field::SCALES].as_ref());
    assert_eq!(reps.len(), 3);
    assert_eq!(reps[1].position, Some(p(0.0, -800.0)));
    support::round_trip(&f);
}

#[test]
fn every_kind_is_created_drawn_and_round_tripped() {
    let mut f = Fixture::drafting();
    let space = f.space("model");
    let circle = f.entity(
        space,
        json!({"kind": "circle", "center": [0.0, 0.0], "radius": 10.0}),
    );
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "dimension", "name": "iso"}),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "tolerance",
        json!("deviation"),
    );
    f.set(
        "core.draft.style.modify",
        &[style],
        "tolerance_upper",
        json!(0.1),
    );
    let free = |x: f64, y: f64| json!({"position": [x, y]});
    let cases = [
        ("linear", json!([free(0.0, 0.0), free(10.0, 0.0)])),
        ("aligned", json!([free(0.0, 0.0), free(10.0, 5.0)])),
        (
            "angular_lines",
            json!([
                free(0.0, 0.0),
                free(10.0, 0.0),
                free(0.0, 0.0),
                free(0.0, 10.0)
            ]),
        ),
        (
            "angular_points",
            json!([free(0.0, 0.0), free(10.0, 0.0), free(0.0, 10.0)]),
        ),
        (
            "angular_arc",
            json!([assoc(circle, "center"), free(10.0, 0.0), free(0.0, 10.0)]),
        ),
        (
            "radial",
            json!([
                assoc(circle, "center"),
                json!({"object": s(circle), "key": "quadrant", "quadrant": 1})
            ]),
        ),
        (
            "diameter",
            json!([
                assoc(circle, "center"),
                json!({"object": s(circle), "key": "parameter", "parameter": 1.0})
            ]),
        ),
        (
            "arc_length",
            json!([free(0.0, 0.0), free(10.0, 0.0), free(0.0, 10.0)]),
        ),
        ("ordinate_x", json!([free(0.0, 0.0), free(10.0, 5.0)])),
        ("ordinate_y", json!([free(0.0, 0.0), free(10.0, 5.0)])),
    ];
    for (kind, points) in cases {
        let d = f.create(
            "core.draft.dimension.create",
            json!({"owner": s(space), "kind": kind, "points": points, "line_position": [7.0, 7.0],
                   "style": s(style), "text_override": "~{}"}),
        );
        let list = entity_list(&f.document.snapshot(), d, &DisplayOptions::default()).unwrap();
        assert!(
            list.primitives.iter().any(|q| q.part == Part::Text as u32),
            "{kind}"
        );
    }
    let radial = dimension::resolve(&f.document.snapshot(), circle, &DisplayOptions::default());
    assert!(radial.is_err());
    // A wrong point count is refused by the type validator.
    assert!(
        f.run(
            "core.draft.dimension.create",
            json!({"owner": s(space), "kind": "linear", "points": [free(0.0, 0.0)], "line_position": [0.0, 0.0]})
        )
        .is_err()
    );
    support::round_trip(&f);
}
