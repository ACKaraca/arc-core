//! Hatches (CORE-GPU-011) and text (CORE-GPU-012) of drafting display lists: a solid hatch with
//! a hole, a pattern hatch of parallel lines on its document lattice under a pan, one item for a
//! large hatch, stroke text at its layout positions, outline text from a font built here, picking
//! of hatches and text over each other, and the bytes written by an edit of one object.

#![cfg(feature = "draft")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "tests fail by panicking; pixel arithmetic on small images; one scene per test"
)]

mod support;

use std::sync::Arc;

use arc_base::ObjectId;
use arc_base::math::{Point2, Point3};
use arc_command::{Invocation, Invoker, Permission, Registry, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_draft::display::{DisplayOptions, Lists, Shape, space_list, text_layout};
use arc_draft::fonts::{FontRegistry, Fonts};
use arc_geom::{Circle2, Curve2, Polyline2, PolylineVertex, Segment2};
use arc_gpu::draft::{self, DraftOptions};
use arc_gpu::{Image, PickRegion, RenderItem, RenderSettings, Renderer, Scene, SceneLimits};
use serde_json::{Value, json};
use support::{coverage, setup, top_view};

struct Fixture {
    document: Document,
    commands: Registry,
    session: Session,
    seed: u64,
}

impl Fixture {
    fn new() -> Self {
        let mut types = TypeRegistry::new();
        arc_draft::register_types(&mut types).unwrap();
        let config = DocumentConfig {
            workers: 0,
            ..DocumentConfig::default()
        };
        let document = Document::open(
            Snapshot::empty(Arc::new(types)),
            Arc::new(Evaluators::new()),
            config,
        )
        .unwrap();
        for validator in arc_draft::validate::validators(64) {
            document.add_validator(validator);
        }
        let mut commands = Registry::new();
        arc_draft::register_commands(&mut commands).unwrap();
        Self {
            document,
            commands,
            session: Session::new(Surface::Automation, [Permission::Edit, Permission::Read]),
            seed: 1,
        }
    }

    fn run(&mut self, command: &str, params: Value) -> Value {
        self.seed += 1;
        let invocation = Invocation {
            seed: self.seed,
            ..Invocation::new(command, params)
        };
        Invoker::new(&self.commands)
            .invoke(&self.document, &self.session, invocation)
            .unwrap()
            .value
    }

    fn create(&mut self, command: &str, params: Value) -> ObjectId {
        self.run(command, params)["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn space(&mut self) -> ObjectId {
        self.create(
            "core.draft.space.create",
            json!({"name": "model", "kind": "model", "unit": "millimeter"}),
        )
    }

    fn set(&mut self, command: &str, object: ObjectId, field: &str, value: Value) {
        self.run(
            command,
            json!({"objects": [object.to_string()], "field": field, "value": value}),
        );
    }

    fn hatch(&mut self, space: ObjectId, boundary: &[Curve2], extra: Value) -> ObjectId {
        let boundary: Vec<Value> = boundary
            .iter()
            .map(|c| serde_json::to_value(c).unwrap())
            .collect();
        let mut params = json!({"owner": space.to_string(), "boundary": boundary});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        self.create("core.draft.hatch.create", params)
    }
}

fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    let v = |x, y| PolylineVertex::new(Point2::new(x, y), 0.0);
    Curve2::Polyline(
        Polyline2::new(vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)], true).unwrap(),
    )
}

fn circle(x: f64, y: f64, r: f64) -> Curve2 {
    Curve2::Circle(Circle2::new(Point2::new(x, y), r).unwrap())
}

const SIZE: u32 = 200;
const SCALE: f64 = 2.0;

/// The pixel of a point of an orthographic top view centered on `center` at `SCALE` pixels per
/// unit, in a `SIZE` square image; fractional.
fn at(p: (f64, f64), center: (f64, f64)) -> (f64, f64) {
    (
        (p.0 - center.0) * SCALE + f64::from(SIZE) / 2.0,
        (center.1 - p.1) * SCALE + f64::from(SIZE) / 2.0,
    )
}

