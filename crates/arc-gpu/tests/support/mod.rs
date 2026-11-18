//! Shared helpers of the rendering tests: device creation, scenes, coverage measurement and the
//! reference-image comparison.
//!
//! # Reference images
//!
//! Reference images live in `tests/reference/<name>.png`. They are compared only on the pinned
//! software adapter (Mesa lavapipe on Vulkan, which CI installs on Linux); on other adapters
//! the analytic assertions of each test are the check, because rasterizers differ in edge
//! rules and rounding. The metric blurs both images with a 3×3 box filter and takes the mean
//! and the maximum absolute difference over all channels (0–255); each test states both
//! thresholds. On failure the rendered and difference images are written to
//! `target/tmp/arc-gpu-image-diffs/`. Set `ARC_UPDATE_REFERENCES=1` to rewrite the references
//! on the pinned adapter instead of comparing.
#![allow(dead_code)]

use std::path::PathBuf;

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, IdGenerator, Limits, ObjectId};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::{BackendKind, Gpu, GpuSettings, Image, RenderSettings, Renderer, Scene, SceneLimits};
use arc_mesh::TriMesh;

/// A device on the adapter the selection rules choose. Fails with a clear message when there
/// is none: the tests never pass silently without rendering.
pub fn gpu() -> Gpu {
    match Gpu::new(&GpuSettings::default()) {
        Ok(gpu) => gpu,
        Err(e) => panic!(
            "no usable graphics adapter ({}); on Linux install the software Vulkan driver \
             (mesa-vulkan-drivers)",
            e.code()
        ),
    }
}

/// A device, a scene and a renderer.
pub fn setup() -> (Gpu, Scene, Renderer) {
    let gpu = gpu();
    let scene = Scene::new(&gpu, SceneLimits::DEFAULT).unwrap();
    let renderer = Renderer::new(&gpu);
    (gpu, scene, renderer)
}

/// Whether the device runs on the pinned software adapter.
pub fn pinned(gpu: &Gpu) -> bool {
    let r = gpu.report();
    r.backend == BackendKind::Vulkan && r.software && r.name.to_lowercase().contains("llvmpipe")
}

/// Distinct object identifiers.
pub fn ids(n: usize) -> Vec<ObjectId> {
    let mut generator = IdGenerator::seeded(42);
    (0..n).map(|_| generator.generate()).collect()
}

/// A unit cube centered on the origin, without normals.
pub fn cube() -> TriMesh {
    let p = |x: f64, y: f64, z: f64| Point3::new(x - 0.5, y - 0.5, z - 0.5);
    let positions = vec![
        p(0.0, 0.0, 0.0),
        p(1.0, 0.0, 0.0),
        p(1.0, 1.0, 0.0),
        p(0.0, 1.0, 0.0),
        p(0.0, 0.0, 1.0),
        p(1.0, 0.0, 1.0),
        p(1.0, 1.0, 1.0),
        p(0.0, 1.0, 1.0),
    ];
    let triangles = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [2, 3, 7],
        [2, 7, 6],
        [1, 2, 6],
        [1, 6, 5],
        [0, 4, 7],
        [0, 7, 3],
    ];
    TriMesh::new(positions, triangles, &Limits::DEFAULT).unwrap()
}

/// A square of side 2 in the xy plane centered on the origin, facing +z (counterclockwise seen
/// from +z).
pub fn square() -> TriMesh {
    let positions = vec![
        Point3::new(-1.0, -1.0, 0.0),
        Point3::new(1.0, -1.0, 0.0),
        Point3::new(1.0, 1.0, 0.0),
        Point3::new(-1.0, 1.0, 0.0),
    ];
    TriMesh::new(positions, vec![[0, 1, 2], [0, 2, 3]], &Limits::DEFAULT).unwrap()
}

/// An orthographic frame looking down −z at `center`, `height` model units tall.
pub fn top_view(center: Point3, height: f64) -> ViewFrame {
    ViewFrame::new(
        center + Vector3::new(0.0, 0.0, 100.0),
        center,
        Vector3::y(),
        Projection::Orthographic { height },
        1.0,
        1000.0,
    )
    .unwrap()
}

