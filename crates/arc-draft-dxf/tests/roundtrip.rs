//! Round trips: random drafting documents through the export and import commands, and
//! hand-built neutral drawings through the import and export.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "test code"
)]

mod support;

use std::collections::BTreeMap;

use arc_base::ObjectId;
use arc_base::math::Point2;
use arc_doc::relationship::ReferenceField;
use arc_doc::{Snapshot, Value};
use arc_draft::types::ENTITIES;
use arc_draft_dxf::fields::Reader;
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, Nurbs2};
use arc_io_dxf::model::{Arc, Ellipse, LwPolyline, LwVertex, Text};
use arc_io_dxf::{Entity, EntityKind};
use proptest::prelude::*;
use serde_json::{Value as Json, json};
use support::{Env, bytes, drawing, layer_entry, on_layer};

/// One comparable entity: its type, layer, appearance, strings and numbers.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    key: String,
    strings: Vec<String>,
    numbers: Vec<f64>,
}

fn close(a: &Item, b: &Item) -> bool {
    a.key == b.key
        && a.strings == b.strings
        && a.numbers.len() == b.numbers.len()
        && a.numbers
            .iter()
            .zip(&b.numbers)
            .all(|(x, y)| (x - y).abs() <= 1e-7 * (1.0 + x.abs()))
}

fn pair(a: Point2, b: Point2) -> [f64; 4] {
    let (p, q) = if (a.x, a.y) <= (b.x, b.y) {
        (a, b)
    } else {
        (b, a)
    };
    [p.x, p.y, q.x, q.y]
}

fn curve_numbers(curve: &Curve2) -> Vec<f64> {
    match curve {
        Curve2::Segment(s) => vec![s.start().x, s.start().y, s.end().x, s.end().y],
        Curve2::Ray(r) => vec![r.origin().x, r.origin().y, r.direction().x, r.direction().y],
        Curve2::Line(l) => vec![l.point().x, l.point().y, l.direction().x, l.direction().y],
        Curve2::Circle(c) => vec![c.center().x, c.center().y, c.radius()],
        Curve2::Arc(a) => {
            let ends = pair(a.point_at(0.0), a.point_at(a.sweep().abs()));
            let mut v = vec![a.center().x, a.center().y, a.radius(), a.sweep().abs()];
            v.extend(ends);
            v
        }
        Curve2::Ellipse(e) => vec![
            e.center().x,
            e.center().y,
            e.major_axis().x,
            e.major_axis().y,
            e.ratio(),
        ],
        Curve2::EllipticalArc(a) => {
            let e = a.ellipse();
            let mut v = vec![
                e.center().x,
                e.center().y,
                e.major_axis().x,
                e.major_axis().y,
                e.ratio(),
                a.sweep().abs(),
            ];
            v.extend(pair(a.point_at(0.0), a.point_at(a.sweep().abs())));
            v
        }
        Curve2::Polyline(p) => {
            let mut v = vec![f64::from(u8::from(p.is_closed()))];
            for x in p.vertices() {
                v.extend([x.point.x, x.point.y, x.bulge, x.start_width, x.end_width]);
            }
            v
        }
        Curve2::Nurbs(_) => Vec::new(),
    }
}

fn point(v: Option<&Value>) -> Vec<f64> {
    match v {
        Some(Value::Point2(p)) => vec![p.x, p.y],
        _ => Vec::new(),
    }
}

