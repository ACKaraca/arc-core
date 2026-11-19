//! CORE-GPU-019 and CORE-GPU-005: levels of detail chosen by projected screen error, and large
//! meshes uploaded as tiles with their own origins.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use arc_base::id::Identifier;
use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Limits, ObjectId, Tolerance};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Extraction, ExtractionResult, ItemKey, MeshInstance, MeshItem, Outcome, RenderItem,
    RenderSettings, Renderer, Scene,
};
use arc_mesh::TriMesh;
use arc_mesh::lod::{LodChain, LodOptions, ViewParams, build_lod_chain};
use support::{ids, setup, top_view};

const SIZE: u32 = 100;
const FOV: f64 = 0.6;
const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);

fn sphere(bands: u32, segments: u32) -> TriMesh {
    let mut positions = vec![Point3::new(0.0, 0.0, 1.0)];
    for i in 1..bands {
        let theta = std::f64::consts::PI * f64::from(i) / f64::from(bands);
        for j in 0..segments {
            let phi = std::f64::consts::TAU * f64::from(j) / f64::from(segments);
            positions.push(Point3::new(
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ));
        }
    }
    positions.push(Point3::new(0.0, 0.0, -1.0));
    let south = u32::try_from(positions.len() - 1).unwrap();
    let ring = |i: u32, j: u32| 1 + (i - 1) * segments + (j % segments);
    let mut triangles = Vec::new();
    for j in 0..segments {
        triangles.push([0, ring(1, j), ring(1, j + 1)]);
        triangles.push([south, ring(bands - 1, j + 1), ring(bands - 1, j)]);
    }
    for i in 1..bands - 1 {
        for j in 0..segments {
            triangles.push([ring(i, j), ring(i + 1, j), ring(i + 1, j + 1)]);
            triangles.push([ring(i, j), ring(i + 1, j + 1), ring(i, j + 1)]);
        }
    }
    TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap()
}

fn chain() -> LodChain {
    let chain = build_lod_chain(
        &sphere(16, 32),
        &LodOptions::default(),
        &Tolerance::DEFAULT,
        &Limits::DEFAULT,
    )
    .unwrap();
    assert!(chain.levels().len() > 3, "{} levels", chain.levels().len());
    chain
}

fn from_distance(distance: f64) -> ViewFrame {
    ViewFrame::new(
        Point3::new(0.0, 0.0, distance),
        Point3::origin(),
        Vector3::y(),
        Projection::Perspective { vertical_fov: FOV },
        0.1,
        100_000.0,
    )
    .unwrap()
}

fn lod_scene(scene: &mut Scene, chain: &LodChain) -> ItemKey {
    let geometry = scene.add_lod_geometry(chain, &Point3::origin()).unwrap();
    let object = ids(1)[0];
    scene
        .update(vec![(
            object,
            vec![RenderItem::Mesh(MeshItem {
                geometry,
                origin: Point3::origin(),
                instances: vec![MeshInstance::new(RED)],
            })],
        )])
        .unwrap();
    ItemKey { object, sub: 0 }
}

#[test]
fn the_level_follows_the_projected_error_and_never_gets_finer_with_distance() {
    let (_, mut scene, mut renderer) = setup();
    let chain = chain();
    let key = lod_scene(&mut scene, &chain);
    let settings = RenderSettings::default();
    // Pixels per unit at unit distance for a perspective view of SIZE pixels.
    let pixels_per_unit = f64::from(SIZE) / (2.0 * (FOV / 2.0).tan());
    let mut previous = 0;
    let mut levels = Vec::new();
    for distance in [2.0, 4.0, 8.0, 16.0, 40.0, 100.0, 300.0, 1000.0, 5000.0] {
        let image = renderer
            .render_to_image(&mut scene, &from_distance(distance), SIZE, SIZE, &settings)
            .unwrap();
        // The item is the sphere of radius 1: its nearest depth is one unit nearer.
        let view = ViewParams {
            distance: distance - 1.0,
            pixels_per_unit,
        };
        let expected = chain
            .select(f64::from(settings.lod_budget_px), &view)
            .unwrap();
        let level = scene.lod_level(key).unwrap();
        assert_eq!(level, expected, "at {distance}");
        assert!(
            level >= previous,
            "level {level} after {previous} at {distance}"
        );
        previous = level;
        levels.push(level);
        if distance <= 40.0 {
            let p = image.pixel(SIZE / 2, SIZE / 2).unwrap();
            assert!(p[0] > 200, "the sphere is drawn at level {level}: {p:?}");
        }
    }
    assert_eq!(levels[0], 0, "close up draws the finest level: {levels:?}");
    assert!(
        levels.last().unwrap() > &0,
        "far away draws a coarser level: {levels:?}"
    );
}

