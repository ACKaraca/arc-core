//! Drawings in a vertical frame: sections and elevations.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test code")]

mod support;

use arc_base::Cancel;
use arc_base::math::{Point3, Vector3};
use arc_view::{PlanRange, ViewDef, ViewDirection, VisibilityClass, build_view};
use support::{Model, TOL, has, id, lines, room};

/// A section through the room at y = 2 looking toward +y: view x is world x, view y is the
/// height, and the cut plane is the frame's origin plane.
fn section() -> ViewDef {
    let mut def = ViewDef::plan(PlanRange::new(0.0, -10.0, None));
    def.direction = ViewDirection::frame(Point3::new(0.0, 2.0, 0.0), Vector3::x(), Vector3::z())
        .expect("frame");
    def
}

#[test]
fn a_vertical_section_cuts_the_side_walls_and_sees_the_back_wall() {
    let mut model = Model::default();
    room(&mut model);
    let view = build_view(&section(), &model, &TOL, &Cancel::new()).expect("view");
    let cut = lines(&view, VisibilityClass::Cut);
    // The side walls are cut over their full height.
    assert!(has(&cut, (0.0, 0.0), (0.0, 3.0)), "{cut:?}");
    assert!(has(&cut, (5.0, 0.0), (5.0, 3.0)), "{cut:?}");
    // The back wall lies beyond the cut; its top edge is at the height of 3 m.
    let beyond = lines(&view, VisibilityClass::Beyond);
    assert!(
        beyond
            .iter()
            .any(|((_, y0), (_, y1))| (y0 - 3.0).abs() < 1e-6 && (y1 - 3.0).abs() < 1e-6),
        "{beyond:?}"
    );
    // The front wall is in front of the cut and is not drawn.
    let front = id(1);
    assert!(view.links.iter().all(|l| l.source != front));
}

#[test]
fn a_frame_maps_points_both_ways_and_rejects_parallel_axes() {
    let frame = ViewDirection::frame(
        Point3::new(1.0, 2.0, 3.0),
        Vector3::new(0.0, 2.0, 0.0),
        Vector3::new(0.0, 1.0, 1.0),
    )
    .expect("frame");
    let p = Point3::new(4.0, -5.0, 6.0);
    let back = frame.to_world(frame.to_view(p));
    assert!((back - p).norm() < 1e-12);
    // View y is the part of `up` perpendicular to `right`: world z.
    assert!((frame.to_view(Point3::new(1.0, 2.0, 4.0)).y - 1.0).abs() < 1e-12);
    assert!(ViewDirection::frame(Point3::origin(), Vector3::x(), Vector3::x() * 3.0).is_err());
    let mut def = section();
    def.direction = ViewDirection::Frame {
        origin: Point3::origin(),
        right: Vector3::x(),
        up: Vector3::x(),
    };
    assert!(def.validate(&TOL).is_err());
}