/// The default render settings: black background, line smoothing on.
pub fn settings() -> RenderSettings {
    RenderSettings::default()
}

/// Renders a `size × size` image.
pub fn render(renderer: &mut Renderer, scene: &mut Scene, frame: &ViewFrame, size: u32) -> Image {
    renderer
        .render_to_image(scene, frame, size, size, &settings())
        .unwrap()
}

/// Linear value of an 8-bit sRGB level.
pub fn linear(level: u8) -> f64 {
    f64::from(Color::from_srgb8([level, 0, 0, 255]).r)
}

/// The coverage of a white-on-black pixel: its linear red value.
pub fn coverage(image: &Image, x: u32, y: u32) -> f64 {
    image.pixel(x, y).map_or(0.0, |p| linear(p[0]))
}

/// The sum of coverage along column `x`.
pub fn column_width(image: &Image, x: u32) -> f64 {
    (0..image.height).map(|y| coverage(image, x, y)).sum()
}

/// The sum of coverage along row `y`.
pub fn row_width(image: &Image, y: u32) -> f64 {
    (0..image.width).map(|x| coverage(image, x, y)).sum()
}

/// The sum of coverage over the image.
pub fn area(image: &Image) -> f64 {
    (0..image.height)
        .flat_map(|y| (0..image.width).map(move |x| (x, y)))
        .map(|(x, y)| coverage(image, x, y))
        .sum()
}

fn blurred(image: &Image) -> Vec<f64> {
    let (w, h) = (i64::from(image.width), i64::from(image.height));
    let mut out = vec![0.0; image.rgba8.len()];
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut sum = 0.0;
                let mut n = 0.0;
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (sx, sy) = (x + dx, y + dy);
                        if sx >= 0 && sy >= 0 && sx < w && sy < h {
                            sum += f64::from(image.rgba8[((sy * w + sx) * 4 + c) as usize]);
                            n += 1.0;
                        }
                    }
                }
                out[((y * w + x) * 4 + c) as usize] = sum / n;
            }
        }
    }
    out
}

/// The image metric: mean and maximum absolute channel difference after a 3×3 box blur.
pub fn difference(a: &Image, b: &Image) -> (f64, f64) {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "image sizes differ"
    );
    let (a, b) = (blurred(a), blurred(b));
    let diffs: Vec<f64> = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    (mean, diffs.iter().copied().fold(0.0, f64::max))
}

fn save(image: &Image, path: &PathBuf) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbaImage::from_raw(image.width, image.height, image.rgba8.clone())
        .unwrap()
        .save(path)
        .unwrap();
}

/// Compares `image` with `tests/reference/<name>.png` on the pinned adapter (see the module
/// documentation). Returns whether a comparison ran.
pub fn compare_reference(
    gpu: &Gpu,
    name: &str,
    image: &Image,
    max_mean: f64,
    max_max: f64,
) -> bool {
    if !pinned(gpu) {
        return false;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/reference")
        .join(format!("{name}.png"));
    if std::env::var_os("ARC_UPDATE_REFERENCES").is_some_and(|v| v == "1") {
        save(image, &path);
        return true;
    }
    let stored = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "reference {} cannot be read ({e}); run with ARC_UPDATE_REFERENCES=1",
                path.display()
            )
        })
        .to_rgba8();
    let reference = Image {
        width: stored.width(),
        height: stored.height(),
        rgba8: stored.into_raw(),
    };
    let (mean, max) = difference(image, &reference);
    if mean > max_mean || max > max_max {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("arc-gpu-image-diffs");
        save(image, &dir.join(format!("{name}.actual.png")));
        let diff = Image {
            width: image.width,
            height: image.height,
            rgba8: image
                .rgba8
                .iter()
                .zip(&reference.rgba8)
                .enumerate()
                .map(|(i, (a, b))| if i % 4 == 3 { 255 } else { a.abs_diff(*b) })
                .collect(),
        };
        save(&diff, &dir.join(format!("{name}.difference.png")));
        panic!(
            "{name}: mean difference {mean:.3} (limit {max_mean}), maximum {max:.1} (limit {max_max}); images in {}",
            dir.display()
        );
    }
    true
}