#[test]
fn a_zero_budget_draws_the_finest_level_and_orthographic_views_use_the_zoom() {
    let (_, mut scene, mut renderer) = setup();
    let chain = chain();
    let key = lod_scene(&mut scene, &chain);
    let zero = RenderSettings {
        lod_budget_px: 0.0,
        ..RenderSettings::default()
    };
    renderer
        .render_to_image(&mut scene, &from_distance(5000.0), SIZE, SIZE, &zero)
        .unwrap();
    assert_eq!(scene.lod_level(key), Some(0));
    // Orthographic: the level depends on the zoom, not on the eye distance.
    let level_at = |height: f64, distance: f64, renderer: &mut Renderer, scene: &mut Scene| {
        let frame = ViewFrame::new(
            Point3::new(0.0, 0.0, distance),
            Point3::origin(),
            Vector3::y(),
            Projection::Orthographic { height },
            0.1,
            100_000.0,
        )
        .unwrap();
        renderer
            .render_to_image(scene, &frame, SIZE, SIZE, &RenderSettings::default())
            .unwrap();
        scene.lod_level(key).unwrap()
    };
    let near = level_at(3.0, 10.0, &mut renderer, &mut scene);
    assert_eq!(near, level_at(3.0, 5000.0, &mut renderer, &mut scene));
    assert!(level_at(3000.0, 10.0, &mut renderer, &mut scene) > near);
    assert!(
        scene
            .lod_level(ItemKey {
                object: ids(2)[1],
                sub: 0
            })
            .is_none()
    );
}

#[test]
fn extractions_carry_levels_and_release_them_with_the_object() {
    let (_, mut scene, mut renderer) = setup();
    let chain = chain();
    let object = ObjectId::from_u128(7);
    let mut extraction = Extraction::default();
    let geometry = extraction.add_lod_geometry(&chain, Point3::origin());
    extraction.items.push(RenderItem::Mesh(MeshItem {
        geometry,
        origin: Point3::origin(),
        instances: vec![MeshInstance::new(RED)],
    }));
    let result = |revision, outcome| ExtractionResult {
        object,
        revision,
        sequence: 0,
        outcome,
    };
    let report = scene.apply_extractions(vec![result(1, Outcome::Items(extraction))]);
    assert_eq!(report.applied, 1, "{:?}", report.failed);
    renderer
        .render_to_image(
            &mut scene,
            &from_distance(5000.0),
            SIZE,
            SIZE,
            &RenderSettings::default(),
        )
        .unwrap();
    let key = ItemKey { object, sub: 0 };
    assert!(scene.lod_level(key).unwrap() > 0);
    assert!(scene.memory().vertex.used_bytes > 0);
    scene.apply_extractions(vec![result(2, Outcome::Removed)]);
    assert_eq!(
        scene.memory().vertex.used_bytes,
        0,
        "every level is released"
    );
}

/// A flat grid of `n × n` squares of `spacing`, its lower left corner at `corner`.
fn grid(n: u32, spacing: f64, corner: Point3) -> TriMesh {
    let mut positions = Vec::new();
    for y in 0..=n {
        for x in 0..=n {
            positions
                .push(corner + Vector3::new(f64::from(x) * spacing, f64::from(y) * spacing, 0.0));
        }
    }
    let mut triangles = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let i = y * (n + 1) + x;
            triangles.push([i, i + 1, i + n + 2]);
            triangles.push([i, i + n + 2, i + n + 1]);
        }
    }
    TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap()
}

#[test]
fn a_four_kilometre_mesh_is_uploaded_as_tiles_that_draw_where_it_was_authored() {
    let (_, mut scene, mut renderer) = setup();
    let corner = Point3::new(10_000_000.0, 4_000_000.0, 3.0);
    let tiles = scene.add_tiled_geometry(&grid(8, 500.0, corner)).unwrap();
    assert!(tiles.len() >= 4, "{} tiles", tiles.len());
    for tile in &tiles {
        assert!((tile.origin - corner).norm() < 4000.0 * 2.0);
    }
    let items = tiles
        .iter()
        .map(|t| RenderItem::Mesh(t.item(vec![MeshInstance::new(RED)])))
        .collect();
    scene.update(vec![(ids(1)[0], items)]).unwrap();
    // The view is centered on the mesh corner at 8 pixels per unit: the mesh fills the upper
    // right quadrant of the image (world y points up).
    let image = renderer
        .render_to_image(
            &mut scene,
            &top_view(corner, 12.0),
            SIZE,
            SIZE,
            &RenderSettings::default(),
        )
        .unwrap();
    assert_eq!(image.pixel(58, 40).unwrap()[0], 255);
    assert_eq!(image.pixel(40, 58).unwrap(), [0, 0, 0, 255]);
    assert_eq!(image.pixel(58, 58).unwrap(), [0, 0, 0, 255]);
    assert_eq!(image.pixel(40, 40).unwrap(), [0, 0, 0, 255]);
    // The corner itself is within a pixel of the center, 10^7 from the origin.
    assert_eq!(image.pixel(51, 48).unwrap()[0], 255);
}
