//! CORE-GPU-002 retained viewport scene and CORE-GPU-006 pooled buffers.

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
    LineItem, LinePath, LineStyle, LineWidth, MeshInstance, MeshItem, RenderItem, Scene,
    SceneLimits, WriteCounters,
};
use support::{ids, setup, square, top_view};

fn mesh(geometry: arc_gpu::GeometryId, x: f64, y: f64, color: Color) -> RenderItem {
    RenderItem::Mesh(MeshItem {
        geometry,
        origin: Point3::new(x, y, 0.0),
        instances: vec![MeshInstance::new(color)],
    })
}

fn line(x: f64, y: f64) -> RenderItem {
    RenderItem::Lines(LineItem {
        origin: Point3::new(x, y, 1.0),
        paths: vec![LinePath {
            points: vec![[-0.4, 0.0, 0.0], [0.4, 0.0, 0.0], [0.4, 0.4, 0.0]],
            closed: false,
        }],
        style: LineStyle::round(LineWidth::Screen(1.0)),
        color: Color::WHITE,
    })
}

/// 10,000 objects on a 100 × 100 grid, alternating meshes and lines.
fn grid(scene: &mut Scene) -> (Vec<arc_base::ObjectId>, arc_gpu::GeometryId) {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let objects = ids(10_000);
    let changes = objects
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let (x, y) = ((i % 100) as f64 * 3.0, (i / 100) as f64 * 3.0);
            let item = if i % 2 == 0 {
                mesh(geometry, x, y, Color::WHITE)
            } else {
                line(x, y)
            };
            (*id, vec![item])
        })
        .collect();
    scene.update(changes).unwrap();
    (objects, geometry)
}

#[test]
fn updating_one_of_ten_thousand_objects_writes_only_its_ranges() {
    let (_, mut scene, mut renderer) = setup();
    let (objects, geometry) = grid(&mut scene);
    let frame = top_view(Point3::new(150.0, 150.0, 0.0), 320.0);
    support::render(&mut renderer, &mut scene, &frame, 64);
    let first = scene.last_frame_writes();
    assert_eq!(
        first.translation,
        12 * (5000 + 5000 * 2),
        "the first frame writes every translation"
    );
    assert_eq!(scene.pending_writes(), WriteCounters::default());

    // A mesh object changes color: one 80-byte instance is written, nothing else.
    scene
        .update(vec![(
            objects[0],
            vec![mesh(geometry, 0.0, 0.0, Color::BLACK)],
        )])
        .unwrap();
    assert_eq!(
        scene.pending_writes(),
        WriteCounters {
            mesh_instance: 80,
            ..WriteCounters::default()
        }
    );
    support::render(&mut renderer, &mut scene, &frame, 64);
    assert_eq!(
        scene.last_frame_writes(),
        WriteCounters {
            mesh_instance: 80,
            translation: 12,
            ..WriteCounters::default()
        },
        "with the camera unchanged only the changed item's translation is written"
    );

    // A line object changes: its two segments are written.
    scene
        .update(vec![(objects[1], vec![line(3.0, 0.0)])])
        .unwrap();
    assert_eq!(
        scene.pending_writes(),
        WriteCounters {
            line_segment: 256,
            ..WriteCounters::default()
        }
    );
    support::render(&mut renderer, &mut scene, &frame, 64);
    assert_eq!(scene.last_frame_writes().translation, 24);

    // Moving the camera rewrites translations, but no item data.
    let moved = top_view(Point3::new(151.0, 150.0, 0.0), 320.0);
    support::render(&mut renderer, &mut scene, &moved, 64);
    let w = scene.last_frame_writes();
    assert_eq!(
        (w.vertex, w.index, w.mesh_instance, w.line_segment),
        (0, 0, 0, 0)
    );
    assert_eq!(w.translation, first.translation);
}

#[test]
fn changed_items_render_and_untouched_items_keep_their_pixels() {
    let (_, mut scene, mut renderer) = setup();
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(2);
    let red = Color::new(1.0, 0.0, 0.0, 1.0);
    let blue = Color::new(0.0, 0.0, 1.0, 1.0);
    scene
        .update(vec![
            (id[0], vec![mesh(geometry, -2.0, 0.0, red)]),
            (id[1], vec![mesh(geometry, 2.0, 0.0, red)]),
        ])
        .unwrap();
    let frame = top_view(Point3::origin(), 8.0);
    let before = support::render(&mut renderer, &mut scene, &frame, 64);
    scene
        .update(vec![(id[1], vec![mesh(geometry, 2.0, 0.0, blue)])])
        .unwrap();
    let after = support::render(&mut renderer, &mut scene, &frame, 64);
    assert_eq!(before.pixel(16, 32), after.pixel(16, 32));
    assert_eq!(after.pixel(16, 32), Some([255, 0, 0, 255]));
    assert_eq!(after.pixel(48, 32), Some([0, 0, 255, 255]));
    // Removing an object clears its pixels.
    scene.update(vec![(id[1], vec![])]).unwrap();
    let removed = support::render(&mut renderer, &mut scene, &frame, 64);
    assert_eq!(removed.pixel(48, 32), Some([0, 0, 0, 255]));
    assert_eq!(scene.keys().len(), 1);
}

