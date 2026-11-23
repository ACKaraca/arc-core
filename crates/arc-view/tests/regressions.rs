//! Regression tests from review: rooms with doors, joins and seams, sloped solids, faces in the
//! cut plane, open and degenerate meshes, concave crops, overrides, back-links, large and tiny
//! models, invalid styles and bounded work.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::type_complexity,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    reason = "test code"
)]

mod support;

use arc_base::Cancel;

use std::time::Instant;

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Color, Limits};
use arc_draft::display::Shape;
use arc_geom::{PlaneFrame, PrismTool};
use arc_mesh::TriMesh;
use arc_view::{
    CutFill, Geometry, GraphicOverride, PlanRange, PrimitiveKind, ViewDef, VisibilityClass,
    build_view,
};
use support::{Model, TOL, cuboid, has, lines, object, profile_solid, region_area, room};

fn plan() -> ViewDef {
    ViewDef::plan(PlanRange::new(1.2, -1.0, Some(3.5)))
}

fn fills(view: &arc_view::ViewResult) -> Vec<&arc_geom::Region> {
    view.list
        .primitives
        .iter()
        .filter_map(|p| match &p.shape {
            Shape::Region(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn door_wall() -> arc_geom::Polyhedron {
    let wall = cuboid((0.0, 5.0), (0.0, 0.2), (0.0, 3.0));
    let profile = support::polygon(&[(2.0, 0.0), (3.0, 0.0), (3.0, 2.1), (2.0, 2.1)]);
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
    wall.subtract_prism(&tool, &TOL, &Limits::DEFAULT)
        .expect("opening")
}

fn mesh_of(solid: &arc_geom::Polyhedron) -> TriMesh {
    TriMesh::from_polyhedron(solid, &TOL, &Limits::DEFAULT).expect("mesh")
}

#[test]
fn qa_room_with_door_and_inner_slab() {
    let mut model = Model::default();
    // Room whose south wall has a door at x 2..3.
    model.add(1, object(door_wall(), "wall", Some(1)));
    for (n, (x, y)) in (2..).zip([
        ((0.0, 5.0), (3.8, 4.0)),
        ((0.0, 0.2), (0.2, 3.8)),
        ((4.8, 5.0), (0.2, 3.8)),
    ]) {
        model.add(n, object(cuboid(x, y, (0.0, 3.0)), "wall", Some(1)));
    }
    // A slab whose edge runs at y = 0.1, under the south wall, through the door.
    model.add(
        10,
        object(cuboid((0.1, 4.9), (0.1, 3.9), (-0.2, 0.0)), "slab", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    let regions = fills(&view);
    assert_eq!(regions.len(), 1, "joined walls");
    // The door opening: no cut line crosses x 2..3 at y 0 or 0.2.
    let cut = lines(&view, VisibilityClass::Cut);
    for &((x0, y0), (x1, y1)) in &cut {
        if (y0 - y1).abs() < 1e-9 && (y0.abs() < 1e-9 || (y0 - 0.2).abs() < 1e-9) {
            assert!(
                x1 <= 2.0 + 1e-6 || x0 >= 3.0 - 1e-6,
                "cut across door {x0}..{x1} y {y0}"
            );
        }
    }
    // Door jambs drawn.
    assert!(has(&cut, (2.0, 0.0), (2.0, 0.2)));
    assert!(has(&cut, (3.0, 0.0), (3.0, 0.2)));
    // Slab edge at y=0.1: visible only in the opening, x 2..3.
    let beyond = lines(&view, VisibilityClass::Beyond);
    assert!(has(&beyond, (2.0, 0.1), (3.0, 0.1)), "{beyond:?}");
    assert_eq!(beyond.len(), 1, "everything else under walls: {beyond:?}");
    // Back-links: one per primitive.
    assert_eq!(view.links.len(), view.list.primitives.len());
}

#[test]
fn qa_l_joined_and_unjoined_seam() {
    let a = cuboid((0.0, 4.0), (0.0, 0.2), (0.0, 3.0));
    let b = cuboid((0.0, 0.2), (0.0, 3.0), (0.0, 3.0));
    let mut model = Model::default();
    model.add(1, object(a.clone(), "wall", Some(7)));
    model.add(2, object(b.clone(), "wall", Some(7)));
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(fills(&view).len(), 1);
    // Overlap square 0..0.2 x 0..0.2: no seam at y=0.2 x 0..0.2 nor x=0.2 y 0..0.2.
    for &((x0, y0), (x1, y1)) in &cut {
        let inside = |x: f64, y: f64| x > 1e-6 && x < 0.2 - 1e-6 && y > 1e-6 && y < 0.2 - 1e-6;
        let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!(!inside(mid.0, mid.1));
        let on_seam =
            ((y0 - 0.2).abs() < 1e-9 && (y1 - 0.2).abs() < 1e-9 && x0.max(x1) <= 0.2 + 1e-9)
                || ((x0 - 0.2).abs() < 1e-9 && (x1 - 0.2).abs() < 1e-9 && y0.max(y1) <= 0.2 + 1e-9);
        assert!(!on_seam, "seam {x0},{y0}-{x1},{y1}");
    }
    assert!(has(&cut, (0.0, 0.0), (4.0, 0.0)) || has(&cut, (0.0, 0.0), (0.2, 0.0)));
    let area: f64 = fills(&view).iter().map(|r| region_area(r)).sum();
    assert!((area - (0.8 + 0.6 - 0.04)).abs() < 1e-9, "{area}");

    // Different keys: seam present, two fills.
    let mut model = Model::default();
    model.add(1, object(a, "wall", Some(7)));
    model.add(2, object(b, "wall", Some(8)));
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(fills(&view).len(), 2);
    assert!(
        cut.iter()
            .any(|&((x0, y0), (x1, y1))| (y0 - 0.2).abs() < 1e-9
                && (y1 - 0.2).abs() < 1e-9
                && x0 < 0.1
                && x1 > 0.1),
        "seam at y=0.2 missing"
    );
}

#[test]
fn qa_overhead_beam_and_omitted_below_bottom() {
    let mut model = Model::default();
    let beam = model.add(
        1,
        object(cuboid((0.0, 5.0), (1.0, 1.3), (2.6, 3.0)), "beam", None),
    );
    let deep = model.add(
        2,
        object(cuboid((0.0, 1.0), (0.0, 1.0), (-3.0, -2.0)), "pit", None),
    );
    let high = model.add(
        3,
        object(cuboid((0.0, 1.0), (0.0, 1.0), (4.0, 5.0)), "roof", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(lines(&view, VisibilityClass::Overhead).len(), 4);
    assert!(view.links.iter().all(|l| l.source == beam));
    assert!(view.primitives_of(deep).is_empty() && view.primitives_of(high).is_empty());
    assert!(view.diagnostics.is_empty());
}

#[test]
fn qa_sloped_solid_crossing_the_cut_and_sloped_occluder() {
    // Ramp: rises from z 0 at x 0 to z 2.4 at x 4, 0.3 thick, y from -1 to 0.
    let ramp = profile_solid(&[(0.0, 0.0), (4.0, 2.4), (4.0, 2.7), (0.0, 0.3)], 1.0);
    let mut model = Model::default();
    model.add(1, object(ramp, "ramp", None));
    // Sloped plate below the cut, and a small box beneath it.
    let plate = profile_solid(&[(5.0, 0.4), (9.0, 1.0), (9.0, 1.1), (5.0, 0.5)], 1.0);
    model.add(2, object(plate, "plate", None));
    let under = model.add(
        3,
        object(cuboid((6.0, 7.0), (-0.8, -0.2), (0.0, 0.1)), "box", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    // Cut at z 1.2: the bottom surface crosses at x = 2, top at x = 1.5. Region x 1.5..2.
    let regions = fills(&view);
    assert_eq!(regions.len(), 1);
    assert!(
        (region_area(regions[0]) - 0.5).abs() < 1e-9,
        "{}",
        region_area(regions[0])
    );
    // Visible edges of the ramp below the cut: the low end at x 0, side edges x 0..1.5.
    let visible = lines(&view, VisibilityClass::Visible);
    assert!(has(&visible, (0.0, -1.0), (0.0, 0.0)), "{visible:?}");
    // The box under the plate is entirely hidden.
    let box_lines = view
        .links
        .iter()
        .filter(|l| l.source == under && l.kind == PrimitiveKind::Line)
        .count();
    assert_eq!(box_lines, 0, "box under the plate must be hidden");
}

#[test]
fn qa_stacked_boxes_meeting_at_the_cut() {
    // A wall built of two solids meeting exactly at the cut height: the lower one is cut.
    let mut model = Model::default();
    model.add(
        1,
        object(cuboid((0.0, 4.0), (0.0, 0.2), (0.0, 1.2)), "wall", Some(1)),
    );
    model.add(
        2,
        object(cuboid((0.0, 4.0), (0.0, 0.2), (1.2, 3.0)), "wall", Some(1)),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(fills(&view).len(), 1);
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 4);
}

#[test]
fn qa_coplanar_top_face_at_cut_in_a_mesh_cut_object() {
    // Two boxes in one mesh: a tall one and a short one whose top face lies in the cut plane.
    let tall = mesh_of(&cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0)));
    let short = mesh_of(&cuboid((2.0, 3.0), (0.0, 1.0), (0.0, 1.2)));
    let mut positions = tall.positions().to_vec();
    let mut triangles = tall.triangles().to_vec();
    let off = positions.len() as u32;
    positions.extend_from_slice(short.positions());
    triangles.extend(short.triangles().iter().map(|t| t.map(|v| v + off)));
    let mesh = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("mesh");
    let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0)), "x", None);
    content.parts[0].geometry = Geometry::Mesh(mesh);
    let mut model = Model::default();
    model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    let regions = fills(&view);
    let area: f64 = regions.iter().map(|r| region_area(r)).sum();
    assert!(view.diagnostics.is_empty());
    assert!(
        (area - 2.0).abs() < 1e-9,
        "both parts cut just below the plane: {area}"
    );
}

#[test]
fn qa_open_mesh_side_missing() {
    let solid = cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0));
    let mesh = mesh_of(&solid);
    let positions = mesh.positions().to_vec();
    let triangles: Vec<[u32; 3]> = mesh
        .triangles()
        .iter()
        .copied()
        .filter(|t| !t.iter().all(|&v| positions[v as usize].y == 1.0))
        .collect();
    let open = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("open");
    let mut content = object(solid, "open", None);
    content.parts[0].geometry = Geometry::Mesh(open);
    let mut model = Model::default();
    model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(view.diagnostics.len(), 1);
    assert_eq!(view.diagnostics[0].error.code(), "view.section.open");
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 3);
    assert!(fills(&view).is_empty());
}

#[test]
fn qa_concave_crop() {
    let mut model = Model::default();
    room(&mut model);
    let mut def = plan();
    // L-shaped crop removing the north-east quadrant.
    def.crop = Some(support::polygon(&[
        (-1.0, -1.0),
        (6.0, -1.0),
        (6.0, 2.0),
        (2.5, 2.0),
        (2.5, 5.0),
        (-1.0, 5.0),
    ]));
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    let cut = lines(&view, VisibilityClass::Cut);
    for &((x0, y0), (x1, y1)) in &cut {
        let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!(
            !(mid.0 > 2.5 + 1e-6 && mid.1 > 2.0 + 1e-6),
            "line in notch {cut:?}"
        );
    }
    // North wall outer face cut at x = 2.5.
    assert!(has(&cut, (0.0, 4.0), (2.5, 4.0)), "{cut:?}");
    // East wall cut at y = 2.
    assert!(
        has(&cut, (5.0, 0.0), (5.0, 2.0))
            || cut
                .iter()
                .any(|&(a, b)| (a.0 - 5.0).abs() < 1e-9 && (b.1 - 2.0).abs() < 1e-6),
        "{cut:?}"
    );
    // Crop edge not drawn.
    assert!(
        !cut.iter()
            .any(|&((x0, _), (x1, _))| (x0 - 2.5).abs() < 1e-9 && (x1 - 2.5).abs() < 1e-9)
    );
    let area: f64 = fills(&view).iter().map(|r| region_area(r)).sum();
    // Room area 20 - 16.56 = 3.44 minus notch part: walls in x>2.5,y>2: north 2.5*0.2 + east 0.2*1.8 = 0.5 + 0.36
    assert!((area - (3.44 - 0.86)).abs() < 1e-9, "{area}");
}

#[test]
fn qa_overrides_object_over_category() {
    let mut model = Model::default();
    room(&mut model);
    let w1 = support::id(1);
    let mut def = plan();
    def.overrides.by_category.insert("wall".into(), {
        let mut o = GraphicOverride::default();
        o.visible = Some(false);
        o
    });
    def.overrides.by_object.insert(w1, {
        let mut o = GraphicOverride::default();
        o.visible = Some(true);
        o
    });
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    assert!(!view.links.is_empty());
    assert!(view.links.iter().all(|l| l.source == w1));
}

#[test]
fn qa_back_links_and_select() {
    let mut model = Model::default();
    room(&mut model);
    model.add(
        10,
        object(cuboid((0.1, 4.9), (0.1, 3.9), (-0.2, 0.0)), "slab", None),
    );
    model.add(
        11,
        object(cuboid((1.0, 2.0), (1.0, 2.0), (0.0, 0.8)), "table", None),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(view.links.len(), view.list.primitives.len());
    for (p, l) in view.list.primitives.iter().zip(&view.links) {
        assert_eq!(p.source, l.source);
    }
    for i in 0..view.links.len() {
        let (src, all) = view.select(i).expect("select");
        assert!(
            all.contains(&i) || !view.links[i].joined.is_empty() || view.links[i].source == src
        );
        let expect: Vec<usize> = (0..view.links.len())
            .filter(|&k| {
                view.links[k].source == src || view.links[k].joined.iter().any(|j| j.0 == src)
            })
            .collect();
        assert_eq!(all, expect);
    }
    // Picking a wall-2 cut line selects wall 2 and the shared fill.
    let w2 = support::id(2);
    let picked = view
        .links
        .iter()
        .position(|l| l.source == w2)
        .expect("w2 line");
    let (src, all) = view.select(picked).unwrap();
    assert_eq!(src, w2);
    assert!(
        all.iter()
            .any(|&k| view.links[k].kind == PrimitiveKind::Fill)
    );
    // Picking the joined fill resolves to the first wall; the others are listed as joined.
    let fill = view
        .links
        .iter()
        .position(|l| l.kind == PrimitiveKind::Fill)
        .unwrap();
    assert_eq!(view.select(fill).unwrap().0, support::id(1));
    assert_eq!(view.links[fill].joined.len(), 3);
}

fn grid_mesh(n: usize, size: f64, z0: f64, amp: f64) -> TriMesh {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let z = z0 + if (i + j) % 2 == 0 { amp } else { 0.0 };
            positions.push(Point3::new(
                size * i as f64 / n as f64,
                size * j as f64 / n as f64,
                z,
            ));
        }
    }
    let w = (n + 1) as u32;
    let mut triangles = Vec::new();
    for j in 0..n as u32 {
        for i in 0..n as u32 {
            let a = j * w + i;
            triangles.push([a, a + 1, a + w + 1]);
            triangles.push([a, a + w + 1, a + w]);
        }
    }
    TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("grid")
}

#[test]
fn qa_twenty_thousand_triangles() {
    // 100^2 * 2 triangles with a sawtooth: every edge sharp.
    let mesh = grid_mesh(100, 50.0, -0.5, 0.2);
    let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.0)), "terrain", None);
    content.parts[0].geometry = Geometry::Mesh(mesh);
    let mut model = Model::default();
    model.add(1, content);
    room(&mut model);
    let start = Instant::now();
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(start.elapsed().as_secs() < 20);
    assert_eq!(fills(&view).len(), 1);
    assert!(view.count(VisibilityClass::Beyond) > 0);
}

