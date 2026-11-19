//! CORE-GPU-021: while the camera moves the expensive effects are lowered, and a frame drawn at
//! rest after a navigation episode is pixel-identical to the one drawn before it.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use std::time::Duration;

use arc_base::math::Point3;
use arc_gpu::display::FLAG_SOLID;
use arc_gpu::quality::AdaptiveQuality;
use arc_gpu::{
    MeshInstance, MeshItem, RenderItem, RenderSettings, Renderer, Scene, SceneLimits, ShadingMode,
};
use support::{cube, gpu, ids, top_view};

const SIZE: u32 = 64;

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

#[test]
fn a_frame_at_rest_is_identical_before_and_after_a_navigation_episode() {
    let gpu = gpu();
    let mut scene = Scene::new(
        &gpu,
        SceneLimits {
            silhouette_edges: true,
            ..SceneLimits::DEFAULT
        },
    )
    .unwrap();
    let geometry = scene.add_geometry(&cube(), &Point3::origin()).unwrap();
    let object = ids(1)[0];
    scene
        .update(vec![(
            object,
            vec![RenderItem::Mesh(MeshItem {
                geometry,
                origin: Point3::origin(),
                instances: vec![MeshInstance {
                    flags: FLAG_SOLID,
                    ..MeshInstance::new(arc_base::Color::new(0.8, 0.3, 0.1, 1.0))
                }],
            })],
        )])
        .unwrap();
    let settings = RenderSettings {
        sample_count: 4,
        mode: ShadingMode::ShadedWithEdges,
        silhouette_edges: true,
        lod_budget_px: 2.0,
        ..RenderSettings::default()
    };
    let mut renderer = Renderer::new(&gpu);
    // The camera is `x` model units to the right of the origin.
    let mut at = |x: f64, q: &AdaptiveQuality, now: Duration| {
        let view = top_view(Point3::new(x, 0.0, 0.0), 3.0);
        renderer
            .render_to_image(&mut scene, &view, SIZE, SIZE, &q.apply(&settings, now))
            .unwrap()
    };

    // A frame at rest before any navigation.
    let before = AdaptiveQuality::default();
    let rest_before = at(0.0, &before, ms(0));

    // A navigation episode: the camera moves away from the start every 100 ms, drawn at the
    // navigation quality.
    let mut quality = AdaptiveQuality::default();
    let mut lowered_frames = 0;
    for step in 0..6 {
        let now = ms(1000 + 100 * step);
        quality.camera_moved(now);
        if quality.apply(&settings, now) != settings {
            lowered_frames += 1;
        }
        let _ = at(0.1 * (step as f64 + 1.0), &quality, now);
    }
    assert_eq!(lowered_frames, 6, "every frame of the episode is lowered");

    // The camera is back at the start and still for the settle time: full quality, and the same
    // frame as before the episode.
    let settled = quality.settled_at().unwrap();
    assert_eq!(quality.apply(&settings, settled), settings);
    let rest_after = at(0.0, &quality, settled);
    assert!(
        rest_before.rgba8 == rest_after.rgba8,
        "a frame at rest changed after navigation"
    );
}
