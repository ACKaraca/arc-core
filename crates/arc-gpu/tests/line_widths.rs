//! CORE-GPU-008: exact screen and paper widths at any zoom, caps, joins, near-plane crossing
//! and zero-length segments, measured in rendered images.

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
use arc_base::math::{Point3, Vector3};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Image, LineCap, LineItem, LineJoin, LinePath, LineStyle, LineWidth, RenderItem, RenderSettings,
};
use support::{area, column_width, coverage, ids, row_width, setup, top_view};

const SIZE: u32 = 128;

fn draw(
    frame: &ViewFrame,
    paths: Vec<(Point3, Vec<[f32; 3]>)>,
    style: LineStyle,
    settings: &RenderSettings,
) -> Image {
    let (_, mut scene, mut renderer) = setup();
    let id = ids(paths.len());
    let changes = paths
        .into_iter()
        .zip(id)
        .map(|((origin, points), id)| {
            let item = LineItem {
                origin,
                paths: vec![LinePath {
                    points,
                    closed: false,
                }],
                style,
                color: Color::WHITE,
            };
            (id, vec![RenderItem::Lines(item)])
        })
        .collect();
    scene.update(changes).unwrap();
    renderer
        .render_to_image(&mut scene, frame, SIZE, SIZE, settings)
        .unwrap()
}

fn style(width: LineWidth, cap: LineCap, join: LineJoin) -> LineStyle {
    LineStyle {
        width,
        cap,
        join,
        miter_limit: 4.0,
        lineweight: false,
    }
}

#[test]
fn screen_widths_are_exact_at_every_zoom_and_angle() {
    for height in [0.01, 1.0, 1e3, 1e5] {
        for degrees in [0.0_f64, 30.0, 60.0, 90.0] {
            for width in [1.0_f32, 3.0, 8.5, 20.0] {
                let (s, c) = degrees.to_radians().sin_cos();
                let end = [(c * height) as f32, (s * height) as f32, 0.0];
                let image = draw(
                    &top_view(Point3::origin(), height),
                    vec![(Point3::origin(), vec![end.map(|v| -v), end])],
                    LineStyle::round(LineWidth::Screen(width)),
                    &RenderSettings::default(),
                );
                // A scan line across a line at angle a crosses width / cos a pixels.
                let measured = if degrees < 45.0 {
                    column_width(&image, SIZE / 2 - 20) * c
                } else {
                    row_width(&image, SIZE / 2 - 20) * s
                };
                assert!(
                    (measured - f64::from(width)).abs() < 0.6,
                    "height {height}, angle {degrees}, width {width}: measured {measured}"
                );
            }
        }
    }
}

