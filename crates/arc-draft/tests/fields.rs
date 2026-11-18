//! Fields (CORE-DRAFT-007): evaluation from every source kind, updates through the dependency
//! graph, the marker and warning of a missing source, and the placeholder rules.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use std::sync::{Arc, Mutex};

use arc_base::ObjectId;
use arc_base::math::Point2;
use arc_doc::TypeKey;
use arc_doc::graph::{DerivedKey, Evaluators};
use arc_draft::display::{self, DisplayOptions};
use arc_draft::fields::{self, Evaluated, MISSING_MARKER};
use arc_draft::types;
use arc_geom::{Circle2, Curve2, Segment2};
use serde_json::{Value, json};
use support::{Fixture, curve_json};

fn s(id: ObjectId) -> String {
    id.to_string()
}

fn segment(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    curve_json(&Curve2::Segment(
        Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap(),
    ))
}

fn circle(r: f64) -> Value {
    curve_json(&Curve2::Circle(
        Circle2::new(Point2::new(0.0, 0.0), r).unwrap(),
    ))
}

fn text(
    f: &mut Fixture,
    owner: ObjectId,
    layer: Option<ObjectId>,
    content: &str,
    fields: Value,
) -> ObjectId {
    let mut params = json!({"owner": s(owner), "content": content, "position": [0.0, 0.0],
                            "fields": fields});
    if let Some(layer) = layer {
        params["layer"] = json!(s(layer));
    }
    f.create("core.draft.text.create", params)
}

fn shown(f: &Fixture, id: ObjectId) -> String {
    let e = fields::resolve(&f.document.snapshot(), id, &DisplayOptions::default()).unwrap();
    e.paragraphs
        .iter()
        .flat_map(|p| p.runs.iter().map(|r| r.text.as_str()))
        .collect()
}

fn object_field(object: ObjectId, path: &str, precision: u32) -> Value {
    json!({"offset": 0, "precision": precision,
           "source": {"kind": "object", "object": s(object), "path": path}})
}

fn warnings(f: &Fixture) -> Vec<String> {
    f.document
        .diagnostics_snapshot()
        .diagnostics
        .iter()
        .map(|d| d.code().to_owned())
        .collect()
}

#[test]
fn every_source_kind_evaluates() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let layer = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let line = f.entity(space, segment(0.0, 0.0, 3.0, 4.0));
    let round = f.entity(space, circle(2.0));

    // Document properties.
    let t = text(
        &mut f,
        space,
        Some(layer),
        "Layer: , space: ",
        json!([
            {"offset": 7, "source": {"kind": "document", "property": "layer_name"}},
            {"offset": 16, "source": {"kind": "document", "property": "space_name"}}
        ]),
    );
    assert_eq!(shown(&f, t), "Layer: walls, space: model");

    // Properties of objects: lengths and areas through arc-geom, and stored fields.
    for (path, precision, expected) in [("length", 3, "5.000"), ("field.linetype_scale", 1, "1.0")]
    {
        let t = text(
            &mut f,
            space,
            None,
            "",
            json!([object_field(line, path, precision)]),
        );
        assert_eq!(shown(&f, t), expected, "{path}");
    }
    let area = text(
        &mut f,
        space,
        None,
        " mm2",
        json!([object_field(round, "area", 2)]),
    );
    assert_eq!(shown(&f, area), "12.57 mm2");
    let name = text(
        &mut f,
        space,
        None,
        "",
        json!([object_field(layer, "field.name", 2)]),
    );
    assert_eq!(shown(&f, name), "walls");

    // Expressions over named properties; the result may be a number or a text.
    let binding = |name: &str, object: ObjectId, path: &str| json!({"name": name, "object": s(object), "path": path});
    let expression = |expression: &str, bindings: Vec<Value>| {
        json!({"offset": 0, "source": {"kind": "expression", "expression": expression,
                                        "bindings": bindings}})
    };
    let t = text(
        &mut f,
        space,
        None,
        "",
        json!([
            expression("len * 2 + 1", vec![binding("len", line, "length")]),
            expression(
                "concat(\"on \", n)",
                vec![binding("n", layer, "field.name")]
            ),
            expression(
                "round(a / b, 0.5)",
                vec![binding("a", round, "area"), binding("b", line, "length")]
            )
        ]),
    );
    assert_eq!(shown(&f, t), "11.00on walls2.50");
    assert!(warnings(&f).is_empty(), "{:?}", warnings(&f));

    // Rich text: a field in a run of its own.
    let rich = f.create(
        "core.draft.rich_text.create",
        json!({"owner": s(space), "position": [0.0, 0.0], "paragraphs": [
            {"runs": [{"text": "Length = ", "bold": true},
                      {"text": " m", "fields": [object_field(line, "length", 1)]}]}]}),
    );
    assert_eq!(shown(&f, rich), "Length = 5.0 m");
}

