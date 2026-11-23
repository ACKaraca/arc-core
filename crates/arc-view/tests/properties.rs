//! Property tests: rotation about the vertical axis, closed sections of random boxes, and no
//! hidden segment reported visible.

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

use arc_base::math::{Matrix4, Point2, Point3, Transform3};
use arc_geom::Polyhedron;
use arc_view::{PlanRange, PrimitiveKind, ViewDef, VisibilityClass, build_view};
use proptest::prelude::*;
use support::{Model, TOL, cuboid, lines, object, region_area};

fn plan() -> ViewDef {
    ViewDef::plan(PlanRange::new(1.0, -2.0, Some(3.0)))
}

fn rotation(axis: [f64; 3], angle: f64) -> Transform3 {
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let (x, y, z) = (axis[0] / n, axis[1] / n, axis[2] / n);
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    Transform3::from_matrix_unchecked(Matrix4::new(
        t * x * x + c,
        t * x * y - s * z,
        t * x * z + s * y,
        0.0,
        t * x * y + s * z,
        t * y * y + c,
        t * y * z - s * x,
        0.0,
        t * x * z - s * y,
        t * y * z + s * x,
        t * z * z + c,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ))
}

type Boxes = Vec<((f64, f64), (f64, f64), (f64, f64), bool)>;

fn boxes() -> impl Strategy<Value = Boxes> {
    prop::collection::vec(
        (
            -3.0..3.0_f64,
            -3.0..3.0_f64,
            0.3..2.0_f64,
            0.3..2.0_f64,
            -1.5..1.5_f64,
            0.2..1.5_f64,
            any::<bool>(),
        )
            .prop_map(|(x, y, w, d, z, h, join)| ((x, x + w), (y, y + d), (z, z + h), join)),
        1..5,
    )
}

fn model(boxes: &Boxes, transform: Option<&Transform3>) -> Model {
    let mut model = Model::default();
    for (n, &(x, y, z, join)) in (1..).zip(boxes) {
        let mut solid = cuboid(x, y, z);
        if let Some(t) = transform {
            solid = solid
                .transformed(t, &TOL, &arc_base::Limits::DEFAULT)
                .expect("rotate");
        }
        model.add(n, object(solid, "box", join.then_some(1)));
    }
    model
}

