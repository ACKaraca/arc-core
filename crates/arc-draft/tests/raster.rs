//! Raster output with analytic pixel checks (feature `raster`).

#![cfg(feature = "raster")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test code"
)]

mod support;

use arc_base::Color;
use arc_base::math::Point2;
use arc_draft::display::{self, DisplayOptions, Lists};
use arc_draft::linetype::{DashOptions, Dashes, Pattern, dashes};
use arc_draft::plot::{
    Offset, Paper, PlotArea, PlotOptions, PlotScale, PlotStream, layout_stream, model_stream,
};
use arc_draft::raster::{Raster, RasterOptions, render};
use serde_json::json;
use support::{BLACK, Fixture, Scene, plot_scene, segment_json};

const DPI: f64 = 300.0;
const PX_PER_MM: f64 = DPI / 25.4;

fn layout_plot(scene: &Scene, options: &PlotOptions) -> PlotStream {
    let snapshot = scene.f.document.snapshot();
    layout_stream(
        &snapshot,
        Lists::Direct,
        scene.sheet,
        options,
        &DisplayOptions::default(),
    )
    .unwrap()
}

fn rendered(stream: &PlotStream) -> Raster {
    render(stream, &RasterOptions::default()).unwrap()
}

/// The darkness of a pixel from 0 (paper) to 1 (black), for black ink on white paper.
fn ink(r: &Raster, x: u32, y: u32) -> f64 {
    let p = r.pixel(x, y).unwrap();
    1.0 - f64::from(p[0]) / 255.0
}

/// The column of pixel centers for the page coordinate `mm`.
fn px(mm: f64) -> u32 {
    (mm * PX_PER_MM) as u32
}

/// The row of the pixel containing the page height `y` mm.
fn row(r: &Raster, y: f64) -> u32 {
    r.height - 1 - (y * PX_PER_MM) as u32
}

#[test]
fn the_background_is_the_paper_color_and_the_image_covers_the_page() {
    let mut scene = plot_scene(Fixture::new());
    let options = PlotOptions {
        paper_color: Color::from_srgb8([250, 240, 230, 255]),
        ..PlotOptions::from_layout(&scene.f.document.snapshot(), scene.sheet).unwrap()
    };
    let r = rendered(&layout_plot(&scene, &options));
    // 297 x 210 mm at 300 dpi.
    assert_eq!((r.width, r.height), (3508, 2481));
    assert_eq!(r.pixel(0, 0), Some([250, 240, 230, 255]));
    assert_eq!(
        r.pixel(r.width - 1, r.height - 1),
        Some([250, 240, 230, 255])
    );
    assert_eq!(r.rgba.len(), 3508 * 2481 * 4);
    assert!(r.pixel(r.width, 0).is_none());
    let _ = &mut scene;
}

#[test]
fn a_half_millimeter_pen_at_300_dpi_is_about_six_pixels_wide() {
    let mut f = Fixture::new();
    let sheet = layout(&mut f);
    let line = f.entity(sheet.0, segment_json(20.0, 50.0, 120.0, 50.0));
    support::pen(&mut f, line, BLACK, 0.5, None);
    let r = rendered(&plot_layout(&f, sheet.0));
    // Sum the ink down a column across the line: the coverage is the width in pixels.
    let x = px(70.0);
    let width: f64 = (0..r.height).map(|y| ink(&r, x, y)).sum();
    let expected = 0.5 * PX_PER_MM;
    assert!((expected - 5.906).abs() < 1e-3);
    assert!((width - expected).abs() < 0.25, "{width} vs {expected}");
    // The line is where it should be, y = 50 mm, and absent elsewhere.
    assert!(ink(&r, x, row(&r, 50.0)) > 0.9);
    assert_eq!(ink(&r, x, row(&r, 60.0)), 0.0);
    // A hairline is one pixel wide at least.
    f.set(
        "core.draft.entity.modify",
        &[line],
        "lineweight",
        json!(0.0),
    );
    let r = rendered(&plot_layout(&f, sheet.0));
    let width: f64 = (0..r.height).map(|y| ink(&r, x, y)).sum();
    assert!((width - 1.0).abs() < 0.25, "{width}");
}

fn layout(f: &mut Fixture) -> (arc_base::ObjectId, arc_base::ObjectId) {
    let created = f
        .run(
            "core.draft.layout.create",
            json!({"name": "sheet", "paper_width": 210.0, "paper_height": 297.0, "orientation": "landscape"}),
        )
        .unwrap();
    (
        created["space"].as_str().unwrap().parse().unwrap(),
        created["layout"].as_str().unwrap().parse().unwrap(),
    )
}