#[test]
fn memory_is_reported_and_compaction_keeps_the_picture() {
    let (_, mut scene, mut renderer) = setup();
    let (objects, _) = grid(&mut scene);
    let full = scene.memory();
    assert_eq!(full.mesh_instance.used_bytes, 5000 * 80);
    assert_eq!(full.line_segment.used_bytes, 10_000 * 128);
    assert_eq!(full.translation.used_bytes, 15_000 * 12);
    assert_eq!(full.vertex.used_bytes, 4 * 24);
    // Two triangles and the square's four boundary edges.
    assert_eq!(full.index.used_bytes, (6 + 4 * 2) * 4);
    assert!(full.total().capacity_bytes >= full.total().used_bytes);
    // Remove three of every four objects, leaving holes.
    let removed = objects
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 4 != 0)
        .map(|(_, id)| (*id, vec![]))
        .collect();
    scene.update(removed).unwrap();
    let frame = top_view(Point3::new(150.0, 150.0, 0.0), 320.0);
    let before = support::render(&mut renderer, &mut scene, &frame, 96);
    scene.compact().unwrap();
    let after = support::render(&mut renderer, &mut scene, &frame, 96);
    assert_eq!(before, after);
    let memory = scene.memory();
    assert_eq!(memory.mesh_instance.used_bytes, 2500 * 80);
    assert_eq!(memory.line_segment.used_bytes, 0);
}

#[test]
fn geometry_lifetime_and_invalid_items_are_errors() {
    let (_, mut scene, _) = setup();
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(1)[0];
    scene
        .update(vec![(id, vec![mesh(geometry, 0.0, 0.0, Color::WHITE)])])
        .unwrap();
    assert_eq!(
        scene.remove_geometry(geometry).unwrap_err().code(),
        "gpu.geometry.in_use"
    );
    scene.update(vec![(id, vec![])]).unwrap();
    scene.remove_geometry(geometry).unwrap();
    assert_eq!(
        scene.remove_geometry(geometry).unwrap_err().code(),
        "gpu.geometry.not_found"
    );
    let code = |scene: &mut Scene, item| scene.update(vec![(id, vec![item])]).unwrap_err().code();
    assert_eq!(
        code(&mut scene, mesh(geometry, 0.0, 0.0, Color::WHITE)),
        "gpu.geometry.not_found"
    );
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let empty = RenderItem::Mesh(MeshItem {
        geometry,
        origin: Point3::origin(),
        instances: vec![],
    });
    assert_eq!(code(&mut scene, empty), "gpu.item.invalid");
    assert_eq!(
        code(&mut scene, mesh(geometry, f64::NAN, 0.0, Color::WHITE)),
        "gpu.value.not_finite"
    );
    assert_eq!(scene.keys(), vec![], "a failed update changes nothing");
}

#[test]
fn limits_bound_counts_and_pools() {
    let gpu = support::gpu();
    let limits = SceneLimits {
        max_objects: 2,
        max_pool_bytes: 4096,
        ..SceneLimits::DEFAULT
    };
    let mut scene = Scene::new(&gpu, limits).unwrap();
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(3);
    let changes = |n: usize| {
        id[..n]
            .iter()
            .map(|i| (*i, vec![mesh(geometry, 0.0, 0.0, Color::WHITE)]))
            .collect()
    };
    scene.update(changes(2)).unwrap();
    assert_eq!(
        scene.update(changes(3)).unwrap_err().code(),
        "gpu.limit.exceeded"
    );
    // 4096 bytes hold 51 mesh instances.
    let many = RenderItem::Mesh(MeshItem {
        geometry,
        origin: Point3::origin(),
        instances: vec![MeshInstance::new(Color::WHITE); 60],
    });
    assert_eq!(
        scene.update(vec![(id[0], vec![many])]).unwrap_err().code(),
        "gpu.limit.exceeded"
    );
    assert_eq!(
        Scene::new(
            &gpu,
            SceneLimits {
                max_objects: 0,
                ..SceneLimits::DEFAULT
            }
        )
        .unwrap_err()
        .code(),
        "gpu.settings.invalid"
    );
}

#[test]
fn bounding_spheres_frame_the_extents_and_the_selection() {
    let (_, mut scene, mut renderer) = setup();
    assert_eq!(scene.bounding_sphere(None), None);
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(2);
    scene
        .update(vec![
            (id[0], vec![mesh(geometry, 100.0, 0.0, Color::WHITE)]),
            (id[1], vec![mesh(geometry, -100.0, 50.0, Color::WHITE)]),
        ])
        .unwrap();
    // The square's corners are √2 from its origin.
    let (center, radius) = scene.bounding_sphere(Some(&id[..1])).unwrap();
    assert!((center - Point3::new(100.0, 0.0, 0.0)).norm() < 1e-12);
    assert!((radius - 2_f64.sqrt()).abs() < 1e-12);
    let (_, all) = scene.bounding_sphere(None).unwrap();
    assert!(all >= 100.0);
    // Zoom to the selection: the selected square fills the middle of a perspective view.
    let start = arc_geom::view::ViewFrame::new(
        Point3::new(0.0, -300.0, 300.0),
        Point3::origin(),
        arc_base::math::Vector3::z(),
        arc_geom::view::Projection::Perspective { vertical_fov: 0.8 },
        0.1,
        10.0,
    )
    .unwrap();
    let fitted = arc_gpu::navigation::zoom_to_sphere(
        &start,
        &center,
        radius,
        1.0,
        &arc_base::Tolerance::DEFAULT,
    )
    .unwrap();
    let image = support::render(&mut renderer, &mut scene, &fitted, 32);
    // Seen at 45 degrees, the face is lit at Lambert cos 45.
    assert!(image.pixel(16, 16).unwrap()[0] > 200);
    assert_eq!(
        image.pixel(0, 0),
        Some([0, 0, 0, 255]),
        "the other square is out of view"
    );
}
