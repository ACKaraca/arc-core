//! CORE-GPU-018 (CPU path): items wholly outside the view are skipped before a frame and before
//! a pick. The culled set never removes an item that has a vertex inside the view, a frame with
//! culling is pixel-identical to a frame without it, and an item behind the camera is culled.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "tests fail by panicking; coordinates and pixel arithmetic of small test scenes"
)]

mod support;

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, ObjectId};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    FrameExtras, Gpu, ItemKey, LineItem, LinePath, LineStyle, LineWidth, MeshInstance, MeshItem,
    PickRegion, RenderItem, RenderSettings, Renderer, Scene, SceneLimits, ShadingMode,
};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use support::{cube, gpu, ids, top_view};

const SIZE: u32 = 64;
const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);

/// The eight corners of the unit cube of `support::cube`.
fn cube_corners() -> Vec<Vector3> {
    let mut out = Vec::new();
    for corner in 0..8 {
        let c = |bit: usize| if corner & (1 << bit) == 0 { -0.5 } else { 0.5 };
        out.push(Vector3::new(c(0), c(1), c(2)));
    }
    out
}

/// Puts one cube item per `(object, origin)` in `scene`.
fn place_cubes(scene: &mut Scene, gpu: &Gpu, items: &[(ObjectId, Point3)]) {
    let _ = gpu;
    let geometry = scene.add_geometry(&cube(), &Point3::origin()).unwrap();
    let changes = items
        .iter()
        .map(|(id, origin)| {
            let item = MeshItem {
                geometry,
                origin: *origin,
                instances: vec![MeshInstance::new(RED)],
            };
            (*id, vec![RenderItem::Mesh(item)])
        })
        .collect();
    scene.update(changes).unwrap();
}

fn perspective(eye: Point3, target: Point3, vertical_fov: f64, up: Vector3) -> Option<ViewFrame> {
    ViewFrame::new(
        eye,
        target,
        up,
        Projection::Perspective { vertical_fov },
        1.0,
        100.0,
    )
    .ok()
}