#[test]
fn qa_pathological_long_edges_over_dense_mesh_hit_the_work_bound() {
    // Long thin rods under a dense terrain: each long edge meets every triangle along it.
    let mesh = grid_mesh(100, 50.0, -0.5, 0.2);
    let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.0)), "terrain", None);
    content.parts[0].geometry = Geometry::Mesh(mesh);
    let mut model = Model::default();
    model.add(1, content);
    for k in 0..100_u32 {
        let y = 0.1 + f64::from(k) * 0.45;
        model.add(
            10 + u128::from(k),
            object(
                cuboid((0.0, 50.0), (y, y + 0.05), (-0.9, -0.85)),
                "rod",
                None,
            ),
        );
    }
    let mut def = plan();
    def.limits.max_occlusion_tests = 200_000;
    let start = Instant::now();
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("bound");
    assert!(start.elapsed().as_secs() < 10);
    assert_eq!(
        error.arguments().get("limit"),
        Some(&arc_base::error::Value::from("occlusion_tests"))
    );
    // A cancelled token stops the build.
    let cancel = Cancel::new();
    cancel.cancel();
    let error = build_view(&plan(), &model, &TOL, &cancel).expect_err("cancelled");
    assert_eq!(error.code(), "base.cancelled");
}

#[test]
fn qa_many_joined_walls() {
    let mut model = Model::default();
    for i in 0..400_u32 {
        let x = f64::from(i) * 1.0;
        model.add(
            u128::from(i) + 1,
            object(
                cuboid((x, x + 1.0), (0.0, 0.2), (0.0, 3.0)),
                "wall",
                Some(1),
            ),
        );
        model.add(
            u128::from(i) + 1001,
            object(
                cuboid((x, x + 0.2), (0.2, 3.0), (0.0, 3.0)),
                "wall",
                Some(1),
            ),
        );
    }
    let start = Instant::now();
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(start.elapsed().as_secs() < 20);
    assert_eq!(fills(&view).len(), 1);
    let area: f64 = fills(&view).iter().map(|r| region_area(r)).sum();
    assert!((area - 400.0 * (0.2 + 0.2 * 2.8)).abs() < 1e-6, "{area}");
    // The join work is bounded.
    let mut def = plan();
    def.limits.max_join_curves = 100;
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("join bound");
    assert_eq!(
        error.arguments().get("limit"),
        Some(&arc_base::error::Value::from("join"))
    );
}

