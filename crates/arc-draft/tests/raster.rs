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

/// The fields a test needs from a file written by `Raster::tiff`.
struct Tiff {
    width: u32,
    height: u32,
    ifd: u32,
    resolution: (u32, u32, u32, u32),
    rgb: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Reads the baseline RGB, one-strip little-endian TIFF that `Raster::tiff` writes, and checks
/// the tags it must carry.
fn read_tiff(b: &[u8]) -> Tiff {
    assert_eq!(&b[0..2], b"II");
    assert_eq!(u16_at(b, 2), 42);
    let ifd = u32_at(b, 4);
    let count = usize::from(u16_at(b, ifd as usize));
    let mut tags = std::collections::BTreeMap::new();
    let mut previous = 0;
    for i in 0..count {
        let at = ifd as usize + 2 + i * 12;
        let tag = u16_at(b, at);
        assert!(tag > previous, "tags are in ascending order");
        previous = tag;
        tags.insert(
            tag,
            (u16_at(b, at + 2), u32_at(b, at + 4), u32_at(b, at + 8)),
        );
    }
    assert_eq!(
        u32_at(b, ifd as usize + 2 + count * 12),
        0,
        "no next directory"
    );
    let width = tags[&256].2;
    let height = tags[&257].2;
    assert_eq!(tags[&259].2, 1, "uncompressed");
    assert_eq!(tags[&262].2, 2, "RGB");
    assert_eq!(tags[&277].2, 3, "three samples");
    assert_eq!(tags[&284].2, 1, "chunky");
    assert_eq!(tags[&296].2, 2, "inches");
    assert_eq!(tags[&278].2, height, "one strip");
    let bits = tags[&258].2 as usize;
    for k in 0..3 {
        assert_eq!(u16_at(b, bits + 2 * k), 8);
    }
    let rational = |tag: u16| {
        let at = tags[&tag].2 as usize;
        (u32_at(b, at), u32_at(b, at + 4))
    };
    let (xn, xd) = rational(282);
    let (yn, yd) = rational(283);
    let offset = tags[&273].2 as usize;
    let bytes = tags[&279].2 as usize;
    assert_eq!(bytes, width as usize * height as usize * 3);
    Tiff {
        width,
        height,
        ifd,
        resolution: (xn, xd, yn, yd),
        rgb: b[offset..offset + bytes].to_vec(),
    }
}

#[test]
fn tiff_pixels_equal_the_png_pixels_and_the_header_fields_are_written() {
    let mut f = Fixture::new();
    let sheet = layout(&mut f);
    let line = f.entity(sheet.0, segment_json(20.0, 50.0, 60.0, 50.0));
    support::pen(&mut f, line, BLACK, 0.5, None);
    let stream = plot_layout(&f, sheet.0);
    let options = RasterOptions {
        dpi: 50.0,
        ..RasterOptions::default()
    };
    let r = render(&stream, &options).unwrap();
    let tiff = read_tiff(&r.tiff().unwrap());
    assert_eq!((tiff.width, tiff.height), (r.width, r.height));
    // 50 dpi: 500 thousandths of a pixel per inch with the denominator 1000.
    assert_eq!(tiff.resolution, (50_000, 1000, 50_000, 1000));
    assert_eq!(tiff.ifd % 2, 0, "the directory starts at an even offset");

    let decoded = image::load_from_memory_with_format(&r.png().unwrap(), image::ImageFormat::Png)
        .unwrap()
        .to_rgb8();
    assert_eq!(decoded.as_raw(), &tiff.rgb);
    let expected: Vec<u8> = r
        .rgba
        .chunks_exact(4)
        .flat_map(|px| px[..3].to_vec())
        .collect();
    assert_eq!(tiff.rgb, expected);
    // Identical renders write identical files.
    assert_eq!(
        r.tiff().unwrap(),
        render(&stream, &options).unwrap().tiff().unwrap()
    );
}

#[test]
fn tiff_refuses_sizes_and_resolutions_it_cannot_write() {
    let huge = Raster {
        width: u32::MAX,
        height: u32::MAX,
        rgba: Vec::new(),
        skipped: 0,
        dpi: 300.0,
    };
    assert_eq!(huge.tiff().unwrap_err().code(), "draft.raster.too_large");
    // Three bytes per pixel and 32-bit offsets: 2^31 pixels is too large.
    let wide = Raster {
        width: 1 << 16,
        height: 1 << 15,
        rgba: Vec::new(),
        skipped: 0,
        dpi: 300.0,
    };
    assert_eq!(wide.tiff().unwrap_err().code(), "draft.raster.too_large");
    let mismatched = Raster {
        width: 2,
        height: 2,
        rgba: vec![0; 15],
        skipped: 0,
        dpi: 300.0,
    };
    assert_eq!(
        mismatched.tiff().unwrap_err().code(),
        "draft.raster.invalid"
    );
    let no_dpi = Raster {
        width: 1,
        height: 1,
        rgba: vec![0; 4],
        skipped: 0,
        dpi: 0.0,
    };
    assert_eq!(no_dpi.tiff().unwrap_err().code(), "draft.raster.invalid");
}

#[test]
fn the_tiff_limit_is_the_most_pixels_a_classic_file_holds() {
    // Three bytes per pixel, and the header, the resolutions and the directory around them:
    // (2^32 - 1 - 193) / 3 pixels.
    let limit: u64 = 1_431_655_700;
    let raster = |width: u32, height: u32| Raster {
        width,
        height,
        rgba: Vec::new(),
        skipped: 0,
        dpi: 300.0,
    };
    // At the limit the size is accepted; the empty pixels are then the only fault.
    let at = raster(limit as u32, 1).tiff().unwrap_err();
    assert_eq!(at.code(), "draft.raster.invalid");
    // One pixel more is too large, and the limit reported is the real maximum.
    let over = raster(limit as u32 + 1, 1).tiff().unwrap_err();
    assert_eq!(over.code(), "draft.raster.too_large");
    let reported = format!("{:?}", over.arguments().get("limit"));
    assert!(reported.contains("1431655700"), "{reported}");
}