#[test]
fn placeholder_rules_are_enforced() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let layer = f.create("core.draft.layer.create", json!({"name": "walls"}));
    let doc = |offset: u32| json!({"offset": offset, "source": {"kind": "document", "property": "layer_name"}});
    for fields in [
        json!([doc(5)]),
        json!([doc(2), doc(1)]),
        json!([{"offset": 0, "precision": 9,
                "source": {"kind": "document", "property": "layer_name"}}]),
    ] {
        let error = f
            .run(
                "core.draft.text.create",
                json!({"owner": s(space), "content": "abc", "position": [0.0, 0.0],
                       "fields": fields, "layer": s(layer)}),
            )
            .unwrap_err();
        assert_eq!(error.code(), "draft.field.placeholder_invalid", "{fields}");
    }
    // An offset inside a multi-byte character is refused too.
    let error = f
        .run(
            "core.draft.text.create",
            json!({"owner": s(space), "content": "ğ", "position": [0.0, 0.0],
                   "fields": [doc(1)]}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.field.placeholder_invalid");
    // The same rule holds in rich text runs.
    let error = f
        .run(
            "core.draft.rich_text.create",
            json!({"owner": s(space), "position": [0.0, 0.0],
                   "paragraphs": [{"runs": [{"text": "ab", "fields": [doc(3)]}]}]}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "draft.field.placeholder_invalid");
}

/// A document whose fields and display lists are evaluated by the graph, counting how often each
/// node runs.
type Runs = Arc<Mutex<Vec<(&'static str, ObjectId)>>>;

fn counting() -> (Fixture, Runs) {
    let runs = Arc::new(Mutex::new(Vec::new()));
    let options = DisplayOptions::default();
    let mut evaluators = Evaluators::new();
    for key in types::ENTITIES {
        let inner = display::evaluator(options.clone());
        let runs = Arc::clone(&runs);
        evaluators
            .register_object(
                TypeKey::new(*key).unwrap(),
                DerivedKey::new(display::DISPLAY).unwrap(),
                move |ctx| {
                    runs.lock()
                        .unwrap()
                        .push(("display", ctx.object_id().unwrap()));
                    inner(ctx)
                },
            )
            .unwrap();
    }
    for key in [arc_draft::text::TEXT, arc_draft::text::RICH_TEXT] {
        let inner = fields::evaluator(&options);
        let runs = Arc::clone(&runs);
        evaluators
            .register_object(
                TypeKey::new(key).unwrap(),
                DerivedKey::new(fields::FIELDS).unwrap(),
                move |ctx| {
                    runs.lock()
                        .unwrap()
                        .push(("fields", ctx.object_id().unwrap()));
                    inner(ctx)
                },
            )
            .unwrap();
    }
    (Fixture::with_evaluators(evaluators), runs)
}

fn node_text(f: &Fixture, id: ObjectId) -> String {
    let view = f
        .document
        .graph()
        .evaluate_now(&fields::node(id).unwrap())
        .unwrap();
    let value = view.result.unwrap();
    let evaluated = value.downcast_ref::<Evaluated>().unwrap();
    evaluated.paragraphs[0].runs[0].text.clone()
}

#[test]
fn a_field_updates_through_the_graph_when_its_source_changes() {
    let (mut f, runs) = counting();
    let space = f.space("model");
    let line = f.entity(space, segment(0.0, 0.0, 3.0, 4.0));
    let other = f.entity(space, segment(0.0, 0.0, 1.0, 0.0));
    let t = text(
        &mut f,
        space,
        None,
        "L=",
        json!([{"offset": 2, "precision": 1,
        "source": {"kind": "object", "object": s(line), "path": "length"}}]),
    );
    let count = |kind: &str, id: ObjectId| {
        runs.lock()
            .unwrap()
            .iter()
            .filter(|(k, o)| *k == kind && *o == id)
            .count()
    };
    assert_eq!(node_text(&f, t), "L=5.0");
    assert_eq!(count("fields", t), 1);
    // A clean node is not evaluated again.
    assert_eq!(node_text(&f, t), "L=5.0");
    assert_eq!(count("fields", t), 1);
    // Changing an object the field does not read leaves it clean.
    f.set(
        "core.draft.entity.modify",
        &[other],
        "geometry",
        segment(0.0, 0.0, 2.0, 0.0),
    );
    assert_eq!(node_text(&f, t), "L=5.0");
    assert_eq!(count("fields", t), 1);
    // Changing the source evaluates it once and the new text follows.
    f.set(
        "core.draft.entity.modify",
        &[line],
        "geometry",
        segment(0.0, 0.0, 6.0, 8.0),
    );
    assert_eq!(node_text(&f, t), "L=10.0");
    assert_eq!(count("fields", t), 2);
    // The display list reads the evaluated text: it regenerates with the field, once.
    let snapshot = f.document.snapshot();
    let before = count("display", t);
    display::space_list(
        &snapshot,
        display::Lists::Graph(f.document.graph()),
        space,
        &DisplayOptions::default(),
    )
    .unwrap();
    assert_eq!(count("display", t), before + 1);
    f.set(
        "core.draft.entity.modify",
        &[line],
        "geometry",
        segment(0.0, 0.0, 3.0, 4.0),
    );
    display::space_list(
        &f.document.snapshot(),
        display::Lists::Graph(f.document.graph()),
        space,
        &DisplayOptions::default(),
    )
    .unwrap();
    assert_eq!(count("display", t), before + 2);
    assert_eq!(count("fields", t), 3);
}

#[test]
fn a_missing_source_shows_the_marker_and_raises_a_warning() {
    let mut f = Fixture::new();
    let space = f.space("model");
    let line = f.entity(space, segment(0.0, 0.0, 3.0, 4.0));
    let t = text(
        &mut f,
        space,
        None,
        "L=",
        json!([{"offset": 2, "source": {"kind": "object", "object": s(line), "path": "length"}},
               {"offset": 2, "source": {"kind": "document", "property": "layer_name"}},
               {"offset": 2, "source": {"kind": "expression", "expression": "x +",
                                        "bindings": []}}]),
    );
    // The entity has no layer, and the expression is malformed: two markers already.
    assert_eq!(
        shown(&f, t),
        format!("L=5.00{MISSING_MARKER}{MISSING_MARKER}")
    );
    let mut codes = warnings(&f);
    codes.sort();
    assert_eq!(
        codes,
        [
            "draft.field.expression_invalid",
            "draft.field.source_missing"
        ]
    );
    // Deleting the object a field reads clears the reference: the marker replaces the value.
    f.run("core.draft.entity.delete", json!({"objects": [s(line)]}))
        .unwrap();
    assert_eq!(
        shown(&f, t),
        format!("L={MISSING_MARKER}{MISSING_MARKER}{MISSING_MARKER}")
    );
    let codes = warnings(&f);
    assert_eq!(
        codes
            .iter()
            .filter(|c| *c == "draft.field.source_missing")
            .count(),
        2
    );
    // An expression whose binding lost its object is missing, not invalid.
    let line2 = f.entity(space, segment(0.0, 0.0, 3.0, 4.0));
    let e = text(
        &mut f,
        space,
        None,
        "",
        json!([{"offset": 0, "source": {"kind": "expression", "expression": "a * 2",
                 "bindings": [{"name": "a", "object": s(line2), "path": "length"}]}}]),
    );
    assert_eq!(shown(&f, e), "10.00");
    f.run("core.draft.entity.delete", json!({"objects": [s(line2)]}))
        .unwrap();
    assert_eq!(shown(&f, e), MISSING_MARKER);
    assert!(
        warnings(&f)
            .iter()
            .filter(|c| *c == "draft.field.source_missing")
            .count()
            >= 3
    );
    // The display list still builds: the marker is drawn.
    let list = display::entity_list(&f.document.snapshot(), e, &DisplayOptions::default()).unwrap();
    assert!(!list.primitives.is_empty());
}
