//! Curve items: chord error below the pixel bound at zoom factors from 10⁻³ to 10³ with
//! re-flattening only when the factor-of-two rule says so (CORE-GPU-010), panning that never
//! re-flattens, the lineweight display switch without re-extraction, and polyline widths.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use arc_base::Color;
use arc_base::math::{Point2, Point3};
use arc_geom::{Circle2, Curve2, Polyline2, PolylineVertex};
use arc_gpu::{
    CurveItem, CurvePath, ItemKey, LineStyle, LineWidth, RenderItem, RenderSettings, WriteCounters,
};
use support::{column_width, coverage, ids, setup, top_view};

const SIZE: u32 = 32;

fn item(curve: Curve2, style: LineStyle) -> RenderItem {
    RenderItem::Curves(CurveItem {
        origin: Point3::origin(),
        paths: vec![CurvePath::curve(curve)],
        style,
        color: Color::WHITE,
        pattern: None,
        clip: None,
    })
}

#[test]
fn chord_error_stays_below_the_pixel_bound_and_only_zoom_steps_beyond_two_reflatten() {
    let (_gpu, mut scene, mut renderer) = setup();
    let object = ids(1)[0];
    let key = ItemKey { object, sub: 0 };
    let radius = 1.0;
    let circle = Curve2::Circle(Circle2::new(Point2::origin(), radius).unwrap());
    scene
        .update(vec![(
            object,
            vec![item(circle, LineStyle::round(LineWidth::Screen(1.0)))],
        )])
        .unwrap();
    let settings = RenderSettings::default();
    let tolerance = f64::from(settings.curve_tolerance_px);
    // Pixels per unit at zoom factor 1: the circle spans 200 pixels.
    let base = 100.0;
    let frame_at = |scale: f64, center_x: f64| {
        top_view(Point3::new(center_x, 0.0, 0.0), f64::from(SIZE) / scale)
    };
    let mut segments = 0_u64;
    let mut flattened_for = 0.0;
    let mut checked = 0;
    let mut reflattened = 0;
    let mut zooms: Vec<f64> = (0..=26).map(|i| 1e-3 * 1.7_f64.powi(i)).collect();
    zooms.extend((0..26).rev().map(|i| 1e-3 * 1.7_f64.powi(i)));
    for (step, zoom) in zooms.iter().enumerate() {
        let scale = base * zoom;
        renderer
            .render_to_image(&mut scene, &frame_at(scale, 0.0), SIZE, SIZE, &settings)
            .unwrap();
        let written = scene.last_frame_writes().line_segment;
        let expected = step == 0 || !(0.5..=2.0).contains(&(scale / flattened_for));
        if expected {
            assert!(
                written > 0,
                "zoom {zoom}: the factor-of-two rule asks for a flattening"
            );
            segments = written / 128;
            flattened_for = scene.flattening(key).unwrap().0;
            reflattened += 1;
        } else {
            assert_eq!(
                written, 0,
                "zoom {zoom}: within a factor of two of the flattening"
            );
        }
        let (flattened, tolerance_px) = scene.flattening(key).unwrap();
        assert!((flattened / scale - 1.0).abs() < 1e-9 || !expected);
        assert_eq!((flattened, tolerance_px), (flattened_for, 0.5));
        // The flattened circle is a regular polygon: its chord error is r·(1 − cos(π/n)).
        #[allow(clippy::cast_precision_loss)]
        let n = segments as f64;
        let error_px = radius * (1.0 - (std::f64::consts::PI / n).cos()) * scale;
        assert!(
            error_px <= tolerance * (1.0 + 1e-9),
            "zoom {zoom}: chord error {error_px} px with {n} segments"
        );
        checked += 1;
        // A pan of a non-integer number of pixels never flattens again.
        renderer
            .render_to_image(
                &mut scene,
                &frame_at(scale, 1234.37 / scale),
                SIZE,
                SIZE,
                &settings,
            )
            .unwrap();
        assert_eq!(
            scene.last_frame_writes().line_segment,
            0,
            "pan at zoom {zoom}"
        );
    }
    assert_eq!(checked, zooms.len());
    // About log2(10⁶)/1 flattenings each way, far fewer than the frames.
    assert!(reflattened < zooms.len() / 2 + 2, "{reflattened}");
    // A changed tolerance flattens again; the circle needs more segments.
    let finer = RenderSettings {
        curve_tolerance_px: 0.1,
        ..settings
    };
    renderer
        .render_to_image(&mut scene, &frame_at(base, 0.0), SIZE, SIZE, &finer)
        .unwrap();
    assert!(scene.last_frame_writes().line_segment / 128 > segments);
}

