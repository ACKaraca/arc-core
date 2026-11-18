//! SVG output: determinism, structure, agreement with the raster output, limits and hostile input.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "tests"
)]

mod common;

use arc_base::{Cancel, Color, Limits};
use arc_draft::plot::ItemKind;
use arc_draft::raster::{RasterOptions, render};
use arc_geom::{Curve2, Polyline2, PolylineVertex};
use arc_io_pdf::{WriteOptions, write_svg};
use common::{
    PAPER, assert_report_valid, empty, glyphs, ids, nurbs, p, pen, reasons, ring, scene, segment,
    stroke,
};

fn text(stream: &arc_draft::plot::PlotStream) -> String {
    let out = write_svg(stream, &mut WriteOptions::default()).unwrap();
    assert_report_valid(&out.report);
    String::from_utf8(out.bytes).unwrap()
}

fn code(result: arc_base::Result<arc_io_pdf::Output>) -> (&'static str, String) {
    let e = result.unwrap_err();
    (
        e.code(),
        e.arguments()
            .values()
            .filter_map(|v| match v {
                arc_base::error::Value::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(","),
    )
}

#[test]
fn identical_streams_give_identical_bytes() {
    let a = write_svg(&scene(), &mut WriteOptions::default()).unwrap();
    let b = write_svg(&scene(), &mut WriteOptions::default()).unwrap();
    assert_eq!(a, b);
    assert!(
        !String::from_utf8(a.bytes)
            .unwrap()
            .to_lowercase()
            .contains("date")
    );
}

#[test]
fn page_is_in_millimeters_with_the_y_axis_flipped_and_arcs_exact() {
    let svg = text(&scene());
    assert!(svg.contains("width=\"100mm\" height=\"80mm\" viewBox=\"0 0 100 80\""));
    // The line at y = 40 stays at 40; the arc from (40, 20) to (20, 20) is one exact command
    // with sweep flag 0 (counterclockwise on the page), at y = 60 in the flipped frame.
    assert!(svg.contains("d=\"M10 40L90 40\""), "{svg}");
    assert!(svg.contains("d=\"M40 60A10 10 0 0 0 20 60\""), "{svg}");
    assert!(svg.contains("stroke=\"#0000ff\" stroke-width=\"1\" stroke-linecap=\"butt\""));
    assert!(svg.contains("fill=\"#ff0000\" fill-rule=\"nonzero\""));
    assert!(svg.contains("<clipPath id=\"c0\">") && svg.contains("<g clip-path=\"url(#c0)\">"));
}

#[test]
fn dashes_are_the_streams_pieces_not_a_dash_array() {
    let svg = text(&scene());
    assert!(!svg.contains("dasharray"));
    assert!(svg.contains("d=\"M10 30L30 30M50 30L70 30\""), "{svg}");
}

#[test]
fn full_circles_and_ellipses_are_two_half_arcs() {
    let id = ids(1);
    let mut s = empty(100.0, 100.0);
    let circle = arc_geom::Circle2::new(p(50.0, 50.0), 10.0).unwrap();
    s.items = vec![stroke(
        id[0],
        Curve2::Circle(circle),
        pen(Color::BLACK, 0.5),
    )];
    let svg = text(&s);
    assert!(
        svg.contains("d=\"M60 50A10 10 0 0 0 40 50A10 10 0 0 0 60 50Z\""),
        "{svg}"
    );
}

#[test]
fn nurbs_are_flattened_and_reported_with_the_tolerance() {
    let id = ids(1);
    let mut s = empty(100.0, 100.0);
    s.items = vec![stroke(id[0], nurbs(), pen(Color::BLACK, 0.5))];
    let out = write_svg(&s, &mut WriteOptions::default()).unwrap();
    assert_report_valid(&out.report);
    let svg = String::from_utf8(out.bytes).unwrap();
    assert!(svg.matches('L').count() > 10 && !svg.contains("<path d=\"M10 90C"));
    assert!(reasons(&out.report).contains(&"io.pdf.nurbs_flattened"));
    let item = out
        .report
        .items
        .iter()
        .find(|i| i.reason == "io.pdf.nurbs_flattened")
        .unwrap();
    assert_eq!(
        item.arguments["tolerance_mm"],
        arc_io_pdf::loss::Argument::Text("0.005".into())
    );
    // A finer tolerance gives more vertices.
    let mut fine = WriteOptions {
        chord_mm: 0.0005,
        ..WriteOptions::default()
    };
    let finer = write_svg(&s, &mut fine).unwrap();
    assert!(finer.bytes.len() > svg.len());
}

#[test]
fn glyph_runs_and_layers_are_reported_not_written() {
    let id = ids(2);
    let mut s = empty(100.0, 100.0);
    let mut tagged = stroke(
        id[1],
        segment(p(1.0, 1.0), p(9.0, 9.0)),
        pen(Color::BLACK, 0.5),
    );
    tagged.layer = Some(id[0]);
    s.items = vec![glyphs(id[0]), tagged];
    s.failed = vec![id[0]];
    let out = write_svg(&s, &mut WriteOptions::default()).unwrap();
    assert_report_valid(&out.report);
    assert!(out.report.has_loss());
    assert_eq!(
        reasons(&out.report),
        [
            "io.pdf.glyphs_dropped",
            "io.pdf.items_written",
            "io.pdf.layers_dropped",
            "io.pdf.sources_failed"
        ]
    );
    assert_eq!(
        out.report.items[0].source_id.as_deref(),
        Some(id[0].to_string().as_str())
    );
}

#[test]
fn nothing_lost_still_gives_a_report() {
    let out = write_svg(&empty(10.0, 10.0), &mut WriteOptions::default()).unwrap();
    assert_report_valid(&out.report);
    assert!(out.report.items.is_empty() && !out.report.has_loss());
    assert_eq!(out.report.header.format, "svg");
}

/// Renders the SVG with `resvg` and the stream with the raster output at the same pixel size
/// (96 dpi, the size `usvg` gives millimeters) and compares probe points.
#[test]
fn rendered_svg_agrees_with_the_raster_output_at_probe_points() {
    let stream = scene();
    let svg = text(&stream);
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let raster = render(
        &stream,
        &RasterOptions {
            dpi: 96.0,
            ..RasterOptions::default()
        },
    )
    .unwrap();
    // The raster is the page rounded up to whole pixels with the page's lower edge at the
    // image's bottom, so the document is shifted down by the rounding.
    let shift = raster.height as f32 - tree.size().height();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(raster.width, raster.height).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_translate(0.0, shift),
        &mut pixmap.as_mut(),
    );

    let px = |mm: f64| (mm * 96.0 / 25.4) as u32;
    let rows = raster.height;
    // (x mm, y mm from the bottom, what to expect)
    let probes = [
        (20.0, 40.0, [0, 0, 0]),       // line present
        (40.0, 50.0, [230, 230, 200]), // dash gap empty
        (20.0, 50.0, [0, 0, 0]),       // dash piece present
        (70.0, 20.0, [255, 0, 0]),     // fill inside
        (20.0, 70.0, [0, 0, 0]),       // clip: inside
        (80.0, 70.0, [230, 230, 200]), // clip: outside the region
        (45.0, 40.0, [230, 230, 200]), // mask in the paper color
        (50.0, 2.0, [230, 230, 200]),  // outside the plotted area
        (2.0, 2.0, [230, 230, 200]),   // paper
    ];
    for (x, y, expected) in probes {
        let (cx, cy) = (px(x), rows - 1 - px(y));
        let r = raster.pixel(cx, cy).unwrap();
        let v = pixmap.pixel(cx, cy).unwrap();
        let svg_rgb = [v.red(), v.green(), v.blue()];
        for k in 0..3 {
            // Anti-aliasing and the rasterizers' different flattening move edge pixels; the
            // probes lie inside shapes or gaps, so a tolerance of 24 of 255 per channel holds.
            let tolerance = 24;
            assert!(
                r[k].abs_diff(svg_rgb[k]) <= tolerance
                    && svg_rgb[k].abs_diff(expected[k]) <= tolerance,
                "probe ({x}, {y}) channel {k}: raster {r:?}, svg {svg_rgb:?}, expected {expected:?}"
            );
        }
    }
    // The arc is drawn: a probe on its upper half.
    let (cx, cy) = (px(30.0), rows - 1 - px(30.0));
    let v = pixmap.pixel(cx, cy).unwrap();
    assert!(v.blue() > 200 && v.red() < 60, "arc top at (30, 30)");
    let r = raster.pixel(cx, cy).unwrap();
    assert!(r[2] > 200 && r[0] < 60);
}

#[test]
fn limits_name_the_exceeded_field() {
    let s = scene();
    let tight = |limits: Limits| WriteOptions {
        limits,
        ..WriteOptions::default()
    };
    let case = |limits| code(write_svg(&s, &mut tight(limits)));
    assert_eq!(
        case(Limits {
            record_count: 3,
            ..Limits::CONSTRAINED
        }),
        ("io.limit_exceeded", "record_count".into())
    );
    assert_eq!(
        case(Limits {
            input_bytes: 200,
            ..Limits::CONSTRAINED
        }),
        ("io.limit_exceeded", "input_bytes".into())
    );
    // Clips count against `record_count` too, so they cannot be used to make unbounded work.
    let mut clipped = empty(10.0, 10.0);
    clipped.clips = vec![common::square(0.0, 0.0, 1.0, 1.0); 5];
    assert_eq!(
        code(write_svg(
            &clipped,
            &mut tight(Limits {
                record_count: 5,
                ..Limits::CONSTRAINED
            })
        )),
        ("io.limit_exceeded", "record_count".into())
    );
    let id = ids(1);
    let mut poly = empty(50.0, 50.0);
    let vertices = (0..5)
        .map(|i| PolylineVertex::new(p(f64::from(i), f64::from(i % 2)), 0.0))
        .collect();
    poly.items = vec![stroke(
        id[0],
        Curve2::Polyline(Polyline2::new(vertices, false).unwrap()),
        pen(Color::BLACK, 0.5),
    )];
    let r = write_svg(
        &poly,
        &mut tight(Limits {
            array_length: 4,
            ..Limits::CONSTRAINED
        }),
    );
    assert_eq!(code(r), ("io.limit_exceeded", "array_length".into()));
    let mut titled = tight(Limits {
        string_bytes: 3,
        ..Limits::CONSTRAINED
    });
    titled.title = Some("long title".into());
    assert_eq!(
        code(write_svg(&s, &mut titled)),
        ("io.limit_exceeded", "string_bytes".into())
    );
}

#[test]
fn cancellation_and_progress() {
    let cancel = Cancel::new();
    cancel.cancel();
    let mut options = WriteOptions {
        cancel,
        ..WriteOptions::default()
    };
    assert_eq!(code(write_svg(&scene(), &mut options)).0, "base.cancelled");

    let mut seen = Vec::new();
    let mut record = |p: arc_io_pdf::Progress| seen.push((p.items_done, p.items_total));
    let mut options = WriteOptions {
        progress: Some(&mut record),
        ..WriteOptions::default()
    };
    write_svg(&scene(), &mut options).unwrap();
    assert_eq!(seen.first(), Some(&(0, 9)));
    assert_eq!(seen.last(), Some(&(9, 9)));
}

#[test]
fn hostile_options_and_streams_are_rejected_with_their_codes() {
    let s = scene();
    for chord in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1e-9, 1e9] {
        let mut o = WriteOptions {
            chord_mm: chord,
            ..WriteOptions::default()
        };
        assert_eq!(
            code(write_svg(&s, &mut o)),
            ("io.pdf.option_invalid", "chord_mm".into())
        );
    }
    let mut o = WriteOptions {
        title: Some("a\u{0}b".into()),
        ..WriteOptions::default()
    };
    assert_eq!(
        code(write_svg(&s, &mut o)),
        ("io.pdf.option_invalid", "title".into())
    );
    let mut o = WriteOptions {
        creation_date: Some("2026-13-01T00:00:00Z".into()),
        ..WriteOptions::default()
    };
    assert_eq!(
        code(write_svg(&s, &mut o)),
        ("io.pdf.option_invalid", "creation_date".into())
    );

    let bad = |f: &dyn Fn(&mut arc_draft::plot::PlotStream)| {
        let mut s = scene();
        f(&mut s);
        code(write_svg(&s, &mut WriteOptions::default()))
    };
    assert_eq!(bad(&|s| s.width = f64::NAN).0, "io.pdf.stream_invalid");
    assert_eq!(bad(&|s| s.height = 0.0).0, "io.pdf.stream_invalid");
    assert_eq!(bad(&|s| s.area[0].x = 1e30).0, "io.pdf.stream_invalid");
    assert_eq!(
        bad(&|s| s.items[0].clip = Some(9)).0,
        "io.pdf.stream_invalid"
    );
    let far = |s: &mut arc_draft::plot::PlotStream| {
        s.items[0].kind = ItemKind::Stroke {
            curve: segment(p(0.0, 0.0), p(1e300, 0.0)),
            pieces: None,
            pen: pen(Color::BLACK, 1.0),
        };
    };
    assert_eq!(bad(&far), ("io.pdf.stream_invalid", "coordinate".into()));
}