/// The document point at the center of pixel `(x, y)`.
fn point_of(x: u32, y: u32, center: (f64, f64)) -> (f64, f64) {
    (
        center.0 + (f64::from(x) + 0.5 - f64::from(SIZE) / 2.0) / SCALE,
        center.1 - (f64::from(y) + 0.5 - f64::from(SIZE) / 2.0) / SCALE,
    )
}

fn render(renderer: &mut Renderer, scene: &mut Scene, center: (f64, f64)) -> Image {
    let frame = top_view(
        Point3::new(center.0, center.1, 0.0),
        f64::from(SIZE) / SCALE,
    );
    renderer
        .render_to_image(scene, &frame, SIZE, SIZE, &RenderSettings::default())
        .unwrap()
}

fn pick(
    renderer: &mut Renderer,
    scene: &mut Scene,
    center: (f64, f64),
    p: (f64, f64),
) -> Vec<ObjectId> {
    let frame = top_view(
        Point3::new(center.0, center.1, 0.0),
        f64::from(SIZE) / SCALE,
    );
    let (x, y) = at(p, center);
    renderer
        .pick(
            scene,
            &frame,
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &PickRegion::pixel(x as u32, y as u32, 1.0),
        )
        .unwrap()
        .iter()
        .map(|h| h.key.object)
        .collect()
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Glyph 0 is empty, glyphs 1..=5 are squares for `a`..`e`, 6 is the space, 7 and 8 are
/// squares for U+05D0 and U+05D1. Every advance is 600 of 1000 units.
fn test_font() -> Vec<u8> {
    let glyphs = 9_u16;
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in 0..glyphs {
        be16(&mut loca, (glyf.len() / 2) as u16);
        if g == 0 || g == 6 {
            continue;
        }
        if g == 3 || g == 4 {
            // Glyph 3: a square with a square hole (two contours, the hole turned the other
            // way). Glyph 4: four off-curve points, a closed quadratic curve.
            let (ends, flag, xs, ys): (Vec<u16>, u8, Vec<i16>, Vec<i16>) = if g == 3 {
                (
                    vec![3, 7],
                    1,
                    vec![50, 200, 0, -200, 50, 0, 100, 0],
                    vec![0, 0, 200, 0, -150, 100, 0, -100],
                )
            } else {
                (vec![3], 0, vec![50, 200, 0, -200], vec![0, 0, 200, 0])
            };
            be16(&mut glyf, ends.len() as u16);
            for v in [50_i16, 0, 250, 200] {
                be16(&mut glyf, v as u16);
            }
            ends.iter().for_each(|e| be16(&mut glyf, *e));
            be16(&mut glyf, 0);
            glyf.extend(std::iter::repeat_n(flag, xs.len()));
            xs.iter().for_each(|v| be16(&mut glyf, *v as u16));
            ys.iter().for_each(|v| be16(&mut glyf, *v as u16));
            continue;
        }
        let side = 100 + 40 * g as i16;
        be16(&mut glyf, 1); // contours
        for v in [50, 0, 50 + side, side] {
            be16(&mut glyf, v as u16);
        }
        be16(&mut glyf, 3); // end point
        be16(&mut glyf, 0); // instructions
        glyf.extend_from_slice(&[1, 1, 1, 1]);
        for dx in [50, side, 0, -side] {
            be16(&mut glyf, dx as u16);
        }
        for dy in [0, 0, side, 0] {
            be16(&mut glyf, dy as u16);
        }
    }
    be16(&mut loca, (glyf.len() / 2) as u16);
    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0);
    be32(&mut head, 0x5F0F_3CF5);
    be16(&mut head, 0x000B);
    be16(&mut head, 1000);
    head.extend_from_slice(&[0; 16]);
    for v in [0_u16, 0, 600, 700, 0, 8, 2, 0, 0] {
        be16(&mut head, v);
    }
    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for v in [800_i16, -200, 0, 600, 0, 0, 600, 1, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut hhea, v as u16);
    }
    be16(&mut hhea, glyphs);
    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, glyphs);
    for v in [4_u16, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut maxp, v);
    }
    let mut hmtx = Vec::new();
    for _ in 0..glyphs {
        be16(&mut hmtx, 600);
        be16(&mut hmtx, 50);
    }
    // cmap format 4: segments (start, end, delta).
    let segments: [(u16, u16, u16); 4] = [
        (0x20, 0x20, 6_u16.wrapping_sub(0x20)),
        (0x61, 0x65, 1_u16.wrapping_sub(0x61)),
        (0x5D0, 0x5D1, 7_u16.wrapping_sub(0x5D0)),
        (0xFFFF, 0xFFFF, 1),
    ];
    let mut sub = Vec::new();
    let n = segments.len() as u16;
    be16(&mut sub, 4);
    be16(&mut sub, 16 + 8 * n);
    be16(&mut sub, 0);
    be16(&mut sub, 2 * n);
    be16(&mut sub, 8);
    be16(&mut sub, 2);
    be16(&mut sub, 0);
    segments.iter().for_each(|s| be16(&mut sub, s.1));
    be16(&mut sub, 0);
    segments.iter().for_each(|s| be16(&mut sub, s.0));
    segments.iter().for_each(|s| be16(&mut sub, s.2));
    segments.iter().for_each(|_| be16(&mut sub, 0));
    let mut cmap = Vec::new();
    for v in [0_u16, 1, 3, 1] {
        be16(&mut cmap, v);
    }
    be32(&mut cmap, 12);
    cmap.extend(sub);
    let family: Vec<u8> = "Test".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    for v in [0_u16, 1, 18, 3, 1, 0x409, 1, family.len() as u16, 0] {
        be16(&mut name, v);
    }
    name.extend(family);
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"name", name),
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    tables.sort_by_key(|t| *t.0);
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, tables.len() as u16);
    for v in [64_u16, 2, 48] {
        be16(&mut out, v);
    }
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0);
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    out
}