fn plot_layout(f: &Fixture, sheet: arc_base::ObjectId) -> PlotStream {
    let snapshot = f.document.snapshot();
    let options = PlotOptions::from_layout(&snapshot, sheet).unwrap();
    layout_stream(
        &snapshot,
        Lists::Direct,
        sheet,
        &options,
        &DisplayOptions::default(),
    )
    .unwrap()
}

#[test]
fn dash_and_gap_lengths_in_pixels_match_the_linetype_generation() {
    let mut f = Fixture::new();
    let sheet = layout(&mut f);
    let dashed = f.create(
        "core.draft.linetype.load",
        json!({"name": "dashed", "elements": [
            {"kind": "dash", "length": 5.0}, {"kind": "gap", "length": 2.5}]}),
    );
    let line = f.entity(sheet.0, segment_json(20.0, 50.0, 120.0, 50.0));
    support::pen(&mut f, line, BLACK, 0.5, Some(dashed));
    let r = rendered(&plot_layout(&f, sheet.0));

    // The expected runs, from the generator itself, in pixels.
    let curve = arc_geom::Curve2::Segment(
        arc_geom::Segment2::new(Point2::new(20.0, 50.0), Point2::new(120.0, 50.0)).unwrap(),
    );
    let pattern = Pattern::from_values(&[
        None,
        None,
        Some(arc_doc::Value::List(vec![
            arc_doc::Value::Record(vec![
                Some(arc_doc::Value::Enum(0)),
                Some(arc_doc::Value::F64(5.0)),
            ]),
            arc_doc::Value::Record(vec![
                Some(arc_doc::Value::Enum(1)),
                Some(arc_doc::Value::F64(2.5)),
            ]),
        ])),
    ])
    .unwrap();
    let Dashes::Pieces(pieces) = dashes(
        &curve,
        &pattern,
        &DashOptions::default(),
        &arc_base::Tolerance::DEFAULT,
    )
    .unwrap() else {
        panic!("continuous");
    };
    let expected: Vec<(f64, f64)> = pieces
        .iter()
        .map(|d| ((20.0 + d.start) * PX_PER_MM, (20.0 + d.end) * PX_PER_MM))
        .collect();
    assert_eq!(expected.len(), 14);

    // The runs of ink along the pen's center row.
    let y = row(&r, 50.0);
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut start: Option<f64> = None;
    for x in 0..r.width {
        let dark = ink(&r, x, y) > 0.5;
        match (dark, start) {
            (true, None) => start = Some(f64::from(x)),
            (false, Some(s)) => {
                runs.push((s, f64::from(x)));
                start = None;
            }
            _ => {}
        }
    }
    assert_eq!(runs.len(), expected.len(), "{runs:?}");
    for ((got_a, got_b), (want_a, want_b)) in runs.iter().zip(&expected) {
        assert!((got_a - want_a).abs() <= 1.5, "{got_a} vs {want_a}");
        assert!((got_b - want_b).abs() <= 1.5, "{got_b} vs {want_b}");
    }
    // A dash is 59 pixels, a gap 30.
    assert!(((runs[0].1 - runs[0].0) - 5.0 * PX_PER_MM).abs() <= 2.0);
    assert!(((runs[1].0 - runs[0].1) - 2.5 * PX_PER_MM).abs() <= 2.0);
}