#[test]
fn qa_huge_and_tiny() {
    // Room at 1e7 - 10.
    let base = 1.0e7 - 10.0;
    let mut model = Model::default();
    for (n, (x, y)) in (1..).zip([
        ((0.0, 5.0), (0.0, 0.2)),
        ((0.0, 5.0), (3.8, 4.0)),
        ((0.0, 0.2), (0.2, 3.8)),
        ((4.8, 5.0), (0.2, 3.8)),
    ]) {
        model.add(
            n,
            object(
                cuboid(
                    (base + x.0, base + x.1),
                    (base + y.0, base + y.1),
                    (0.0, 3.0),
                ),
                "wall",
                Some(1),
            ),
        );
    }
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    let cut = lines(&view, VisibilityClass::Cut);
    assert_eq!(cut.len(), 12, "{cut:?}");
    assert_eq!(fills(&view).len(), 1);

    // Tiny: 1e-6 and 1e-5 cubes as meshes (built by hand).
    for s in [1e-6, 1e-5, 1e-7] {
        let p = |x: f64, y: f64, z: f64| Point3::new(x * s, y * s, 1.0 + (z - 0.5) * s);
        let positions = vec![
            p(0., 0., 0.),
            p(1., 0., 0.),
            p(1., 1., 0.),
            p(0., 1., 0.),
            p(0., 0., 1.),
            p(1., 0., 1.),
            p(1., 1., 1.),
            p(0., 1., 1.),
        ];
        let triangles = vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [1, 2, 6],
            [1, 6, 5],
            [2, 3, 7],
            [2, 7, 6],
            [3, 0, 4],
            [3, 4, 7],
        ];
        let mesh = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("tiny");
        let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.0)), "tiny", None);
        content.parts[0].geometry = Geometry::Mesh(mesh);
        let mut model = Model::default();
        model.add(1, content);
        let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
        let codes: Vec<&str> = view.diagnostics.iter().map(|d| d.error.code()).collect();
        if s > TOL.linear {
            assert!(
                codes.is_empty() && view.count(VisibilityClass::Beyond) > 0,
                "{s}: {codes:?}"
            );
        } else {
            // Too small to draw: reported, not lost silently.
            assert_eq!(codes, ["view.geometry.degenerate"], "{s}");
        }
    }
}