/// Extracts every entity of `space` into a fresh scene.
fn load(f: &Fixture, space: ObjectId, options: &DraftOptions) -> (arc_gpu::Gpu, Scene, Renderer) {
    let snapshot = f.document.snapshot();
    let objects = draft::space_extractions(&snapshot, space, options).unwrap();
    let (gpu, mut scene, renderer) = setup();
    let report = draft::apply(&mut scene, snapshot.revision(), objects);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    (gpu, scene, renderer)
}

fn lit_extent(image: &Image) -> (f64, f64, f64, f64) {
    let mut e = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for y in 0..image.height {
        for x in 0..image.width {
            if coverage(image, x, y) > 0.3 {
                let (px, py) = (f64::from(x), f64::from(y));
                e = (
                    e.0.min(px),
                    e.1.min(py),
                    e.2.max(px + 1.0),
                    e.3.max(py + 1.0),
                );
            }
        }
    }
    e
}

#[test]
fn a_solid_hatch_fills_the_ring_around_its_hole_and_nothing_else() {
    let mut f = Fixture::new();
    let space = f.space();
    let hatch = f.hatch(
        space,
        &[square(20.0, 20.0, 80.0, 80.0), circle(50.0, 50.0, 20.0)],
        json!({"kind": "solid"}),
    );
    let (_gpu, mut scene, mut renderer) = load(&f, space, &DraftOptions::default());
    let center = (50.0, 50.0);
    let image = render(&mut renderer, &mut scene, center);
    let value = |p: (f64, f64)| {
        let (x, y) = at(p, center);
        coverage(&image, x as u32, y as u32)
    };
    // The ring: between the circle (radius 20) and the square (half side 30).
    for p in [
        (50.0, 75.0),
        (50.0, 25.0),
        (25.0, 50.0),
        (75.0, 50.0),
        (30.0, 30.0),
    ] {
        assert!(value(p) > 0.99, "{p:?} is not filled: {}", value(p));
    }
    // The hole and everything outside the square.
    for p in [
        (50.0, 50.0),
        (60.0, 55.0),
        (50.0, 35.0),
        (10.0, 10.0),
        (50.0, 85.0),
        (90.0, 50.0),
    ] {
        assert!(value(p) < 0.01, "{p:?} is drawn: {}", value(p));
    }
    // Pixel by pixel: filled inside the ring, empty in the hole and outside, away from edges.
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = point_of(x, y, center);
            let (dx, dy) = (p.0 - 50.0, p.1 - 50.0);
            let r = dx.hypot(dy);
            let side = dx.abs().max(dy.abs());
            let expect = if r > 21.0 && side < 29.0 {
                Some(true)
            } else if r < 19.0 || side > 31.0 {
                Some(false)
            } else {
                None
            };
            if let Some(filled) = expect {
                assert_eq!(coverage(&image, x, y) > 0.5, filled, "pixel ({x}, {y})");
            }
        }
    }
    // A hit in the ring is the hatch; a hit in the hole is nothing.
    assert_eq!(
        pick(&mut renderer, &mut scene, center, (50.0, 75.0)),
        vec![hatch]
    );
    assert!(pick(&mut renderer, &mut scene, center, (50.0, 50.0)).is_empty());
}

