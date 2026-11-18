//! The phase 1 scenario "DXF open, display and plot": a hand-written DXF file is imported with
//! `core.draft.import.dxf`, shown in a layout viewport and plotted with `core.draft.plot` to PDF,
//! SVG and PNG, and the outputs are read back at chosen paper positions.

#![allow(
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_base::ObjectId;
use arc_command::{Invocation, Invoker, Permission, Registry, Roots, Session, Surface};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, DocumentConfig, Snapshot, TypeRegistry};
use serde_json::{Value, json};

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

struct Env {
    document: Document,
    commands: Registry,
    session: Session,
    dir: PathBuf,
    seed: u64,
}

impl Env {
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
        for validator in arc_draft::validate::validators(arc_draft::validate::DEFAULT_MAX_NESTING) {
            document.add_validator(validator);
        }
        let mut commands = Registry::new();
        arc_draft::register_commands(&mut commands).unwrap();
        arc_draft_dxf::register_commands(&mut commands).unwrap();
        arc_io_pdf::register_commands(&mut commands).unwrap();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("arc-io-pdf-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = Session::new(
            Surface::Automation,
            [
                Permission::Edit,
                Permission::Read,
                Permission::FileRead,
                Permission::FileWrite,
            ],
        );
        session.roots = Roots::new([&dir], [&dir]).unwrap();
        Self {
            document,
            commands,
            session,
            dir,
            seed: 1,
        }
    }

    fn try_run(&mut self, command: &str, params: Value) -> arc_base::Result<Value> {
        self.seed += 1;
        let invocation = Invocation {
            seed: self.seed,
            ..Invocation::new(command, params)
        };
        Invoker::new(&self.commands)
            .invoke(&self.document, &self.session, invocation)
            .map(|o| o.value)
    }

    fn run(&mut self, command: &str, params: Value) -> Value {
        self.try_run(command, params).unwrap()
    }

    fn id(value: &Value, key: &str) -> ObjectId {
        value[key].as_str().unwrap().parse().unwrap()
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Imports the DXF into a model space and builds a landscape A4 layout with one viewport.
/// Returns the environment and the layout space.
fn scene() -> (Env, ObjectId) {
    let mut env = Env::new();
    let model = Env::id(
        &env.run(
            "core.draft.space.create",
            json!({"name": "model", "kind": "model", "unit": "millimeter"}),
        ),
        "id",
    );
    let file = env.path("drawing.dxf");
    std::fs::write(&file, dxf()).unwrap();
    let imported = env.run(
        "core.draft.import.dxf",
        json!({"path": file, "space": model.to_string()}),
    );
    assert_eq!(imported["entities"], 8, "{imported}");
    let created = env.run(
        "core.draft.layout.create",
        json!({"name": "sheet", "paper_width": 210.0, "paper_height": 297.0,
               "orientation": "landscape"}),
    );
    let layout = Env::id(&created, "space");
    env.run(
        "core.draft.viewport.create",
        json!({"layout": layout.to_string(), "model": model.to_string(),
               "center": [148.5, 105.0], "width": 260.0, "height": 170.0,
               "view_center": [80.0, 30.0], "scale": 1.5}),
    );
    (env, layout)
}

fn latin1(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| if b.is_ascii() { char::from(*b) } else { '?' })
        .collect()
}

#[test]
fn pdf_has_one_page_of_the_sheet_size_with_strokes_and_fills() {
    let (mut env, layout) = scene();
    let result = env.run(
        "core.draft.plot",
        json!({"layouts": [layout.to_string()], "format": "pdf", "path": env.path("out.pdf")}),
    );
    assert_eq!(result["pages"], 1);
    assert_eq!(result["paths"], json!([env.path("out.pdf")]));
    assert_eq!(result["reports"][0]["header"]["format"], "pdf");
    assert_eq!(
        result["reports"][0]["header"]["file_name"],
        env.path("out.pdf")
    );
    let bytes = std::fs::read(env.path("out.pdf")).unwrap();
    let text = latin1(&bytes);
    assert!(text.starts_with("%PDF-1.7"));
    // Minimal parse: the cross-reference table points at every object and one page exists.
    let tail = text.rsplit_once("startxref\n").unwrap().1;
    let xref_at: usize = tail.lines().next().unwrap().parse().unwrap();
    assert!(text[xref_at..].starts_with("xref\n"));
    assert_eq!(text.matches("/Type /Page ").count(), 1);
    // 297 x 210 mm in points.
    assert!(text.contains("/MediaBox [0 0 841.8898 595.2756]"), "{text}");
    let content = text.split("stream\n").nth(1).unwrap();
    for operator in [" m\n", " l\n", " c\n", "\nS\n", "\nf\n", " RG\n", " rg\n"] {
        assert!(content.contains(operator), "no {operator:?}");
    }
}