/// The comparable content of the entity `id` of `snapshot`.
fn item(snapshot: &Snapshot, id: ObjectId, names: &BTreeMap<ObjectId, String>) -> Item {
    let handle = snapshot.resolve(id).unwrap();
    let key = snapshot.type_key(handle).unwrap().as_str().to_owned();
    let object = snapshot.get(handle).unwrap();
    let r = Reader::new(snapshot.registry(), &key, &object.values);
    let mut strings = vec![
        r.reference("layer")
            .and_then(|l| names.get(&l).cloned())
            .unwrap_or_else(|| "0".to_owned()),
        format!(
            "{:?}{:?}{:?}",
            r.enum_code("color_by"),
            r.enum_code("lineweight_by"),
            r.bool_or("visible", true)
        ),
    ];
    let mut numbers = Vec::new();
    if let Some(Value::Color(c)) = r.get("color") {
        numbers.extend(c.rgba.to_srgb8().map(f64::from));
    }
    numbers.extend(r.f64("lineweight").map(|w| (w * 100.0).round()));
    match key.as_str() {
        "core.draft.point" => numbers.extend(point(r.get("position"))),
        "core.draft.spline" => {
            let Some(Value::Geometry(b)) = r.get("geometry") else {
                panic!()
            };
            let n = Nurbs2::decode(&b.0, &arc_base::Limits::DEFAULT).unwrap();
            numbers.push(n.degree() as f64);
            numbers.extend(n.control_points().iter().flat_map(|p| [p.x, p.y]));
            numbers.extend(n.knots());
            numbers.extend(n.weights());
        }
        "core.draft.text" => {
            strings.push(r.text("content").unwrap().to_owned());
            numbers.extend(point(r.get("position")));
            numbers.extend([r.f64_or("height", 0.0), r.f64_or("rotation", 0.0)]);
            numbers.extend(
                [
                    r.enum_code("halign").unwrap_or(0),
                    r.enum_code("valign").unwrap_or(0),
                ]
                .map(f64::from),
            );
        }
        "core.draft.rich_text" => {
            let text: Vec<String> = arc_draft::text::paragraphs_of(&object.values)
                .iter()
                .map(|p| p.runs.iter().map(|x| x.text.clone()).collect::<String>())
                .collect();
            strings.push(text.join("|"));
            numbers.extend(point(r.get("position")));
            numbers.extend([r.f64_or("height", 0.0), r.f64_or("width", 0.0)]);
        }
        "core.draft.block_ref" => {
            let block = r
                .reference("block")
                .and_then(|b| names.get(&b).cloned())
                .unwrap_or_default();
            strings.push(block);
            if let Some(Value::Map(m)) = r.get("attributes") {
                strings.push(format!("{m:?}"));
            }
            numbers.extend(point(r.get("position")));
            if let Some(Value::Vector3(s)) = r.get("scale") {
                numbers.extend([s.x, s.y, s.z]);
            }
            numbers.push(r.f64_or("rotation", 0.0));
        }
        "core.draft.hatch" => {
            numbers.push(f64::from(r.enum_code("kind").unwrap()));
            if let Some(Value::List(loops)) = r.get("loops") {
                for l in loops {
                    let Value::Geometry(b) = l else { panic!() };
                    numbers.extend(curve_numbers(
                        &Curve2::decode(&b.0, &arc_base::Limits::DEFAULT).unwrap(),
                    ));
                }
            }
            if let Some(Value::List(families)) = r.get("inline") {
                for f in families {
                    let Value::Record(f) = f else { panic!() };
                    for v in f.iter().flatten() {
                        match v {
                            Value::F64(x) => numbers.push(*x),
                            Value::Point2(p) => numbers.extend([p.x, p.y]),
                            Value::List(l) => numbers.extend(l.iter().filter_map(|d| {
                                if let Value::F64(x) = d {
                                    Some(*x)
                                } else {
                                    None
                                }
                            })),
                            _ => {}
                        }
                    }
                }
            }
        }
        "core.draft.dimension" => {
            numbers.push(f64::from(r.enum_code("kind").unwrap()));
            if let Some(Value::List(points)) = r.get("points") {
                for p in points {
                    if let Some(p) = arc_draft::dimension::DefinitionPoint::from_value(p) {
                        numbers.extend([p.position.x, p.position.y]);
                    }
                }
            }
            numbers.push(r.f64_or("rotation", 0.0));
        }
        _ => {
            if let Some(Value::Geometry(b)) = r.get("geometry") {
                numbers.extend(curve_numbers(
                    &Curve2::decode(&b.0, &arc_base::Limits::DEFAULT).unwrap(),
                ));
            }
        }
    }
    Item {
        key,
        strings,
        numbers,
    }
}

