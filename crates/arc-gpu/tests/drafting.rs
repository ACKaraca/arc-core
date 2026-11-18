//! Drafting display lists (CORE-GPU-013) with GPU dashes (CORE-GPU-009): a model space with
//! lines, an arc, a polyline with widths, a block inserted twice and a wipeout checked at
//! chosen pixels with picking of a block member; dashes against the CPU dash ends, continuous
//! across a polyline vertex and fixed in the document under a pan; a layout sheet with one
//! viewport at 1:100 from meters to millimeters clipped to its rectangle; the registered
//! per-entity extractor.

#![cfg(feature = "draft")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "tests fail by panicking; pixel arithmetic on small images; one scene per test"
)]

mod support;

use std::sync::Arc;

use arc_base::math::{Point2, Point3};
use arc_base::{Color, ObjectId};
use arc_command::{Invocation, Invoker, Permission, Registry, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_draft::linetype::{DashOptions, Dashes, Pattern, dashes};
use arc_geom::{Arc2, Curve2, Polyline2, PolylineVertex, Segment2};
use arc_gpu::draft::{self, DraftOptions, SubObject};
use arc_gpu::{
    ExtractorRegistry, Image, PickRegion, RenderItem, RenderSettings, Renderer, Scene,
    WriteCounters,
};
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
        id(&self.run(command, params), "id")
    }

    fn space(&mut self, unit: &str) -> ObjectId {
        self.create(
            "core.draft.space.create",
            json!({"name": format!("model {}", self.seed), "kind": "model", "unit": unit}),
        )
    }

    fn entity(&mut self, owner: ObjectId, curve: &Curve2) -> ObjectId {
        self.create(
            "core.draft.entity.create",
            json!({"owner": owner.to_string(), "geometry": serde_json::to_value(curve).unwrap()}),
        )
    }

    fn set(&mut self, command: &str, objects: &[ObjectId], field: &str, value: Value) {
        let ids: Vec<String> = objects.iter().map(ToString::to_string).collect();
        self.run(
            command,
            json!({"objects": ids, "field": field, "value": value}),
        );
    }

    fn modify(&mut self, objects: &[ObjectId], field: &str, value: Value) {
        self.set("core.draft.entity.modify", objects, field, value);
    }
}

fn id(value: &Value, key: &str) -> ObjectId {
    value[key].as_str().unwrap().parse().unwrap()
}

fn segment(x0: f64, y0: f64, x1: f64, y1: f64) -> Curve2 {
    Curve2::Segment(Segment2::new(Point2::new(x0, y0), Point2::new(x1, y1)).unwrap())
}

fn polyline(points: &[(f64, f64)], closed: bool) -> Curve2 {
    let vertices = points
        .iter()
        .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
        .collect();
    Curve2::Polyline(Polyline2::new(vertices, closed).unwrap())
}

/// The pixel of a point of an orthographic top view centered on `center`, `scale` pixels per
/// unit, in an image of `size` pixels.
fn pixel(p: (f64, f64), center: (f64, f64), scale: f64, size: (u32, u32)) -> (u32, u32) {
    let x = (p.0 - center.0) * scale + f64::from(size.0) / 2.0;
    let y = (center.1 - p.1) * scale + f64::from(size.1) / 2.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (x.floor() as u32, y.floor() as u32)
}

fn render(
    renderer: &mut Renderer,
    scene: &mut Scene,
    center: (f64, f64),
    scale: f64,
    size: (u32, u32),
    settings: &RenderSettings,
) -> Image {
    let frame = top_view(
        Point3::new(center.0, center.1, 0.0),
        f64::from(size.1) / scale,
    );
    renderer
        .render_to_image(scene, &frame, size.0, size.1, settings)
        .unwrap()
}