#[test]
fn a_wipeout_hides_what_is_beneath_it_and_a_clip_stops_the_content() {
    let mut scene = plot_scene(Fixture::new());
    // The viewport's model line is dashed; use the long solid one, at sheet y = 120.
    let boundary = json!({"kind": "polyline", "closed": true, "vertices": [
        {"point": [140.0, 110.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [160.0, 110.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [160.0, 130.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [140.0, 130.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}]});
    let options = PlotOptions::from_layout(&scene.f.document.snapshot(), scene.sheet).unwrap();
    let before = rendered(&layout_plot(&scene, &options));
    let y = row(&before, 120.0);
    assert!(ink(&before, px(145.0), y) > 0.9, "the line is drawn");
    scene.f.create(
        "core.draft.wipeout.create",
        json!({"owner": scene.sheet.to_string(), "boundary": boundary}),
    );
    let after = rendered(&layout_plot(&scene, &options));
    // Hidden inside the wipeout, kept on both sides.
    assert_eq!(after.pixel(px(145.0), y), Some([255, 255, 255, 255]));
    assert_eq!(after.pixel(px(155.0), y), Some([255, 255, 255, 255]));
    assert!(ink(&after, px(135.0), y) > 0.9);
    assert!(ink(&after, px(165.0), y) > 0.9);

    // The far line leaves the viewport at x = 250 mm: ink up to the clip, none beyond.
    assert!(ink(&after, px(249.0), y) > 0.9);
    assert_eq!(ink(&after, px(251.0), y), 0.0);
    assert_eq!(ink(&after, px(280.0), y), 0.0);
    // Nothing is drawn outside the viewport rectangle in y either: the model line at y = 100
    // ends inside, and the rows above the clip (y > 150 mm) are paper.
    let top = row(&after, 160.0);
    assert!((0..after.width).all(|x| ink(&after, x, top) == 0.0));
}

#[test]
fn text_laid_out_by_the_shared_layout_lands_at_its_paper_position() {
    let mut f = Fixture::new();
    let model = f.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "millimeter"}),
    );
    let text = f.create(
        "core.draft.text.create",
        json!({"owner": model.to_string(), "content": "HIM", "position": [30.0, 20.0], "height": 8.0}),
    );
    support::pen(&mut f, text, BLACK, 0.35, None);
    let snapshot = f.document.snapshot();
    let laid = display::text_layout(&snapshot, text, &DisplayOptions::default()).unwrap();
    let (lo, hi) = laid.bounds.unwrap();
    // Model millimeters at 1:1 onto a 100 x 60 mm paper: the box is placed at the position.
    let paper = Paper {
        width: 100.0,
        height: 60.0,
        margins: [0.0; 4],
    };
    let options = PlotOptions {
        area: PlotArea::Window {
            min: Point2::new(0.0, 0.0),
            max: Point2::new(100.0, 60.0),
        },
        scale: PlotScale::Ratio(1.0),
        offset: Offset::Origin(Point2::new(0.0, 0.0)),
        ..PlotOptions::default()
    };
    let stream = model_stream(
        &snapshot,
        Lists::Direct,
        model,
        &paper,
        &options,
        &DisplayOptions::default(),
    )
    .unwrap();
    let r = rendered(&stream);
    // Bounding box of the dark pixels, in millimeters on the page.
    let (mut x0, mut x1, mut y0, mut y1) = (u32::MAX, 0, u32::MAX, 0);
    for y in 0..r.height {
        for x in 0..r.width {
            if ink(&r, x, y) > 0.5 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    assert!(x0 < x1, "something is drawn");
    let mm = |p: u32| f64::from(p) / PX_PER_MM;
    let (left, right) = (mm(x0), mm(x1 + 1));
    let (bottom, top) = (60.0 - mm(y1 + 1), 60.0 - mm(y0));
    // The glyphs lie inside the line box placed at the text position, give or take the pen.
    let pen = 0.35;
    assert!(
        left >= 30.0 + lo.x - pen && right <= 30.0 + hi.x + pen,
        "{left} {right}"
    );
    assert!(
        bottom >= 20.0 + lo.y - pen && top <= 20.0 + hi.y + pen,
        "{bottom} {top}"
    );
    // And they fill most of it: the text is at its place, not merely inside the page.
    assert!(right - left > 0.6 * (hi.x - lo.x));
    assert!(top - bottom > 0.6 * 8.0);
    // The baseline is at y = 20: the letters stand on it.
    assert!((bottom - 20.0).abs() < 1.0, "{bottom}");
}

#[test]
fn png_bytes_decode_to_the_same_pixels_and_options_are_checked() {
    let mut f = Fixture::new();
    let sheet = layout(&mut f);
    let line = f.entity(sheet.0, segment_json(20.0, 50.0, 60.0, 50.0));
    support::pen(&mut f, line, BLACK, 0.5, None);
    let stream = plot_layout(&f, sheet.0);
    let r = render(
        &stream,
        &RasterOptions {
            dpi: 50.0,
            ..RasterOptions::default()
        },
    )
    .unwrap();
    let bytes = r.png().unwrap();
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .unwrap()
        .to_rgba8();
    assert_eq!((decoded.width(), decoded.height()), (r.width, r.height));
    assert_eq!(decoded.as_raw(), &r.rgba);
    // Identical streams render to identical pixels.
    assert_eq!(
        render(
            &stream,
            &RasterOptions {
                dpi: 50.0,
                ..RasterOptions::default()
            }
        )
        .unwrap(),
        r
    );

    for dpi in [0.0, 0.5, 20_000.0, f64::NAN] {
        let err = render(
            &stream,
            &RasterOptions {
                dpi,
                ..RasterOptions::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "draft.raster.invalid");
    }
    let err = render(
        &stream,
        &RasterOptions {
            dpi: 600.0,
            max_pixels: 1000,
        },
    )
    .unwrap_err();
    assert_eq!(err.code(), "draft.raster.too_large");
}
