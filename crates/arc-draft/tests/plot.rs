//! Plot streams: determinism, content for a layout with a viewport at 1:100 between a model in
//! meters and a sheet in millimeters, and the plot options.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "test code"
)]

mod support;

use arc_base::math::Point2;
use arc_base::{Color, ObjectId};
use arc_draft::clip::Piece;
use arc_draft::display::{DisplayOptions, Lists};
use arc_draft::plot::{
    ItemKind, Offset, Paper, PlotArea, PlotOptions, PlotScale, PlotStream, layout_stream,
    model_stream,
};
use arc_draft::plot_style::DEFAULT_LINEWEIGHT_MM;
use arc_geom::Curve2;
use serde_json::json;
use support::{BLACK, Fixture, Scene, plot_scene, segment_json};

fn stream(scene: &Scene, options: &PlotOptions) -> PlotStream {
    let snapshot = scene.f.document.snapshot();
    layout_stream(
        &snapshot,
        Lists::Direct,
        scene.sheet,
        options,
        &DisplayOptions::default(),
    )
    .unwrap()
}

fn defaults(scene: &Scene) -> PlotOptions {
    PlotOptions::from_layout(&scene.f.document.snapshot(), scene.sheet).unwrap()
}

fn close(a: Point2, b: (f64, f64)) -> bool {
    (a.x - b.0).abs() < 1e-6 && (a.y - b.1).abs() < 1e-6
}

fn segment(curve: &Curve2) -> (Point2, Point2) {
    let Curve2::Segment(s) = curve else {
        panic!("not a segment: {curve:?}");
    };
    (s.start(), s.end())
}

fn piece_length(piece: &Piece) -> f64 {
    let Piece::Curve(c) = piece else {
        panic!("not a curve piece");
    };
    let (a, b) = segment(c);
    (b - a).norm()
}

#[test]
fn identical_input_gives_byte_identical_streams() {
    let scene = plot_scene(Fixture::new());
    let options = defaults(&scene);
    let first = stream(&scene, &options).encode();
    let second = stream(&scene, &options).encode();
    assert_eq!(first, second);
    assert!(first.starts_with(b"ARCP"));
    // The graph-built lists give the same bytes as direct evaluation.
    let graph = plot_scene(Fixture::drafting());
    let snapshot = graph.f.document.snapshot();
    let built = layout_stream(
        &snapshot,
        Lists::Graph(graph.f.document.graph()),
        graph.sheet,
        &defaults(&graph),
        &DisplayOptions::default(),
    )
    .unwrap();
    // The two documents draw the same scene with different identifiers, so compare structure.
    let direct = stream(&graph, &defaults(&graph));
    assert_eq!(built.encode(), direct.encode());
    assert_eq!(built.items.len(), direct.items.len());
}