#[test]
fn the_title_is_escaped_and_the_page_has_paper_color() {
    let mut o = WriteOptions {
        title: Some("A & <B>".into()),
        ..WriteOptions::default()
    };
    let out = write_svg(&empty(10.0, 10.0), &mut o).unwrap();
    let svg = String::from_utf8(out.bytes).unwrap();
    assert!(svg.contains("<title>A &amp; &lt;B&gt;</title>"));
    assert!(svg.contains(&format!(
        "<rect width=\"100%\" height=\"100%\" fill=\"#{:02x}{:02x}{:02x}\"/>",
        PAPER[0], PAPER[1], PAPER[2]
    )));
}

#[test]
fn region_fill_is_one_even_odd_path_that_agrees_with_the_raster_output() {
    let stream = ring();
    let svg = text(&stream);
    assert_eq!(svg.matches("fill-rule=\"evenodd\"").count(), 1, "{svg}");
    // Outer loop, then the hole as two exact half-circle arcs; no flattening.
    assert!(svg.contains("M20 60L60 60L60 20L20 20L20 60Z"), "{svg}");
    assert_eq!(svg.matches('A').count(), 2, "{svg}");
    let out = write_svg(&stream, &mut WriteOptions::default()).unwrap();
    assert_eq!(
        out,
        write_svg(&ring(), &mut WriteOptions::default()).unwrap()
    );
    assert!(
        out.report
            .items
            .iter()
            .any(|i| i.record_kind.as_deref() == Some("region_fill"))
    );

    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    let raster = render(
        &stream,
        &RasterOptions {
            dpi: 96.0,
            ..RasterOptions::default()
        },
    )
    .unwrap();
    let shift = raster.height as f32 - tree.size().height();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(raster.width, raster.height).unwrap();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_translate(0.0, shift),
        &mut pixmap.as_mut(),
    );
    let px = |mm: f64| (mm * 96.0 / 25.4) as u32;
    for (x, y, expected) in [
        (25.0, 25.0, [255, 0, 0]),     // ring
        (40.0, 52.0, [255, 0, 0]),     // ring above the hole
        (40.0, 40.0, [230, 230, 200]), // hole
        (80.0, 70.0, [230, 230, 200]), // outside
    ] {
        let (cx, cy) = (px(x), raster.height - 1 - px(y));
        let r = raster.pixel(cx, cy).unwrap();
        let v = pixmap.pixel(cx, cy).unwrap();
        let svg_rgb = [v.red(), v.green(), v.blue()];
        for k in 0..3 {
            assert!(
                r[k].abs_diff(svg_rgb[k]) <= 24 && svg_rgb[k].abs_diff(expected[k]) <= 24,
                "probe ({x}, {y}) channel {k}: raster {r:?}, svg {svg_rgb:?}, expected {expected:?}"
            );
        }
    }
}