#[test]
fn qa_degenerate_and_duplicate_vertices() {
    let solid = cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0));
    let mesh = mesh_of(&solid);
    // Triangle soup: each triangle its own vertices.
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for t in mesh.triangles() {
        let base = positions.len() as u32;
        for &v in t {
            positions.push(mesh.positions()[v as usize]);
        }
        triangles.push([base, base + 1, base + 2]);
    }
    let soup = TriMesh::new(positions.clone(), triangles.clone(), &Limits::DEFAULT).expect("soup");
    let mut content = object(solid.clone(), "soup", None);
    content.parts[0].geometry = Geometry::Mesh(soup);
    let mut model = Model::default();
    model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    assert_eq!(fills(&view).len(), 1);
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 4);

    // Zero-area triangle along a vertical edge that crosses the cut.
    let mut positions = mesh.positions().to_vec();
    let mut triangles = mesh.triangles().to_vec();
    let a = positions
        .iter()
        .position(|p| p.x == 0.0 && p.y == 0.0 && p.z == 0.0)
        .unwrap() as u32;
    let b = positions
        .iter()
        .position(|p| p.x == 0.0 && p.y == 0.0 && p.z == 3.0)
        .unwrap() as u32;
    positions.push(Point3::new(0.0, 0.0, 1.5));
    let m = positions.len() as u32 - 1;
    triangles.push([a, b, m]);
    // A sliver triangle (zero area, repeated position index-distinct).
    positions.push(Point3::new(0.0, 0.0, 0.0));
    let d = positions.len() as u32 - 1;
    triangles.push([a, d, b]);
    let degenerate = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("deg");
    let mut content = object(solid, "deg", None);
    content.parts[0].geometry = Geometry::Mesh(degenerate);
    let mut model = Model::default();
    model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    assert_eq!(fills(&view).len(), 1);
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 4);
}