#[test]
fn culling_never_removes_an_item_with_a_vertex_inside_the_view() {
    let gpu = gpu();
    let mut runner = TestRunner::new(Config::with_cases(24));
    let strategy = (
        prop::collection::vec((-6.0..6.0f64, -6.0..6.0f64, -6.0..6.0f64), 1..8),
        (-1.0..1.0f64, -1.0..1.0f64, -1.0..1.0f64),
        0.3..1.2f64,
    );
    runner
        .run(&strategy, |(origins, dir, fov)| {
            let dir = Vector3::new(dir.0, dir.1, dir.2);
            if dir.norm() < 0.2 {
                return Ok(());
            }
            let eye = Point3::origin() + dir.normalize() * 14.0;
            let Some(frame) = perspective(eye, Point3::origin(), fov, Vector3::z()) else {
                return Ok(());
            };
            let origins: Vec<Point3> = origins
                .into_iter()
                .map(|(x, y, z)| Point3::new(x, y, z))
                .collect();
            let objects = ids(origins.len());
            let placed: Vec<(ObjectId, Point3)> = objects
                .iter()
                .copied()
                .zip(origins.iter().copied())
                .collect();
            let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
            place_cubes(&mut scene, &gpu, &placed);
            let mut renderer = Renderer::new(&gpu);
            renderer
                .render_to_image(&mut scene, &frame, SIZE, SIZE, &RenderSettings::default())
                .map_err(|e| TestCaseError::fail(format!("{e:?}")))?;

            let tan = (fov / 2.0).tan();
            for (object, origin) in placed {
                let inside = cube_corners().iter().any(|v| {
                    let c = frame.world_to_view_point(&(origin + v));
                    let depth = -c.z;
                    depth >= frame.near()
                        && depth <= frame.far()
                        && c.x.abs() <= depth * tan
                        && c.y.abs() <= depth * tan
                });
                let visible = scene
                    .item_visible(ItemKey { object, sub: 0 })
                    .unwrap_or(true);
                prop_assert!(
                    !inside || visible,
                    "an item with a vertex inside the view was culled"
                );
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn an_item_moved_behind_the_camera_is_culled() {
    let gpu = gpu();
    let frame = perspective(
        Point3::new(0.0, 0.0, 10.0),
        Point3::origin(),
        0.8,
        Vector3::y(),
    )
    .unwrap();
    let [front, behind] = [ids(2)[0], ids(2)[1]];
    let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
    place_cubes(
        &mut scene,
        &gpu,
        &[
            (front, Point3::origin()),
            (behind, Point3::new(0.0, 0.0, 20.0)),
        ],
    );
    let mut renderer = Renderer::new(&gpu);
    renderer
        .render_to_image(&mut scene, &frame, SIZE, SIZE, &RenderSettings::default())
        .unwrap();
    assert_eq!(
        scene.item_visible(ItemKey {
            object: front,
            sub: 0
        }),
        Some(true)
    );
    assert_eq!(
        scene.item_visible(ItemKey {
            object: behind,
            sub: 0
        }),
        Some(false)
    );
    let stats = scene.cull_stats();
    assert_eq!((stats.items, stats.visible), (2, 1));
}

#[test]
fn an_item_partly_inside_the_view_is_drawn_and_one_outside_is_culled() {
    let gpu = gpu();
    // The view covers x in [-1.5, 1.5] at 64 pixels per 1.5 units.
    let frame = top_view(Point3::origin(), 3.0);
    let [partly, outside] = [ids(2)[0], ids(2)[1]];
    let mut scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
    place_cubes(
        &mut scene,
        &gpu,
        &[
            (partly, Point3::new(1.6, 0.0, 0.0)),
            (outside, Point3::new(3.0, 0.0, 0.0)),
        ],
    );
    let mut renderer = Renderer::new(&gpu);
    renderer
        .render_to_image(&mut scene, &frame, SIZE, SIZE, &RenderSettings::default())
        .unwrap();
    assert_eq!(
        scene.item_visible(ItemKey {
            object: partly,
            sub: 0
        }),
        Some(true)
    );
    assert_eq!(
        scene.item_visible(ItemKey {
            object: outside,
            sub: 0
        }),
        Some(false)
    );
}

/// A scene with items at, beside and well outside the edges of the view, with lines and
/// selections, so that every pass and the pick see the culling.
fn edge_scene(gpu: &Gpu) -> (Scene, Vec<ObjectId>) {
    let objects = ids(6);
    let mut scene = Scene::new(gpu, SceneLimits::DEFAULT).unwrap();
    let geometry = scene.add_geometry(&cube(), &Point3::origin()).unwrap();
    let mesh = |origin: Point3| {
        RenderItem::Mesh(MeshItem {
            geometry,
            origin,
            instances: vec![MeshInstance::new(BLUE)],
        })
    };
    let line = |points: Vec<[f32; 3]>| {
        RenderItem::Lines(LineItem {
            origin: Point3::origin(),
            paths: vec![LinePath {
                points,
                closed: false,
            }],
            style: LineStyle::round(LineWidth::Screen(6.0)),
            color: RED,
        })
    };
    // Partly inside, just outside by about a pixel, well outside, and a line on the edge.
    let changes = vec![
        (objects[0], vec![mesh(Point3::origin())]),
        (objects[1], vec![mesh(Point3::new(1.6, 0.0, 0.0))]),
        (objects[2], vec![mesh(Point3::new(2.52, 0.2, 0.0))]),
        (objects[3], vec![mesh(Point3::new(-3.0, -2.0, 0.0))]),
        (
            objects[4],
            vec![line(vec![[1.55, -1.2, 0.0], [1.55, 1.2, 0.0]])],
        ),
        (
            objects[5],
            vec![line(vec![[2.5, -1.0, 0.0], [2.5, 1.0, 0.0]])],
        ),
    ];
    scene.update(changes).unwrap();
    (scene, objects)
}

#[test]
fn a_frame_with_culling_is_pixel_identical_to_one_without() {
    let gpu = gpu();
    let frame = top_view(Point3::origin(), 3.0);
    let (mut scene, objects) = edge_scene(&gpu);
    let mut renderer = Renderer::new(&gpu);
    let extras = FrameExtras {
        hovered: Some(objects[1]),
        selected: &[objects[0], objects[4]],
        ..FrameExtras::default()
    };
    for mode in [
        ShadingMode::Wireframe,
        ShadingMode::HiddenLine,
        ShadingMode::Shaded,
        ShadingMode::ShadedWithEdges,
        ShadingMode::XRay,
    ] {
        let settings = RenderSettings {
            mode,
            silhouette_edges: true,
            ..RenderSettings::default()
        };
        let culled = RenderSettings {
            cull_items: true,
            ..settings
        };
        let plain = RenderSettings {
            cull_items: false,
            ..settings
        };
        let with = renderer
            .render_with(&mut scene, &frame, SIZE, SIZE, &culled, &extras)
            .unwrap();
        let stats = scene.cull_stats();
        assert!(
            stats.visible < stats.items,
            "the scene must contain items to cull"
        );
        let without = renderer
            .render_with(&mut scene, &frame, SIZE, SIZE, &plain, &extras)
            .unwrap();
        assert_eq!(scene.cull_stats().visible, scene.cull_stats().items);
        assert!(
            with.rgba8 == without.rgba8,
            "{mode:?}: culling changed the image"
        );
    }
}

#[test]
fn a_pick_with_culling_returns_the_hits_of_a_pick_without() {
    let gpu = gpu();
    let frame = top_view(Point3::origin(), 3.0);
    let (mut scene, _) = edge_scene(&gpu);
    let mut renderer = Renderer::new(&gpu);
    let region = PickRegion {
        x: 0,
        y: 0,
        width: SIZE,
        height: SIZE,
        tolerance_px: 3.0,
    };
    let culled = RenderSettings::default();
    let plain = RenderSettings {
        cull_items: false,
        ..culled
    };
    let with = renderer
        .pick(&mut scene, &frame, SIZE, SIZE, &culled, &region)
        .unwrap();
    let without = renderer
        .pick(&mut scene, &frame, SIZE, SIZE, &plain, &region)
        .unwrap();
    assert!(!with.is_empty());
    assert_eq!(with, without);
}

#[test]
fn without_culling_every_item_counts_as_visible() {
    let gpu = gpu();
    let frame = top_view(Point3::origin(), 3.0);
    let (mut scene, objects) = edge_scene(&gpu);
    let mut renderer = Renderer::new(&gpu);
    let plain = RenderSettings {
        cull_items: false,
        ..RenderSettings::default()
    };
    renderer
        .render_to_image(&mut scene, &frame, SIZE, SIZE, &plain)
        .unwrap();
    let stats = scene.cull_stats();
    assert_eq!((stats.items, stats.visible), (6, 6));
    assert_eq!(
        scene.item_visible(ItemKey {
            object: objects[3],
            sub: 0
        }),
        Some(true)
    );
}
