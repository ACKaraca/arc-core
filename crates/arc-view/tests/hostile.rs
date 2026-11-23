//! Degenerate and hostile input, overrides, patterns and picking.

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

use std::time::Instant;

use arc_base::error::Value;
use arc_base::math::Point2;
use arc_base::{Color, Limits, ObjectId};
use arc_draft::display::Shape;
use arc_mesh::TriMesh;
use arc_view::{
    CutFill, Geometry, GraphicOverride, PlanRange, PrimitiveKind, ViewDef, VisibilityClass,
    build_view, crop_rectangle,
};
use support::{Model, TOL, cuboid, has, lines, object, room, write_svg};

fn plan() -> ViewDef {
    ViewDef::plan(PlanRange::new(1.2, -1.0, Some(3.5)))
}

fn limit_of(error: &arc_base::Error) -> Option<&Value> {
    error.arguments().get("limit")
}

#[test]
fn empty_source_gives_an_empty_view() {
    let view = build_view(&plan(), &Model::default(), &TOL, &Cancel::new()).expect("view");
    assert!(view.list.primitives.is_empty() && view.links.is_empty());
    assert!(view.diagnostics.is_empty());
}

#[test]
fn faces_on_the_cut_plane_belong_to_the_part_above() {
    let mut model = Model::default();
    // Top exactly at the cut: cut just below its top. Bottom exactly at the cut: overhead.
    model.add(
        1,
        object(cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 1.2)), "a", None),
    );
    model.add(
        2,
        object(cuboid((2.0, 3.0), (0.0, 1.0), (1.2, 2.0)), "b", None),
    );
    // A stepped solid whose step lies in the cut plane: cut below the step.
    let stepped = support::profile_solid(
        &[
            (4.0, 0.0),
            (6.0, 0.0),
            (6.0, 2.0),
            (5.0, 2.0),
            (5.0, 1.2),
            (4.0, 1.2),
        ],
        1.0,
    );
    model.add(3, object(stepped, "c", None));
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty(), "{:?}", view.diagnostics);
    assert_eq!(lines(&view, VisibilityClass::Beyond).len(), 0);
    assert_eq!(lines(&view, VisibilityClass::Overhead).len(), 4);
    let cut = lines(&view, VisibilityClass::Cut);
    assert!(has(&cut, (4.0, -1.0), (6.0, -1.0)), "{cut:?}");
    assert!(has(&cut, (0.0, 0.0), (1.0, 0.0)), "{cut:?}");
    assert_eq!(cut.len(), 8);
}

#[test]
fn two_solids_meeting_at_the_cut_height_cut_the_lower_one() {
    let mut model = Model::default();
    let lower = model.add(
        1,
        object(cuboid((0.0, 4.0), (0.0, 0.2), (0.0, 1.2)), "w", Some(1)),
    );
    model.add(
        2,
        object(cuboid((0.0, 4.0), (0.0, 0.2), (1.2, 3.0)), "w", Some(1)),
    );
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty());
    let fill = view
        .links
        .iter()
        .filter(|l| l.kind == PrimitiveKind::Fill)
        .collect::<Vec<_>>();
    assert_eq!(fill.len(), 1);
    assert_eq!(fill.first().map(|l| l.source), Some(lower));
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 4);
}

#[test]
fn open_mesh_is_reported_and_drawn_without_fill() {
    let solid = cuboid((0.0, 1.0), (0.0, 1.0), (0.0, 3.0));
    let mesh = TriMesh::from_polyhedron(&solid, &TOL, &Limits::DEFAULT).expect("mesh");
    // Drop the triangles of the face whose normal points to negative x.
    let positions = mesh.positions().to_vec();
    let triangles: Vec<[u32; 3]> = mesh
        .triangles()
        .iter()
        .copied()
        .filter(|t| !t.iter().all(|&v| positions[v as usize].x == 0.0))
        .collect();
    let open = TriMesh::new(positions, triangles, &Limits::DEFAULT).expect("open mesh");
    let mut model = Model::default();
    let mut content = object(solid, "open", None);
    content.parts[0].geometry = Geometry::Mesh(open);
    let id = model.add(1, content);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(view.diagnostics.len(), 1);
    let d = view.diagnostics.first().expect("diagnostic");
    assert_eq!((d.object, d.error.code()), (id, "view.section.open"));
    assert_eq!(lines(&view, VisibilityClass::Cut).len(), 3);
    assert!(!view.links.iter().any(|l| l.kind != PrimitiveKind::Line));
}