#[cfg(feature = "raster")]
mod raster {
    use super::*;

    /// Where the model point `(x, y)` lands on the sheet: the viewport shows `(80, 30)` at the
    /// sheet point `(148.5, 105)` at scale 1.5.
    fn sheet(x: f64, y: f64) -> (f64, f64) {
        (148.5 + 1.5 * (x - 80.0), 105.0 + 1.5 * (y - 30.0))
    }

    /// The image of a PNG file as rows of RGBA.
    struct Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        dpi: f64,
    }

    impl Image {
        fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
            let at = ((y * self.width + x) * 4) as usize;
            self.rgba[at..at + 4].try_into().unwrap()
        }

        /// The pixel of a sheet point in millimeters.
        fn at(&self, x: f64, y: f64) -> (u32, u32) {
            let k = self.dpi / 25.4;
            ((x * k) as u32, (f64::from(self.height) - y * k) as u32)
        }

        /// The pixels within one pixel of the sheet point that are not the paper color.
        fn ink(&self, x: f64, y: f64) -> Vec<[u8; 4]> {
            let (px, py) = self.at(x, y);
            let mut found = Vec::new();
            for dy in -1_i64..=1 {
                for dx in -1_i64..=1 {
                    let (cx, cy) = (i64::from(px) + dx, i64::from(py) + dy);
                    if cx < 0
                        || cy < 0
                        || cx >= i64::from(self.width)
                        || cy >= i64::from(self.height)
                    {
                        continue;
                    }
                    let p = self.pixel(cx as u32, cy as u32);
                    if p != [255, 255, 255, 255] {
                        found.push(p);
                    }
                }
            }
            found
        }

        /// The number of pixels satisfying `want` in a box of sheet millimeters.
        fn count(&self, from: (f64, f64), to: (f64, f64), want: impl Fn([u8; 4]) -> bool) -> usize {
            let (x0, y1) = self.at(from.0, from.1);
            let (x1, y0) = self.at(to.0, to.1);
            let mut n = 0;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if want(self.pixel(x, y)) {
                        n += 1;
                    }
                }
            }
            n
        }

        /// The number of inked pixels in a box of sheet millimeters.
        fn ink_in(&self, from: (f64, f64), to: (f64, f64)) -> usize {
            self.count(from, to, |p| p != [255, 255, 255, 255])
        }
    }

    const DPI: f64 = 100.0;

    fn png(env: &mut Env, layout: ObjectId, name: &str) -> Image {
        let result = env.run(
            "core.draft.plot",
            json!({"layouts": [layout.to_string()], "format": "png", "path": env.path(name),
                   "dpi": DPI}),
        );
        assert_eq!(result["pages"], 1);
        assert_eq!(result["reports"][0]["header"]["format"], "png");
        let decoded = image::load_from_memory(&std::fs::read(env.path(name)).unwrap())
            .unwrap()
            .to_rgba8();
        Image {
            width: decoded.width(),
            height: decoded.height(),
            rgba: decoded.into_raw(),
            dpi: DPI,
        }
    }

    /// The probes every format must agree on: sheet point and whether it has ink.
    fn probes() -> Vec<(&'static str, (f64, f64), bool)> {
        let on = |name, (x, y)| (name, sheet(x, y), true);
        let off = |name, (x, y)| (name, sheet(x, y), false);
        vec![
            on("bottom edge", (50.0, 0.0)),
            on("top edge", (50.0, 60.0)),
            on("left edge", (0.0, 30.0)),
            on("right edge", (100.0, 30.0)),
            on("arc top", (130.0, 50.0)),
            on(
                "arc diagonal",
                (130.0 + 20.0 * 0.5_f64.sqrt(), 30.0 + 20.0 * 0.5_f64.sqrt()),
            ),
            on("dimension line left", (30.0, -15.0)),
            on("dimension line right", (70.0, -15.0)),
            on("hatch", (50.0, 17.0)),
            on("hatch corner", (22.0, 12.0)),
            off("rectangle interior", (50.0, 45.0)),
            off("inside the arc's chord", (130.0, 20.0)),
            off("hatch outside", (50.0, 30.0)),
            off("right of the drawing", (170.0, 30.0)),
            off("paper corner", (-60.0, -50.0)),
        ]
    }

    #[test]
    fn png_has_ink_where_the_drawing_is_and_paper_color_elsewhere() {
        let (mut env, layout) = scene();
        let image = png(&mut env, layout, "out.png");
        // 297 mm at 100 dpi, rounded up.
        assert_eq!((image.width, image.height), (1170, 827));
        for (name, (x, y), expect_ink) in probes() {
            let ink = image.ink(x, y);
            assert_eq!(
                !ink.is_empty(),
                expect_ink,
                "{name} at ({x:.1}, {y:.1}) mm: {ink:?}"
            );
        }
        // Red walls: the rectangle edge and the hatch interior are red.
        let wall = image.ink(sheet(50.0, 0.0).0, sheet(50.0, 0.0).1);
        assert!(
            wall.iter().any(|p| p[0] > 200 && p[1] < 80 && p[2] < 80),
            "{wall:?}"
        );
        let (hx, hy) = image.at(sheet(50.0, 17.0).0, sheet(50.0, 17.0).1);
        assert_eq!(image.pixel(hx, hy), [255, 0, 0, 255]);
        // The text on the white layer plots black (the paper rule) inside its bounding area
        // and nowhere above it.
        let (x0, y0) = sheet(9.0, 69.0);
        let (x1, y1) = sheet(45.0, 80.0);
        assert!(image.ink_in((x0, y0), (x1, y1)) > 30);
        let blacks = image.count((x0, y0), (x1, y1), |p| p[0] < 40 && p[1] < 40 && p[2] < 40);
        assert!(blacks > 0, "the white text is not plotted black");
        let (a0, b0) = sheet(0.0, 84.0);
        let (a1, b1) = sheet(60.0, 100.0);
        assert_eq!(image.ink_in((a0, b0), (a1, b1)), 0);
    }

    #[test]
    fn svg_rendered_with_resvg_agrees_with_the_png_at_the_probes() {
        let (mut env, layout) = scene();
        let image = png(&mut env, layout, "out.png");
        let result = env.run(
            "core.draft.plot",
            json!({"layouts": [layout.to_string()], "format": "svg", "path": env.path("out.svg")}),
        );
        assert_eq!(result["reports"][0]["header"]["format"], "svg");
        let svg = std::fs::read_to_string(env.path("out.svg")).unwrap();
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        // Render at the PNG's resolution: usvg sizes millimeters at 96 dpi.
        let k = (DPI / 96.0) as f32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(image.width, image.height).unwrap();
        let shift = image.height as f32 - tree.size().height() * k;
        pixmap.fill(resvg::tiny_skia::Color::WHITE);
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(k, k).post_translate(0.0, shift),
            &mut pixmap.as_mut(),
        );
        let from_svg = Image {
            width: image.width,
            height: image.height,
            rgba: pixmap.data().to_vec(),
            dpi: DPI,
        };
        for (name, (x, y), expect_ink) in probes() {
            assert_eq!(!from_svg.ink(x, y).is_empty(), expect_ink, "svg: {name}");
        }
        // The same colors at the hatch interior.
        let (hx, hy) = image.at(sheet(50.0, 17.0).0, sheet(50.0, 17.0).1);
        let (a, b) = (image.pixel(hx, hy), from_svg.pixel(hx, hy));
        assert!(
            a.iter().zip(b).all(|(p, q)| p.abs_diff(q) <= 2),
            "{a:?} {b:?}"
        );
    }

    #[test]
    fn several_layouts_give_one_numbered_file_each_and_one_pdf() {
        let (mut env, layout) = scene();
        let again = Env::id(
            &env.run(
                "core.draft.layout.create",
                json!({"name": "second", "paper_width": 100.0, "paper_height": 100.0}),
            ),
            "space",
        );
        let ids = json!([layout.to_string(), again.to_string()]);
        let result = env.run(
            "core.draft.plot",
            json!({"layouts": ids, "format": "png", "path": env.path("set.png"), "dpi": 20}),
        );
        assert_eq!(
            result["paths"],
            json!([env.path("set-1.png"), env.path("set-2.png")])
        );
        assert_eq!(result["reports"].as_array().unwrap().len(), 2);
        assert!(std::fs::metadata(env.path("set-2.png")).is_ok());
        assert!(std::fs::metadata(env.path("set.png")).is_err());
        let pdf = env.run(
            "core.draft.plot",
            json!({"layouts": ids, "format": "pdf", "path": env.path("set.pdf")}),
        );
        assert_eq!(
            (pdf["pages"].clone(), pdf["paths"].as_array().unwrap().len()),
            (json!(2), 1)
        );
        let text = latin1(&std::fs::read(env.path("set.pdf")).unwrap());
        assert_eq!(text.matches("/Type /Page ").count(), 2);
    }
}

