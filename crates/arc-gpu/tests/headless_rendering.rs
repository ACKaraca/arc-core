//! CORE-GPU-031 headless rendering and CORE-GPU-001 adapter selection on the test machine.

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
    BackendKind, Gpu, GpuSettings, LineItem, LinePath, LineStyle, LineWidth, MeshInstance,
    MeshItem, ReducedFeature, RenderItem, RenderSettings, SceneLimits,
};
use support::{cube, ids, setup};

fn perspective() -> ViewFrame {
    ViewFrame::new(
        Point3::new(2.2, -2.6, 1.8),
        Point3::origin(),
        Vector3::z(),
        Projection::Perspective { vertical_fov: 0.8 },
        0.1,
        100.0,
    )
    .unwrap()
}

#[test]
fn the_chosen_adapter_is_reported() {
    let gpu = support::gpu();
    let report = gpu.report();
    assert!(!report.name.is_empty());
    assert_eq!(
        report.software,
        report.reduced.contains(&ReducedFeature::SoftwareRasterizer)
    );
    assert!(matches!(report.max_samples, 1 | 4));
    assert_eq!(
        report.max_samples == 1,
        report.reduced.contains(&ReducedFeature::Multisampling)
    );
    // A software adapter is always a fallback.
    assert!(!report.software || report.fallback);
}

#[test]
fn forced_settings_never_fall_back_to_another_adapter() {
    let none = GpuSettings {
        adapter_name: Some("no adapter has this name".to_owned()),
        ..GpuSettings::default()
    };
    assert_eq!(Gpu::new(&none).unwrap_err().code(), "gpu.adapter.not_found");
    let chosen = support::gpu().report().clone();
    let forced = GpuSettings {
        backend: Some(chosen.backend),
        adapter_name: Some(chosen.name.to_uppercase()),
        ..GpuSettings::default()
    };
    assert_eq!(Gpu::new(&forced).unwrap().report().name, chosen.name);
    if chosen.fallback {
        let strict = GpuSettings {
            backend: Some(chosen.backend),
            adapter_name: Some(chosen.name.clone()),
            forbid_fallback: true,
        };
        assert_eq!(
            Gpu::new(&strict).unwrap_err().code(),
            "gpu.adapter.not_found"
        );
    }
    // A backend that does not exist on this platform finds nothing.
    let foreign = if cfg!(target_os = "macos") {
        BackendKind::Dx12
    } else {
        BackendKind::Metal
    };
    let settings = GpuSettings {
        backend: Some(foreign),
        ..GpuSettings::default()
    };
    assert_eq!(
        Gpu::new(&settings).unwrap_err().code(),
        "gpu.adapter.not_found"
    );
}

#[test]
fn an_empty_scene_renders_the_background_at_any_row_padding() {
    let (_, mut scene, mut renderer) = setup();
    let settings = RenderSettings {
        background: Color::from_srgb8([10, 200, 30, 255]),
        ..RenderSettings::default()
    };
    // 13 × 7 pixels: 52-byte rows, padded to 256 bytes on readback.
    let image = renderer
        .render_to_image(&mut scene, &perspective(), 13, 7, &settings)
        .unwrap();
    assert_eq!(
        (image.width, image.height, image.rgba8.len()),
        (13, 7, 13 * 7 * 4)
    );
    assert!(image.rgba8.chunks(4).all(|p| p == [10, 200, 30, 255]));
}

#[test]
fn sizes_and_settings_are_bounded() {
    let (_, mut scene, mut renderer) = setup();
    let s = RenderSettings::default();
    let code = |r: Result<_, arc_base::Error>| r.unwrap_err().code();
    let frame = perspective();
    assert_eq!(
        code(renderer.render_to_image(&mut scene, &frame, 0, 8, &s)),
        "gpu.limit.exceeded"
    );
    let side = SceneLimits::DEFAULT.max_image_side + 1;
    assert_eq!(
        code(renderer.render_to_image(&mut scene, &frame, side, 8, &s)),
        "gpu.limit.exceeded"
    );
    for bad in [
        RenderSettings {
            sample_count: 2,
            ..s
        },
        RenderSettings {
            ambient: f32::NAN,
            ..s
        },
        RenderSettings {
            plot_scale: 0.0,
            ..s
        },
    ] {
        assert_eq!(
            code(renderer.render_to_image(&mut scene, &frame, 8, 8, &bad)),
            "gpu.settings.invalid"
        );
    }
}

fn reference_scene(scene: &mut arc_gpu::Scene) {
    let geometry = scene.add_geometry(&cube(), &Point3::origin()).unwrap();
    let id = ids(2);
    let mut instance = MeshInstance::new(Color::from_srgb8([200, 120, 40, 255]));
    instance.transform[2][3] = 0.0;
    scene
        .update(vec![
            (
                id[0],
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin: Point3::origin(),
                    instances: vec![instance],
                })],
            ),
            (
                id[1],
                vec![RenderItem::Lines(LineItem {
                    origin: Point3::new(0.0, 0.0, -0.5),
                    paths: vec![LinePath {
                        points: vec![
                            [-1.2, -1.2, 0.0],
                            [1.2, -1.2, 0.0],
                            [1.2, 1.2, 0.0],
                            [-1.2, 1.2, 0.0],
                        ],
                        closed: true,
                    }],
                    style: LineStyle::round(LineWidth::Screen(3.0)),
                    color: Color::WHITE,
                })],
            ),
        ])
        .unwrap();
}

#[test]
fn a_lit_scene_matches_its_reference_and_is_deterministic() {
    let (gpu, mut scene, mut renderer) = setup();
    reference_scene(&mut scene);
    let s = RenderSettings::default();
    let a = renderer
        .render_to_image(&mut scene, &perspective(), 128, 128, &s)
        .unwrap();
    let b = renderer
        .render_to_image(&mut scene, &perspective(), 128, 128, &s)
        .unwrap();
    assert_eq!(a, b, "rendering is deterministic on one adapter");
    // Analytic checks: the corners show the background, the center shows the lit cube, whose
    // three visible faces have distinct brightness under the headlight.
    assert_eq!(a.pixel(0, 0), Some([0, 0, 0, 255]));
    let center = a.pixel(64, 64).unwrap();
    assert!(
        center[0] > center[2],
        "cube color at the center: {center:?}"
    );
    assert!(
        a.rgba8.chunks(4).any(|p| p[..3] == [255, 255, 255]),
        "the square of lines is drawn"
    );
    support::compare_reference(&gpu, "lit_cube_and_lines", &a, 1.0, 64.0);
}

#[test]
fn multisampling_resolves_to_the_same_picture() {
    let (gpu, mut scene, mut renderer) = setup();
    reference_scene(&mut scene);
    let one = RenderSettings::default();
    let four = RenderSettings {
        sample_count: 4,
        ..one
    };
    let a = renderer
        .render_to_image(&mut scene, &perspective(), 96, 96, &one)
        .unwrap();
    let b = renderer
        .render_to_image(&mut scene, &perspective(), 96, 96, &four)
        .unwrap();
    let (mean, _) = support::difference(&a, &b);
    assert!(mean < 3.0, "mean difference {mean}");
    if gpu.report().max_samples == 4 {
        assert_ne!(a, b, "four samples smooth the mesh edges");
    }
}