#[test]
fn huge_coordinates_in_range_are_fast_and_exact_and_beyond_range_are_reported() {
    let base = 9.0e6;
    let mut model = Model::default();
    model.add(
        1,
        object(
            cuboid((base, base + 5.0), (base, base + 0.2), (0.0, 3.0)),
            "w",
            None,
        ),
    );
    model.add(
        2,
        object(
            cuboid(
                (base - 1.0, base + 6.0),
                (base - 1.0, base + 3.0),
                (-0.2, 0.0),
            ),
            "s",
            None,
        ),
    );
    let far = model.add(
        3,
        object(
            cuboid((2.0e7, 2.0e7 + 1.0), (0.0, 1.0), (0.0, 3.0)),
            "f",
            None,
        ),
    );
    let start = Instant::now();
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(start.elapsed().as_secs() < 5);
    let cut = lines(&view, VisibilityClass::Cut);
    assert!(has(&cut, (base, base), (base + 5.0, base)), "{cut:?}");
    assert_eq!(lines(&view, VisibilityClass::Beyond).len(), 4);
    assert_eq!(view.diagnostics.len(), 1);
    let d = view.diagnostics.first().expect("diagnostic");
    assert_eq!(
        (d.object, d.error.code()),
        (far, "view.geometry.out_of_range")
    );
}

#[test]
fn too_many_objects_edges_or_tests_fail_with_the_limit_code() {
    let mut model = Model::default();
    room(&mut model);
    model.add(
        10,
        object(cuboid((-0.5, 5.5), (-0.5, 4.5), (-0.2, 0.0)), "slab", None),
    );
    let mut def = plan();
    def.limits.max_objects = 4;
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("objects");
    assert_eq!(error.code(), "view.limit.exceeded");
    assert_eq!(limit_of(&error), Some(&Value::from("objects")));
    let mut def = plan();
    def.limits.max_edges = 30;
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("edges");
    assert_eq!(limit_of(&error), Some(&Value::from("edges")));
    let mut def = plan();
    def.limits.max_occlusion_tests = 10;
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("tests");
    assert_eq!(limit_of(&error), Some(&Value::from("occlusion_tests")));
    let mut def = plan();
    def.limits.max_primitives = 3;
    let error = build_view(&def, &model, &TOL, &Cancel::new()).expect_err("primitives");
    assert_eq!(limit_of(&error), Some(&Value::from("primitives")));
}

#[test]
fn invalid_definitions_are_refused() {
    for change in [
        |d: &mut ViewDef| d.range.bottom = 2.0,
        |d: &mut ViewDef| d.range.top = Some(0.5),
        |d: &mut ViewDef| d.range.cut = f64::NAN,
        |d: &mut ViewDef| d.crease_angle = 4.0,
        |d: &mut ViewDef| d.scale = 0.0,
    ] {
        let mut def = plan();
        change(&mut def);
        let error = build_view(&def, &Model::default(), &TOL, &Cancel::new()).expect_err("invalid");
        assert_eq!(error.code(), "view.definition.invalid");
    }
}

#[test]
fn overrides_by_object_win_over_category() {
    let mut model = Model::default();
    room(&mut model);
    let slab = model.add(
        10,
        object(cuboid((-0.5, 5.5), (-0.5, 4.5), (-0.2, 0.0)), "slab", None),
    );
    let red = red();
    let mut def = plan();
    def.overrides.by_category.insert("wall".to_owned(), {
        let mut o = GraphicOverride::default();
        o.visible = Some(false);
        o
    });
    def.overrides.by_category.insert("slab".to_owned(), {
        let mut o = GraphicOverride::default();
        o.color = Some(red.clone());
        o.halftone = Some(true);
        o
    });
    def.overrides.by_object.insert(slab, {
        let mut o = GraphicOverride::default();
        o.halftone = Some(false);
        o.lineweight = Some(Some(0.5));
        o
    });
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    assert_eq!(view.count(VisibilityClass::Cut), 0, "walls are hidden");
    assert_eq!(view.count(VisibilityClass::Beyond), 4);
    for p in &view.list.primitives {
        assert_eq!(p.color.as_ref().map(|c| c.value.rgba), Some(red.rgba));
        assert_eq!(p.lineweight.as_ref().map(|w| w.value), Some(Some(0.5)));
    }
    let mut def = plan();
    def.overrides.by_object.insert(slab, {
        let mut o = GraphicOverride::default();
        o.halftone = Some(true);
        o
    });
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    let beyond = view
        .links
        .iter()
        .position(|l| l.class == VisibilityClass::Beyond)
        .and_then(|i| view.list.primitives.get(i))
        .expect("beyond");
    assert_eq!(
        beyond.color.as_ref().map(|c| c.value.rgba),
        Some(Color::new(0.5, 0.5, 0.5, 1.0))
    );
}

