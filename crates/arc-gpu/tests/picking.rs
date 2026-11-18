//! CORE-GPU-022: the identifier pass returns the object and sub-object under a pixel and the
//! set within a rectangle; lines pick with a pixel tolerance.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tests fail by panicking; pixel arithmetic on small images"
)]

mod support;

use std::io::Write;
use std::time::Instant;

use arc_base::math::Point3;
use arc_base::{Color, ObjectId};
use arc_geom::view::ViewFrame;
use arc_gpu::{
    ItemKey, LineItem, LinePath, LineStyle, LineWidth, MeshInstance, MeshItem, PickHit, PickRegion,
    RenderItem, RenderSettings, Renderer, Scene, ShadingMode,
};
use support::{ids, setup, square, top_view};

const SIZE: u32 = 96;

fn at(x: f32, y: f32, z: f32, half: f32) -> MeshInstance {
    MeshInstance {
        transform: [
            [half, 0.0, 0.0, x],
            [0.0, half, 0.0, y],
            [0.0, 0.0, half, z],
        ],
        ..MeshInstance::new(Color::WHITE)
    }
}

/// One model unit per pixel; pixel `(px, py)` has its center at
/// `(px + 0.5 − 48, 48 − py − 0.5)`.
///
/// Object P: item 0, a mesh with three instances of side 10 centered at x = −30, −10, 10;
/// item 1, lines: a segment at y = 30 and a two-segment polyline at y = −30.
/// Object Q: a square of side 20 at x = 20, z = 5, in front of P's third instance.
fn populate(scene: &mut Scene) -> (ObjectId, ObjectId) {
    let geometry = scene.add_geometry(&square(), &Point3::origin()).unwrap();
    let id = ids(2);
    let lines = RenderItem::Lines(LineItem {
        origin: Point3::origin(),
        paths: vec![
            LinePath {
                points: vec![[-40.0, 30.0, 0.0], [40.0, 30.0, 0.0]],
                closed: false,
            },
            LinePath {
                points: vec![[-40.0, -30.0, 0.0], [0.0, -30.0, 0.0], [40.0, -30.0, 0.0]],
                closed: false,
            },
        ],
        style: LineStyle::round(LineWidth::Screen(1.0)),
        color: Color::WHITE,
    });
    let mesh = |instances| {
        RenderItem::Mesh(MeshItem {
            geometry,
            origin: Point3::origin(),
            instances,
        })
    };
    scene
        .update(vec![
            (
                id[0],
                vec![
                    mesh(vec![
                        at(-30.0, 0.0, 0.0, 5.0),
                        at(-10.0, 0.0, 0.0, 5.0),
                        at(10.0, 0.0, 0.0, 5.0),
                    ]),
                    lines,
                ],
            ),
            (id[1], vec![mesh(vec![at(20.0, 0.0, 5.0, 10.0)])]),
        ])
        .unwrap();
    (id[0], id[1])
}

fn hit(object: ObjectId, sub: u32, element: u32) -> PickHit {
    PickHit {
        key: ItemKey { object, sub },
        element,
    }
}

fn pick(
    renderer: &mut Renderer,
    scene: &mut Scene,
    frame: &ViewFrame,
    settings: &RenderSettings,
    region: PickRegion,
) -> Vec<PickHit> {
    renderer
        .pick(scene, frame, SIZE, SIZE, settings, &region)
        .unwrap()
}