#[test]
fn model_space_draws_lines_arcs_widths_blocks_and_wipeouts_and_picks_block_members() {
    let mut f = Fixture::new();
    let model = f.space("millimeter");
    // Two pixels per unit around (50, 50); coordinates ending in .25 or .75 are pixel centers.
    let line = f.entity(model, &segment(5.0, 89.75, 95.0, 89.75));
    let arc =
        Curve2::Arc(Arc2::new(Point2::new(25.25, 50.25), 10.0, 0.0, std::f64::consts::PI).unwrap());
    f.entity(model, &arc);
    let wide = Curve2::Polyline(
        Polyline2::new(
            vec![
                PolylineVertex {
                    point: Point2::new(60.25, 20.25),
                    bulge: 0.0,
                    start_width: 4.0,
                    end_width: 4.0,
                },
                PolylineVertex::new(Point2::new(90.25, 20.25), 0.0),
            ],
            false,
        )
        .unwrap(),
    );
    f.entity(model, &wide);
    let block = f.create("core.draft.block.define", json!({"name": "b"}));
    let member = f.entity(block, &segment(0.0, 0.0, 5.0, 0.0));
    let insert = |f: &mut Fixture, position: [f64; 2], scale: f64, rotation: f64| {
        f.create(
            "core.draft.block.insert",
            json!({"owner": model.to_string(), "block": block.to_string(), "position": position,
                   "scale": [scale, scale, 1.0], "rotation": rotation}),
        )
    };
    let r1 = insert(&mut f, [10.25, 30.25], 1.0, 0.0);
    let r2 = insert(&mut f, [30.25, 30.25], 2.0, std::f64::consts::FRAC_PI_2);
    // A line under a wipeout, then a line over it.
    f.entity(model, &segment(5.0, 70.25, 95.0, 70.25));
    f.run(
        "core.draft.wipeout.create",
        json!({"owner": model.to_string(),
               "boundary": serde_json::to_value(polyline(&[(40.0, 65.0), (60.0, 65.0), (60.0, 75.0), (40.0, 75.0)], true)).unwrap()}),
    );
    f.entity(model, &segment(50.25, 60.0, 50.25, 80.0));

    let snapshot = f.document.snapshot();
    let options = DraftOptions::default();
    let objects = draft::space_extractions(&snapshot, model, &options).unwrap();
    let r2_subs = objects
        .iter()
        .find(|(o, _)| *o == r2)
        .map(|(_, d)| d.sub_objects.clone())
        .unwrap();
    let (_gpu, mut scene, mut renderer) = setup();
    let report = draft::apply(&mut scene, snapshot.revision(), objects);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let (center, scale, size) = ((50.0, 50.0), 2.0, (200, 200));
    let settings = RenderSettings::default();
    let image = render(&mut renderer, &mut scene, center, scale, size, &settings);
    let at = |p: (f64, f64)| {
        let (x, y) = pixel(p, center, scale, size);
        coverage(&image, x, y)
    };
    let lit = |p: (f64, f64)| assert!(at(p) > 0.9, "{p:?} is not drawn: {}", at(p));
    let dark = |p: (f64, f64)| assert!(at(p) < 0.02, "{p:?} is drawn: {}", at(p));
    // The line, one pixel wide (0.25 mm at 96 pixels per inch).
    lit((50.25, 89.75));
    dark((50.25, 88.75));
    // The arc's top and its empty center.
    lit((25.25, 60.25));
    dark((25.25, 50.75));
    // The polyline 4 units (8 pixels) wide.
    lit((75.25, 20.25 + 1.5));
    lit((75.25, 20.25 - 1.5));
    dark((75.25, 20.25 + 2.5));
    dark((75.25, 20.25 - 2.5));
    // The block at scale 1 and at scale 2 turned a quarter.
    lit((12.75, 30.25));
    dark((17.75, 30.25));
    lit((30.25, 35.25));
    lit((30.25, 39.75));
    dark((35.25, 30.25));
    // The wipeout hides the line beneath it, not the line above it.
    lit((20.25, 70.25));
    dark((45.25, 70.25));
    lit((50.25, 72.25));
    lit((50.25, 70.25));

    // Picking a pixel of the second insertion's member gives the reference and the member.
    let (x, y) = pixel((30.25, 37.25), center, scale, size);
    let frame = top_view(Point3::new(50.0, 50.0, 0.0), 100.0);
    let hits = renderer
        .pick(
            &mut scene,
            &frame,
            200,
            200,
            &settings,
            &PickRegion::pixel(x, y, 1.0),
        )
        .unwrap();
    let [hit] = hits[..] else { panic!("{hits:?}") };
    assert_eq!(hit.key.object, r2);
    let path = scene.path_of(&hit).unwrap();
    assert_eq!(
        r2_subs[hit.key.sub as usize][path as usize],
        SubObject {
            source: r2,
            path: vec![member],
            part: 0
        }
    );
    let (x, y) = pixel((60.25, 89.75), center, scale, size);
    let hits = renderer
        .pick(
            &mut scene,
            &frame,
            200,
            200,
            &settings,
            &PickRegion::pixel(x, y, 1.0),
        )
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.key.object).collect::<Vec<_>>(),
        vec![line]
    );
    assert_ne!(r1, r2);
}