/// The distance in pixels from the center of a pixel to the nearest line of the 45 degree
/// family `y - x = k * 10 * sqrt(2)` (spacing 10, origin zero).
fn lattice_distance(p: (f64, f64)) -> f64 {
    let across = (p.1 - p.0) / std::f64::consts::SQRT_2 / 10.0;
    (across - across.round()).abs() * 10.0 * SCALE
}

fn check_lattice(image: &Image, center: (f64, f64)) -> (usize, usize) {
    let (mut on, mut off) = (0, 0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = point_of(x, y, center);
            let inside = p.0 > 20.0 + 2.0 / SCALE
                && p.0 < 80.0 - 2.0 / SCALE
                && p.1 > 20.0 + 2.0 / SCALE
                && p.1 < 80.0 - 2.0 / SCALE;
            let outside = p.0 < 20.0 - 2.0 / SCALE
                || p.0 > 80.0 + 2.0 / SCALE
                || p.1 < 20.0 - 2.0 / SCALE
                || p.1 > 80.0 + 2.0 / SCALE;
            let c = coverage(image, x, y);
            if outside {
                assert!(
                    c < 0.02,
                    "pixel ({x}, {y}) outside the boundary is drawn: {c}"
                );
            } else if inside {
                let d = lattice_distance(p);
                if d < 0.3 {
                    assert!(c > 0.4, "pixel ({x}, {y}) on a line: {c}");
                    on += 1;
                } else if d > 1.6 {
                    assert!(c < 0.05, "pixel ({x}, {y}) between lines: {c}");
                    off += 1;
                }
            }
        }
    }
    (on, off)
}

#[test]
fn pattern_lines_lie_on_their_document_lattice_inside_the_boundary_and_stay_there_under_a_pan() {
    let mut f = Fixture::new();
    let space = f.space();
    let hatch = f.hatch(
        space,
        &[square(20.0, 20.0, 80.0, 80.0)],
        json!({"inline": [{"angle": 45.0, "spacing": 10.0}]}),
    );
    let options = DraftOptions::default();
    let snapshot = f.document.snapshot();
    // The extractor draws the pattern, not the solid fill a bounded generator falls back to.
    let ex = draft::extract_entity(&snapshot, hatch, &options).unwrap();
    assert!(
        matches!(&ex.extraction.items[..], [RenderItem::Curves(c)] if c.paths.len() >= 4),
        "{:?}",
        ex.extraction.items.len()
    );
    let (_gpu, mut scene, mut renderer) = load(&f, space, &options);
    let (on, off) = check_lattice(
        &render(&mut renderer, &mut scene, (50.0, 50.0)),
        (50.0, 50.0),
    );
    assert!(on > 100 && off > 1000, "{on} {off}");
    // Panned by 6.74 by 3.62 pixels: the same document lattice.
    let panned = (53.37, 48.19);
    let (on, off) = check_lattice(&render(&mut renderer, &mut scene, panned), panned);
    assert!(on > 100 && off > 1000, "{on} {off}");
    // A pixel on a line picks the hatch; one between lines picks nothing.
    let on_line = (40.0, 40.0 + 10.0 * std::f64::consts::SQRT_2);
    assert_eq!(
        pick(&mut renderer, &mut scene, (50.0, 50.0), on_line),
        vec![hatch]
    );
    assert!(
        pick(
            &mut renderer,
            &mut scene,
            (50.0, 50.0),
            (40.0, 40.0 + 5.0 * std::f64::consts::SQRT_2)
        )
        .is_empty()
    );
}