fn names(snapshot: &Snapshot) -> BTreeMap<ObjectId, String> {
    let mut out = BTreeMap::new();
    for key in [
        arc_draft::types::LAYER,
        arc_draft::types::BLOCK,
        arc_draft::types::LINETYPE,
        arc_draft::style::TEXT_STYLE,
    ] {
        for (_, o) in snapshot.iter_type(key).unwrap() {
            let r = Reader::new(snapshot.registry(), key, &o.values);
            out.insert(o.id, r.text("name").unwrap().to_owned());
        }
    }
    out
}

fn entities_of(snapshot: &Snapshot, container: ObjectId) -> Vec<Item> {
    let names = names(snapshot);
    let mut members: Vec<(i64, ObjectId)> = Vec::new();
    for r in snapshot.referrers(container) {
        if r.field != ReferenceField::Owner {
            continue;
        }
        let h = snapshot.resolve(r.source).unwrap();
        let key = snapshot.type_key(h).unwrap().as_str();
        if !ENTITIES.contains(&key) {
            continue;
        }
        let o = snapshot.get(h).unwrap();
        let order = match Reader::new(snapshot.registry(), key, &o.values).get("draw_order") {
            Some(Value::I64(k)) => *k,
            _ => 0,
        };
        members.push((order, r.source));
    }
    members.sort();
    members
        .into_iter()
        .map(|(_, id)| item(snapshot, id, &names))
        .collect()
}

/// Everything compared of a document: definitions by name and the content of the space and of
/// every block.
fn content(
    snapshot: &Snapshot,
    space: ObjectId,
) -> (Vec<Item>, BTreeMap<String, Vec<Item>>, Vec<String>) {
    let mut blocks = BTreeMap::new();
    let mut defs = Vec::new();
    for (_, o) in snapshot.iter_type(arc_draft::types::BLOCK).unwrap() {
        let r = Reader::new(snapshot.registry(), arc_draft::types::BLOCK, &o.values);
        let name = r.text("name").unwrap().to_owned();
        defs.push(format!(
            "block {name} {:?} {:?} {:?}",
            r.get("base_point"),
            r.enum_code("unit"),
            r.get("attributes")
        ));
        blocks.insert(name, entities_of(snapshot, o.id));
    }
    for (_, o) in snapshot.iter_type(arc_draft::types::LAYER).unwrap() {
        let r = Reader::new(snapshot.registry(), arc_draft::types::LAYER, &o.values);
        let color = match r.get("color") {
            Some(Value::Color(c)) => c.rgba.to_srgb8(),
            _ => [0; 4],
        };
        defs.push(format!(
            "layer {} {color:?} {:?} {} {} {} {}",
            r.text("name").unwrap(),
            r.f64("lineweight").map(|w| (w * 100.0).round()),
            r.bool_or("on", true),
            r.bool_or("frozen", false),
            r.bool_or("locked", false),
            r.bool_or("plottable", true),
        ));
    }
    for (_, o) in snapshot.iter_type(arc_draft::types::LINETYPE).unwrap() {
        let r = Reader::new(snapshot.registry(), arc_draft::types::LINETYPE, &o.values);
        defs.push(format!(
            "linetype {} {:?}",
            r.text("name").unwrap(),
            r.get("elements")
        ));
    }
    defs.sort();
    (entities_of(snapshot, space), blocks, defs)
}

