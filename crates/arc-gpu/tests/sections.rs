//! CORE-GPU-024: section planes and box clip meshes in every backend, cap cut solids with the
//! section color, are respected by picking and leave overlays alone.

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
use arc_gpu::display::FLAG_SOLID;
use arc_gpu::{
    Anchor, FrameExtras, MarkerShape, MeshInstance, MeshItem, Overlay, OverlayPrimitive,
    PickRegion, RenderItem, RenderSettings, Renderer, Scene, Section, SectionBox, SectionPlane,
    WriteCounters,
};
use support::{cube, ids, setup, top_view};

const SIZE: u32 = 96;
const RED: Color = Color::new(1.0, 0.0, 0.0, 1.0);
const GREEN: Color = Color::new(0.0, 1.0, 0.0, 1.0);

/// A unit cube at `origin`, flagged as a closed solid, seen from above at 32 pixels per unit.
fn solid_at(scene: &mut Scene, origin: Point3, flags: u32) -> arc_base::ObjectId {
    let geometry = scene.add_geometry(&cube(), &Point3::origin()).unwrap();
    let id = ids(1)[0];
    scene
        .update(vec![(
            id,
            vec![RenderItem::Mesh(MeshItem {
                geometry,
                origin,
                instances: vec![MeshInstance {
                    flags,
                    ..MeshInstance::new(RED)
                }],
            })],
        )])
        .unwrap();
    id
}

fn render(
    renderer: &mut Renderer,
    scene: &mut Scene,
    center: Point3,
    section: Option<&Section>,
    overlay: Option<&Overlay>,
) -> arc_gpu::Image {
    renderer
        .render_with(
            scene,
            &top_view(center, 3.0),
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &FrameExtras {
                section,
                overlay,
                ..FrameExtras::default()
            },
        )
        .unwrap()
}

fn is(image: &arc_gpu::Image, x: u32, y: u32, want: [u8; 3]) -> bool {
    let p = image.pixel(x, y).unwrap();
    p[..3].iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 2)
}

fn plane(point: [f64; 3], normal: [f64; 3]) -> SectionPlane {
    SectionPlane {
        point: Point3::from(point),
        normal: Vector3::from(normal),
    }
}

#[test]
fn a_plane_through_a_solid_shows_a_cap_in_the_section_color() {
    let (_, mut scene, mut renderer) = setup();
    solid_at(&mut scene, Point3::origin(), FLAG_SOLID);
    let origin = Point3::origin();
    // Removes everything above z = 0, the top face included.
    let cut = |cap: Option<Color>| Section {
        planes: vec![plane([0.0, 0.0, 0.0], [0.0, 0.0, 1.0])],
        cap_color: cap,
        ..Section::default()
    };
    let uncut = render(&mut renderer, &mut scene, origin, None, None);
    assert!(is(&uncut, 48, 48, [255, 0, 0]));
    let capped = render(
        &mut renderer,
        &mut scene,
        origin,
        Some(&cut(Some(GREEN))),
        None,
    );
    assert!(
        is(&capped, 48, 48, [0, 255, 0]),
        "{:?}",
        capped.pixel(48, 48)
    );
    assert!(
        is(&capped, 36, 60, [0, 255, 0]),
        "the cap fills the whole cut"
    );
    assert!(
        is(&capped, 10, 10, [0, 0, 0]),
        "outside the solid is the background"
    );
    // Without a cap color the inside shows the lit back faces of the solid.
    let open = render(&mut renderer, &mut scene, origin, Some(&cut(None)), None);
    assert!(!is(&open, 48, 48, [0, 255, 0]));
    assert!(open.pixel(48, 48).unwrap()[0] > 200);
}

#[test]
fn only_solids_are_capped() {
    let (_, mut scene, mut renderer) = setup();
    solid_at(&mut scene, Point3::origin(), 0);
    let section = Section {
        planes: vec![plane([0.0; 3], [0.0, 0.0, 1.0])],
        cap_color: Some(GREEN),
        ..Section::default()
    };
    let image = render(
        &mut renderer,
        &mut scene,
        Point3::origin(),
        Some(&section),
        None,
    );
    assert!(
        is(&image, 48, 48, [255, 0, 0]),
        "a surface is clipped but not capped"
    );
}