#[test]
fn a_large_pattern_hatch_is_one_item_of_one_draw() {
    let mut f = Fixture::new();
    let space = f.space();
    let hatch = f.hatch(
        space,
        &[square(0.0, 0.0, 1000.0, 1000.0)],
        json!({"inline": [{"angle": 0.0, "spacing": 0.5}]}),
    );
    let options = DraftOptions {
        limits: SceneLimits {
            max_items_per_object: 2,
            ..SceneLimits::DEFAULT
        },
        ..DraftOptions::default()
    };
    let ex = draft::extract_entity(&f.document.snapshot(), hatch, &options).unwrap();
    let [RenderItem::Curves(item)] = &ex.extraction.items[..] else {
        panic!("{} items", ex.extraction.items.len())
    };
    assert!(item.paths.len() >= 1999, "{}", item.paths.len());
    assert_eq!(ex.sub_objects.len(), 1);
    assert_eq!(ex.sub_objects[0].len(), item.paths.len());
    assert_eq!(ex.extraction.geometries.len(), 0);
}

#[test]
fn stroke_text_is_drawn_at_its_layout_positions() {
    let mut f = Fixture::new();
    let space = f.space();
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": "ARC", "position": [20.0, 30.0],
               "height": 10.0}),
    );
    let options = DraftOptions::default();
    let snapshot = f.document.snapshot();
    let list = space_list(
        &snapshot,
        Lists::Direct,
        space,
        &draft::display_options(&options),
    )
    .unwrap();
    let strokes: Vec<&Curve2> = list
        .primitives
        .iter()
        .filter(|p| p.source == text)
        .filter_map(|p| match &p.shape {
            Shape::Stroke { curve, .. } => Some(curve),
            _ => None,
        })
        .collect();
    assert!(strokes.len() >= 5, "{}", strokes.len());
    let mut bounds = strokes[0].bounding_box().unwrap();
    for c in &strokes {
        bounds = bounds.union(&c.bounding_box().unwrap());
    }
    let (_gpu, mut scene, mut renderer) = load(&f, space, &options);
    let center = (50.0, 50.0);
    let image = render(&mut renderer, &mut scene, center);
    // A known stroke: the middle of the first one is lit.
    let (t0, t1) = strokes[0].parameter_range();
    let mid = strokes[0].point_at(0.5 * (t0 + t1));
    let (x, y) = at((mid.x, mid.y), center);
    let near: f64 = (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
        .map(|(dx, dy)| coverage(&image, (x as i32 + dx) as u32, (y as i32 + dy) as u32))
        .fold(0.0, f64::max);
    assert!(near > 0.5, "{near}");
    // The lit pixels span the strokes within a pixel, which lie in the layout's bounds.
    let (x0, y0, x1, y1) = lit_extent(&image);
    let (lo, hi) = (
        at((bounds.min().x, bounds.max().y), center),
        at((bounds.max().x, bounds.min().y), center),
    );
    for (got, want, what) in [
        (x0, lo.0, "left"),
        (y0, lo.1, "top"),
        (x1, hi.0, "right"),
        (y1, hi.1, "bottom"),
    ] {
        assert!(
            (got - want).abs() <= 1.5,
            "{what}: lit {got}, strokes {want}"
        );
    }
    let layout = text_layout(&snapshot, text, &DisplayOptions::default()).unwrap();
    let (lb0, lb1) = layout.bounds.unwrap();
    let (lb0, lb1) = (
        at((20.0 + lb0.x, 30.0 + lb0.y), center),
        at((20.0 + lb1.x, 30.0 + lb1.y), center),
    );
    let (left, right) = (lb0.0.min(lb1.0), lb0.0.max(lb1.0));
    let (top, bottom) = (lb0.1.min(lb1.1), lb0.1.max(lb1.1));
    assert!(
        x0 >= left - 1.5 && x1 <= right + 1.5,
        "{x0} {x1} in {left} {right}"
    );
    assert!(
        y0 >= top - 1.5 && y1 <= bottom + 1.5,
        "{y0} {y1} in {top} {bottom}"
    );
    assert_eq!(
        pick(&mut renderer, &mut scene, center, (mid.x, mid.y)),
        vec![text]
    );
}

