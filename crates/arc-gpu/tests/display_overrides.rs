//! CORE-GPU-016: per-view display overrides by object and category: color, transparency,
//! ghosting, isolation and hiding, evaluated per frame without writing the scene.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use arc_base::math::Point3;
use arc_base::{Color, ObjectId};
use arc_gpu::{
    DisplayOverrides, FrameExtras, Isolation, MeshInstance, MeshItem, Others, OverrideStyle,
    PickRegion, RenderItem, RenderSettings, Renderer, Scene, WriteCounters, category_flags,
};
use support::{ids, setup, square, top_view};

const SIZE: u32 = 96;
const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);
const BLUE: Color = Color::new(0.0, 0.0, 1.0, 1.0);

/// Three squares of side 2 at x = −4, 0 and 4, seen from above at 8 pixels per unit, so their
/// centers are at pixels 16, 48 and 80 of the middle row.
const CENTERS: [u32; 3] = [16, 48, 80];

fn scene_of_three(scene: &mut Scene, flags: u32) -> Vec<ObjectId> {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let objects = ids(3);
    let changes = objects
        .iter()
        .zip([RED, GREEN, BLUE])
        .zip([-4.0, 0.0, 4.0])
        .map(|((id, color), x)| {
            (
                *id,
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin: Point3::new(x, 0.0, 0.0),
                    instances: vec![MeshInstance {
                        flags,
                        ..MeshInstance::new(color)
                    }],
                })],
            )
        })
        .collect();
    scene.update(changes).unwrap();
    objects
}

fn render(
    renderer: &mut Renderer,
    scene: &mut Scene,
    overrides: Option<&DisplayOverrides>,
) -> arc_gpu::Image {
    renderer
        .render_with(
            scene,
            &top_view(Point3::origin(), 12.0),
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &FrameExtras {
                overrides,
                ..FrameExtras::default()
            },
        )
        .unwrap()
}

fn center(image: &arc_gpu::Image, i: usize) -> [u8; 4] {
    image.pixel(CENTERS[i], 48).unwrap()
}

fn near(got: [u8; 4], want: [u8; 3], tolerance: u8) -> bool {
    got[..3]
        .iter()
        .zip(want)
        .all(|(g, w)| g.abs_diff(w) <= tolerance)
}

/// The sRGB level of a linear value.
fn srgb(linear: f32) -> u8 {
    Color::new(linear, 0.0, 0.0, 1.0).to_srgb8()[0]
}

#[test]
fn objects_and_categories_can_be_recolored_and_hidden() {
    let (_, mut scene, mut renderer) = setup();
    let objects = scene_of_three(&mut scene, category_flags(0, 3));
    let plain = render(&mut renderer, &mut scene, None);
    assert!(near(center(&plain, 0), [255, 0, 0], 1));
    assert!(near(center(&plain, 1), [0, 255, 0], 1));

    let yellow = Color::new(1.0, 1.0, 0.0, 1.0);
    let by_object = DisplayOverrides {
        objects: vec![(objects[0], OverrideStyle::colored(yellow))],
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&by_object));
    assert!(
        near(center(&image, 0), [255, 255, 0], 1),
        "{:?}",
        center(&image, 0)
    );
    assert!(
        near(center(&image, 1), [0, 255, 0], 1),
        "others are unchanged"
    );

    // Every instance carries category 3: one style covers all three objects, and an object's
    // own style wins over it.
    let by_category = DisplayOverrides {
        categories: vec![(3, OverrideStyle::colored(yellow))],
        objects: vec![(objects[2], OverrideStyle::colored(Color::WHITE))],
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&by_category));
    assert!(near(center(&image, 0), [255, 255, 0], 1));
    assert!(near(center(&image, 1), [255, 255, 0], 1));
    assert!(
        near(center(&image, 2), [255, 255, 255], 1),
        "the object beats the category"
    );

    let hidden = DisplayOverrides {
        objects: vec![(objects[1], OverrideStyle::hidden())],
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&hidden));
    assert!(
        near(center(&image, 1), [0, 0, 0], 0),
        "hidden shows the background"
    );
    assert!(near(center(&image, 0), [255, 0, 0], 1));
    let hidden_category = DisplayOverrides {
        categories: vec![(3, OverrideStyle::hidden())],
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&hidden_category));
    assert!((0..3).all(|i| near(center(&image, i), [0, 0, 0], 0)));
}

