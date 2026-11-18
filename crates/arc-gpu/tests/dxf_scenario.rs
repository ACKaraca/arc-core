//! The phase 1 scenario "DXF open, display and plot", display half: a hand-written DXF file is
//! imported with `core.draft.import.dxf` and the model space is drawn through
//! `draft::space_extractions` on the software adapter; pixels are lit at the model positions of
//! the rectangle edges, the arc, the text, the dimension line and the hatch interior.

#![cfg(feature = "draft")]
#![allow(
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_base::ObjectId;
use arc_base::math::Point3;
use arc_command::{Invocation, Invoker, Permission, Registry, Roots, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use arc_gpu::draft::{self, DraftOptions};
use arc_gpu::{Image, RenderSettings};
use serde_json::{Value, json};
use support::{setup, top_view};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The DXF text: group code and value pairs. Millimeters; a rectangle (red layer WALLS), an
/// upper half circle, a solid hatch inside the rectangle, and on layer NOTES (color 7, white)
/// a text and a rotated dimension below the rectangle.
fn dxf() -> String {
    let mut out = String::new();
    let mut g = |code: i32, value: &str| out.push_str(&format!("{code:>3}\n{value}\n"));
    let line = |g: &mut dyn FnMut(i32, &str), a: (f64, f64), b: (f64, f64)| {
        g(0, "LINE");
        g(8, "WALLS");
        for (code, v) in [
            (10, a.0),
            (20, a.1),
            (30, 0.0),
            (11, b.0),
            (21, b.1),
            (31, 0.0),
        ] {
            g(code, &v.to_string());
        }
    };
    g(0, "SECTION");
    g(2, "HEADER");
    g(9, "$ACADVER");
    g(1, "AC1015");
    g(9, "$INSUNITS");
    g(70, "4");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "TABLES");
    g(0, "TABLE");
    g(2, "LTYPE");
    g(70, "1");
    g(0, "LTYPE");
    g(2, "CONTINUOUS");
    g(70, "0");
    g(3, "Solid line");
    g(72, "65");
    g(73, "0");
    g(40, "0.0");
    g(0, "ENDTAB");
    g(0, "TABLE");
    g(2, "LAYER");
    g(70, "2");
    for (name, color) in [("WALLS", "1"), ("NOTES", "7")] {
        g(0, "LAYER");
        g(2, name);
        g(70, "0");
        g(62, color);
        g(6, "CONTINUOUS");
    }
    g(0, "ENDTAB");
    g(0, "ENDSEC");
    g(0, "SECTION");
    g(2, "ENTITIES");
    line(&mut g, (0.0, 0.0), (100.0, 0.0));
    line(&mut g, (100.0, 0.0), (100.0, 60.0));
    line(&mut g, (100.0, 60.0), (0.0, 60.0));
    line(&mut g, (0.0, 60.0), (0.0, 0.0));
    // The upper half of a circle of radius 20 around (130, 30).
    g(0, "ARC");
    g(8, "WALLS");
    for (code, v) in [
        (10, "130"),
        (20, "30"),
        (30, "0"),
        (40, "20"),
        (50, "0"),
        (51, "180"),
    ] {
        g(code, v);
    }
    g(0, "TEXT");
    g(8, "NOTES");
    for (code, v) in [(10, "10"), (20, "70"), (30, "0"), (40, "8")] {
        g(code, v);
    }
    g(1, "ARC");
    // A rotated dimension of the bottom edge, its line 15 below.
    g(0, "DIMENSION");
    g(8, "NOTES");
    g(2, "*D1");
    for (code, v) in [
        (10, "100"),
        (20, "-15"),
        (30, "0"),
        (11, "50"),
        (21, "-15"),
        (31, "0"),
    ] {
        g(code, v);
    }
    g(70, "0");
    g(3, "Standard");
    for (code, v) in [
        (13, "0"),
        (23, "0"),
        (33, "0"),
        (14, "100"),
        (24, "0"),
        (34, "0"),
    ] {
        g(code, v);
    }
    g(50, "0");
    // A solid hatch with one polyline boundary: (20, 10) to (80, 25), on WALLS.
    g(0, "HATCH");
    g(8, "WALLS");
    g(100, "AcDbHatch");
    for (code, v) in [
        (10, "0"),
        (20, "0"),
        (30, "0"),
        (210, "0"),
        (220, "0"),
        (230, "1"),
    ] {
        g(code, v);
    }
    g(2, "SOLID");
    for (code, v) in [
        (70, "1"),
        (71, "0"),
        (91, "1"),
        (92, "7"),
        (72, "0"),
        (73, "1"),
    ] {
        g(code, v);
    }
    g(93, "4");
    for (x, y) in [(20.0, 10.0), (80.0, 10.0), (80.0, 25.0), (20.0, 25.0)] {
        g(10, &x.to_string());
        g(20, &y.to_string());
    }
    for (code, v) in [(97, "0"), (75, "0"), (76, "1"), (98, "0")] {
        g(code, v);
    }
    g(0, "ENDSEC");
    g(0, "EOF");
    out
}

const WIDTH: u32 = 600;
const HEIGHT: u32 = 400;
/// Pixels per millimeter: the view is 120 millimeters tall, centered on (75, 30).
const K: f64 = 400.0 / 120.0;

