//! CORE-GPU-025: overlays draw above the scene regardless of depth and of the clip planes.

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
use arc_gpu::{
    Anchor, FrameExtras, Grid, MarkerShape, MeshInstance, MeshItem, Overlay, OverlayPrimitive,
    RenderItem, RenderSettings, WriteCounters,
};
use support::{ids, linear, setup, square, top_view};

const SIZE: u32 = 96;
const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const GRAY: [u8; 4] = [128, 128, 128, 255];

fn line(points: Vec<Anchor>, width_px: f32, color: Color) -> OverlayPrimitive {
    OverlayPrimitive::Polyline {
        points,
        closed: false,
        width_px,
        color,
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one overlay with every primitive kind"
)]
fn overlays_draw_above_the_scene_regardless_of_depth() {
    let (_, mut scene, mut renderer) = setup();
    // One model unit per pixel: a red square of side 40 on pixels 28..=67, at z = 0. The eye
    // is at z = 100 with the far plane at 1,000.
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let red = MeshInstance {
        transform: [
            [20.0, 0.0, 0.0, 0.0],
            [0.0, 20.0, 0.0, 0.0],
            [0.0, 0.0, 20.0, 0.0],
        ],
        ..MeshInstance::new(Color::new(1.0, 0.0, 0.0, 1.0))
    };
    scene
        .update(vec![(
            ids(1)[0],
            vec![RenderItem::Mesh(MeshItem {
                geometry,
                origin: Point3::origin(),
                instances: vec![red],
            })],
        )])
        .unwrap();
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    let settings = RenderSettings::DEFAULT;
    let world = |x, y, z| Anchor::World(Point3::new(x, y, z));
    let gray = Color::from_srgb8(GRAY);
    let overlay = Overlay {
        primitives: vec![
            // Behind the square, and far beyond the far plane.
            line(
                vec![world(-30.0, 5.5, -50.0), world(30.0, 5.5, -50.0)],
                3.0,
                Color::WHITE,
            ),
            line(
                vec![world(-30.0, 15.5, -5000.0), world(30.0, 15.5, -5000.0)],
                3.0,
                Color::WHITE,
            ),
            OverlayPrimitive::Marker {
                at: Anchor::Screen([10.0, 10.0]),
                shape: MarkerShape::Square,
                size_px: 8.0,
                filled: true,
                width_px: 1.0,
                color: Color::new(0.0, 1.0, 0.0, 1.0),
            },
            // A rubber band: a half-transparent fill.
            OverlayPrimitive::Fill {
                points: vec![
                    Anchor::Screen([72.0, 72.0]),
                    Anchor::Screen([92.0, 72.0]),
                    Anchor::Screen([92.0, 92.0]),
                    Anchor::Screen([72.0, 92.0]),
                ],
                color: Color::new(0.0, 0.0, 1.0, 0.5),
            },
            OverlayPrimitive::Grid(Grid {
                origin: Point3::new(0.0, 0.0, -10.0),
                u: Vector3::x() * 10.0,
                v: Vector3::y() * 10.0,
                half_count: 4,
                major_every: 0,
                color: gray,
                major_color: gray,
                width_px: 2.0,
            }),
            OverlayPrimitive::AxisTriad {
                center: [80.0, 15.0],
                length_px: 12.0,
                width_px: 2.0,
                colors: [
                    Color::new(1.0, 0.0, 0.0, 1.0),
                    Color::new(0.0, 1.0, 0.0, 1.0),
                    Color::new(0.0, 0.0, 1.0, 1.0),
                ],
            },
        ],
    };
    renderer
        .render_to_image(&mut scene, &frame, SIZE, SIZE, &settings)
        .unwrap();
    let image = renderer
        .render_with(
            &mut scene,
            &frame,
            SIZE,
            SIZE,
            &settings,
            &FrameExtras {
                overlay: Some(&overlay),
                ..FrameExtras::default()
            },
        )
        .unwrap();
    assert_eq!(
        scene.last_frame_writes(),
        WriteCounters::default(),
        "overlays write no scene buffer"
    );
    let at = |x, y| image.pixel(x, y).unwrap();
    // The line at y = 5.5 is row 42; behind the square it still shows. Grid lines are drawn
    // later, so the probes avoid them.
    assert_eq!(at(42, 42), WHITE, "above a nearer face");
    assert_eq!(at(42, 32), WHITE, "beyond the far plane");
    assert_eq!(at(42, 44), RED, "the scene elsewhere");
    // The marker covers pixels 6..=13; grid lines run along column 7 and row 7.
    assert_eq!(at(10, 10), GREEN, "screen marker");
    assert_eq!(at(13, 12), GREEN);
    assert_eq!(at(14, 12), [0, 0, 0, 255], "marker size");
    let band = at(82, 85);
    assert!(
        (linear(band[2]) - 0.5).abs() < 0.01 && band[0] == 0,
        "{band:?}"
    );
    // Grid line x = 10 covers columns 57 and 58, above the square.
    assert_eq!(at(57, 40), GRAY);
    assert_eq!(at(58, 40), GRAY);
    assert_eq!(at(60, 40), RED);
    // Axis triad in a top view: x to the right, y up, z toward the eye (a dot).
    assert_eq!(at(89, 15), RED, "x axis");
    assert_eq!(at(80, 6), GREEN, "y axis");
    assert_eq!(at(80, 22), [0, 0, 0, 255]);
}
