//! Plot streams built through the public `arc-draft` types, and report validation.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::panic,
    clippy::collapsible_if,
    reason = "test helpers"
)]

use arc_base::Tolerance;
use arc_base::id::IdGenerator;
use arc_base::math::{Point2, Transform2};
use arc_base::{Color, ObjectId};
use arc_draft::clip::piece;
use arc_draft::plot::{Item, ItemKind, PlotStream};
use arc_draft::plot_style::{EndStyle, JoinStyle, Pen};
use arc_geom::{
    Arc2, Circle2, Curve2, Loop2, Nurbs2, Polyline2, PolylineVertex, Region, RegionPart, Segment2,
};
use serde_json::Value as Json;

pub const PAPER: [u8; 4] = [230, 230, 200, 255];

pub fn p(x: f64, y: f64) -> Point2 {
    Point2::new(x, y)
}

pub fn pen(color: Color, width: f64) -> Pen {
    Pen {
        color,
        width,
        cap: EndStyle::Butt,
        join: JoinStyle::Miter,
        linetype: None,
        linetype_overridden: false,
    }
}

pub fn ids(n: usize) -> Vec<ObjectId> {
    let mut g = IdGenerator::seeded(7);
    (0..n).map(|_| g.generate::<ObjectId>()).collect()
}

pub fn segment(a: Point2, b: Point2) -> Curve2 {
    Curve2::Segment(Segment2::new(a, b).unwrap())
}

pub fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Polyline2 {
    let v = [p(x0, y0), p(x1, y0), p(x1, y1), p(x0, y1)];
    Polyline2::new(
        v.iter().map(|q| PolylineVertex::new(*q, 0.0)).collect(),
        true,
    )
    .unwrap()
}

pub fn item(source: ObjectId, kind: ItemKind) -> Item {
    Item {
        source,
        part: 0,
        layer: None,
        clip: None,
        kind,
    }
}

pub fn stroke(source: ObjectId, curve: Curve2, pen: Pen) -> Item {
    item(
        source,
        ItemKind::Stroke {
            curve,
            pieces: None,
            pen,
        },
    )
}

pub fn glyphs(source: ObjectId) -> Item {
    item(
        source,
        ItemKind::Glyphs {
            font: arc_draft::fonts::OutlineFont {
                family: "Sample".into(),
                data: std::sync::Arc::from(&[][..]),
                index: 0,
            },
            size: 3.0,
            glyphs: vec![],
            placement: Transform2::identity(),
            pen: pen(Color::BLACK, 0.25),
        },
    )
}

pub fn empty(width: f64, height: f64) -> PlotStream {
    PlotStream {
        width,
        height,
        paper_color: Color::from_srgb8(PAPER),
        area: [p(0.0, 0.0), p(width, height)],
        clips: vec![],
        items: vec![],
        failed: vec![],
    }
}

/// A 100 by 80 mm page whose plotted area leaves a 5 mm margin, with one item of every kind the
/// probes look at.
pub fn scene() -> PlotStream {
    let id = ids(8);
    let red = Color::new(1.0, 0.0, 0.0, 1.0);
    let blue = Color::new(0.0, 0.0, 1.0, 1.0);
    let line = segment(p(10.0, 40.0), p(90.0, 40.0));
    let dashed = segment(p(10.0, 50.0), p(90.0, 50.0));
    let pieces = vec![
        piece(&dashed, 0.0, 0.25).unwrap(),
        piece(&dashed, 0.5, 0.75).unwrap(),
    ];
    let mut s = empty(100.0, 80.0);
    s.area = [p(5.0, 5.0), p(95.0, 75.0)];
    s.clips = vec![square(0.0, 60.0, 50.0, 80.0)];
    s.items = vec![
        stroke(id[0], line, pen(Color::BLACK, 1.0)),
        stroke(id[1], dashed.clone(), pen(Color::BLACK, 1.0)),
        item(
            id[2],
            ItemKind::Fill {
                region: square(60.0, 10.0, 80.0, 30.0),
                pen: pen(red, 0.25),
            },
        ),
        stroke(
            id[3],
            Curve2::Arc(Arc2::new(p(30.0, 20.0), 10.0, 0.0, std::f64::consts::PI).unwrap()),
            pen(blue, 1.0),
        ),
        Item {
            clip: Some(0),
            ..stroke(
                id[4],
                segment(p(0.0, 70.0), p(100.0, 70.0)),
                pen(Color::BLACK, 1.0),
            )
        },
        item(
            id[5],
            ItemKind::Mask {
                region: square(40.0, 35.0, 50.0, 45.0),
            },
        ),
        stroke(
            id[6],
            segment(p(0.0, 2.0), p(100.0, 2.0)),
            pen(Color::BLACK, 1.0),
        ),
    ];
    // The dashed item draws its pieces only.
    if let Some(Item {
        kind: ItemKind::Stroke { pieces: slot, .. },
        ..
    }) = s.items.get_mut(1)
    {
        *slot = Some(pieces);
    }
    s
}