#[cfg(not(feature = "raster"))]
#[test]
fn png_without_the_feature_is_a_specific_error() {
    let (mut env, layout) = scene();
    let error = env
        .try_run(
            "core.draft.plot",
            json!({"layouts": [layout.to_string()], "format": "png", "path": env.path("a.png")}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "io.pdf.raster_unavailable");
}

#[test]
fn a_model_window_plots_and_options_override_the_layout() {
    let (mut env, layout) = scene();
    let model = {
        let snapshot = env.document.snapshot();
        let v = arc_draft::layout::view(
            &snapshot,
            snapshot
                .iter()
                .find(|(h, _)| {
                    snapshot.type_key(*h).map(arc_doc::TypeKey::as_str)
                        == Some("core.draft.viewport")
                })
                .map(|(_, o)| o.id)
                .unwrap(),
        )
        .unwrap();
        v.model
    };
    let result = env.run(
        "core.draft.plot",
        json!({"model": {"space": model.to_string(), "window": [-5.0, -20.0, 155.0, 80.0],
                         "scale": 1.0, "paper": [200.0, 120.0]},
               "format": "svg", "path": env.path("m.svg"),
               "options": {"lineweights": false, "title": "Window"}}),
    );
    assert_eq!(result["pages"], 1);
    let svg = std::fs::read_to_string(env.path("m.svg")).unwrap();
    assert!(svg.contains("width=\"200mm\"") && svg.contains("<title>Window</title>"));
    // Layout overrides: a rotated, fitted plot is still a valid SVG of the sheet size.
    let rotated = env.run(
        "core.draft.plot",
        json!({"layouts": [layout.to_string()], "format": "svg", "path": env.path("r.svg"),
               "options": {"fit": true, "rotation": 1}}),
    );
    assert_eq!(rotated["pages"], 1);
}

#[test]
fn bad_parameters_have_specific_codes_and_write_nothing() {
    let (mut env, layout) = scene();
    let l = layout.to_string();
    let path = env.path("x.pdf");
    let cases = [
        (json!({"format": "pdf", "path": path}), "layouts"),
        (
            json!({"layouts": [l], "model": {"space": l, "window": [0,0,1,1], "scale": 1.0},
                   "format": "pdf", "path": path}),
            "model",
        ),
        (
            json!({"layouts": [l], "format": "pdf", "path": path, "dpi": 0.5}),
            "dpi",
        ),
        (
            json!({"layouts": [l], "format": "pdf", "path": path,
                   "options": {"scale": -1.0}}),
            "options.scale",
        ),
        (
            json!({"layouts": [l], "format": "pdf", "path": path,
                   "options": {"area": "window"}}),
            "options.window",
        ),
        (
            json!({"layouts": [l], "format": "pdf", "path": path,
                   "options": {"fit": true, "scale": 1.0}}),
            "options.scale",
        ),
    ];
    for (params, what) in cases {
        let error = env.try_run("core.draft.plot", params).unwrap_err();
        assert_eq!(error.code(), "io.pdf.plot_params_invalid", "{what}");
    }
    assert!(std::fs::metadata(&path).is_err());
    // Outside the write root: refused by the context.
    let outside = std::env::temp_dir().join("arc-io-pdf-outside.pdf");
    let error = env
        .try_run(
            "core.draft.plot",
            json!({"layouts": [l], "format": "pdf", "path": outside.to_str().unwrap()}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.path.outside_roots");
    assert!(!outside.exists());
    // Without the file_write class the command is not permitted.
    env.session.permissions.remove(&Permission::FileWrite);
    let error = env
        .try_run(
            "core.draft.plot",
            json!({"layouts": [l], "format": "pdf", "path": path}),
        )
        .unwrap_err();
    assert_eq!(error.code(), "cmd.permission_denied");
}