#[test]
fn a_layout_with_a_viewport_at_one_to_one_hundred_plots_in_paper_millimeters() {
    let scene = plot_scene(Fixture::new());
    let s = stream(&scene, &defaults(&scene));
    assert_eq!((s.width, s.height), (297.0, 210.0));
    assert!(s.failed.is_empty());
    // The viewport's items come first, then the layout's own.
    let order: Vec<ObjectId> = s.items.iter().map(|i| i.source).collect();
    let model_at = order.iter().position(|id| *id == scene.model_line).unwrap();
    let far_at = order.iter().position(|id| *id == scene.far_line).unwrap();
    let sheet_at = order.iter().position(|id| *id == scene.sheet_line).unwrap();
    assert!(model_at < sheet_at && far_at < sheet_at);

    // The clip is the viewport rectangle, in millimeters on the page.
    assert_eq!(s.clips.len(), 1);
    let corners: Vec<Point2> = s.clips[0].vertices().iter().map(|v| v.point).collect();
    assert!(close(corners[0], (50.0, 50.0)) && close(corners[2], (250.0, 150.0)));
    let model_item = &s.items[model_at];
    assert_eq!(model_item.clip, Some(0));
    assert_eq!(s.items[sheet_at].clip, None);

    // The model segment (0,0)-(10,0) m is (100,100)-(200,100) mm; its pen is 0.5 mm.
    let ItemKind::Stroke { curve, pieces, pen } = &model_item.kind else {
        panic!("not a stroke");
    };
    let (a, b) = segment(curve);
    assert!(close(a, (100.0, 100.0)) && close(b, (200.0, 100.0)));
    assert_eq!(pen.width, 0.5);
    assert_eq!(pen.color, Color::BLACK);
    // Dashes of 0.5 m and 0.25 m are 5 mm and 2.5 mm on the page.
    let pieces = pieces.as_ref().unwrap();
    assert_eq!(pieces.len(), 14);
    for (i, p) in pieces.iter().enumerate() {
        let Piece::Curve(c) = p else { panic!() };
        let (from, to) = segment(c);
        let start = 100.0 + 7.5 * i as f64;
        assert!(close(from, (start, 100.0)), "{i}: {from:?}");
        assert!(
            close(to, (start + if i == 13 { 2.5 } else { 5.0 }, 100.0)),
            "{i}: {to:?}"
        );
    }
    assert!((piece_length(&pieces[0]) - 5.0).abs() < 1e-9);

    // The long line is cut at the clip, x = 250.
    let ItemKind::Stroke { pieces, .. } = &s.items[far_at].kind else {
        panic!()
    };
    let last = pieces.as_ref().unwrap().last().unwrap();
    let Piece::Curve(c) = last else { panic!() };
    assert!(close(segment(c).1, (250.0, 120.0)));

    // The sheet's own line is on the page as it is: 1 mm pen, coordinates in millimeters.
    let ItemKind::Stroke { curve, pen, .. } = &s.items[sheet_at].kind else {
        panic!()
    };
    let (a, b) = segment(curve);
    assert!(close(a, (10.0, 10.0)) && close(b, (60.0, 10.0)));
    assert_eq!(pen.width, 1.0);
}