pub fn nurbs() -> Curve2 {
    Curve2::Nurbs(
        Nurbs2::new(
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![p(10.0, 10.0), p(30.0, 50.0), p(50.0, 10.0)],
            vec![1.0, 1.0, 1.0],
        )
        .unwrap(),
    )
}

/// Checks `value` against a JSON Schema, supporting the keywords the repository's schemas use:
/// `$ref` to `#/$defs`, `type`, `enum`, `const`, `required`, `properties`,
/// `additionalProperties`, `items`, `oneOf`, `minimum` and `pattern` (prefix anchors only checked
/// for non-emptiness). Returns the paths that fail.
pub fn validate(schema: &Json, root: &Json, value: &Json, path: &str, errors: &mut Vec<String>) {
    if let Some(reference) = schema.get("$ref").and_then(Json::as_str) {
        let name = reference.strip_prefix("#/$defs/").unwrap();
        return validate(&root["$defs"][name], root, value, path, errors);
    }
    if let Some(types) = schema.get("type") {
        let names: Vec<&str> = match types {
            Json::String(s) => vec![s.as_str()],
            Json::Array(a) => a.iter().map(|t| t.as_str().unwrap()).collect(),
            _ => panic!("bad type keyword"),
        };
        let ok = names.iter().any(|t| match *t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => panic!("unknown type {other}"),
        });
        if !ok {
            errors.push(format!("{path}: type {types}"));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Json::as_array) {
        if !options.contains(value) {
            errors.push(format!("{path}: not in enum"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            errors.push(format!("{path}: not the constant"));
        }
    }
    if let Some(minimum) = schema.get("minimum").and_then(Json::as_f64) {
        if value.as_f64().is_some_and(|v| v < minimum) {
            errors.push(format!("{path}: below minimum"));
        }
    }
    if schema.get("pattern").is_some() && value.as_str().is_some_and(str::is_empty) {
        errors.push(format!("{path}: empty string for a pattern"));
    }
    if let Some(options) = schema.get("oneOf").and_then(Json::as_array) {
        let matching = options
            .iter()
            .filter(|option| {
                let mut e = Vec::new();
                validate(option, root, value, path, &mut e);
                e.is_empty()
            })
            .count();
        if matching != 1 {
            errors.push(format!("{path}: {matching} oneOf branches match"));
        }
    }
    if let Some(object) = value.as_object() {
        for key in schema
            .get("required")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if !object.contains_key(key.as_str().unwrap()) {
                errors.push(format!("{path}: missing {key}"));
            }
        }
        let properties = schema.get("properties").and_then(Json::as_object);
        for (key, member) in object {
            let member_path = format!("{path}/{key}");
            match properties.and_then(|p| p.get(key)) {
                Some(sub) => validate(sub, root, member, &member_path, errors),
                None => match schema.get("additionalProperties") {
                    Some(Json::Bool(false)) => errors.push(format!("{member_path}: not allowed")),
                    Some(sub @ Json::Object(_)) => {
                        validate(sub, root, member, &member_path, errors)
                    }
                    _ => {}
                },
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            validate(items, root, item, &format!("{path}/{i}"), errors);
        }
    }
}

/// Validates a loss report against `schemas/loss-report.schema.json`.
pub fn assert_report_valid(report: &arc_io_pdf::LossReport) {
    let text = std::fs::read_to_string(format!(
        "{}/../../schemas/loss-report.schema.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let schema: Json = serde_json::from_str(&text).unwrap();
    let value = serde_json::to_value(report).unwrap();
    let mut errors = Vec::new();
    validate(&schema, &schema, &value, "", &mut errors);
    assert!(errors.is_empty(), "{errors:?}\n{value:#}");
}

pub fn reasons(report: &arc_io_pdf::LossReport) -> Vec<&str> {
    report.items.iter().map(|i| i.reason.as_str()).collect()
}

/// A 100 by 80 mm page with a red square (20..60) that has a circular hole of radius 10 at
/// (40, 40), as one region fill.
pub fn ring() -> PlotStream {
    let tol = Tolerance::DEFAULT;
    let outer = Loop2::new(vec![Curve2::Polyline(square(20.0, 20.0, 60.0, 60.0))], &tol).unwrap();
    let hole = Loop2::new(
        vec![Curve2::Circle(Circle2::new(p(40.0, 40.0), 10.0).unwrap())],
        &tol,
    )
    .unwrap();
    let region = Region::new(vec![RegionPart::new(outer, vec![hole])]).unwrap();
    let mut s = empty(100.0, 80.0);
    s.items = vec![item(
        ids(1)[0],
        ItemKind::RegionFill {
            region,
            pen: pen(Color::new(1.0, 0.0, 0.0, 1.0), 0.25),
        },
    )];
    s
}
