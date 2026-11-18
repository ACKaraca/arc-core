//! CORE-GPU-014: shading modes. Each mode matches its analytic picture, and switching mode
//! writes no scene buffer.

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
use arc_base::math::Point3;
use arc_gpu::{
    Image, MeshInstance, MeshItem, RenderItem, RenderSettings, Scene, ShadingMode, WriteCounters,
};
use support::{ids, linear, setup, square, top_view};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const YELLOW: [u8; 4] = [255, 255, 0, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const SIZE: u32 = 96;

/// A square of side `2 · scale` centered on `center`, tilted about x by `tilt` degrees.
fn square_at(center: Point3, scale: f32, tilt: f32, color: Color) -> MeshInstance {
    let (s, c) = tilt.to_radians().sin_cos();
    MeshInstance {
        transform: [
            [scale, 0.0, 0.0, center.x as f32],
            [0.0, scale * c, -scale * s, center.y as f32],
            [0.0, scale * s, scale * c, center.z as f32],
        ],
        ..MeshInstance::new(color)
    }
}

/// One model unit per pixel; pixel `(px, py)` has its center at
/// `(px + 0.5 − 48, 48 − py − 0.5)`.
///
/// - A: red square, side 20, at z = 0, pixels 38..=57 on both axes.
/// - B: green square, side 20, at z = −5, behind A, pixels 48..=67 by 38..=57: its left
///   edge (x = 0, the boundary of column 48) runs behind A.
/// - C: white square tilted 60 degrees about x (Lambert 0.5), centered on pixel (18, 18).
fn populate(scene: &mut Scene) {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(3);
    let item = |instance| {
        vec![RenderItem::Mesh(MeshItem {
            geometry,
            origin: Point3::origin(),
            instances: vec![instance],
        })]
    };
    scene
        .update(vec![
            (
                id[0],
                item(square_at(
                    Point3::origin(),
                    10.0,
                    0.0,
                    Color::new(1.0, 0.0, 0.0, 1.0),
                )),
            ),
            (
                id[1],
                item(square_at(
                    Point3::new(10.0, 0.0, -5.0),
                    10.0,
                    0.0,
                    Color::new(0.0, 1.0, 0.0, 1.0),
                )),
            ),
            (
                id[2],
                item(square_at(
                    Point3::new(-29.5, 29.5, 0.0),
                    10.0,
                    60.0,
                    Color::WHITE,
                )),
            ),
        ])
        .unwrap();
}

/// Whether one of the columns `columns` of row `row` has exactly `color`: a one-pixel edge on
/// a pixel boundary lights the column on either side, depending on the rasterizer.
fn edge_at(image: &Image, columns: [u32; 2], row: u32, color: [u8; 4]) -> bool {
    columns.iter().any(|x| image.pixel(*x, row) == Some(color))
}

#[test]
fn every_mode_matches_its_analytic_picture_and_switching_writes_nothing() {
    let (_, mut scene, mut renderer) = setup();
    populate(&mut scene);
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    let mut settings = RenderSettings {
        edge_color: Color::new(1.0, 1.0, 0.0, 1.0),
        ..RenderSettings::DEFAULT
    };
    renderer
        .render_to_image(&mut scene, &frame, SIZE, SIZE, &settings)
        .unwrap();
    for mode in ShadingMode::ALL {
        settings.mode = mode;
        let image = renderer
            .render_to_image(&mut scene, &frame, SIZE, SIZE, &settings)
            .unwrap();
        assert_eq!(
            scene.last_frame_writes(),
            WriteCounters::default(),
            "{mode:?}: switching mode writes no scene buffer"
        );
        check(mode, &image, &settings);
    }
}

/// The analytic picture of `mode`.
fn check(mode: ShadingMode, image: &Image, settings: &RenderSettings) {
    let at = |x, y| image.pixel(x, y).unwrap();
    // A's left edge is the boundary of column 38; B's hidden edge that of column 48; B's
    // right edge that of column 68.
    let a_edge = edge_at(image, [37, 38], 44, RED);
    let b_hidden_edge = edge_at(image, [47, 48], 44, GREEN);
    let b_right_edge = edge_at(image, [67, 68], 44, GREEN);
    let tilted = linear(at(18, 18)[0]);
    match mode {
        ShadingMode::Shaded | ShadingMode::ConsistentColors => {
            assert_eq!(at(43, 44), RED, "{mode:?}: A's face");
            assert_eq!(at(52, 44), RED, "{mode:?}: A hides B");
            assert_eq!(at(62, 44), GREEN, "{mode:?}: B beside A");
            assert_eq!(at(37, 44), BLACK, "{mode:?}: no edges");
            let expected = if mode == ShadingMode::Shaded {
                0.25 + 0.75 * 0.5
            } else {
                1.0
            };
            assert!((tilted - expected).abs() < 0.01, "{mode:?}: {tilted}");
        }
        ShadingMode::ShadedWithEdges => {
            assert_eq!(at(43, 44), RED);
            assert!(edge_at(image, [37, 38], 44, YELLOW), "A's edge");
            assert!(edge_at(image, [67, 68], 44, YELLOW), "B's visible edge");
            assert!(!edge_at(image, [47, 48], 44, YELLOW), "B's edge behind A");
            // The diagonal of A's two coplanar triangles is not a feature edge.
            for (x, y) in [(47, 47), (48, 47), (48, 48), (47, 48), (49, 46)] {
                assert_eq!(at(x, y), RED, "diagonal at ({x}, {y})");
            }
            assert!((tilted - 0.625).abs() < 0.01);
        }
        ShadingMode::Wireframe => {
            assert_eq!(at(43, 44), BLACK, "no faces");
            assert!(a_edge && b_right_edge);
            assert!(b_hidden_edge, "no hidden-line removal");
        }
        ShadingMode::HiddenLine => {
            assert_eq!(at(43, 44), BLACK, "faces only hide");
            assert!(a_edge && b_right_edge);
            assert!(!b_hidden_edge, "B's edge behind A is hidden");
            assert!(
                !edge_at(image, [47, 48], 44, GREEN) && at(47, 44)[1] == 0,
                "no trace of B behind A"
            );
        }
        ShadingMode::XRay => {
            let opacity = f64::from(settings.xray_opacity);
            let red = linear(at(43, 44)[0]);
            assert!((red - opacity).abs() < 0.01, "{red}");
            assert!(b_hidden_edge, "edges are visible through faces");
            assert!(a_edge && b_right_edge);
        }
    }
}

#[test]
fn edges_are_uploaded_once_with_the_geometry() {
    let (_, mut scene, _) = setup();
    scene
        .add_geometry(&support::cube(), &Point3::origin())
        .unwrap();
    // 12 triangles (36 indices) and the cube's 12 feature edges (24 indices).
    assert_eq!(scene.pending_writes().index, (36 + 24) * 4);
}
