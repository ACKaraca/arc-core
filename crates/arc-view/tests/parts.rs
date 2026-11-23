//! Parts with their own fills and join keys, product symbols and stable sub-object keys.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod support;

use arc_base::math::{Point2, Vector2};
use arc_base::{Cancel, Color};
use arc_geom::{Arc2, Curve2, Line2};
use arc_view::{
    CutFill, Geometry, GraphicOverride, Part, PlanRange, PrimitiveKind, SourceObject, Symbol,
    ViewDef, VisibilityClass, build_view, crop_rectangle,
};
use support::{Model, TOL, cuboid, region_area, styles};

fn plan() -> ViewDef {
    ViewDef::plan(PlanRange::new(1.2, -1.0, Some(3.5)))
}

/// A wall from `x0` to `x1` of a 0.2 m concrete core (key 10, join 1) and a 0.02 m finish
/// (key 20, join 2).
fn layered_wall(x0: f64, x1: f64) -> SourceObject {
    SourceObject::new("wall", styles())
        .with_part(
            Part::new(
                10,
                Geometry::Solid(cuboid((x0, x1), (0.0, 0.2), (0.0, 3.0))),
            )
            .with_fill(Some(CutFill::solid(Color::new(0.4, 0.4, 0.4, 1.0))))
            .with_join(Some(1)),
        )
        .with_part(
            Part::new(
                20,
                Geometry::Solid(cuboid((x0, x1), (0.2, 0.22), (0.0, 3.0))),
            )
            .with_fill(Some(CutFill::solid(Color::new(0.9, 0.9, 0.9, 1.0))))
            .with_join(Some(2)),
        )
}

#[test]
fn layers_fill_on_their_own_and_join_by_material_across_objects() {
    let mut model = Model::default();
    let a = model.add(1, layered_wall(0.0, 3.0));
    let b = model.add(2, layered_wall(3.0, 5.0));
    let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
    assert!(view.diagnostics.is_empty(), "{:?}", view.diagnostics);
    let fills: Vec<_> = view
        .list
        .primitives
        .iter()
        .zip(&view.links)
        .filter(|(_, l)| l.kind == PrimitiveKind::Fill)
        .collect();
    assert_eq!(fills.len(), 2, "one fill per material");
    for (p, l) in &fills {
        let arc_draft::display::Shape::Region(r) = &p.shape else {
            panic!("fill shape")
        };
        assert_eq!(l.source, a);
        assert_eq!(l.joined, vec![(b, l.sub_key)]);
        let expected = if l.sub_key == 10 {
            5.0 * 0.2
        } else {
            5.0 * 0.02
        };
        assert!((region_area(r) - expected).abs() < 1e-9);
    }
    // Every line carries the stable key of its part; no seam at x = 3 within a material.
    for l in &view.links {
        assert!(l.sub_key == 10 || l.sub_key == 20);
    }
    let seams = support::lines(&view, VisibilityClass::Cut)
        .into_iter()
        .filter(|((x0, _), (x1, _))| (x0 - 3.0).abs() < 1e-9 && (x1 - 3.0).abs() < 1e-9)
        .count();
    assert_eq!(seams, 0);
}

#[test]
fn symbols_are_cropped_overridden_ordered_and_linked() {
    let swing = Arc2::new(Point2::new(2.0, 0.2), 0.9, 0.0, std::f64::consts::FRAC_PI_2).unwrap();
    let door = SourceObject::new("door", styles())
        .with_symbol(Symbol::new(Curve2::Arc(swing), VisibilityClass::Visible, 7))
        .with_symbol(Symbol::new(
            Curve2::Line(Line2::new(Point2::origin(), Vector2::x()).unwrap()),
            VisibilityClass::Visible,
            8,
        ));
    let mut model = Model::default();
    model.add(
        1,
        support::object(cuboid((0.0, 5.0), (0.0, 0.2), (0.0, 3.0)), "wall", None),
    );
    let id = model.add(2, door);
    let mut def = plan();
    def.crop = Some(crop_rectangle(Point2::new(-1.0, -1.0), Point2::new(2.5, 5.0), &TOL).unwrap());
    let mut over = GraphicOverride::default();
    over.color = Some(arc_doc::ColorValue {
        rgba: Color::new(0.0, 0.0, 1.0, 1.0),
        source: None,
    });
    def.overrides.by_category.insert("door".into(), over);
    let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
    // The unbounded line is refused with a diagnostic; the arc is drawn up to the crop.
    assert_eq!(view.diagnostics.len(), 1);
    assert_eq!(view.diagnostics[0].error.code(), "view.style.invalid");
    let symbols: Vec<usize> = view.primitives_of(id);
    assert_eq!(symbols.len(), 1);
    let (p, l) = (&view.list.primitives[symbols[0]], &view.links[symbols[0]]);
    assert_eq!(
        (l.kind, l.sub_key, l.face),
        (PrimitiveKind::Symbol, 7, None)
    );
    assert_eq!(
        p.color.as_ref().unwrap().value.rgba,
        Color::new(0.0, 0.0, 1.0, 1.0)
    );
    let arc_draft::display::Shape::Stroke { curve, .. } = &p.shape else {
        panic!("stroke")
    };
    let b = curve.bounding_box().unwrap();
    assert!(
        b.max().x <= 2.5 + 1e-9 && b.max().x > 2.4,
        "cropped at x 2.5"
    );
    // Visible symbols draw after the cut lines' predecessors and before cut lines.
    let cut_order = view
        .list
        .primitives
        .iter()
        .zip(&view.links)
        .find(|(_, l)| l.class == VisibilityClass::Cut && l.kind == PrimitiveKind::Line)
        .map(|(p, _)| p.draw_order)
        .unwrap();
    assert!(p.draw_order < cut_order);
}