#[test]
fn screen_widths_do_not_shrink_with_distance_and_have_a_one_pixel_minimum() {
    let frame = ViewFrame::new(
        Point3::origin(),
        Point3::new(0.0, 0.0, -1.0),
        Vector3::y(),
        Projection::Perspective { vertical_fov: 1.2 },
        0.1,
        100.0,
    )
    .unwrap();
    // Horizontal lines at 2 m (upper half of the image) and 40 m (lower half).
    let image = draw(
        &frame,
        vec![
            (
                Point3::new(0.0, 0.5, -2.0),
                vec![[-10.0, 0.0, 0.0], [10.0, 0.0, 0.0]],
            ),
            (
                Point3::new(0.0, -10.0, -40.0),
                vec![[-100.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
            ),
        ],
        LineStyle::round(LineWidth::Screen(6.0)),
        &RenderSettings::default(),
    );
    let upper: f64 = (0..SIZE / 2).map(|y| coverage(&image, 20, y)).sum();
    let lower: f64 = (SIZE / 2..SIZE).map(|y| coverage(&image, 20, y)).sum();
    assert!(
        (upper - 6.0).abs() < 0.6 && (lower - 6.0).abs() < 0.6,
        "{upper} {lower}"
    );

    let thin = draw(
        &top_view(Point3::origin(), 10.0),
        vec![(Point3::origin(), vec![[-10.0, 0.0, 0.0], [10.0, 0.0, 0.0]])],
        LineStyle::round(LineWidth::Screen(0.25)),
        &RenderSettings::default(),
    );
    let measured = column_width(&thin, 40);
    assert!((measured - 1.0).abs() < 0.3, "{measured}");
}

#[test]
fn paper_widths_follow_the_plot_scale_and_the_view() {
    // 10 model units per paper millimetre: 1.5 mm is 15 units.
    let settings = RenderSettings {
        plot_scale: 10.0,
        ..RenderSettings::default()
    };
    for (height, pixels) in [(256.0, 7.5), (128.0, 15.0), (64.0, 30.0)] {
        let image = draw(
            &top_view(Point3::origin(), height),
            vec![(
                Point3::origin(),
                vec![[-500.0, 0.0, 0.0], [500.0, 0.0, 0.0]],
            )],
            LineStyle::round(LineWidth::Paper(1.5)),
            &settings,
        );
        let measured = column_width(&image, 30);
        assert!(
            (measured - pixels).abs() < 0.6,
            "height {height}: {measured} vs {pixels}"
        );
    }
}

/// Model coordinates of the center of pixel `(x, y)` in a view where one unit is one pixel.
fn model(x: u32, y: u32) -> (f64, f64) {
    let half = f64::from(SIZE) / 2.0;
    (f64::from(x) + 0.5 - half, half - (f64::from(y) + 0.5))
}

fn segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

/// Pixels whose rendered (unsmoothed) coverage differs from the analytic region.
fn mismatches(image: &Image, inside: impl Fn(f64, f64) -> bool) -> usize {
    (0..SIZE)
        .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (mx, my) = model(x, y);
            (coverage(image, x, y) > 0.5) != inside(mx, my)
        })
        .count()
}

fn pixel_view() -> ViewFrame {
    top_view(Point3::origin(), f64::from(SIZE))
}

const HARD: RenderSettings = RenderSettings {
    line_smoothing: false,
    ..RenderSettings::DEFAULT
};

#[test]
fn caps_match_their_analytic_shapes() {
    let segment = vec![(Point3::origin(), vec![[-30.0, 0.0, 0.0], [30.0, 0.0, 0.0]])];
    let round = draw(
        &pixel_view(),
        segment.clone(),
        style(LineWidth::Screen(16.0), LineCap::Round, LineJoin::Round),
        &HARD,
    );
    assert_eq!(
        mismatches(&round, |x, y| segment_distance(
            (x, y),
            (-30.0, 0.0),
            (30.0, 0.0)
        ) <= 8.0),
        0
    );
    let square = draw(
        &pixel_view(),
        segment.clone(),
        style(LineWidth::Screen(16.0), LineCap::Square, LineJoin::Round),
        &HARD,
    );
    assert_eq!(
        mismatches(&square, |x, y| x.abs() <= 38.0 && y.abs() <= 8.0),
        0
    );
    // Smoothed edges keep the area: coverage sums to the capsule area within 1 %.
    let smooth = draw(
        &pixel_view(),
        segment,
        style(LineWidth::Screen(16.0), LineCap::Round, LineJoin::Round),
        &RenderSettings::default(),
    );
    let exact = 60.0 * 16.0 + std::f64::consts::PI * 64.0;
    assert!(
        (area(&smooth) / exact - 1.0).abs() < 0.01,
        "{} vs {exact}",
        area(&smooth)
    );
}