/// The dashes along row `y` between columns `x0..x1`: (center, length) in pixels from the
/// coverage of each run summed over rows `y − 1` to `y + 1`, exact for runs whose ends are
/// more than a pixel apart.
fn runs(image: &Image, y: u32, columns: std::ops::Range<u32>) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (mut sum, mut moment) = (0.0, 0.0);
    for x in columns {
        let c: f64 = (y - 1..=y + 1).map(|r| coverage(image, x, r)).sum();
        if c > 1e-3 {
            sum += c;
            moment += c * (f64::from(x) + 0.5);
        } else if sum > 0.0 {
            out.push((moment / sum, sum));
            (sum, moment) = (0.0, 0.0);
        }
    }
    if sum > 0.0 {
        out.push((moment / sum, sum));
    }
    out
}

#[test]
fn gpu_dashes_match_the_cpu_dash_ends_continue_across_vertices_and_stay_fixed_under_a_pan() {
    let mut f = Fixture::new();
    let model = f.space("millimeter");
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dash-dot", "elements": [
            {"kind": "dash", "length": 2.0}, {"kind": "gap", "length": 0.75},
            {"kind": "dot", "length": 0.0}, {"kind": "gap", "length": 0.75}]}),
    );
    // A polyline with a corner at (17.25, 0.25), one pattern over both legs, linetype scale
    // 1.5: the dash from 15.75 to 18.75 along the path spans the corner.
    let path = polyline(&[(0.25, 0.25), (17.25, 0.25), (17.25, 12.25)], false);
    let entity = f.entity(model, &path);
    f.modify(&[entity], "linetype_by", json!("object"));
    f.modify(&[entity], "linetype", json!(dashed.to_string()));
    f.modify(&[entity], "linetype_scale", json!(1.5));
    f.modify(&[entity], "continuous_pattern", json!(true));
    let snapshot = f.document.snapshot();
    let options = DraftOptions::default();
    let objects = draft::space_extractions(&snapshot, model, &options).unwrap();
    let [(_, extraction)] = &objects[..] else {
        panic!("{objects:?}")
    };
    let [RenderItem::Curves(item)] = &extraction.extraction.items[..] else {
        panic!("not drawn by the shader: {extraction:?}")
    };
    assert!(item.pattern.is_some());
    let (_gpu, mut scene, mut renderer) = setup();
    draft::apply(&mut scene, snapshot.revision(), objects);

    // The CPU path: exact dash ends along the polyline.
    let pattern = Pattern::from_values(
        &snapshot
            .get(snapshot.resolve(dashed).unwrap())
            .unwrap()
            .values,
    )
    .unwrap();
    let dash_options = DashOptions {
        object_scale: 1.5,
        continuous: true,
        ..DashOptions::default()
    };
    let Dashes::Pieces(cpu) =
        dashes(&path, &pattern, &dash_options, &options.display.tolerance).unwrap()
    else {
        panic!("continuous")
    };

    let scale = 16.0;
    let size = (384, 32);
    let settings = RenderSettings::default();
    // Measured in document x along the first leg; y = 0.25 is the center of a pixel row.
    let measure = |renderer: &mut Renderer, scene: &mut Scene, center: (f64, f64)| {
        let image = render(renderer, scene, center, scale, size, &settings);
        let (_, row) = pixel((0.0, 0.25), center, scale, size);
        runs(&image, row, 0..size.0)
            .into_iter()
            .map(|(c, l)| (center.0 + (c - f64::from(size.0) / 2.0) / scale, l / scale))
            .collect::<Vec<_>>()
    };
    let center = (11.0, 0.25 + 0.5 / scale);
    // Away from the corner, where the second leg adds coverage to the band.
    let corner = 17.25;
    let measured: Vec<(f64, f64)> = measure(&mut renderer, &mut scene, center)
        .into_iter()
        .filter(|(c, l)| c + l / 2.0 < corner - 0.5)
        .collect();
    let pixel_doc = 1.0 / scale;
    // Expected along the first leg: dashes cut at the corner, dots as disks one pixel wide.
    let leg = 17.0;
    let left = center.0 - f64::from(size.0) / 2.0 / scale;
    let expected: Vec<(f64, f64)> = cpu
        .iter()
        .filter(|d| 0.25 + d.start.max(d.end.min(leg)) < corner - 0.5 && 0.25 + d.start >= left)
        .map(|d| {
            let end = d.end.min(leg);
            if d.start == d.end {
                (0.25 + d.start, pixel_doc)
            } else {
                (0.25 + (d.start + end) / 2.0, end - d.start)
            }
        })
        .collect();
    assert_eq!(measured.len(), expected.len(), "{measured:?}\n{expected:?}");
    for ((mc, ml), (ec, el)) in measured.iter().zip(&expected) {
        // Within a pixel of the CPU ends: both the center and the length.
        assert!((mc - ec).abs() < pixel_doc, "{mc} vs {ec}");
        assert!((ml - el).abs() < pixel_doc, "{ml} vs {el}");
    }
    // The dash that reaches the corner continues up the second leg by the CPU's amount.
    let spanning = cpu.iter().find(|d| d.start < leg && d.end > leg).unwrap();
    let beyond = spanning.end - leg;
    let image = render(
        &mut renderer,
        &mut scene,
        (17.0, 6.0),
        scale,
        (32, 384),
        &settings,
    );
    let (column, _) = pixel((17.25, 6.0), (17.0, 6.0), scale, (32, 384));
    let up: f64 = (0..384)
        .filter(|y| {
            let doc_y = 6.0 + (192.0 - (f64::from(*y) + 0.5)) / scale;
            doc_y > 0.25 + 0.5 && doc_y < 0.25 + beyond + 0.5
        })
        .map(|y| {
            (column - 1..=column + 1)
                .map(|x| coverage(&image, x, y))
                .sum::<f64>()
        })
        .sum::<f64>()
        / scale;
    // The covered length above the corner region up to half a unit past the dash end.
    assert!(
        (up - (beyond - 0.5)).abs() < 2.0 * pixel_doc,
        "{up} vs {beyond}"
    );

    // A pan of 13.37 pixels: the same dashes in document space, to a tenth of a pixel (a phase
    // fixed on the screen would move them by 13.37 pixels).
    let panned = measure(
        &mut renderer,
        &mut scene,
        (center.0 + 13.37 / scale, center.1),
    );
    // Away from the round start cap and the corner, whose coverage is not linear in a shift.
    let inside = |x: f64| x > 4.0 && x < corner - 2.0;
    let a: Vec<_> = measured.iter().filter(|(c, _)| inside(*c)).collect();
    let b: Vec<_> = panned.iter().filter(|(c, _)| inside(*c)).collect();
    assert_eq!(a.len(), b.len());
    for ((ac, al), (bc, bl)) in a.iter().zip(&b) {
        // Dash ends move linearly with a shift; a dot is a sampled disk of one pixel.
        let bound = if *al > 2.0 * pixel_doc { 0.1 } else { 0.5 } * pixel_doc;
        assert!((ac - bc).abs() < bound, "{ac} vs {bc}");
        assert!((al - bl).abs() < bound, "{al} vs {bl}");
    }
    assert_eq!(scene.last_frame_writes().line_segment, 0);
}