#[test]
fn lineweight_display_switches_widths_without_writing_the_scene() {
    let (_gpu, mut scene, mut renderer) = setup();
    let object = ids(1)[0];
    let segment = Curve2::Segment(
        arc_geom::Segment2::new(Point2::new(0.25, -20.0), Point2::new(0.25, 20.0)).unwrap(),
    );
    let style = LineStyle {
        lineweight: true,
        ..LineStyle::round(LineWidth::Screen(5.0))
    };
    scene
        .update(vec![(object, vec![item(segment, style)])])
        .unwrap();
    let frame = top_view(Point3::origin(), 64.0);
    let on = RenderSettings::default();
    let image = renderer
        .render_to_image(&mut scene, &frame, 64, 64, &on)
        .unwrap();
    // A vertical line at x = 0.25 lies on the center of column 32; measure across row 32.
    let across = |image: &arc_gpu::Image| (0..64).map(|x| coverage(image, x, 32)).sum::<f64>();
    assert!((across(&image) - 5.0).abs() < 0.05, "{}", across(&image));
    let off = RenderSettings {
        lineweights: false,
        ..on
    };
    let image = renderer
        .render_to_image(&mut scene, &frame, 64, 64, &off)
        .unwrap();
    assert!((across(&image) - 1.0).abs() < 0.05, "{}", across(&image));
    assert_eq!(scene.last_frame_writes(), WriteCounters::default());
    // A width that is not a lineweight is not affected.
    scene
        .update(vec![(
            object,
            vec![item(
                Curve2::Segment(
                    arc_geom::Segment2::new(Point2::new(0.25, -20.0), Point2::new(0.25, 20.0))
                        .unwrap(),
                ),
                LineStyle::round(LineWidth::Screen(5.0)),
            )],
        )])
        .unwrap();
    let image = renderer
        .render_to_image(&mut scene, &frame, 64, 64, &off)
        .unwrap();
    assert!((across(&image) - 5.0).abs() < 0.05);
}

#[test]
fn polyline_widths_taper_in_item_units_and_end_flat() {
    let (_gpu, mut scene, mut renderer) = setup();
    let object = ids(1)[0];
    // From x = −20 to 20 along y = 0.5 (the center of row 31 at one pixel per unit), the full
    // width growing from 2 to 10 units.
    let polyline = Polyline2::new(
        vec![
            PolylineVertex {
                point: Point2::new(-20.0, 0.5),
                bulge: 0.0,
                start_width: 2.0,
                end_width: 10.0,
            },
            PolylineVertex::new(Point2::new(20.0, 0.5), 0.0),
        ],
        false,
    )
    .unwrap();
    scene
        .update(vec![(
            object,
            vec![item(
                Curve2::Polyline(polyline),
                LineStyle::round(LineWidth::Screen(1.0)),
            )],
        )])
        .unwrap();
    let frame = top_view(Point3::origin(), 64.0);
    let image = renderer
        .render_to_image(&mut scene, &frame, 64, 64, &RenderSettings::default())
        .unwrap();
    // Column x covers pixel centers x + 0.5 − 32; the width there is 2 + 8 · (t + 20) / 40.
    for x in [16_u32, 32, 48] {
        let t = f64::from(x) + 0.5 - 32.0;
        let expected = 2.0 + 8.0 * (t + 20.0) / 40.0;
        let measured = column_width(&image, x);
        assert!(
            (measured - expected).abs() < 0.15,
            "x {x}: {measured} vs {expected}"
        );
    }
    // Flat ends: nothing beyond x = ±20 (columns 12 and 52 are entirely outside).
    assert!(column_width(&image, 11) < 1e-3);
    assert!(column_width(&image, 52) < 1e-3);
    // Zooming in by four scales the width with the view.
    let near = top_view(Point3::origin(), 16.0);
    let image = renderer
        .render_to_image(&mut scene, &near, 64, 64, &RenderSettings::default())
        .unwrap();
    let measured = column_width(&image, 32);
    // Column 32 covers x = 0.125 at four pixels per unit: width 6.025 units.
    assert!((measured - 4.0 * 6.025).abs() < 0.3, "{measured}");
}