#[test]
fn several_planes_and_the_box_clip_where_they_say() {
    let (_, mut scene, mut renderer) = setup();
    solid_at(&mut scene, Point3::origin(), 0);
    let origin = Point3::origin();
    // Remove x > 0.25 and y > 0.25: the lower left quarter stays. Image y points down.
    let planes = Section {
        planes: vec![
            plane([0.25, 0.0, 0.0], [1.0, 0.0, 0.0]),
            plane([0.0, 0.25, 0.0], [0.0, 2.0, 0.0]),
        ],
        ..Section::default()
    };
    let image = render(&mut renderer, &mut scene, origin, Some(&planes), None);
    assert!(is(&image, 36, 60, [255, 0, 0]), "kept: x < 0, y < 0");
    assert!(is(&image, 60, 60, [0, 0, 0]), "x = 0.375 is removed");
    assert!(is(&image, 36, 36, [0, 0, 0]), "y = 0.375 is removed");
    // The box keeps |x|, |y| <= 0.25: pixels 40 to 56.
    let boxed = Section {
        volume: Some(SectionBox {
            min: Point3::new(-0.25, -0.25, -1.0),
            max: Point3::new(0.25, 0.25, 1.0),
        }),
        ..Section::default()
    };
    let image = render(&mut renderer, &mut scene, origin, Some(&boxed), None);
    assert!(is(&image, 48, 48, [255, 0, 0]));
    assert!(is(&image, 36, 48, [0, 0, 0]));
    assert!(is(&image, 60, 48, [0, 0, 0]));
    assert!(is(&image, 48, 36, [0, 0, 0]));
}

#[test]
fn six_planes_are_allowed_and_a_seventh_is_an_error() {
    let (_, mut scene, mut renderer) = setup();
    solid_at(&mut scene, Point3::origin(), 0);
    let many = |n: usize| Section {
        planes: (0..n)
            .map(|i| plane([0.0, 0.0, 10.0 + i as f64], [0.0, 0.0, 1.0]))
            .collect(),
        ..Section::default()
    };
    let image = render(
        &mut renderer,
        &mut scene,
        Point3::origin(),
        Some(&many(6)),
        None,
    );
    assert!(is(&image, 48, 48, [255, 0, 0]));
    let error = renderer
        .render_with(
            &mut scene,
            &top_view(Point3::origin(), 3.0),
            SIZE,
            SIZE,
            &RenderSettings::default(),
            &FrameExtras {
                section: Some(&many(7)),
                ..FrameExtras::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), arc_gpu::error::codes::SETTINGS_INVALID.code);
}

#[test]
fn sections_hold_far_from_the_origin_and_leave_overlays_and_writes_alone() {
    let (_, mut scene, mut renderer) = setup();
    let far = Point3::new(10_000_000.0, -2_000_000.0, 5.0);
    solid_at(&mut scene, far, FLAG_SOLID);
    let boxed = Section {
        volume: Some(SectionBox {
            min: far + Vector3::new(-0.25, -0.25, -1.0),
            max: far + Vector3::new(0.25, 0.25, 1.0),
        }),
        ..Section::default()
    };
    let mut overlay = Overlay::new();
    overlay.primitives.push(OverlayPrimitive::Marker {
        at: Anchor::Screen([36.5, 48.5]),
        shape: MarkerShape::Square,
        size_px: 6.0,
        filled: true,
        width_px: 1.0,
        color: Color::WHITE,
    });
    let image = render(&mut renderer, &mut scene, far, Some(&boxed), Some(&overlay));
    assert!(is(&image, 48, 48, [255, 0, 0]), "inside the box");
    assert!(is(&image, 44, 48, [255, 0, 0]));
    assert!(
        is(&image, 60, 48, [0, 0, 0]),
        "outside the box, 10^7 from the origin"
    );
    assert!(
        is(&image, 36, 48, [255, 255, 255]),
        "the overlay is not clipped"
    );
    // A frame with the same section writes nothing to the scene.
    render(&mut renderer, &mut scene, far, Some(&boxed), None);
    assert_eq!(scene.last_frame_writes(), WriteCounters::default());
}

#[test]
fn picking_respects_the_section() {
    let (_, mut scene, mut renderer) = setup();
    let id = solid_at(&mut scene, Point3::origin(), 0);
    let boxed = Section {
        volume: Some(SectionBox {
            min: Point3::new(-0.25, -0.25, -1.0),
            max: Point3::new(0.25, 0.25, 1.0),
        }),
        ..Section::default()
    };
    let frame = top_view(Point3::origin(), 3.0);
    let extras = FrameExtras {
        section: Some(&boxed),
        ..FrameExtras::default()
    };
    let pick = |x: u32, renderer: &mut Renderer, scene: &mut Scene| {
        renderer
            .pick_with(
                scene,
                &frame,
                (SIZE, SIZE),
                &RenderSettings::default(),
                &PickRegion::pixel(x, 48, 0.0),
                &extras,
            )
            .unwrap()
    };
    assert_eq!(pick(48, &mut renderer, &mut scene)[0].key.object, id);
    assert!(
        pick(36, &mut renderer, &mut scene).is_empty(),
        "the clipped part is not picked"
    );
}