#[test]
fn qa_bad_fill_parameters() {
    let library = arc_draft::hatch_pattern::library().expect("library");
    let pattern = library
        .get("diagonal-lines")
        .expect("pattern")
        .lines
        .clone();
    for (scale, angle, reported) in [
        (0.0, 0.0, true),
        (f64::NAN, 0.0, true),
        (1.0, f64::INFINITY, true),
        (1e-12, 0.0, false),
    ] {
        let mut model = Model::default();
        room(&mut model);
        model.add(
            10,
            object(cuboid((10.0, 11.0), (0.0, 1.0), (0.0, 3.0)), "col", None),
        );
        model.objects[0].1.parts[0].cut_fill = Some(CutFill::pattern(
            pattern.clone(),
            scale,
            angle,
            Color::BLACK,
        ));
        let start = Instant::now();
        let view =
            build_view(&plan(), &model, &TOL, &Cancel::new()).expect("a bad fill is not fatal");
        assert!(start.elapsed().as_secs() < 10);
        let codes: Vec<&str> = view.diagnostics.iter().map(|d| d.error.code()).collect();
        if reported {
            assert_eq!(codes, ["view.style.invalid"], "{scale} {angle}");
            assert_eq!(view.diagnostics[0].object, support::id(1));
        }
        // Solid fallback for the joined room, and the column's own fill.
        assert_eq!(fills(&view).len(), 2, "{scale} {angle}");
    }
    let mut model = Model::default();
    room(&mut model);
    model.objects[0].1.styles.cut.as_mut().unwrap().lineweight = Some(f64::NAN);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(view.diagnostics[0].error.code(), "view.style.invalid");
    assert!(view.list.primitives.iter().all(|p| {
        p.lineweight
            .as_ref()
            .is_none_or(|w| w.value.is_none_or(f64::is_finite))
    }));
    let crop = arc_view::crop_polygon(
        &[
            Point2::new(0.0, 0.0),
            Point2::new(f64::NAN, 0.0),
            Point2::new(1.0, 1.0),
        ],
        &TOL,
    );
    assert!(crop.is_err());
    for change in [
        |t: &mut arc_base::Tolerance| t.linear = f64::NAN,
        |t: &mut arc_base::Tolerance| t.tessellation_chord = f64::NAN,
        |t: &mut arc_base::Tolerance| t.tessellation_angle = 0.0,
    ] {
        let mut tol = TOL;
        change(&mut tol);
        let error = build_view(&plan(), &model, &tol, &Cancel::new()).expect_err("tolerance");
        assert_eq!(error.code(), "view.definition.invalid");
    }
    let mut def = plan();
    def.limits.max_coordinate = f64::INFINITY;
    assert!(build_view(&def, &model, &TOL, &Cancel::new()).is_err());
}

