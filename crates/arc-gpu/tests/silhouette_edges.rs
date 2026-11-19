//! CORE-GPU-017: silhouette edges from derived edge data, drawn with the feature edges over
//! shaded faces.

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
use arc_base::{Color, Limits};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{
    Gpu, MeshInstance, MeshItem, RenderItem, RenderSettings, Renderer, Scene, SceneLimits,
    ShadingMode,
};
use arc_mesh::TriMesh;
use support::{gpu, ids};

const SIZE: u32 = 96;
const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);

/// A unit sphere with its poles on the z axis: `bands` latitude bands (even, so there is an
/// equator row of edges) and `segments` meridians. Neighboring faces meet at 22.5 degrees or
/// less, so no edge is a crease.
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

fn scene_with(gpu: &Gpu, silhouettes: bool) -> Scene {
    let mut scene = Scene::new(
        gpu,
        SceneLimits {
            silhouette_edges: silhouettes,
            ..SceneLimits::DEFAULT
        },
    )
    .unwrap();
    let geometry = scene
        .add_geometry(&sphere(8, 24), &Point3::origin())
        .unwrap();
    scene
        .update(vec![(
            ids(1)[0],
            vec![RenderItem::Mesh(MeshItem {
                geometry,
                origin: Point3::origin(),
                instances: vec![MeshInstance::new(BLUE)],
            })],
        )])
        .unwrap();
    scene
}

fn view(direction: Vector3, projection: Projection) -> ViewFrame {
    ViewFrame::new(
        Point3::from(direction.normalize() * 10.0),
        Point3::origin(),
        Vector3::y(),
        projection,
        1.0,
        100.0,
    )
    .unwrap()
}

/// Pixels whose red channel shows an edge: the faces are pure blue and the background black.
fn edge_pixels(image: &arc_gpu::Image) -> usize {
    image
        .rgba8
        .chunks_exact(4)
        .filter(|p| p[0] > 200 && p[1] > 200)
        .count()
}

fn render(scene: &mut Scene, frame: &ViewFrame, silhouettes: bool) -> arc_gpu::Image {
    let settings = RenderSettings {
        mode: ShadingMode::ShadedWithEdges,
        edge_color: Color::WHITE,
        silhouette_edges: silhouettes,
        ..RenderSettings::default()
    };
    Renderer::new(scene.gpu())
        .render_to_image(scene, frame, SIZE, SIZE, &settings)
        .unwrap()
}

#[test]
fn the_contour_of_a_smooth_solid_is_drawn_only_when_asked() {
    let gpu = gpu();
    let mut scene = scene_with(&gpu, true);
    // Looking down the pole axis the equator is the contour: a ring of radius 32 pixels.
    let top = view(Vector3::z(), Projection::Orthographic { height: 3.0 });
    let without = render(&mut scene, &top, false);
    assert_eq!(
        edge_pixels(&without),
        0,
        "smooth edges are not feature edges"
    );
    let with = render(&mut scene, &top, true);
    let ring = edge_pixels(&with);
    // A circle of radius 32 pixels has a circumference of about 200 pixels; a one-pixel
    // line covers about one pixel per step along it.
    assert!((60..=320).contains(&ring), "{ring} ring pixels");
    // The ring lies at the rim: nothing white near the center.
    assert!(with.pixel(48, 48).unwrap()[0] < 50);
    let rim_near = (44..=52)
        .flat_map(|y| (76..=84).map(move |x| (x, y)))
        .any(|(x, y)| with.pixel(x, y).unwrap()[0] > 200);
    assert!(rim_near, "no white ring pixel at the rim");

    // Tilted, the contour is another great circle: still a closed ring of white pixels,
    // in the perspective view too.
    for projection in [
        Projection::Orthographic { height: 3.0 },
        Projection::Perspective { vertical_fov: 0.3 },
    ] {
        let oblique = view(Vector3::new(1.0, 0.3, 1.0), projection);
        assert_eq!(edge_pixels(&render(&mut scene, &oblique, false)), 0);
        let n = edge_pixels(&render(&mut scene, &oblique, true));
        assert!(n > 60, "{n} contour pixels");
    }
}

#[test]
fn geometry_without_silhouette_data_draws_no_contour() {
    let gpu = gpu();
    let mut scene = scene_with(&gpu, false);
    let top = view(Vector3::z(), Projection::Orthographic { height: 3.0 });
    assert_eq!(edge_pixels(&render(&mut scene, &top, true)), 0);
}

#[test]
fn silhouette_data_is_counted_in_the_vertex_pool() {
    let gpu = gpu();
    let with = scene_with(&gpu, true);
    let without = scene_with(&gpu, false);
    let (a, b) = (
        with.memory().vertex.used_bytes,
        without.memory().vertex.used_bytes,
    );
    // The 8 × 24 sphere has 24 × 8 + 24 × 6 smooth interior edges at 72 bytes each, within
    // the pool's unit rounding.
    assert!(a > b, "{a} vs {b}");
    assert_eq!((a - b) % 72, 0);
}