#[test]
fn a_sheet_draws_its_paper_and_a_viewport_at_one_to_one_hundred_clipped_to_its_rectangle() {
    let mut f = Fixture::new();
    let model = f.space("meter");
    let red = json!([1.0, 0.0, 0.0, 1.0]);
    // A long line through the viewport, a thick one reaching its right edge, a short one.
    // Model y = k + 0.025 m is sheet y = 100.25 + 10 k mm, a pixel center.
    let long = f.entity(model, &segment(-20.0, 0.025, 20.0, 0.025));
    let thick = f.entity(model, &segment(0.0, -1.975, 20.0, -1.975));
    let short = f.entity(model, &segment(1.0, 1.025, 2.0, 1.025));
    f.modify(&[long, thick, short], "color_by", json!("object"));
    f.modify(&[long, thick, short], "color", red);
    f.modify(&[thick], "lineweight_by", json!("object"));
    f.modify(&[thick], "lineweight", json!(2.0));
    let created = f.run(
        "core.draft.layout.create",
        json!({"name": "sheet", "paper_width": 297.0, "paper_height": 210.0}),
    );
    let (sheet_space, layout) = (id(&created, "space"), id(&created, "layout"));
    for field in ["margin_left", "margin_bottom", "margin_right", "margin_top"] {
        f.set("core.draft.layout.modify", &[layout], field, json!(10.0));
    }
    // 1:100 from meters to millimeters: one model meter is ten sheet millimeters.
    let viewport = f.create(
        "core.draft.viewport.create",
        json!({"layout": sheet_space.to_string(), "model": model.to_string(),
               "center": [100.0, 100.0], "width": 80.0, "height": 60.0,
               "view_center": [0.0, 0.0], "scale": 0.01}),
    );
    let snapshot = f.document.snapshot();
    let sheet = draft::sheet_extractions(&snapshot, sheet_space, &DraftOptions::default()).unwrap();
    assert_eq!(sheet.size, (297.0, 210.0));
    assert!((sheet.plot_scale - 1.0).abs() < 1e-12);
    let keys: Vec<ObjectId> = sheet.objects.iter().map(|(o, _)| *o).collect();
    assert_eq!(keys, vec![layout, viewport]);
    let (_gpu, mut scene, mut renderer) = setup();
    draft::apply(&mut scene, snapshot.revision(), sheet.objects);
    let settings = RenderSettings {
        background: Color::new(0.2, 0.2, 0.2, 1.0),
        plot_scale: sheet.plot_scale,
        ..RenderSettings::default()
    };
    // Two pixels per millimeter, the sheet centered.
    let (center, scale, size) = ((148.5, 105.0), 2.0, (640, 460));
    let image = render(&mut renderer, &mut scene, center, scale, size, &settings);
    let color = |p: (f64, f64)| {
        let (x, y) = pixel(p, center, scale, size);
        image.pixel(x, y).unwrap()
    };
    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    let gray = Color::new(0.2, 0.2, 0.2, 1.0).to_srgb8();
    // Paper and the area outside it.
    assert_eq!(color((20.25, 20.25)), WHITE);
    assert_eq!(color((-3.0, 105.0)), gray);
    assert_eq!(color((300.0, 105.0)), gray);
    // The printable-area outline at the margins is gray on the paper.
    let margin = color((10.25, 105.25));
    assert!(margin[0] < 240 && margin[0] == margin[1], "{margin:?}");
    // The long line inside the clip and not outside it.
    assert_eq!(color((100.25, 100.25)), RED);
    assert_eq!(color((139.75, 100.25)), RED);
    assert_eq!(color((140.75, 100.25)), WHITE);
    assert_eq!(color((59.25, 100.25)), WHITE);
    assert_eq!(color((150.25, 100.25)), WHITE);
    // The short line at 1:100: model (1..2, 1.025) m is sheet (110..120, 110.25) mm.
    assert_eq!(color((115.25, 110.25)), RED);
    assert_eq!(color((121.25, 110.25)), WHITE);
    // The 2 mm line ends at the clip edge: its width and round cap are scissored there.
    assert_eq!(color((139.75, 80.25)), RED);
    assert_eq!(color((139.75, 80.25 + 0.75)), RED);
    assert_eq!(color((140.25, 80.25)), WHITE);
    assert_eq!(color((140.75, 80.25 + 0.5)), WHITE);

    // Lineweight display off: the thick line is one pixel wide, and nothing is written.
    let thin = RenderSettings {
        lineweights: false,
        ..settings
    };
    let image = render(&mut renderer, &mut scene, center, scale, size, &thin);
    assert_eq!(scene.last_frame_writes(), WriteCounters::default());
    let (x, y) = pixel((120.25, 80.25 + 0.75), center, scale, size);
    assert_eq!(image.pixel(x, y), Some(WHITE));
    let (x, y) = pixel((120.25, 80.25), center, scale, size);
    assert_eq!(image.pixel(x, y), Some(RED));
}