#[test]
fn qa_vertex_exactly_at_cut_on_sloped_face_does_not_hide_far_edges() {
    // Profile in xz: flat top at the cut height from x 0 to 3, then a slope up to z 3 at x 6.
    let solid = profile_solid(
        &[(0.0, 0.0), (6.0, 0.0), (6.0, 3.0), (3.0, 1.2), (0.0, 1.2)],
        1.0,
    );
    let mut model = Model::default();
    model.add(1, object(solid, "ramp", None));
    // An open single-triangle sheet below the cut whose long edge runs from (0,-3) to (6,9).
    let mesh = TriMesh::new(
        vec![
            Point3::new(0.0, -3.0, 0.1),
            Point3::new(6.0, 9.0, 0.1),
            Point3::new(-2.0, 9.0, 0.1),
        ],
        vec![[0, 1, 2]],
        &Limits::DEFAULT,
    )
    .expect("sheet");
    let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.0)), "sheet", None);
    content.parts[0].geometry = Geometry::Mesh(mesh);
    model.add(2, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    // The diagonal is under the ramp (y in -1..0, x in 1..1.5) only; from x 1.5 to 6 it is visible.
    let diag: Vec<_> = lines(&view, VisibilityClass::Beyond)
        .into_iter()
        .filter(|&((x0, y0), (x1, y1))| ((y1 - y0) - 2.0 * (x1 - x0)).abs() < 1e-6)
        .collect();
    assert!(
        has(&diag, (1.5, 0.0), (6.0, 9.0)),
        "diagonal hidden outside the ramp: {diag:?}"
    );
}