#[derive(Debug, Clone)]
enum Spec {
    Segment([f64; 4]),
    Circle([f64; 3]),
    Arc([f64; 5]),
    Ellipse([f64; 5]),
    EllipticalArc([f64; 7]),
    Ray([f64; 4]),
    Xline([f64; 4]),
    Polyline(Vec<[f64; 3]>, bool),
    Spline(Vec<[f64; 2]>),
    Point([f64; 2]),
    Text(String, [f64; 4], u8, u8),
    Rich(String, [f64; 3]),
    Hatch([f64; 4], bool),
    Wipeout([f64; 4]),
    Dimension([f64; 6], u8),
    Insert([f64; 5]),
}

fn coord() -> impl Strategy<Value = f64> {
    -500.0..500.0_f64
}

fn word() -> impl Strategy<Value = String> {
    "[A-Za-z0-9]{1,8}( [A-Za-z0-9]{1,6}){0,3}"
}

fn spec() -> impl Strategy<Value = Spec> {
    prop_oneof![
        (coord(), coord(), 1.0..200.0_f64, 0.0..6.0_f64).prop_map(|(x, y, l, a)| Spec::Segment([
            x,
            y,
            x + l * a.cos(),
            y + l * a.sin()
        ])),
        (coord(), coord(), 0.5..300.0_f64).prop_map(|(x, y, r)| Spec::Circle([x, y, r])),
        (
            coord(),
            coord(),
            0.5..300.0_f64,
            0.0..6.2_f64,
            prop_oneof![0.2..6.0_f64, -6.0..-0.2_f64]
        )
            .prop_map(|(x, y, r, s, w)| Spec::Arc([x, y, r, s, w])),
        (coord(), coord(), 1.0..200.0_f64, 0.0..3.0_f64, 0.1..1.0_f64)
            .prop_map(|(x, y, m, a, k)| Spec::Ellipse([x, y, m, a, k])),
        (
            coord(),
            coord(),
            1.0..200.0_f64,
            0.0..3.0_f64,
            0.1..1.0_f64,
            0.0..6.0_f64,
            prop_oneof![0.2..6.0_f64, -6.0..-0.2_f64]
        )
            .prop_map(|(x, y, m, a, k, s, w)| Spec::EllipticalArc([x, y, m, a, k, s, w])),
        (coord(), coord(), 1.0..100.0_f64, 1.0..100.0_f64)
            .prop_map(|(x, y, a, b)| Spec::Ray([x, y, a, b])),
        (coord(), coord(), 1.0..100.0_f64, 1.0..100.0_f64)
            .prop_map(|(x, y, a, b)| Spec::Xline([x, y, a, b])),
        (
            prop::collection::vec(
                (
                    1.0..50.0_f64,
                    coord(),
                    prop_oneof![Just(0.0), 0.1..2.0_f64, -2.0..-0.1_f64]
                ),
                2..6
            ),
            any::<bool>()
        )
            .prop_map(|(v, closed)| {
                let mut x = 0.0;
                Spec::Polyline(
                    v.into_iter()
                        .map(|(dx, y, b)| {
                            x += dx;
                            [x, y, b]
                        })
                        .collect(),
                    closed,
                )
            }),
        prop::collection::vec((coord(), coord()), 4..8).prop_map(|v| {
            Spec::Spline(
                v.into_iter()
                    .enumerate()
                    .map(|(i, (x, y))| [x + i as f64 * 1000.0, y])
                    .collect(),
            )
        }),
        (coord(), coord()).prop_map(|(x, y)| Spec::Point([x, y])),
        (
            word(),
            coord(),
            coord(),
            0.5..20.0_f64,
            0.0..6.0_f64,
            0..3u8,
            0..4u8
        )
            .prop_map(|(t, x, y, h, a, ha, va)| Spec::Text(t, [x, y, h, a], ha, va)),
        (word(), word(), coord(), coord(), 1.0..10.0_f64)
            .prop_map(|(a, b, x, y, h)| Spec::Rich(format!("{a}\n{b}"), [x, y, h])),
        (
            coord(),
            coord(),
            1.0..100.0_f64,
            1.0..100.0_f64,
            any::<bool>()
        )
            .prop_map(|(x, y, w, h, solid)| Spec::Hatch([x, y, w, h], solid)),
        (coord(), coord(), 1.0..100.0_f64, 1.0..100.0_f64)
            .prop_map(|(x, y, w, h)| Spec::Wipeout([x, y, w, h])),
        (
            coord(),
            coord(),
            1.0..100.0_f64,
            coord(),
            1.0..50.0_f64,
            0..4u8
        )
            .prop_map(|(x, y, l, _, d, k)| Spec::Dimension([x, y, x + l, y + 20.0, d, 0.0], k)),
        (coord(), coord(), 0.5..3.0_f64, 0.0..6.0_f64, word(), 0..2u8)
            .prop_map(|(x, y, s, r, v, _)| Spec::Insert([x, y, s, r, v.len() as f64])),
    ]
}