/// The pixel of a model point.
fn at(x: f64, y: f64) -> (i64, i64) {
    (
        ((x - 75.0) * K + f64::from(WIDTH) / 2.0) as i64,
        ((30.0 - y) * K + f64::from(HEIGHT) / 2.0) as i64,
    )
}

/// The brightest pixel within two pixels of a model point, as RGB.
fn lit(image: &Image, x: f64, y: f64) -> [u8; 3] {
    let (px, py) = at(x, y);
    let mut best = [0_u8; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let (cx, cy) = (px + dx, py + dy);
            if cx < 0 || cy < 0 {
                continue;
            }
            if let Some(p) = image.pixel(cx as u32, cy as u32)
                && p[0].max(p[1]).max(p[2]) > best[0].max(best[1]).max(best[2])
            {
                best = [p[0], p[1], p[2]];
            }
        }
    }
    best
}

fn is_lit(image: &Image, x: f64, y: f64) -> bool {
    lit(image, x, y).iter().any(|c| *c > 60)
}

fn drawing() -> (Document, ObjectId, PathBuf) {
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
    for validator in arc_draft::validate::validators(arc_draft::validate::DEFAULT_MAX_NESTING) {
        document.add_validator(validator);
    }
    let mut commands = Registry::new();
    arc_draft::register_commands(&mut commands).unwrap();
    arc_draft_dxf::register_commands(&mut commands).unwrap();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("arc-gpu-dxf-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::new(
        Surface::Automation,
        [Permission::Edit, Permission::Read, Permission::FileRead],
    );
    session.roots = Roots::new([&dir], std::iter::empty::<&PathBuf>()).unwrap();
    let run = |command: &str, params: Value, seed: u64| -> Value {
        let invocation = Invocation {
            seed,
            ..Invocation::new(command, params)
        };
        Invoker::new(&commands)
            .invoke(&document, &session, invocation)
            .unwrap()
            .value
    };
    let model: ObjectId = run(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "millimeter"}),
        2,
    )["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let file = dir.join("drawing.dxf");
    std::fs::write(&file, dxf()).unwrap();
    let imported = run(
        "core.draft.import.dxf",
        json!({"path": file.to_str().unwrap(), "space": model.to_string()}),
        3,
    );
    assert_eq!(imported["entities"], 8, "{imported}");
    (document, model, dir)
}

#[test]
fn an_imported_dxf_drawing_lights_pixels_at_its_model_positions() {
    let (document, model, dir) = drawing();
    let snapshot = document.snapshot();
    let objects = draft::space_extractions(&snapshot, model, &DraftOptions::default()).unwrap();
    let (_gpu, mut scene, mut renderer) = setup();
    let report = draft::apply(&mut scene, snapshot.revision(), objects);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let frame = top_view(Point3::new(75.0, 30.0, 0.0), 120.0);
    let image = renderer
        .render_to_image(
            &mut scene,
            &frame,
            WIDTH,
            HEIGHT,
            &RenderSettings::default(),
        )
        .unwrap();
    let _ = std::fs::remove_dir_all(dir);

    // The rectangle edges, red.
    for (x, y) in [(50.0, 0.0), (50.0, 60.0), (0.0, 30.0), (100.0, 30.0)] {
        let p = lit(&image, x, y);
        assert!(
            p[0] > 150 && p[1] < 80 && p[2] < 80,
            "edge ({x}, {y}): {p:?}"
        );
    }
    // The arc.
    let d = 20.0 * 0.5_f64.sqrt();
    for (x, y) in [(130.0, 50.0), (130.0 + d, 30.0 + d), (130.0 - d, 30.0 + d)] {
        assert!(is_lit(&image, x, y), "arc ({x}, {y})");
    }
    // The hatch interior is solid red, exactly at the pixel.
    let (hx, hy) = at(50.0, 17.0);
    let p = image.pixel(hx as u32, hy as u32).unwrap();
    assert!(p[0] > 200 && p[1] < 40 && p[2] < 40, "hatch {p:?}");
    // The dimension line.
    for x in [30.0, 70.0] {
        assert!(is_lit(&image, x, -15.0), "dimension line at {x}");
    }
    // The text: lit pixels in its bounding area, none above it.
    let (x0, y1) = at(9.0, 69.0);
    let (x1, y0) = at(45.0, 80.0);
    let count = |x0: i64, y0: i64, x1: i64, y1: i64| {
        let mut n = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if let Some(p) = image.pixel(x as u32, y as u32)
                    && p[0].max(p[1]).max(p[2]) > 60
                {
                    n += 1;
                }
            }
        }
        n
    };
    assert!(count(x0, y0, x1, y1) > 30);
    let (a0, b1) = at(0.0, 84.0);
    let (a1, b0) = at(60.0, 100.0);
    assert_eq!(count(a0, b0, a1, b1), 0);
    // Unlit: the rectangle interior away from the hatch and the space right of the drawing.
    for (x, y) in [
        (50.0, 45.0),
        (50.0, 30.0),
        (130.0, 20.0),
        (160.0 - 5.0, 70.0),
    ] {
        assert!(!is_lit(&image, x, y), "({x}, {y}) is lit");
    }
}