#[test]
fn a_wipeout_becomes_a_mask_after_the_content_it_hides() {
    let mut scene = plot_scene(Fixture::new());
    let boundary = json!({"kind": "polyline", "closed": true, "vertices": [
        {"point": [120.0, 90.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [140.0, 90.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [140.0, 110.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0},
        {"point": [120.0, 110.0], "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}]});
    let wipeout = scene.f.create(
        "core.draft.wipeout.create",
        json!({"owner": scene.sheet.to_string(), "boundary": boundary}),
    );
    let s = stream(&scene, &defaults(&scene));
    let at = s.items.iter().position(|i| i.source == wipeout).unwrap();
    let ItemKind::Mask { region } = &s.items[at].kind else {
        panic!("not a mask");
    };
    assert!(region.is_closed());
    let viewport_at = s
        .items
        .iter()
        .position(|i| i.source == scene.model_line)
        .unwrap();
    assert!(viewport_at < at);
    // The sheet line is not under the mask and is kept.
    assert!(s.items.iter().any(|i| i.source == scene.sheet_line));
}

#[test]
fn plot_area_scale_offset_and_rotation_place_the_content() {
    let scene = plot_scene(Fixture::new());
    let start_of = |s: &PlotStream| {
        let item = s
            .items
            .iter()
            .find(|i| i.source == scene.sheet_line)
            .unwrap();
        let ItemKind::Stroke { curve, .. } = &item.kind else {
            panic!()
        };
        segment(curve)
    };
    let base = defaults(&scene);

    // A window with fit scale: window (10,10)-(110,60) is 100 x 50; the page is 297 x 210, so
    // k = min(297/100, 210/50) = 2.97, centered: the plotted area is 297 x 148.5 at y = 30.75.
    let fit = PlotOptions {
        area: PlotArea::Window {
            min: Point2::new(10.0, 10.0),
            max: Point2::new(110.0, 60.0),
        },
        scale: PlotScale::Fit,
        offset: Offset::Center,
        ..base.clone()
    };
    let s = stream(&scene, &fit);
    let (a, b) = start_of(&s);
    assert!(close(a, (0.0, 30.75)), "{a:?}");
    assert!(close(b, (148.5, 30.75)), "{b:?}");
    assert!(close(s.area[0], (0.0, 30.75)) && close(s.area[1], (297.0, 179.25)));

    // A ratio of 2 with an offset of (5, 7) mm from the lower left of the printable area.
    let ratio = PlotOptions {
        area: PlotArea::Window {
            min: Point2::new(10.0, 10.0),
            max: Point2::new(110.0, 60.0),
        },
        scale: PlotScale::Ratio(2.0),
        offset: Offset::Origin(Point2::new(5.0, 7.0)),
        ..base.clone()
    };
    let (a, b) = start_of(&stream(&scene, &ratio));
    assert!(close(a, (5.0, 7.0)) && close(b, (105.0, 7.0)));

    // One quarter turn counterclockwise: the window turns about its lower left corner, and the
    // 50 mm segment, 2 mm... (x along the sheet) now runs up the page.
    let turned = PlotOptions {
        rotation: 1,
        ..ratio.clone()
    };
    let s = stream(&scene, &turned);
    let (a, b) = start_of(&s);
    // The window is 200 x 100 on the page: turned it is 100 wide and 200 high from (5, 7);
    // the point (10, 10) is the window's lower left corner, which goes to the lower right.
    assert!(close(a, (105.0, 7.0)), "{a:?}");
    assert!(close(b, (105.0, 107.0)), "{b:?}");
    // Rotation is reduced modulo four.
    let four = PlotOptions {
        rotation: 5,
        ..ratio
    };
    assert_eq!(stream(&scene, &four).encode(), s.encode());
}

#[test]
fn lineweights_and_plot_styles_can_be_switched_off() {
    let mut scene = plot_scene(Fixture::new());
    let table = scene.f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "office", "kind": "named", "styles": [
            {"name": "thin", "lineweight": 0.1, "color": [1.0, 0.0, 0.0, 1.0], "screening": 50.0, "end": "round"}]}),
    );
    let _ = table;
    scene.f.set(
        "core.draft.entity.modify",
        &[scene.sheet_line],
        "plot_style",
        json!("thin"),
    );
    scene.f.set(
        "core.draft.layout.modify",
        &[scene.settings],
        "plot_style_table",
        json!("office"),
    );
    let width_color = |options: &PlotOptions| {
        let s = stream(&scene, options);
        let item = s
            .items
            .iter()
            .find(|i| i.source == scene.sheet_line)
            .unwrap();
        let ItemKind::Stroke { pen, .. } = &item.kind else {
            panic!()
        };
        *pen
    };
    let on = defaults(&scene);
    assert_eq!(on.style_table.as_deref(), Some("office"));
    let pen = width_color(&on);
    assert_eq!(pen.width, 0.1);
    assert_eq!(pen.color.r, 1.0);
    assert!((pen.color.b - 0.5).abs() < 1e-6, "{pen:?}");
    // Lineweights off: every pen has the default width.
    let flat = PlotOptions {
        lineweights: false,
        ..on.clone()
    };
    assert_eq!(width_color(&flat).width, DEFAULT_LINEWEIGHT_MM);
    // Styles off: the object's pen.
    let plain = PlotOptions {
        styles: false,
        ..on.clone()
    };
    let pen = width_color(&plain);
    assert_eq!(pen.width, 1.0);
    assert_eq!(pen.color, Color::BLACK);
    // An unknown table name is no table.
    let unknown = PlotOptions {
        style_table: Some("none".into()),
        ..on
    };
    assert_eq!(width_color(&unknown).width, 1.0);
}

#[test]
fn a_style_linetype_regenerates_dashes_in_the_scale_of_the_view() {
    let mut scene = plot_scene(Fixture::new());
    scene.f.create(
        "core.draft.plot_style_table.create",
        json!({"name": "dash", "kind": "named", "styles": [
            {"name": "d", "linetype": scene.dashed.to_string()}]}),
    );
    scene.f.set(
        "core.draft.entity.modify",
        &[scene.far_line],
        "plot_style",
        json!("d"),
    );
    scene.f.set(
        "core.draft.entity.modify",
        &[scene.sheet_line],
        "plot_style",
        json!("d"),
    );
    let options = PlotOptions {
        style_table: Some("dash".into()),
        ..defaults(&scene)
    };
    let s = stream(&scene, &options);
    let pieces_of = |id| {
        let item = s.items.iter().find(|i| i.source == id).unwrap();
        let ItemKind::Stroke { pieces, .. } = &item.kind else {
            panic!()
        };
        pieces.clone().unwrap()
    };
    // In the viewport the pattern keeps its model scale: 0.5 m is 5 mm.
    let far = pieces_of(scene.far_line);
    assert!((piece_length(&far[0]) - 5.0).abs() < 1e-9);
    // On the sheet the pattern is in sheet units: 0.5 mm.
    let own = pieces_of(scene.sheet_line);
    assert!((piece_length(&own[0]) - 0.5).abs() < 1e-9);
}

#[test]
fn invalid_options_and_sources_have_their_codes() {
    let scene = plot_scene(Fixture::new());
    let snapshot = scene.f.document.snapshot();
    let run = |options: &PlotOptions| {
        layout_stream(
            &snapshot,
            Lists::Direct,
            scene.sheet,
            options,
            &DisplayOptions::default(),
        )
        .unwrap_err()
        .code()
    };
    let base = defaults(&scene);
    for bad in [
        PlotOptions {
            scale: PlotScale::Ratio(0.0),
            ..base.clone()
        },
        PlotOptions {
            scale: PlotScale::Ratio(f64::NAN),
            ..base.clone()
        },
        PlotOptions {
            area: PlotArea::Window {
                min: Point2::new(5.0, 5.0),
                max: Point2::new(5.0, 9.0),
            },
            ..base.clone()
        },
        PlotOptions {
            offset: Offset::Origin(Point2::new(f64::INFINITY, 0.0)),
            ..base.clone()
        },
    ] {
        assert_eq!(run(&bad), "draft.plot.option_invalid");
    }
    let tiny = PlotOptions {
        max_items: 1,
        ..base
    };
    assert_eq!(run(&tiny), "draft.plot.too_large");
    // A model space is not a layout, and a layout needs its settings.
    let err = layout_stream(
        &snapshot,
        Lists::Direct,
        scene.model,
        &PlotOptions::default(),
        &DisplayOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.code(), "draft.plot.source_invalid");
    let err = PlotOptions::from_layout(&snapshot, scene.model).unwrap_err();
    assert_eq!(err.code(), "draft.plot.source_invalid");
}

#[test]
fn model_space_windows_plot_at_a_scale_and_cut_unbounded_curves() {
    let mut f = Fixture::new();
    let model = f.create(
        "core.draft.space.create",
        json!({"name": "model", "kind": "model", "unit": "meter"}),
    );
    let line = f.entity(model, segment_json(0.0, 5.0, 20.0, 5.0));
    support::pen(&mut f, line, BLACK, 0.35, None);
    let ray = f.entity(
        model,
        json!({"kind": "ray", "origin": [2.0, 2.0], "direction": [1.0, 0.0]}),
    );
    let paper = Paper {
        width: 200.0,
        height: 100.0,
        margins: [0.0; 4],
    };
    let snapshot = f.document.snapshot();
    let options = PlotOptions {
        area: PlotArea::Window {
            min: Point2::new(0.0, 0.0),
            max: Point2::new(10.0, 8.0),
        },
        // 1:100 from meters: 10 m is 100 mm.
        scale: PlotScale::Ratio(0.01),
        offset: Offset::Origin(Point2::new(10.0, 10.0)),
        ..PlotOptions::default()
    };
    let s = model_stream(
        &snapshot,
        Lists::Direct,
        model,
        &paper,
        &options,
        &DisplayOptions::default(),
    )
    .unwrap();
    let ItemKind::Stroke { curve, .. } = &s.items.iter().find(|i| i.source == line).unwrap().kind
    else {
        panic!()
    };
    let (a, b) = segment(curve);
    assert!(close(a, (10.0, 60.0)) && close(b, (210.0, 60.0)));
    // The ray is cut to the window: x from 2 to 10 m.
    let ItemKind::Stroke { pieces, .. } = &s.items.iter().find(|i| i.source == ray).unwrap().kind
    else {
        panic!()
    };
    let Piece::Curve(c) = &pieces.as_ref().unwrap()[0] else {
        panic!()
    };
    let (a, b) = segment(c);
    assert!(
        close(a, (30.0, 30.0)) && close(b, (110.0, 30.0)),
        "{a:?} {b:?}"
    );
    // The plotted area is the window on the page: 100 x 80 mm from (10, 10).
    assert!(close(s.area[0], (10.0, 10.0)) && close(s.area[1], (110.0, 90.0)));
}