fn red() -> arc_doc::ColorValue {
    arc_doc::ColorValue {
        rgba: Color::new(1.0, 0.0, 0.0, 1.0),
        source: None,
    }
}

fn hatched_room(crop: Option<(f64, f64)>) -> arc_view::ViewResult {
    let library = arc_draft::hatch_pattern::library().expect("library");
    let pattern = library
        .get("diagonal-lines")
        .expect("pattern")
        .lines
        .clone();
    let mut model = Model::default();
    room(&mut model);
    for (_, o) in &mut model.objects {
        o.parts[0].cut_fill = Some(CutFill::pattern(
            pattern.clone(),
            0.05,
            0.0,
            Color::new(1.0, 0.0, 0.0, 1.0),
        ));
    }
    let mut def = plan();
    if let Some((x, y)) = crop {
        def.crop =
            Some(crop_rectangle(Point2::new(x, y), Point2::new(4.0, 3.5), &TOL).expect("crop"));
    }
    build_view(&def, &model, &TOL, &Cancel::new()).expect("view")
}

#[test]
fn fill_patterns_keep_their_origin_when_the_crop_changes() {
    let whole = hatched_room(None);
    write_svg("pattern", &whole);
    let cropped = hatched_room(Some((-0.37, 0.13)));
    let marks = |v: &arc_view::ViewResult| -> Vec<(Point2, Point2)> {
        v.list
            .primitives
            .iter()
            .zip(&v.links)
            .filter(|(_, l)| l.kind == PrimitiveKind::Pattern)
            .filter_map(|(p, _)| match &p.shape {
                Shape::Stroke {
                    curve: arc_geom::Curve2::Segment(s),
                    ..
                } => Some((s.start(), s.end())),
                _ => None,
            })
            .collect()
    };
    let (a, b) = (marks(&whole), marks(&cropped));
    assert!(!a.is_empty() && !b.is_empty());
    for (p, q) in &b {
        let d = q - p;
        let on_a_line = a.iter().any(|(r, _)| {
            let cross = d.x * (r.y - p.y) - d.y * (r.x - p.x);
            cross.abs() / d.norm() < 1e-6
        });
        assert!(on_a_line, "mark {p:?}-{q:?} moved");
    }
    let fill = whole
        .links
        .iter()
        .find(|l| l.kind == PrimitiveKind::Pattern)
        .expect("pattern link");
    assert_eq!(fill.joined.len(), 3);
}

#[test]
fn picking_resolves_to_the_source_and_all_its_primitives() {
    let mut model = Model::default();
    room(&mut model);
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    let first_wall: ObjectId = support::id(1);
    let picked = view
        .links
        .iter()
        .position(|l| l.source == first_wall && l.kind == PrimitiveKind::Line)
        .expect("a wall line");
    let (source, all) = view.select(picked).expect("selection");
    assert_eq!(source, first_wall);
    // Its four boundary pieces (long faces and the two end pieces) and the joined fill.
    assert_eq!(all.len(), 5, "{all:?}");
    let second = view.primitives_of(support::id(2));
    assert!(
        second
            .iter()
            .any(|&i| view.links[i].kind == PrimitiveKind::Fill)
    );
    assert!(view.select(view.links.len()).is_none());
    assert!(
        view.links
            .iter()
            .all(|l| l.sub_key == 0 && (l.face.is_some() || l.kind != PrimitiveKind::Line))
    );
}

#[test]
fn a_grid_of_many_objects_builds_in_bounded_time() {
    let mut model = Model::default();
    model.add(
        0,
        object(
            cuboid((-1.0, 61.0), (-1.0, 61.0), (-0.3, 0.0)),
            "slab",
            None,
        ),
    );
    for i in 0..30_u32 {
        for j in 0..30_u32 {
            let (x, y) = (2.0 * f64::from(i), 2.0 * f64::from(j));
            let n = 1 + u128::from(i * 30 + j);
            let top = if (i + j) % 2 == 0 { 3.0 } else { 0.8 };
            model.add(
                n,
                object(
                    cuboid((x, x + 0.4), (y, y + 0.4), (0.0, top)),
                    "column",
                    None,
                ),
            );
        }
    }
    let start = Instant::now();
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(start.elapsed().as_secs() < 10);
    assert_eq!(view.count(VisibilityClass::Cut), 450 * 4);
    assert_eq!(view.count(VisibilityClass::Beyond), 450 * 4 + 4);
}