fn fonts() -> Fonts {
    let mut registry = FontRegistry::with_builtin().unwrap();
    registry.add_outline("Test Sans", Arc::from(test_font()), 0);
    Fonts {
        provider: Arc::new(registry),
        ..Fonts::default()
    }
}

fn outline_options() -> DraftOptions {
    DraftOptions {
        display: DisplayOptions {
            fonts: fonts(),
            ..DisplayOptions::default()
        },
        ..DraftOptions::default()
    }
}

fn outline_text(f: &mut Fixture, space: ObjectId, content: &str, position: [f64; 2]) -> ObjectId {
    let style = f.create(
        "core.draft.style.create",
        json!({"kind": "text", "name": format!("sans {}", f.seed)}),
    );
    f.set("core.draft.style.modify", style, "font", json!("Test Sans"));
    f.create(
        "core.draft.text.create",
        json!({"owner": space.to_string(), "content": content, "position": position,
               "style": style.to_string(), "height": 20.0}),
    )
}

#[test]
fn outline_text_fills_its_glyphs_at_the_layout_positions_and_wins_over_a_fill_beneath() {
    let mut f = Fixture::new();
    let space = f.space();
    // A solid hatch under the text, a line over the hatch away from the text.
    let hatch = f.hatch(
        space,
        &[square(10.0, 10.0, 90.0, 90.0)],
        json!({"kind": "solid"}),
    );
    let text = outline_text(&mut f, space, "ab", [20.0, 40.0]);
    let options = outline_options();
    let snapshot = f.document.snapshot();
    let list = space_list(
        &snapshot,
        Lists::Direct,
        space,
        &draft::display_options(&options),
    )
    .unwrap();
    let Some(Shape::Glyphs {
        size,
        glyphs,
        placement,
        ..
    }) = list
        .primitives
        .iter()
        .find(|p| p.source == text)
        .map(|p| p.shape.clone())
    else {
        panic!("no glyph run")
    };
    assert_eq!(glyphs.len(), 2);
    let objects = draft::space_extractions(&snapshot, space, &options).unwrap();
    let extraction = &objects.iter().find(|(o, _)| *o == text).unwrap().1;
    assert_eq!(extraction.skipped_glyphs, 0);
    // One item per distinct glyph, one sub-object per instance.
    assert_eq!(extraction.extraction.items.len(), 2);
    assert!(extraction.sub_objects.iter().all(|s| s.len() == 1));
    assert_eq!(options.glyphs.len(), 2);
    let (_gpu, mut scene, mut renderer) = setup();
    let report = draft::apply(&mut scene, snapshot.revision(), objects);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let center = (50.0, 50.0);
    let image = render(&mut renderer, &mut scene, center);
    // The hatch is white and so is the text: color the hatch gray through its alpha for contrast.
    // Here: only the hatch's own pixels away from the glyphs are checked by picking, and the
    // glyph squares by their pick below. Square of glyph id g: x 50..50+side, y 0..side (1/1000 em).
    for g in &glyphs {
        let side = f64::from(100 + 40 * g.id);
        let origin = placement * g.position;
        let (x0, x1) = (
            origin.x + size * 50.0 / 1000.0,
            origin.x + size * (50.0 + side) / 1000.0,
        );
        let (y0, y1) = (origin.y, origin.y + size * side / 1000.0);
        let center_of = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        // Inside the square the glyph is picked, not the hatch beneath it.
        assert_eq!(
            pick(&mut renderer, &mut scene, center, center_of),
            vec![text]
        );
        // Just outside it (1.5 pixels) the hatch is picked.
        let gap = 1.5 / SCALE;
        for p in [
            (x0 - gap, center_of.1),
            (x1 + gap, center_of.1),
            (center_of.0, y1 + gap),
        ] {
            assert_eq!(
                pick(&mut renderer, &mut scene, center, p),
                vec![hatch],
                "{p:?}"
            );
        }
    }
    assert!(coverage(&image, 100, 100) > 0.99);
}