#[test]
fn transparency_blends_over_what_is_behind_and_ghosts_are_faint_and_not_picked() {
    let (_, mut scene, mut renderer) = setup();
    // A blue square behind (z = −1) and a red one in front (z = 1) of the same pixels.
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let [back, front] = [ids(2)[0], ids(2)[1]];
    let item = |z: f64, color| {
        vec![RenderItem::Mesh(MeshItem {
            geometry,
            origin: Point3::new(0.0, 0.0, z),
            instances: vec![MeshInstance::new(color)],
        })]
    };
    scene
        .update(vec![(back, item(-1.0, BLUE)), (front, item(1.0, RED))])
        .unwrap();
    let plain = render(&mut renderer, &mut scene, None);
    assert!(near(center(&plain, 1), [255, 0, 0], 1));

    let half = DisplayOverrides {
        objects: vec![(front, OverrideStyle::transparent(0.5))],
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&half));
    let level = srgb(0.5);
    assert!(
        near(center(&image, 1), [level, 0, level], 3),
        "{:?} vs {level}",
        center(&image, 1)
    );

    // A ghost is drawn at the ghost opacity over the background.
    let ghost = DisplayOverrides {
        objects: vec![(front, OverrideStyle::ghost())],
        ghost_opacity: 0.2,
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&ghost));
    let blend = |x: f32| srgb(x);
    // Over the blue square: 0.2 red plus 0.8 blue in linear light.
    assert!(near(center(&image, 1), [blend(0.2), 0, blend(0.8)], 3));

    // The ghost is not picked: the object behind it is.
    let frame = top_view(Point3::origin(), 12.0);
    let settings = RenderSettings::default();
    let region = PickRegion::pixel(48, 48, 0.0);
    let extras = FrameExtras {
        overrides: Some(&ghost),
        ..FrameExtras::default()
    };
    let hits = renderer
        .pick_with(
            &mut scene,
            &frame,
            (SIZE, SIZE),
            &settings,
            &region,
            &extras,
        )
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.key.object).collect::<Vec<_>>(),
        vec![back]
    );
    let plain_hits = renderer
        .pick(&mut scene, &frame, SIZE, SIZE, &settings, &region)
        .unwrap();
    assert_eq!(plain_hits[0].key.object, front);
}

#[test]
fn isolation_hides_or_ghosts_the_other_objects() {
    let (_, mut scene, mut renderer) = setup();
    let objects = scene_of_three(&mut scene, 0);
    let hide = DisplayOverrides {
        isolation: Some(Isolation {
            objects: vec![objects[1]],
            others: Others::Hide,
        }),
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&hide));
    assert!(near(center(&image, 1), [0, 255, 0], 1));
    assert!(near(center(&image, 0), [0, 0, 0], 0));
    assert!(near(center(&image, 2), [0, 0, 0], 0));

    let ghost = DisplayOverrides {
        isolation: Some(Isolation {
            objects: vec![objects[1]],
            others: Others::Ghost,
        }),
        ghost_opacity: 0.25,
        ..DisplayOverrides::default()
    };
    let image = render(&mut renderer, &mut scene, Some(&ghost));
    assert!(near(center(&image, 1), [0, 255, 0], 1));
    assert!(
        near(center(&image, 0), [srgb(0.25), 0, 0], 3),
        "{:?}",
        center(&image, 0)
    );

    // Hidden objects are neither picked nor highlighted.
    let frame = top_view(Point3::origin(), 12.0);
    let extras = FrameExtras {
        overrides: Some(&hide),
        ..FrameExtras::default()
    };
    let region = PickRegion::pixel(CENTERS[0], 48, 0.0);
    let hits = renderer
        .pick_with(
            &mut scene,
            &frame,
            (SIZE, SIZE),
            &RenderSettings::default(),
            &region,
            &extras,
        )
        .unwrap();
    assert!(hits.is_empty());
    let selected = [objects[0]];
    let highlighted = renderer
        .render_with(
            &mut scene,
            &frame,
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &FrameExtras {
                selected: &selected,
                ..extras
            },
        )
        .unwrap();
    assert!(near(center(&highlighted, 0), [0, 0, 0], 0));
}

#[test]
fn overrides_write_no_scene_buffer() {
    let (_, mut scene, mut renderer) = setup();
    let objects = scene_of_three(&mut scene, 0);
    render(&mut renderer, &mut scene, None);
    let overrides = DisplayOverrides {
        objects: vec![(objects[0], OverrideStyle::hidden())],
        ..DisplayOverrides::default()
    };
    render(&mut renderer, &mut scene, Some(&overrides));
    assert_eq!(scene.last_frame_writes(), WriteCounters::default());
}

#[test]
fn invalid_overrides_are_errors() {
    let (_, mut scene, mut renderer) = setup();
    scene_of_three(&mut scene, 0);
    let bad = DisplayOverrides {
        categories: vec![(16, OverrideStyle::hidden())],
        ..DisplayOverrides::default()
    };
    let result = renderer.render_with(
        &mut scene,
        &top_view(Point3::origin(), 12.0),
        SIZE,
        SIZE,
        &RenderSettings::default(),
        &FrameExtras {
            overrides: Some(&bad),
            ..FrameExtras::default()
        },
    );
    assert_eq!(
        result.unwrap_err().code(),
        arc_gpu::error::codes::SETTINGS_INVALID.code
    );
}