#[test]
fn pixels_and_rectangles_resolve_to_objects_and_sub_objects() {
    let (_, mut scene, mut renderer) = setup();
    let (p, q) = populate(&mut scene);
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    for mode in [ShadingMode::Shaded, ShadingMode::Wireframe] {
        let settings = RenderSettings {
            mode,
            ..RenderSettings::DEFAULT
        };
        let mut px = |x, y, tolerance| {
            pick(
                &mut renderer,
                &mut scene,
                &frame,
                &settings,
                PickRegion::pixel(x, y, tolerance),
            )
        };
        assert_eq!(px(18, 48, 0.0), vec![hit(p, 0, 0)], "first instance");
        assert_eq!(px(38, 48, 0.0), vec![hit(p, 0, 1)], "second instance");
        assert_eq!(px(54, 48, 0.0), vec![hit(p, 0, 2)], "third instance");
        assert_eq!(px(60, 48, 0.0), vec![hit(q, 0, 0)], "the nearer face wins");
        assert_eq!(
            px(48, 18, 0.0),
            vec![hit(p, 1, 0)],
            "the first path's segment"
        );
        assert_eq!(
            px(30, 78, 0.0),
            vec![hit(p, 1, 1)],
            "the second path's first segment"
        );
        assert_eq!(px(70, 78, 0.0), vec![hit(p, 1, 2)], "and its second");
        assert_eq!(px(5, 5, 0.0), vec![], "background");
        // Row 21 is 3.5 pixels from the line at y = 30 (row boundary 18), whose half width is
        // 0.5: a tolerance of 4 reaches it, 2 does not.
        assert_eq!(px(48, 21, 4.0), vec![hit(p, 1, 0)]);
        assert_eq!(px(48, 21, 2.0), vec![]);
    }
    let settings = RenderSettings::DEFAULT;
    let all = pick(
        &mut renderer,
        &mut scene,
        &frame,
        &settings,
        PickRegion {
            x: 0,
            y: 0,
            width: SIZE,
            height: SIZE,
            tolerance_px: 0.0,
        },
    );
    let mut expected = vec![
        hit(p, 0, 0),
        hit(p, 0, 1),
        hit(p, 0, 2),
        hit(p, 1, 0),
        hit(p, 1, 1),
        hit(p, 1, 2),
        hit(q, 0, 0),
    ];
    expected.sort();
    assert_eq!(all, expected);
    // Columns 36..=55 by rows 44..=52 contain parts of the second and third instances only.
    let window = pick(
        &mut renderer,
        &mut scene,
        &frame,
        &settings,
        PickRegion {
            x: 36,
            y: 44,
            width: 20,
            height: 9,
            tolerance_px: 0.0,
        },
    );
    assert_eq!(window, vec![hit(p, 0, 1), hit(p, 0, 2)]);
}

#[test]
fn invalid_regions_are_errors() {
    let (_, mut scene, mut renderer) = setup();
    populate(&mut scene);
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    let settings = RenderSettings::DEFAULT;
    let mut code = |region: PickRegion| {
        renderer
            .pick(&mut scene, &frame, SIZE, SIZE, &settings, &region)
            .unwrap_err()
            .code()
    };
    let base = PickRegion::pixel(0, 0, 1.0);
    for region in [
        PickRegion { width: 0, ..base },
        PickRegion { x: SIZE, ..base },
        PickRegion {
            y: 90,
            height: 7,
            ..base
        },
        PickRegion {
            x: u32::MAX,
            width: 2,
            ..base
        },
        PickRegion {
            tolerance_px: f32::NAN,
            ..base
        },
        PickRegion {
            tolerance_px: -1.0,
            ..base
        },
    ] {
        assert_eq!(code(region), "gpu.pick.invalid", "{region:?}");
    }
}

/// Pick times on the software adapter, written to the test output. The bound is loose: only
/// a hang or a full-image render per pick would exceed it.
#[test]
fn picks_render_only_the_region_of_interest() {
    let (_, mut scene, mut renderer) = setup();
    let (p, _) = populate(&mut scene);
    let frame = top_view(Point3::origin(), f64::from(SIZE));
    let settings = RenderSettings::DEFAULT;
    let region = PickRegion::pixel(18, 48, 4.0);
    // The first pick builds the pipelines.
    pick(&mut renderer, &mut scene, &frame, &settings, region);
    let runs = 20;
    let start = Instant::now();
    for _ in 0..runs {
        assert_eq!(
            pick(&mut renderer, &mut scene, &frame, &settings, region),
            vec![hit(p, 0, 0)]
        );
    }
    let pixel = start.elapsed() / runs;
    let rect = PickRegion {
        x: 0,
        y: 0,
        width: SIZE,
        height: SIZE,
        tolerance_px: 0.0,
    };
    let start = Instant::now();
    for _ in 0..runs {
        pick(&mut renderer, &mut scene, &frame, &settings, rect);
    }
    let whole = start.elapsed() / runs;
    writeln!(
        std::io::stderr(),
        "pick: 1 pixel {pixel:?}, {SIZE}x{SIZE} rectangle {whole:?}"
    )
    .unwrap();
    assert!(pixel.as_secs_f64() < 1.0, "{pixel:?}");
}