#[test]
fn the_registered_extractor_draws_entities_of_spaces_but_not_of_block_definitions() {
    let mut f = Fixture::new();
    let model = f.space("millimeter");
    let line = f.entity(model, &segment(0.0, 0.0, 4.0, 0.0));
    let block = f.create("core.draft.block.define", json!({"name": "b"}));
    let member = f.entity(block, &segment(0.0, 0.0, 1.0, 0.0));
    let mut registry = ExtractorRegistry::new();
    draft::register_extractors(&mut registry, DraftOptions::default()).unwrap();
    let snapshot = f.document.snapshot();
    let extract = |id| draft::extract_entity(&snapshot, id, &DraftOptions::default()).unwrap();
    assert_eq!(extract(line).extraction.items.len(), 1);
    assert!(extract(member).extraction.items.is_empty());
    // The registry runs the same extractor on the worker pool.
    let driver = arc_gpu::ExtractionDriver::new(registry, 1).unwrap();
    driver.submit(&snapshot, [line]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut results = Vec::new();
    while results.is_empty() && std::time::Instant::now() < deadline {
        results.extend(driver.take_results());
        std::thread::yield_now();
    }
    let (_gpu, mut scene, _renderer) = setup();
    let report = scene.apply_extractions(results);
    assert_eq!(report.applied, 1);
    assert_eq!(scene.keys().len(), 1);
}