#[test]
fn qa_vertex_at_cut_control() {
    for cut in [1.2, 1.2 + 1e-9, 1.25] {
        let solid = profile_solid(
            &[(0.0, 0.0), (6.0, 0.0), (6.0, 3.0), (3.0, 1.2), (0.0, 1.2)],
            1.0,
        );
        let mut model = Model::default();
        model.add(1, object(solid, "ramp", None));
        let mesh = TriMesh::new(
            vec![
                Point3::new(0.0, -3.0, 0.1),
                Point3::new(6.0, 9.0, 0.1),
                Point3::new(-2.0, 9.0, 0.1),
            ],
            vec![[0, 1, 2]],
            &Limits::DEFAULT,
        )
        .expect("sheet");
        let mut content = object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.0)), "sheet", None);
        content.parts[0].geometry = Geometry::Mesh(mesh);
        model.add(2, content);
        let mut def = plan();
        def.range.cut = cut;
        let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
        let diag: Vec<_> = lines(&view, VisibilityClass::Beyond)
            .into_iter()
            .filter(|&((x0, y0), (x1, y1))| ((y1 - y0) - 2.0 * (x1 - x0)).abs() < 1e-6)
            .collect();
        // Hidden only where it passes under the ramp, between x 1 and 1.5.
        let hidden: f64 = 13.416_407_864_998_739
            - diag
                .iter()
                .map(|(a, b)| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt())
                .sum::<f64>();
        assert!((hidden - 0.5 * 5f64.sqrt()).abs() < 1e-5, "{cut}: {diag:?}");
    }
}

#[test]
fn qa_only_collinear_sliver() {
    let solid = cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0));
    let mesh = mesh_of(&solid);
    let mut positions = mesh.positions().to_vec();
    let mut triangles = mesh.triangles().to_vec();
    let a = positions
        .iter()
        .position(|p| p.x == 0.0 && p.y == 0.0 && p.z == 0.0)
        .unwrap() as u32;
    let b = positions
        .iter()
        .position(|p| p.x == 0.0 && p.y == 0.0 && p.z == 3.0)
        .unwrap() as u32;
    positions.push(Point3::new(0.0, 0.0, 1.5));
    let m = positions.len() as u32 - 1;
    triangles.push([a, b, m]);
    let degenerate = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("deg");
    let mut content = object(solid, "deg", None);
    content.parts[0].geometry = Geometry::Mesh(degenerate);
    let mut model = Model::default();
    model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    assert_eq!(fills(&view).len(), 1);
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 4);
}