#[test]
fn joins_match_their_analytic_shapes() {
    // An L turning left at (20, −30), 16 pixels wide, with square caps.
    let path = vec![(
        Point3::origin(),
        vec![[-40.0, -30.0, 0.0], [20.0, -30.0, 0.0], [20.0, 40.0, 0.0]],
    )];
    let rects = |x: f64, y: f64| {
        ((-48.0..=28.0).contains(&x) && (-38.0..=-22.0).contains(&y))
            || ((12.0..=28.0).contains(&x) && (-38.0..=48.0).contains(&y))
    };
    let miter = draw(
        &pixel_view(),
        path.clone(),
        style(LineWidth::Screen(16.0), LineCap::Square, LineJoin::Miter),
        &HARD,
    );
    assert!(mismatches(&miter, rects) <= 2);
    let bevel = draw(
        &pixel_view(),
        path.clone(),
        style(LineWidth::Screen(16.0), LineCap::Square, LineJoin::Bevel),
        &HARD,
    );
    // The outer corner (28, −38) is cut by the line through (20, −38) and (28, −30).
    assert!(mismatches(&bevel, |x, y| rects(x, y) && x - y <= 58.0) <= 2);
    let round = draw(
        &pixel_view(),
        path,
        style(LineWidth::Screen(16.0), LineCap::Round, LineJoin::Round),
        &HARD,
    );
    assert_eq!(
        mismatches(&round, |x, y| {
            segment_distance((x, y), (-40.0, -30.0), (20.0, -30.0)) <= 8.0
                || segment_distance((x, y), (20.0, -30.0), (20.0, 40.0)) <= 8.0
        }),
        0
    );
}

#[test]
fn a_miter_longer_than_the_limit_becomes_a_bevel() {
    // A sharp turn of 160 degrees: the miter would reach 5.8 half-widths, beyond the limit 4.
    let path = vec![(
        Point3::origin(),
        vec![[-40.0, 0.0, 0.0], [30.0, 0.0, 0.0], [-40.0, 25.0, 0.0]],
    )];
    let s = |join| style(LineWidth::Screen(10.0), LineCap::Square, join);
    let miter = draw(&pixel_view(), path.clone(), s(LineJoin::Miter), &HARD);
    let bevel = draw(&pixel_view(), path, s(LineJoin::Bevel), &HARD);
    assert!(area(&miter) > 500.0);
    assert_eq!(miter, bevel);
}

#[test]
fn zero_length_segments_are_dots() {
    let dot = vec![(Point3::origin(), vec![[3.0, 4.0, 0.0]])];
    let round = draw(
        &pixel_view(),
        dot.clone(),
        style(LineWidth::Screen(20.0), LineCap::Round, LineJoin::Round),
        &HARD,
    );
    assert_eq!(
        mismatches(&round, |x, y| (x - 3.0).hypot(y - 4.0) <= 10.0),
        0
    );
    let square = draw(
        &pixel_view(),
        dot,
        style(LineWidth::Screen(20.0), LineCap::Square, LineJoin::Round),
        &HARD,
    );
    assert_eq!(
        mismatches(&square, |x, y| (x - 3.0).abs() <= 10.0
            && (y - 4.0).abs() <= 10.0),
        0
    );
}

#[test]
fn segments_crossing_the_near_plane_are_clipped_before_expansion() {
    let frame = ViewFrame::new(
        Point3::origin(),
        Point3::new(0.0, 0.0, -1.0),
        Vector3::y(),
        Projection::Perspective { vertical_fov: 1.2 },
        0.1,
        100.0,
    )
    .unwrap();
    // Below the eye, from 1 m behind it to 30 m in front: a vertical line in the lower half.
    let crossing = draw(
        &frame,
        vec![(
            Point3::new(0.0, -0.5, 0.0),
            vec![[0.0, 0.0, 1.0], [0.0, 0.0, -30.0]],
        )],
        LineStyle::round(LineWidth::Screen(5.0)),
        &RenderSettings::default(),
    );
    for y in [80, 100, 120] {
        let measured = row_width(&crossing, y);
        assert!((measured - 5.0).abs() < 0.6, "row {y}: {measured}");
    }
    assert_eq!(row_width(&crossing, 20), 0.0, "nothing above the horizon");
    let behind = draw(
        &frame,
        vec![(
            Point3::new(0.0, -0.5, 0.0),
            vec![[0.0, 0.0, 1.0], [0.0, 0.0, 5.0]],
        )],
        LineStyle::round(LineWidth::Screen(5.0)),
        &RenderSettings::default(),
    );
    assert_eq!(area(&behind), 0.0);
}