#[test]
fn outline_glyphs_are_lit_inside_their_squares_and_dark_outside() {
    let mut f = Fixture::new();
    let space = f.space();
    let text = outline_text(&mut f, space, "ab", [20.0, 40.0]);
    let options = outline_options();
    let snapshot = f.document.snapshot();
    let list = space_list(
        &snapshot,
        Lists::Direct,
        space,
        &draft::display_options(&options),
    )
    .unwrap();
    let Some(Shape::Glyphs {
        size,
        glyphs,
        placement,
        ..
    }) = list
        .primitives
        .iter()
        .find(|p| p.source == text)
        .map(|p| p.shape.clone())
    else {
        panic!("no glyph run")
    };
    let (_gpu, mut scene, mut renderer) = load(&f, space, &options);
    let center = (50.0, 50.0);
    let image = render(&mut renderer, &mut scene, center);
    let mut squares = Vec::new();
    for g in &glyphs {
        let side = f64::from(100 + 40 * g.id);
        let origin = placement * g.position;
        squares.push((
            origin.x + size * 50.0 / 1000.0,
            origin.y,
            origin.x + size * (50.0 + side) / 1000.0,
            origin.y + size * side / 1000.0,
        ));
    }
    // 'a' is 2.8 units (5.6 pixels) wide at an em of 20 units.
    assert!((squares[0].2 - squares[0].0 - 2.8).abs() < 1e-9);
    let margin = 1.5 / SCALE;
    let (mut lit, mut dark) = (0, 0);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = point_of(x, y, center);
            let inside = squares.iter().any(|s| {
                p.0 > s.0 + margin && p.0 < s.2 - margin && p.1 > s.1 + margin && p.1 < s.3 - margin
            });
            let outside = squares.iter().all(|s| {
                p.0 < s.0 - margin || p.0 > s.2 + margin || p.1 < s.1 - margin || p.1 > s.3 + margin
            });
            let c = coverage(&image, x, y);
            if inside {
                assert!(c > 0.99, "pixel ({x}, {y}) inside a glyph: {c}");
                lit += 1;
            } else if outside {
                assert!(c < 0.01, "pixel ({x}, {y}) outside the glyphs: {c}");
                dark += 1;
            }
        }
    }
    assert!(lit >= 10 && dark > 30_000, "{lit} {dark}");
}

fn writes_of(scene: &Scene) -> arc_gpu::WriteCounters {
    scene.last_frame_writes()
}