fn section_area(solid: &Polyhedron, h: f64) -> f64 {
    // The plane section of a convex solid: the crossing points of its edges, around their
    // centroid.
    let v = solid.vertices();
    let mut points: Vec<Point2> = Vec::new();
    for face in solid.faces() {
        for lp in face.loops() {
            for (i, &a) in lp.iter().enumerate() {
                let b = lp[(i + 1) % lp.len()];
                let (p, q) = (v[a as usize], v[b as usize]);
                if (p.z < h) != (q.z < h) {
                    let t = (h - p.z) / (q.z - p.z);
                    points.push(Point2::new(p.x + t * (q.x - p.x), p.y + t * (q.y - p.y)));
                }
            }
        }
    }
    let n = points.len() as f64;
    let c = points
        .iter()
        .fold(Point2::origin(), |s, p| s + p.coords / n);
    points.sort_by(|a, b| {
        (a.y - c.y)
            .atan2(a.x - c.x)
            .total_cmp(&(b.y - c.y).atan2(b.x - c.x))
    });
    points.dedup_by(|a, b| (*a - *b).norm() < 1e-12);
    (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            a.x * b.y - a.y * b.x
        })
        .sum::<f64>()
        / 2.0
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn rotating_about_the_vertical_rotates_the_drawing(boxes in boxes(), angle in 0.0..std::f64::consts::TAU) {
        let turn = rotation([0.0, 0.0, 1.0], angle);
        let plain = build_view(&plan(), &model(&boxes, None), &TOL, &Cancel::new()).expect("view");
        let turned = build_view(&plan(), &model(&boxes, Some(&turn)), &TOL, &Cancel::new()).expect("view");
        let (s, c) = angle.sin_cos();
        let rotate = |(x, y): (f64, f64)| (c * x - s * y, s * x + c * y);
        for class in [VisibilityClass::Cut, VisibilityClass::Visible, VisibilityClass::Beyond, VisibilityClass::Overhead] {
            let a = lines(&plain, class);
            let b = lines(&turned, class);
            prop_assert_eq!(a.len(), b.len(), "{:?}", class);
            for &(p, q) in &a {
                let (p, q) = (rotate(p), rotate(q));
                let near = |u: (f64, f64), v: (f64, f64)| (u.0 - v.0).abs() < 1e-5 && (u.1 - v.1).abs() < 1e-5;
                prop_assert!(
                    b.iter().any(|&(u, v)| (near(u, p) && near(v, q)) || (near(u, q) && near(v, p))),
                    "{:?} line {:?}-{:?} has no rotated image", class, p, q
                );
            }
        }
        let area = |v: &arc_view::ViewResult| -> f64 {
            v.list.primitives.iter().filter_map(|p| match &p.shape {
                arc_draft::display::Shape::Region(r) => Some(region_area(r)),
                _ => None,
            }).sum()
        };
        prop_assert!((area(&plain) - area(&turned)).abs() < 1e-6);
    }

    #[test]
    fn a_random_box_crossing_the_cut_has_one_closed_region(
        axis in (-1.0..1.0_f64, -1.0..1.0_f64, 0.1..1.0_f64),
        angle in 0.0..std::f64::consts::TAU,
        size in (0.2..2.0_f64, 0.2..2.0_f64, 0.2..2.0_f64),
        shift in -0.4..0.4_f64,
    ) {
        let solid = cuboid((-size.0, size.0), (-size.1, size.1), (-size.2, size.2));
        let turn = rotation([axis.0, axis.1, axis.2], angle);
        let solid = solid.transformed(&turn, &TOL, &arc_base::Limits::DEFAULT).expect("rotate");
        let bounds = solid.bounding_box();
        let h = bounds.min().z + (0.5 + shift) * (bounds.max().z - bounds.min().z);
        let mut model = Model::default();
        model.add(1, object(solid.clone(), "box", None));
        let def = ViewDef::plan(PlanRange::new(h, h - 10.0, None));
        let view = build_view(&def, &model, &TOL, &Cancel::new()).expect("view");
        prop_assert!(view.diagnostics.is_empty(), "{:?}", view.diagnostics);
        let fills: Vec<f64> = view.list.primitives.iter().zip(&view.links)
            .filter(|(_, l)| l.kind == PrimitiveKind::Fill)
            .filter_map(|(p, _)| match &p.shape {
                arc_draft::display::Shape::Region(r) => Some(region_area(r)),
                _ => None,
            })
            .collect();
        prop_assert_eq!(fills.len(), 1);
        let expected = section_area(&solid, h);
        prop_assert!((fills[0] - expected).abs() < 1e-9 * expected.max(1.0), "{} vs {}", fills[0], expected);
    }

    #[test]
    fn hidden_segments_are_never_reported_visible(boxes in boxes()) {
        // All below the cut: every box is beyond.
        let boxes: Boxes = boxes.into_iter().map(|(x, y, z, j)| (x, y, (z.0 - 1.0, z.1 - 1.0), j)).collect();
        let model = model(&boxes, None);
        let view = build_view(&plan(), &model, &TOL, &Cancel::new()).expect("view");
        for (p, l) in view.list.primitives.iter().zip(&view.links) {
            if l.class != VisibilityClass::Beyond { continue; }
            let arc_draft::display::Shape::Stroke { curve: arc_geom::Curve2::Segment(s), .. } = &p.shape else { continue };
            let own = model.objects.iter().position(|o| o.0 == l.source).expect("source");
            let top = boxes[own].2.1;
            for t in [0.1, 0.5, 0.9] {
                let q = s.point_at(t);
                for (other, &(x, y, z, _)) in boxes.iter().enumerate() {
                    if other == own || z.1 <= top + 1e-6 { continue; }
                    let inside = q.x > x.0 + 1e-6 && q.x < x.1 - 1e-6 && q.y > y.0 + 1e-6 && q.y < y.1 - 1e-6;
                    prop_assert!(!inside, "visible point {:?} of box {} under box {}", q, own, other);
                }
            }
        }
    }
}

#[test]
fn rotation_helper_keeps_points_on_the_axis() {
    let t = rotation([0.0, 0.0, 1.0], 1.0);
    let p = t * Point3::new(0.0, 0.0, 2.0);
    assert!((p.z - 2.0).abs() < 1e-12);
}
