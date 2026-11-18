//! CORE-GPU-004: geometry 10,000 km from the origin renders like geometry at the origin.

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
    Image, LineItem, LinePath, LineStyle, LineWidth, MeshInstance, MeshItem, RenderItem,
    RenderSettings,
};
use support::{cube, ids, setup};

/// A small scene placed at `base`: two cubes 2 mm apart and a polyline with millimetre steps,
/// seen from 3 m away. Any vertex error near a millimetre would be visible at this zoom.
fn render_at(base: Point3) -> Image {
    let (_, mut scene, mut renderer) = setup();
    let mesh = cube();
    let geometry = scene.add_geometry(&mesh, &Point3::origin()).unwrap();
    let id = ids(3);
    let instance = |color| MeshInstance::new(color);
    scene
        .update(vec![
            (
                id[0],
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin: base,
                    instances: vec![instance(Color::new(0.8, 0.3, 0.1, 1.0))],
                })],
            ),
            (
                id[1],
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin: base + Vector3::new(1.002, 0.0, 0.0),
                    instances: vec![instance(Color::new(0.1, 0.4, 0.8, 1.0))],
                })],
            ),
            (
                id[2],
                vec![RenderItem::Lines(LineItem {
                    origin: base + Vector3::new(0.0, 0.0, 0.6),
                    paths: vec![LinePath {
                        points: (0..20)
                            .map(|i| [i as f32 * 0.05 - 0.5, (i % 2) as f32 * 0.001, 0.0])
                            .collect(),
                        closed: false,
                    }],
                    style: LineStyle::round(LineWidth::Screen(2.0)),
                    color: Color::WHITE,
                })],
            ),
        ])
        .unwrap();
    let frame = ViewFrame::new(
        base + Vector3::new(1.2, -2.4, 1.6),
        base + Vector3::new(0.5, 0.0, 0.0),
        Vector3::z(),
        Projection::Perspective { vertical_fov: 0.7 },
        0.05,
        50.0,
    )
    .unwrap();
    renderer
        .render_to_image(&mut scene, &frame, 160, 120, &RenderSettings::default())
        .unwrap()
}

#[test]
fn ten_thousand_kilometres_from_the_origin_renders_like_the_origin() {
    let reference = render_at(Point3::origin());
    for offset in [
        Vector3::new(1e7, 0.0, 0.0),
        Vector3::new(0.0, 1e7, 0.0),
        Vector3::new(0.0, 0.0, 1e7),
        Vector3::new(-1e7, 1e7, -1e7),
    ] {
        let far = render_at(Point3::origin() + offset);
        let (mean, max) = support::difference(&reference, &far);
        // The only differences allowed are rounding of the relative translation, below 0.1 mm.
        assert!(
            mean < 0.05 && max < 8.0,
            "offset {offset:?}: mean {mean}, max {max}"
        );
    }
}
