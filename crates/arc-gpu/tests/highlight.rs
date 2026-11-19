//! CORE-GPU-023: pre-selection and selection highlights draw an outline and a tint in every
//! shading mode, and changing them rewrites no geometry.

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
    FrameExtras, Image, LineItem, LinePath, LineStyle, LineWidth, MeshInstance, MeshItem,
    RenderItem, RenderSettings, Renderer, Scene, ShadingMode, WriteCounters,
};
use support::{ids, linear, setup, square, top_view};

const SIZE: u32 = 64;
const SELECTION: [u8; 4] = [0, 0, 255, 255];
const HOVER: [u8; 4] = [255, 128, 0, 255];

fn settings(mode: ShadingMode) -> RenderSettings {
    RenderSettings {
        mode,
        selection_color: Color::from_srgb8(SELECTION),
        hover_color: Color::from_srgb8(HOVER),
        tint: 0.35,
        outline_px: 2,
        ..RenderSettings::DEFAULT
    }
}

/// One model unit per pixel. A: a red square of side 20 on pixels 22..=41 of both axes, its
/// sides on pixel boundaries. L: a line, far from A.
fn populate(scene: &mut Scene) -> (ObjectId, ObjectId) {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(2);
    let red = MeshInstance {
        transform: [
            [10.0, 0.0, 0.0, 0.0],
            [0.0, 10.0, 0.0, 0.0],
            [0.0, 0.0, 10.0, 0.0],
        ],
        ..MeshInstance::new(Color::new(1.0, 0.0, 0.0, 1.0))
    };
    let line = LineItem {
        origin: Point3::origin(),
        paths: vec![LinePath {
            points: vec![[-20.0, 26.0, 0.0], [20.0, 26.0, 0.0]],
            closed: false,
        }],
        style: LineStyle::round(LineWidth::Screen(2.0)),
        color: Color::WHITE,
    };
    scene
        .update(vec![
            (
                id[0],
                vec![RenderItem::Mesh(MeshItem {
                    geometry,
                    origin: Point3::origin(),
                    instances: vec![red],
                })],
            ),
            (id[1], vec![RenderItem::Lines(line)]),
        ])
        .unwrap();
    (id[0], id[1])
}

fn count(image: &Image, color: [u8; 4]) -> usize {
    image.rgba8.chunks(4).filter(|p| *p == color).count()
}

fn draw(
    renderer: &mut Renderer,
    scene: &mut Scene,
    settings: &RenderSettings,
    selected: &[ObjectId],
    hovered: Option<ObjectId>,
) -> Image {
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    renderer
        .render_with(
            scene,
            &frame,
            SIZE,
            SIZE,
            settings,
            &FrameExtras {
                hovered,
                selected,
                ..FrameExtras::default()
            },
        )
        .unwrap()
}

/// The outline of a 20-pixel square with a 2-pixel radius: two pixels along each side, and at
/// each corner the one offset (1, 1) within the radius.
const SQUARE_OUTLINE: usize = 4 * 2 * 20 + 4;

#[test]
fn outline_and_tint_are_drawn_in_every_mode() {
    let (_, mut scene, mut renderer) = setup();
    let (a, _) = populate(&mut scene);
    for mode in ShadingMode::ALL {
        let s = settings(mode);
        draw(&mut renderer, &mut scene, &s, &[], None);
        let selected = draw(&mut renderer, &mut scene, &s, &[a], None);
        assert_eq!(
            scene.last_frame_writes(),
            WriteCounters::default(),
            "{mode:?}: selecting writes no scene buffer"
        );
        assert_eq!(count(&selected, SELECTION), SQUARE_OUTLINE, "{mode:?}");
        assert_eq!(selected.pixel(20, 30), Some(SELECTION), "{mode:?}: left");
        assert_eq!(selected.pixel(42, 30), Some(SELECTION), "{mode:?}: right");
        assert_eq!(
            selected.pixel(19, 30),
            Some([0, 0, 0, 255]),
            "{mode:?}: width"
        );
        let hovered = draw(&mut renderer, &mut scene, &s, &[a], Some(a));
        assert_eq!(scene.last_frame_writes(), WriteCounters::default());
        assert_eq!(
            count(&hovered, HOVER),
            SQUARE_OUTLINE,
            "{mode:?}: hover wins"
        );
        assert_eq!(count(&hovered, SELECTION), 0, "{mode:?}");

        // The tint: the face color blended with the selection color at the tint strength.
        let blue = f64::from(Color::from_srgb8(SELECTION).b) * 0.35;
        let tinted = |p: [u8; 4], face: f64| {
            (linear(p[0]) - face * 0.65).abs() < 0.01 && (linear(p[2]) - blue).abs() < 0.01
        };
        match mode {
            ShadingMode::Wireframe => {
                // Without faces the edges carry the tint; the outline is off so the edge
                // columns on either side of the boundary stay visible.
                let plain = RenderSettings { outline_px: 0, ..s };
                let image = draw(&mut renderer, &mut scene, &plain, &[a], None);
                assert!(
                    [21, 22]
                        .iter()
                        .any(|x| tinted(image.pixel(*x, 30).unwrap(), 1.0)),
                    "tinted edge"
                );
                assert_eq!(image.pixel(31, 30), Some([0, 0, 0, 255]));
            }
            ShadingMode::HiddenLine => {
                assert!(tinted(selected.pixel(31, 30).unwrap(), 0.0), "{mode:?}");
            }
            ShadingMode::XRay => {
                let face = f64::from(s.xray_opacity);
                assert!(tinted(selected.pixel(31, 30).unwrap(), face), "{mode:?}");
            }
            _ => assert!(tinted(selected.pixel(31, 30).unwrap(), 1.0), "{mode:?}"),
        }
    }
}

#[test]
fn lines_are_outlined_and_unknown_objects_are_ignored() {
    let (_, mut scene, mut renderer) = setup();
    let (_, line) = populate(&mut scene);
    let s = settings(ShadingMode::Shaded);
    let image = draw(&mut renderer, &mut scene, &s, &[line, ids(3)[2]], None);
    // The line covers rows 5 and 6 (y = 26, half width 1); the outline adds two rows above and
    // below.
    for row in [3, 4, 7, 8] {
        assert_eq!(image.pixel(31, row), Some(SELECTION), "row {row}");
    }
    assert_eq!(image.pixel(31, 9), Some([0, 0, 0, 255]));
    let none = draw(&mut renderer, &mut scene, &s, &[ids(3)[2]], None);
    assert_eq!(count(&none, SELECTION), 0);
}