fn curve_json(c: &Curve2) -> Json {
    serde_json::to_value(c).unwrap()
}

fn build(env: &mut Env, space: ObjectId, specs: &[Spec]) {
    use arc_base::math::Vector2;
    use arc_geom::{
        Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Polyline2, PolylineVertex, Ray2, Segment2,
    };
    let p = |x: f64, y: f64| Point2::new(x, y);
    env.create("core.draft.layer.create", json!({"name": "0"}));
    let layer_a = env.create(
        "core.draft.layer.create",
        json!({"name": "Walls", "color": [1.0, 0.0, 0.0, 1.0], "lineweight": 0.35}),
    );
    let layer_b = env.create("core.draft.layer.create", json!({"name": "Notes"}));
    env.create(
        "core.draft.linetype.load",
        json!({"name": "Dashed", "description": "d", "elements": [{"kind": "dash", "length": 6.0}, {"kind": "gap", "length": 3.0}, {"kind": "dot"}]}),
    );
    let style = env.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": "Plain"}),
    );
    let block = env.create(
        "core.draft.block.define",
        json!({"name": "Door", "base_point": [1.0, 2.0], "unit": "millimeter",
               "attributes": [{"tag": "ID", "prompt": "Id", "default": "x"}]}),
    );
    env.create("core.draft.entity.create", json!({"owner": block.to_string(), "geometry": curve_json(&Curve2::Segment(Segment2::new(p(0.0, 0.0), p(900.0, 0.0)).unwrap()))}));
    let owner = space.to_string();
    for (i, s) in specs.iter().enumerate() {
        let layer = if i % 2 == 0 { layer_a } else { layer_b };
        let geometry = |c: Curve2| json!({"owner": owner, "layer": layer.to_string(), "geometry": curve_json(&c)});
        let id = match s {
            Spec::Segment([a, b, c, d]) => env.create("core.draft.entity.create", geometry(Curve2::Segment(Segment2::new(p(*a, *b), p(*c, *d)).unwrap()))),
            Spec::Circle([x, y, r]) => env.create("core.draft.entity.create", geometry(Curve2::Circle(Circle2::new(p(*x, *y), *r).unwrap()))),
            Spec::Arc([x, y, r, s, w]) => env.create("core.draft.entity.create", geometry(Curve2::Arc(Arc2::new(p(*x, *y), *r, *s, *w).unwrap()))),
            Spec::Ellipse([x, y, m, a, k]) => env.create(
                "core.draft.entity.create",
                geometry(Curve2::Ellipse(Ellipse2::new(p(*x, *y), Vector2::new(m * a.cos(), m * a.sin()), *k).unwrap())),
            ),
            Spec::EllipticalArc([x, y, m, a, k, s, w]) => {
                let e = Ellipse2::new(p(*x, *y), Vector2::new(m * a.cos(), m * a.sin()), *k).unwrap();
                env.create("core.draft.entity.create", geometry(Curve2::EllipticalArc(EllipticalArc2::new(e, *s, *w).unwrap())))
            }
            Spec::Ray([x, y, a, b]) => env.create("core.draft.entity.create", geometry(Curve2::Ray(Ray2::new(p(*x, *y), Vector2::new(*a, *b)).unwrap()))),
            Spec::Xline([x, y, a, b]) => env.create("core.draft.entity.create", geometry(Curve2::Line(Line2::new(p(*x, *y), Vector2::new(*a, *b)).unwrap()))),
            Spec::Polyline(v, closed) => {
                let mut vertices: Vec<PolylineVertex> = v
                    .iter()
                    .enumerate()
                    .map(|(i, [x, y, b])| PolylineVertex { point: p(*x, *y), bulge: *b, start_width: if i % 2 == 0 { 0.5 } else { 0.0 }, end_width: if i % 2 == 0 { 1.0 } else { 0.0 } })
                    .collect();
                if v.len() < 3 {
                    vertices.truncate(2);
                }
                match Polyline2::new(vertices, *closed && v.len() >= 3) {
                    Ok(pl) => env.create("core.draft.entity.create", geometry(Curve2::Polyline(pl))),
                    Err(_) => continue,
                }
            }
            Spec::Spline(points) => env.create(
                "core.draft.spline.create",
                json!({"owner": owner, "layer": layer.to_string(), "degree": 3, "control_points": points}),
            ),
            Spec::Point([x, y]) => env.create("core.draft.entity.create", json!({"owner": owner, "geometry": {"kind": "point", "position": [x, y]}})),
            Spec::Text(t, [x, y, h, a], ha, va) => {
                let halign = ["left", "center", "right"][usize::from(*ha)];
                let valign = ["baseline", "bottom", "middle", "top"][usize::from(*va)];
                env.create(
                    "core.draft.text.create",
                    json!({"owner": owner, "content": t, "position": [x, y], "height": h, "rotation": a, "style": style.to_string(),
                           "halign": halign, "valign": valign}),
                )
            }
            Spec::Rich(t, [x, y, h]) => env.create("core.draft.rich_text.create", json!({"owner": owner, "text": t, "position": [x, y], "height": h})),
            Spec::Hatch([x, y, w, h], solid) => {
                let rect = Polyline2::new(
                    vec![
                        PolylineVertex::new(p(*x, *y), 0.0),
                        PolylineVertex::new(p(x + w, *y), 0.0),
                        PolylineVertex::new(p(x + w, y + h), 0.0),
                        PolylineVertex::new(p(*x, y + h), 0.0),
                    ],
                    true,
                )
                .unwrap();
                let mut params = json!({"owner": owner, "layer": layer.to_string(), "boundary": [curve_json(&Curve2::Polyline(rect))], "kind": if *solid { "solid" } else { "pattern" }});
                if !*solid {
                    params["inline"] = json!([{"angle": 45.0, "origin": [0.0, 0.0], "shift": 0.0, "spacing": 5.0, "dashes": [3.0, -1.0]}]);
                }
                env.create("core.draft.hatch.create", params)
            }
            Spec::Wipeout([x, y, w, h]) => {
                let rect = Polyline2::new(
                    vec![
                        PolylineVertex::new(p(*x, *y), 0.0),
                        PolylineVertex::new(p(x + w, *y), 0.0),
                        PolylineVertex::new(p(x + w, y + h), 0.0),
                        PolylineVertex::new(p(*x, y + h), 0.0),
                    ],
                    true,
                )
                .unwrap();
                env.create("core.draft.wipeout.create", json!({"owner": owner, "boundary": curve_json(&Curve2::Polyline(rect))}))
            }
            Spec::Dimension([x0, y0, x1, y1, d, _], k) => {
                let (kind, points, line) = match k {
                    0 => ("linear", vec![[*x0, *y0], [*x1, *y0 + 5.0]], [*x0, y0 + d + 10.0]),
                    1 => ("aligned", vec![[*x0, *y0], [*x1, *y1]], [*x0, y1 + d]),
                    2 => ("radial", vec![[*x0, *y0], [x0 + d, *y0]], [x0 + d, y0 + 5.0]),
                    _ => ("diameter", vec![[*x0, *y0], [x0 + d, *y0]], [x0 + d, y0 + 5.0]),
                };
                env.create(
                    "core.draft.dimension.create",
                    json!({"owner": owner, "kind": kind, "points": points.iter().map(|p| json!({"position": p})).collect::<Vec<_>>(), "line_position": line}),
                )
            }
            Spec::Insert([x, y, s, r, n]) => env.create(
                "core.draft.block.insert",
                json!({"owner": owner, "block": block.to_string(), "position": [x, y], "scale": [s, *s, 1.0], "rotation": r, "attributes": {"ID": format!("v{n}")}}),
            ),
        };
        // Some entities carry their own appearance.
        if i % 3 == 0 {
            for (field, value) in [
                ("color_by", json!("object")),
                ("color", json!([0.2, 0.4, 0.6, 1.0])),
                ("lineweight_by", json!("object")),
                ("lineweight", json!(0.5)),
                ("visible", json!(false)),
            ] {
                env.run(
                    "core.draft.entity.modify",
                    json!({"objects": [id.to_string()], "field": field, "value": value}),
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_drafting_document_survives_export_and_import(specs in prop::collection::vec(spec(), 1..9)) {
        let mut a = Env::new();
        let space_a = a.space("model", "millimeter");
        build(&mut a, space_a, &specs);
        let file = a.path("rt.dxf");
        a.run("core.draft.export.dxf", json!({"path": file.to_str().unwrap(), "space": space_a.to_string(), "version": "2013"}));
        let mut b = Env::new();
        let space_b = b.space("model", "millimeter");
        let path = b.write("rt.dxf", &std::fs::read(&file).unwrap());
        b.import(&path, space_b).unwrap();
        let (ea, ba, da) = content(&a.snapshot(), space_a);
        let (eb, bb, db) = content(&b.snapshot(), space_b);
        prop_assert_eq!(&da, &db);
        prop_assert_eq!(ba.keys().collect::<Vec<_>>(), bb.keys().collect::<Vec<_>>());
        for (name, items) in &ba {
            let other = &bb[name];
            prop_assert_eq!(items.len(), other.len());
            for (x, y) in items.iter().zip(other) {
                prop_assert!(close(x, y), "block {name}: {x:?} vs {y:?}");
            }
        }
        prop_assert_eq!(ea.len(), eb.len());
        for (x, y) in ea.iter().zip(&eb) {
            prop_assert!(close(x, y), "{x:?} vs {y:?}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_neutral_drawing_survives_import_and_export(
        a in (coord(), coord(), coord(), coord()),
        c in (coord(), coord(), 0.5..300.0_f64),
        arc in (coord(), coord(), 0.5..300.0_f64, 0.0..359.0_f64, 1.0..359.0_f64),
        vertices in prop::collection::vec((1.0..50.0_f64, coord(), prop_oneof![Just(0.0), 0.1..2.0_f64]), 2..6),
        text in (word(), coord(), coord(), 0.5..20.0_f64, 0.0..360.0_f64),
        ellipse in (coord(), coord(), 1.0..100.0_f64, 0.1..1.0_f64, 0.0..3.0_f64, 3.2..6.2_f64),
        color in 1i16..255,
    ) {
        let mut x = 0.0;
        let last = vertices.len() - 1;
        let lw = LwPolyline {
            flags: 0,
            constant_width: None,
            vertices: vertices
                .iter()
                .enumerate()
                .map(|(i, (dx, y, b))| {
                    x += dx;
                    LwVertex { location: [x, *y], bulge: Some(*b).filter(|b| *b != 0.0 && i != last), ..LwVertex::default() }
                })
                .collect(),
        };
        let entities = vec![
            on_layer(support::line([a.0, a.1, 0.0], [a.2 + 1000.0, a.3, 0.0]), "WALLS"),
            support::circle([c.0, c.1, 0.0], c.2),
            Entity::new(EntityKind::Arc(Arc { center: [arc.0, arc.1, 0.0], radius: arc.2, start_angle: arc.3, end_angle: (arc.3 + arc.4) % 360.0 })),
            Entity::new(EntityKind::LwPolyline(lw)),
            Entity::new(EntityKind::Text(Text { insert: [text.1, text.2, 0.0], height: text.3, value: text.0.clone(), rotation: Some(text.4), ..Text::default() })),
            Entity::new(EntityKind::Ellipse(Ellipse { center: [ellipse.0, ellipse.1, 0.0], major_axis: [ellipse.2, 0.0, 0.0], ratio: ellipse.3, start_param: ellipse.4, end_param: ellipse.5 })),
        ];
        let mut d = drawing(entities.clone(), Some(4));
        d.tables.layers.as_mut().unwrap().entries.push(layer_entry("INDEXED", color));
        let mut env = Env::new();
        let space = env.space("model", "millimeter");
        let path = env.write("n.dxf", &bytes(&d));
        env.import(&path, space).unwrap();
        let out = env.exported(space);
        prop_assert_eq!(out.entities.len(), entities.len());
        let near = |p: f64, q: f64| (p - q).abs() <= 1e-7 * (1.0 + p.abs());
        for (written, read) in entities.iter().zip(&out.entities) {
            match (&written.kind, &read.kind) {
                (EntityKind::Line(l), EntityKind::Line(m)) => prop_assert!((0..3).all(|i| near(l.start[i], m.start[i]) && near(l.end[i], m.end[i]))),
                (EntityKind::Circle(l), EntityKind::Circle(m)) => prop_assert!((0..3).all(|i| near(l.center[i], m.center[i])) && near(l.radius, m.radius)),
                (EntityKind::Arc(l), EntityKind::Arc(m)) => {
                    prop_assert!(near(l.radius, m.radius) && near(l.start_angle.rem_euclid(360.0), m.start_angle.rem_euclid(360.0)));
                    prop_assert!(near(l.end_angle.rem_euclid(360.0), m.end_angle.rem_euclid(360.0)));
                }
                (EntityKind::LwPolyline(l), EntityKind::LwPolyline(m)) => {
                    prop_assert_eq!(l.vertices.len(), m.vertices.len());
                    for (p, q) in l.vertices.iter().zip(&m.vertices) {
                        prop_assert!(near(p.location[0], q.location[0]) && near(p.location[1], q.location[1]));
                        prop_assert!(near(p.bulge.unwrap_or(0.0), q.bulge.unwrap_or(0.0)));
                    }
                }
                (EntityKind::Text(l), EntityKind::Text(m)) => {
                    prop_assert_eq!(&l.value, &m.value);
                    prop_assert!(near(l.height, m.height) && near(l.insert[0], m.insert[0]));
                    prop_assert!(near(l.rotation.unwrap_or(0.0), m.rotation.unwrap_or(0.0)));
                }
                (EntityKind::Ellipse(l), EntityKind::Ellipse(m)) => {
                    prop_assert!(near(l.ratio, m.ratio) && near(l.major_axis[0], m.major_axis[0]));
                    prop_assert!(near(l.start_param.rem_euclid(std::f64::consts::TAU), m.start_param.rem_euclid(std::f64::consts::TAU)));
                    prop_assert!(near(l.end_param.rem_euclid(std::f64::consts::TAU), m.end_param.rem_euclid(std::f64::consts::TAU)));
                }
                other => prop_assert!(false, "{other:?}"),
            }
        }
        let layers = out.tables.layers.as_ref().unwrap();
        let indexed = layers.entries.iter().find(|e| e.name == "INDEXED").unwrap();
        prop_assert_eq!(indexed.data.color, color);
        prop_assert_eq!(&out.entities[0].props.layer, "WALLS");
    }
}