#[test]
fn editing_a_hatch_or_a_text_writes_only_that_objects_buffers() {
    let mut f = Fixture::new();
    let space = f.space();
    let hatch = f.hatch(
        space,
        &[square(10.0, 10.0, 40.0, 40.0)],
        json!({"inline": [{"angle": 0.0, "spacing": 3.0}]}),
    );
    let text = outline_text(&mut f, space, "ab", [50.0, 50.0]);
    f.create(
        "core.draft.entity.create",
        json!({"owner": space.to_string(), "geometry": serde_json::to_value(Curve2::Segment(
            Segment2::new(Point2::new(60.0, 10.0), Point2::new(90.0, 10.0)).unwrap())).unwrap()}),
    );
    let options = outline_options();
    let (_gpu, mut scene, mut renderer) = load(&f, space, &options);
    let center = (50.0, 50.0);
    render(&mut renderer, &mut scene, center);
    assert_ne!(writes_of(&scene), arc_gpu::WriteCounters::default());
    render(&mut renderer, &mut scene, center);
    assert_eq!(writes_of(&scene), arc_gpu::WriteCounters::default());

    // Editing the hatch writes line segments only: no vertices, indices or mesh instances.
    let alone = |f: &Fixture, id: ObjectId| {
        let snapshot = f.document.snapshot();
        let ex = draft::extract_entity(&snapshot, id, &options).unwrap();
        let (_gpu, mut scene, mut renderer) = setup();
        draft::apply(&mut scene, snapshot.revision(), vec![(id, ex)]);
        render(&mut renderer, &mut scene, center);
        writes_of(&scene)
    };
    f.set("core.draft.hatch.modify", hatch, "scale", json!(0.5));
    let snapshot = f.document.snapshot();
    let ex = draft::extract_entity(&snapshot, hatch, &options).unwrap();
    let report = draft::apply(&mut scene, snapshot.revision(), vec![(hatch, ex)]);
    assert_eq!(report.applied, 1);
    render(&mut renderer, &mut scene, center);
    let w = writes_of(&scene);
    let expected = alone(&f, hatch);
    assert!(w.line_segment > 0);
    assert_eq!((w.vertex, w.index, w.mesh_instance), (0, 0, 0));
    assert_eq!(w.line_segment, expected.line_segment);

    // Editing the text writes vertices, indices and instances of its glyphs only.
    f.set("core.draft.entity.modify", text, "content", json!("aab"));
    let snapshot = f.document.snapshot();
    let ex = draft::extract_entity(&snapshot, text, &options).unwrap();
    let instances: u64 = ex
        .extraction
        .items
        .iter()
        .map(|i| match i {
            RenderItem::Mesh(m) => m.instances.len() as u64,
            _ => 0,
        })
        .sum();
    let vertices: u64 = ex
        .extraction
        .geometries
        .iter()
        .map(|(m, _)| m.vertex_count() as u64)
        .sum();
    assert_eq!((instances, ex.extraction.geometries.len()), (3, 2));
    let report = draft::apply(&mut scene, snapshot.revision(), vec![(text, ex)]);
    assert_eq!(report.applied, 1);
    render(&mut renderer, &mut scene, center);
    let w = writes_of(&scene);
    assert_eq!(w.line_segment, 0);
    assert_eq!(w.mesh_instance, instances * 80);
    assert_eq!(w.vertex, vertices * 24);
    assert!(w.index > 0);
    assert_eq!(w, alone(&f, text));
}

#[test]
fn outline_glyphs_keep_their_holes_and_follow_their_curves() {
    let mut f = Fixture::new();
    let space = f.space();
    let text = outline_text(&mut f, space, "cd", [20.0, 40.0]);
    let options = outline_options();
    let snapshot = f.document.snapshot();
    let list = space_list(
        &snapshot,
        Lists::Direct,
        space,
        &draft::display_options(&options),
    )
    .unwrap();
    let Some(Shape::Glyphs {
        size,
        glyphs,
        placement,
        ..
    }) = list
        .primitives
        .iter()
        .find(|p| p.source == text)
        .map(|p| p.shape.clone())
    else {
        panic!("no glyph run")
    };
    let (_gpu, mut scene, mut renderer) = load(&f, space, &options);
    let center = (50.0, 50.0);
    let image = render(&mut renderer, &mut scene, center);
    // A point in font units (x, y) of glyph `i` of the run, in the drawing.
    let doc = |i: usize, x: f64, y: f64| {
        let o = placement * glyphs[i].position;
        (o.x + size * x / 1000.0, o.y + size * y / 1000.0)
    };
    let value = |p: (f64, f64)| {
        let (x, y) = at(p, center);
        coverage(&image, x as u32, y as u32)
    };
    // 'c': the ring (hole from 100..200 by 50..150 of the outline 50..250 by 0..200).
    assert!(value(doc(0, 75.0, 100.0)) > 0.99);
    assert!(value(doc(0, 225.0, 100.0)) > 0.99);
    assert!(value(doc(0, 150.0, 25.0)) > 0.99);
    assert!(value(doc(0, 150.0, 100.0)) < 0.01, "the hole is filled");
    assert!(value(doc(0, 150.0, 100.0 + 30.0)) < 0.01);
    // 'd': the center and the middle of each side are inside; the corners are cut by the curve.
    assert!(value(doc(1, 150.0, 100.0)) > 0.99);
    assert!(value(doc(1, 215.0, 35.0)) > 0.99);
    assert!(
        value(doc(1, 245.0, 5.0)) < 0.01,
        "the corner is not rounded"
    );
    assert!(value(doc(1, 55.0, 195.0)) < 0.01);
    assert_eq!(options.glyphs.len(), 2);
}
