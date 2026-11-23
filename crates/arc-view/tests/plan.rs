//! Reference drawings of small standard models in plan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::type_complexity,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod support;

use arc_base::Cancel;

use arc_base::math::{Point3, Vector3};
use arc_base::{Color, Limits};
use arc_geom::{PlaneFrame, PrismTool};
use arc_view::{
    LineStyle, PlanRange, PrimitiveKind, ViewDef, VisibilityClass, build_view, crop_rectangle,
};
use support::{
    Model, TOL, cuboid, has, lines, object, profile_solid, region_area, room, write_svg,
};

fn plan() -> ViewDef {
    ViewDef::plan(PlanRange::new(1.2, -1.0, Some(3.5)))
}

fn fills(view: &arc_view::ViewResult) -> Vec<&arc_geom::Region> {
    view.list
        .primitives
        .iter()
        .filter_map(|p| match &p.shape {
            arc_draft::display::Shape::Region(r) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn box_room_with_four_joined_walls() {
    let mut model = Model::default();
    room(&mut model);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    write_svg("room", &view);
    // Each boundary piece belongs to the wall it lies on: the end faces split into three pieces
    // at the wall joints, and no line crosses a joint.
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(cut.len(), 12, "{cut:?}");
    for (a, b) in [
        ((0.0, 0.0), (5.0, 0.0)),
        ((5.0, 0.2), (5.0, 3.8)),
        ((0.0, 4.0), (5.0, 4.0)),
        ((0.0, 0.0), (0.0, 0.2)),
        ((0.0, 0.2), (0.0, 3.8)),
        ((0.2, 0.2), (4.8, 0.2)),
        ((4.8, 0.2), (4.8, 3.8)),
        ((0.2, 3.8), (4.8, 3.8)),
        ((0.2, 0.2), (0.2, 3.8)),
    ] {
        assert!(has(&cut, a, b), "missing {a:?}-{b:?} in {cut:?}");
    }
    assert!(
        !has(&cut, (0.0, 0.2), (0.2, 0.2)),
        "no seam between joined walls"
    );
    assert_eq!(view.count(VisibilityClass::Visible), 0);
    assert_eq!(view.count(VisibilityClass::Beyond), 0);
    let regions = fills(&view);
    assert_eq!(regions.len(), 1, "the four walls join into one region");
    let region = regions.first().expect("region");
    assert_eq!(region.parts().len(), 1);
    assert_eq!(region.parts().first().map(|p| p.holes().len()), Some(1));
    assert!((region_area(region) - (20.0 - 4.6 * 3.6)).abs() < 1e-9);
    assert!(view.diagnostics.is_empty());
    let fill = view
        .links
        .iter()
        .find(|l| l.kind == PrimitiveKind::Fill)
        .expect("fill link");
    assert_eq!(fill.joined.len(), 3);
}

#[test]
fn wall_with_a_door_hole() {
    let wall = cuboid((0.0, 5.0), (0.0, 0.2), (0.0, 3.0));
    let profile = support::polygon(&[(2.0, 0.0), (3.0, 0.0), (3.0, 2.1), (2.0, 2.1)]);
    // The frame's normal is -y; offsets 0..2 from y = 1 run through the wall.
    let frame = PlaneFrame::new(Point3::new(0.0, 1.0, 0.0), Vector3::x(), Vector3::z(), &TOL)
        .expect("frame");
    let tool = PrismTool {
        profile: &profile,
        frame,
        start: 0.0,
        end: 2.0,
        chord: 1e-3,
        opening: 0,
        allow_split: true,
    };
    let wall = wall
        .subtract_prism(&tool, &TOL, &Limits::DEFAULT)
        .expect("opening");
    let mut model = Model::default();
    model.add(1, object(wall, "wall", Some(1)));
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    write_svg("door", &view);
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(cut.len(), 8, "{cut:?}");
    for (a, b) in [
        ((0.0, 0.0), (2.0, 0.0)),
        ((2.0, 0.0), (2.0, 0.2)),
        ((0.0, 0.2), (2.0, 0.2)),
        ((3.0, 0.0), (5.0, 0.0)),
        ((3.0, 0.0), (3.0, 0.2)),
    ] {
        assert!(has(&cut, a, b), "missing {a:?}-{b:?} in {cut:?}");
    }
    assert_eq!(view.count(VisibilityClass::Visible), 0);
    let regions = fills(&view);
    assert_eq!(regions.len(), 1);
    assert_eq!(regions.first().map(|r| r.parts().len()), Some(2));
    assert!(view.diagnostics.is_empty());
}

#[test]
fn slab_below_the_cut_is_beyond_and_hidden_under_walls() {
    let mut model = Model::default();
    room(&mut model);
    let slab = model.add(
        10,
        object(cuboid((-0.5, 5.5), (-0.5, 4.5), (-0.2, 0.0)), "slab", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    write_svg("slab", &view);
    let beyond = lines(&view, VisibilityClass::Beyond);
    assert_eq!(beyond.len(), 4, "{beyond:?}");
    assert!(has(&beyond, (-0.5, -0.5), (5.5, -0.5)));
    assert!(has(&beyond, (5.5, -0.5), (5.5, 4.5)));
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 12);
    let (source, all) = view
        .select(
            view.links
                .iter()
                .position(|l| l.class == VisibilityClass::Beyond)
                .expect("beyond line"),
        )
        .expect("selection");
    assert_eq!(source, slab);
    assert_eq!(all.len(), 4);
}

#[test]
fn stepped_solid_shows_treads_below_the_cut() {
    // Six steps of 0.25 m run and 0.2 m rise, 1 m wide (y from -1 to 0); cut at 0.7.
    let mut profile = vec![(0.0, 0.0), (1.5, 0.0), (1.5, 1.2)];
    for k in (0..6).rev() {
        let x = 0.25 * f64::from(k);
        let z = 0.2 * f64::from(k + 1);
        profile.push((x, z));
        if k > 0 {
            profile.push((x, z - 0.2));
        }
    }
    let mut model = Model::default();
    model.add(1, object(profile_solid(&profile, 1.0), "stair", None));
    let def = ViewDef::plan(PlanRange::new(0.7, -1.0, None));
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    write_svg("stair", &view);
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(cut.len(), 4, "{cut:?}");
    assert!(has(&cut, (0.75, -1.0), (0.75, 0.0)));
    assert!(has(&cut, (0.75, 0.0), (1.5, 0.0)));
    let visible = lines(&view, VisibilityClass::Visible);
    assert_eq!(visible.len(), 5, "{visible:?}");
    for x in [0.0, 0.25, 0.5] {
        assert!(has(&visible, (x, -1.0), (x, 0.0)), "nosing at {x}");
    }
    assert!(has(&visible, (0.0, 0.0), (0.75, 0.0)), "{visible:?}");
    assert!(has(&visible, (0.0, -1.0), (0.75, -1.0)));
    assert!(view.diagnostics.is_empty());
}

#[test]
fn overhead_element_is_drawn_with_its_own_class() {
    let mut model = Model::default();
    room(&mut model);
    model.add(
        20,
        object(cuboid((0.0, 5.0), (1.9, 2.1), (2.6, 3.0)), "beam", None),
    );
    model.add(
        21,
        object(cuboid((0.0, 5.0), (1.0, 1.2), (4.0, 4.5)), "beam", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    write_svg("overhead", &view);
    let overhead = lines(&view, VisibilityClass::Overhead);
    assert_eq!(
        overhead.len(),
        4,
        "the beam above the top is omitted: {overhead:?}"
    );
    assert!(has(&overhead, (0.0, 1.9), (5.0, 1.9)));
    assert!(has(&overhead, (0.0, 1.9), (0.0, 2.1)));
}

#[test]
fn crop_cuts_lines_and_regions_without_drawing_the_crop_edge() {
    let mut model = Model::default();
    room(&mut model);
    let mut def = plan();
    def.crop = Some(
        crop_rectangle(
            arc_base::math::Point2::new(-1.0, -1.0),
            arc_base::math::Point2::new(2.5, 5.0),
            &TOL,
        )
        .expect("crop"),
    );
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    write_svg("crop", &view);
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(cut.len(), 8, "{cut:?}");
    assert!(has(&cut, (0.0, 0.0), (2.5, 0.0)));
    assert!(
        !cut.iter().any(|(a, b)| a.0 == 2.5 && b.0 == 2.5),
        "{cut:?}"
    );
    let regions = fills(&view);
    let area: f64 = regions.iter().map(|r| region_area(r)).sum();
    assert!((area - (2.5 * 4.0 - 2.3 * 3.6)).abs() < 1e-9, "{area}");
}

#[test]
fn hidden_class_is_drawn_only_when_styled() {
    let mut model = Model::default();
    let mut slab = object(cuboid((0.0, 4.0), (0.0, 4.0), (-0.3, 0.0)), "slab", None);
    let mut room_box = object(cuboid((1.0, 2.0), (1.0, 2.0), (-1.0, 0.5)), "box", None);
    room_box.styles.hidden = Some(LineStyle::solid(Color::BLACK));
    slab.styles.hidden = Some(LineStyle::solid(Color::BLACK));
    model.add(1, slab);
    model.add(2, room_box);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    // Both top outlines are beyond lines. A bottom outline lies on the outline of its own top
    // face in plan, so it is not inside it and merges with the top outline; only the box's
    // bottom outline, inside the slab's top face, is hidden.
    assert_eq!(lines(&view, VisibilityClass::Beyond).len(), 8);
    assert_eq!(lines(&view, VisibilityClass::Hidden).len(), 4);
}
