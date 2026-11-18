//! CORE-GPU-007: instanced meshes with per-instance transforms and colors, the headlight model,
//! back-face culling and reversed-Z depth.

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
use arc_gpu::{MeshInstance, MeshItem, RenderItem, RenderSettings, Scene};
use support::{ids, setup, square, top_view};

const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);

fn place(scene: &mut Scene, items: Vec<(Point3, Vec<MeshInstance>)>) {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(items.len());
    let changes = items
        .into_iter()
        .zip(id)
        .map(|((origin, instances), id)| {
            (
                id,
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin,
                    instances,
                })],
            )
        })
        .collect();
    scene.update(changes).unwrap();
}

#[test]
fn instances_carry_their_transforms_and_colors_and_face_the_headlight() {
    let (gpu, mut scene, mut renderer) = setup();
    let at = |x: f32, color| MeshInstance {
        transform: [
            [0.5, 0.0, 0.0, x],
            [0.0, 0.5, 0.0, 0.0],
            [0.0, 0.0, 0.5, 0.0],
        ],
        color,
        material: 3,
        flags: 0,
    };
    place(
        &mut scene,
        vec![(
            Point3::new(10.0, 0.0, 0.0),
            vec![at(-2.0, RED), at(0.0, GREEN), at(2.0, Color::WHITE)],
        )],
    );
    let image = support::render(
        &mut renderer,
        &mut scene,
        &top_view(Point3::new(10.0, 0.0, 0.0), 6.0),
        96,
    );
    // Squares of side 1 centered at x = 8, 10 and 12 (pixels 16 apart): flat regions face the
    // headlight, so Lambert is 1 and the stored color is the base color.
    assert_eq!(image.pixel(16, 48), Some([255, 0, 0, 255]));
    assert_eq!(image.pixel(48, 48), Some([0, 255, 0, 255]));
    assert_eq!(image.pixel(80, 48), Some([255, 255, 255, 255]));
    assert_eq!(
        image.pixel(32, 48),
        Some([0, 0, 0, 255]),
        "gaps between instances"
    );
    assert_eq!(
        image.pixel(48, 36),
        Some([0, 0, 0, 255]),
        "scaled to half size"
    );
    support::compare_reference(&gpu, "three_instances", &image, 0.5, 32.0);
}

#[test]
fn oblique_faces_are_darker_down_to_the_ambient_term() {
    let (_, mut scene, mut renderer) = setup();
    // Tilted 60 degrees about x: Lambert is cos 60 = 0.5.
    let (s, c) = (60_f32.to_radians().sin(), 60_f32.to_radians().cos());
    let tilted = MeshInstance {
        transform: [[1.0, 0.0, 0.0, 0.0], [0.0, c, -s, 0.0], [0.0, s, c, 0.0]],
        ..MeshInstance::new(Color::WHITE)
    };
    place(&mut scene, vec![(Point3::origin(), vec![tilted])]);
    let image = support::render(
        &mut renderer,
        &mut scene,
        &top_view(Point3::origin(), 4.0),
        64,
    );
    let expected = 0.25 + 0.75 * 0.5;
    let got = support::coverage(&image, 32, 32);
    assert!((got - expected).abs() < 0.01, "{got} vs {expected}");
}

#[test]
fn back_faces_are_culled_only_when_asked() {
    let (_, mut scene, mut renderer) = setup();
    // Flipped about x: the square faces −z, away from the eye above it.
    let flipped = MeshInstance {
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, -1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
        ],
        ..MeshInstance::new(RED)
    };
    place(&mut scene, vec![(Point3::origin(), vec![flipped])]);
    let frame = top_view(Point3::origin(), 4.0);
    let mut settings = RenderSettings::default();
    let shown = renderer
        .render_to_image(&mut scene, &frame, 32, 32, &settings)
        .unwrap();
    assert_eq!(
        shown.pixel(16, 16),
        Some([255, 0, 0, 255]),
        "two-sided lighting"
    );
    settings.cull_back_faces = true;
    let culled = renderer
        .render_to_image(&mut scene, &frame, 32, 32, &settings)
        .unwrap();
    assert_eq!(culled.pixel(16, 16), Some([0, 0, 0, 255]));
}

fn nearer_wins(frame: &ViewFrame, near: Point3, far: Point3) {
    for order in [[near, far], [far, near]] {
        let (_, mut scene, mut renderer) = setup();
        let color = |p: Point3| if p == near { GREEN } else { RED };
        place(
            &mut scene,
            order
                .iter()
                .map(|p| (*p, vec![MeshInstance::new(color(*p))]))
                .collect(),
        );
        let image = support::render(&mut renderer, &mut scene, frame, 32);
        assert_eq!(
            image.pixel(16, 16),
            Some([0, 255, 0, 255]),
            "drawing order {order:?}"
        );
    }
}

#[test]
fn reversed_depth_orders_surfaces_at_a_near_far_ratio_of_a_million() {
    // Near 1 cm, far 10 km; squares 1 m wide at 5 km, 2 cm apart, scaled up to fill the view.
    let frame = ViewFrame::new(
        Point3::new(0.0, 0.0, 5000.0),
        Point3::origin(),
        Vector3::y(),
        Projection::Perspective {
            vertical_fov: 0.0002,
        },
        0.01,
        1e4,
    )
    .unwrap();
    nearer_wins(&frame, Point3::new(0.0, 0.0, 0.02), Point3::origin());
    // Close to the near plane.
    let close = ViewFrame::new(
        Point3::new(0.0, 0.0, 0.05),
        Point3::origin(),
        Vector3::y(),
        Projection::Perspective { vertical_fov: 1.5 },
        0.01,
        1e4,
    )
    .unwrap();
    nearer_wins(&close, Point3::new(0.0, 0.0, 0.0001), Point3::origin());
}

#[test]
fn orthographic_depth_is_fitted_to_the_scene() {
    // A depth range of 2,000 km; surfaces 1 mm apart are still ordered.
    let frame = ViewFrame::new(
        Point3::new(0.0, 0.0, 1e6),
        Point3::origin(),
        Vector3::y(),
        Projection::Orthographic { height: 3.0 },
        -1e6,
        1e6,
    )
    .unwrap();
    nearer_wins(&frame, Point3::new(0.0, 0.0, 0.001), Point3::origin());
}
