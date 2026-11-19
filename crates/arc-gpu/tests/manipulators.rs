//! CORE-GPU-026: manipulator handles of constant screen size with their own hit testing.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use arc_base::math::{Point3, Vector3};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Axis, FrameExtras, Image, Manipulator, ManipulatorKind, RenderSettings, Renderer, Scene,
    SceneLimits,
};
use support::gpu;

const SIZE: u32 = 128;

fn top(projection: Projection, distance: f64) -> ViewFrame {
    ViewFrame::new(
        Point3::new(0.0, 0.0, distance),
        Point3::origin(),
        Vector3::y(),
        projection,
        0.1,
        10_000.0,
    )
    .unwrap()
}

fn render(manipulator: &Manipulator, frame: &ViewFrame) -> Image {
    let gpu = gpu();
    let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
    Renderer::new(&gpu)
        .render_with(
            &mut scene,
            frame,
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &FrameExtras {
                manipulator: Some(manipulator),
                ..FrameExtras::default()
            },
        )
        .unwrap()
}

/// Whether the pixel is mostly red, green or blue.
fn dominant(image: &Image, x: u32, y: u32) -> Option<usize> {
    let p = image.pixel(x, y)?;
    let (best, value) = p[..3].iter().enumerate().max_by_key(|(_, v)| **v)?;
    let others = p[..3]
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != best)
        .map(|(_, v)| *v);
    (*value > 180 && others.into_iter().all(|v| v + 60 < *value)).then_some(best)
}

/// The number of pixels right of the center on the middle row that are red.
fn red_extent(image: &Image) -> u32 {
    (65..SIZE)
        .filter(|x| dominant(image, *x, 64) == Some(0))
        .count() as u32
}

#[test]
fn move_handles_are_drawn_as_axes_with_arrowheads() {
    let frame = top(Projection::Orthographic { height: 4.0 }, 10.0);
    let mut m = Manipulator::new(ManipulatorKind::Move, Point3::origin());
    m.size_px = 40.0;
    let image = render(&m, &frame);
    // The x axis runs right, the y axis up the image, from the pixel center (64, 64).
    assert_eq!(dominant(&image, 84, 64), Some(0));
    assert_eq!(dominant(&image, 64, 44), Some(1));
    assert_eq!(
        dominant(&image, 64, 4),
        None,
        "beyond the tip and the arrowhead"
    );
    // The arrowhead extends 14 pixels past the tip at 40 pixels.
    assert_eq!(
        dominant(&image, 64 + 40 + 8, 64),
        Some(0),
        "inside the arrowhead"
    );
    // Hit testing agrees with what was drawn.
    let hit = |x: f64, y: f64| m.hit_test(&frame, (SIZE, SIZE), [x, y], 4.0).unwrap();
    assert_eq!(hit(84.5, 64.5), Some(Axis::X));
    assert_eq!(hit(64.5, 44.5), Some(Axis::Y));
    assert_eq!(hit(10.0, 120.0), None);
}

#[test]
fn handles_keep_their_screen_size_at_two_camera_distances() {
    let mut m = Manipulator::new(ManipulatorKind::Move, Point3::origin());
    m.size_px = 40.0;
    for projection in [
        |d: f64| Projection::Orthographic { height: d / 2.0 },
        |_: f64| Projection::Perspective { vertical_fov: 0.6 },
    ] {
        let near = red_extent(&render(&m, &top(projection(4.0), 4.0)));
        let far = red_extent(&render(&m, &top(projection(400.0), 400.0)));
        assert!((45..=60).contains(&near), "{near}");
        assert!(near.abs_diff(far) <= 1, "{near} at 4 and {far} at 400");
    }
}

#[test]
fn scale_squares_and_rotation_rings_are_drawn_where_their_hit_tests_say() {
    let frame = top(Projection::Orthographic { height: 4.0 }, 10.0);
    let mut scale = Manipulator::new(ManipulatorKind::Scale, Point3::origin());
    scale.size_px = 40.0;
    let image = render(&scale, &frame);
    assert_eq!(
        dominant(&image, 64 + 40, 64),
        Some(0),
        "the square at the tip"
    );
    assert_eq!(dominant(&image, 64 + 40 + 4, 64), Some(0));

    let mut rotate = Manipulator::new(ManipulatorKind::Rotate, Point3::origin());
    rotate.size_px = 40.0;
    let image = render(&rotate, &frame);
    // The ring about z is a circle of radius 40 around the center; at 45 degrees it is blue.
    assert_eq!(dominant(&image, 64 + 28, 64 - 28), Some(2));
    assert_eq!(
        dominant(&image, 80, 80),
        None,
        "nothing inside the ring off the edge-on rings"
    );
    let hit = |x: f64, y: f64| rotate.hit_test(&frame, (SIZE, SIZE), [x, y], 3.0).unwrap();
    assert_eq!(hit(64.5 + 28.3, 64.5 - 28.3), Some(Axis::Z));
    assert_eq!(hit(80.5, 80.5), None);
}

#[test]
fn the_active_handle_is_drawn_in_the_active_color() {
    let frame = top(Projection::Orthographic { height: 4.0 }, 10.0);
    let mut m = Manipulator::new(ManipulatorKind::Move, Point3::origin());
    m.size_px = 40.0;
    m.active = Some(Axis::X);
    let image = render(&m, &frame);
    let p = image.pixel(84, 64).unwrap();
    assert!(p[0] > 200 && p[1] > 200 && p[2] < 100, "yellow: {p:?}");
    assert_eq!(
        dominant(&image, 64, 44),
        Some(1),
        "the other handles keep their colors"
    );
}
