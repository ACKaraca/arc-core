//! Building regions from closed curves (CORE-GEOM-016).
//!
//! [`build_region`] takes closed composites (a single closed curve, such as a circle, an
//! ellipse or a closed polyline, is a composite of one curve) and returns the region they
//! bound:
//!
//! 1. Every composite must close within `tolerance.linear`; otherwise the call fails with
//!    `geom.region.loop_open` and the detail `curves` listing every open one. NURBS curves fail
//!    with `geom.curve.kind_unsupported` and the detail `curves`, because their intersections
//!    are not implemented.
//! 2. The smooth elements of all composites are intersected pairwise (candidate pairs from an
//!    R*-tree of their boxes). Two consecutive elements of one composite may meet only at their
//!    shared joint, within the linear tolerance; any other contact, crossing, touching or
//!    overlap within one composite fails with `geom.region.self_intersecting` and any contact
//!    between two composites with `geom.region.curves_intersect`, each with the detail `curves`
//!    listing the offending composites. Nothing is guessed: touching loops are rejected because
//!    a region's loops neither cross nor touch.
//! 3. Each composite becomes a loop. Its containment depth is the number of other loops that
//!    contain it, decided by classifying a point of it against each loop whose box contains its
//!    box. Loops of even depth are outer loops; a loop of odd depth is a hole of the containing
//!    loop of least area. [`crate::Region::new`] then turns outer loops counterclockwise and
//!    holes clockwise.

use arc_base::{Error, Limits, Result, Tolerance};

use crate::arrange::{
    check_tolerance, explode, index_list, loop_inside, max_count, nest, pairwise, too_large,
};
use crate::curve::Curve2;
use crate::edit::Composite2;
use crate::error::codes;
use crate::index::SpatialIndex2;
use crate::intersect::Intersection;
use crate::region::{Loop2, Region};

/// The region bounded by closed composites; see the [module documentation](self).
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance or a zero linear tolerance;
/// `geom.region.empty` for no composites; `geom.region.loop_open`,
/// `geom.curve.kind_unsupported`, `geom.region.self_intersecting` and
/// `geom.region.curves_intersect` with the detail `curves` as described; as [`Loop2::new`]
/// (with the detail `curve` set to the composite) for a loop of zero area;
/// `geom.arrangement.too_large` when the elements, candidate pairs or intersections exceed
/// `limits.array_length`; `geom.precision.insufficient` when containment cannot be decided.
pub fn build_region(
    curves: &[Composite2],
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Region> {
    check_tolerance(tolerance)?;
    if curves.is_empty() {
        return Err(Error::new(codes::REGION_EMPTY));
    }
    let open: Vec<usize> = curves
        .iter()
        .enumerate()
        .filter(|(_, c)| crate::length(&(c.start_point() - c.end_point())) > tolerance.linear)
        .map(|(i, _)| i)
        .collect();
    if !open.is_empty() {
        return Err(Error::new(codes::REGION_LOOP_OPEN).with_detail("curves", index_list(&open)));
    }
    let nurbs: Vec<usize> = curves
        .iter()
        .enumerate()
        .filter(|(_, c)| c.curves().iter().any(|k| matches!(k, Curve2::Nurbs(_))))
        .map(|(i, _)| i)
        .collect();
    if !nurbs.is_empty() {
        return Err(Error::new(codes::CURVE_KIND_UNSUPPORTED)
            .with_detail("operation", "region")
            .with_detail("curves", index_list(&nurbs)));
    }
    check_contacts(curves, tolerance, limits)?;
    let loops: Vec<Loop2> = curves
        .iter()
        .enumerate()
        .map(|(i, c)| {
            Loop2::new(c.curves().to_vec(), tolerance)
                .map_err(|e| e.with_detail("curve", crate::arrange::detail(i)))
        })
        .collect::<Result<_>>()?;
    let boxes = loops.iter().map(Loop2::bounding_box);
    let index = SpatialIndex2::from_entries(boxes.enumerate())?;
    let mut outers = Vec::new();
    let mut holes = Vec::new();
    for (i, lp) in loops.iter().enumerate() {
        let mut depth = 0_usize;
        for j in index.crossing(&lp.bounding_box())? {
            let Some(other) = loops.get(j).filter(|_| j != i) else {
                continue;
            };
            match loop_inside(lp, other, tolerance)? {
                Some(true) => depth += 1,
                Some(false) => {}
                None => {
                    return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                        .with_detail("operation", "containment"));
                }
            }
        }
        if depth.is_multiple_of(2) {
            outers.push(lp.clone());
        } else {
            holes.push(lp.clone());
        }
    }
    nest(outers, holes, tolerance)?.ok_or_else(|| Error::new(codes::REGION_EMPTY))
}

/// Fails when the composites touch themselves away from their joints or touch each other.
fn check_contacts(curves: &[Composite2], tolerance: &Tolerance, limits: &Limits) -> Result<()> {
    let limit = max_count(limits);
    // The elements with their composite, position in it, and the composite's element count.
    let mut elements = Vec::new();
    let mut owner = Vec::new();
    for (i, composite) in curves.iter().enumerate() {
        let start = elements.len();
        for curve in composite.curves() {
            elements.extend(explode(curve, i)?);
            if elements.len() > limit {
                return Err(too_large("elements"));
            }
        }
        let count = elements.len() - start;
        owner.extend((0..count).map(|k| (i, k, count)));
    }
    let mut selfish = Vec::new();
    let mut crossing = Vec::new();
    pairwise(&elements, tolerance, limits, |i, j, list| {
        let (Some(&(ci, ki, n)), Some(&(cj, kj, _))) = (owner.get(i), owner.get(j)) else {
            return Ok(());
        };
        if ci != cj {
            crossing.push(ci);
            crossing.push(cj);
            return Ok(());
        }
        // The joints the two elements share: where one ends and the next starts.
        let (Some(a), Some(b)) = (elements.get(i), elements.get(j)) else {
            return Ok(());
        };
        let mut joints = Vec::new();
        if (ki + 1) % n == kj {
            joints.extend(a.end_point());
        }
        if (kj + 1) % n == ki {
            joints.extend(b.end_point());
        }
        let allowed = list.iter().all(|x| match x {
            Intersection::Point { point, .. } => joints
                .iter()
                .any(|j| crate::length(&(point - j)) <= 2.0 * tolerance.linear),
            Intersection::Overlap { .. } => false,
        });
        if !allowed {
            selfish.push(ci);
        }
        Ok(())
    })?;
    for (list, code) in [
        (&mut selfish, codes::REGION_SELF_INTERSECTING),
        (&mut crossing, codes::REGION_CURVES_INTERSECT),
    ] {
        list.sort_unstable();
        list.dedup();
        if !list.is_empty() {
            return Err(Error::new(code).with_detail("curves", index_list(list)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use arc_base::math::{Point2, Vector2};
    use proptest::prelude::*;

    use super::*;
    use crate::nurbs::Nurbs2;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::{Circle2, Ellipse2, Segment2};
    use crate::region::Containment;
    use crate::region::tests::TOL;

    const LIMITS: Limits = Limits::DEFAULT;

    fn closed(points: &[(f64, f64)]) -> Composite2 {
        let vertices = points
            .iter()
            .map(|&(x, y)| PolylineVertex::new(Point2::new(x, y), 0.0))
            .collect();
        Composite2::new(
            vec![Curve2::Polyline(Polyline2::new(vertices, true).unwrap())],
            &TOL,
        )
        .unwrap()
    }

    fn rect(c: (f64, f64), h: f64, ccw: bool) -> Composite2 {
        let mut p = vec![
            (c.0 - h, c.1 - h),
            (c.0 + h, c.1 - h),
            (c.0 + h, c.1 + h),
            (c.0 - h, c.1 + h),
        ];
        if !ccw {
            p.reverse();
        }
        closed(&p)
    }

    fn disk(c: (f64, f64), r: f64) -> Composite2 {
        Composite2::new(
            vec![Curve2::Circle(
                Circle2::new(Point2::new(c.0, c.1), r).unwrap(),
            )],
            &TOL,
        )
        .unwrap()
    }

    fn code(r: Result<Region>) -> (&'static str, String) {
        let e = r.unwrap_err();
        let curves = e
            .details()
            .get("curves")
            .map(|v| format!("{v:?}"))
            .unwrap_or_default();
        (e.code(), curves)
    }

    #[test]
    fn nesting_and_orientation() {
        let r = build_region(
            &[
                rect((0.0, 0.0), 1.0, false),
                rect((0.0, 0.0), 5.0, true),
                disk((0.0, 0.0), 3.0),
                rect((20.0, 0.0), 1.0, false),
            ],
            &TOL,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(r.parts().len(), 3);
        let holes: usize = r.parts().iter().map(|p| p.holes().len()).sum();
        assert_eq!(holes, 1);
        for p in r.parts() {
            assert!(p.outer().is_counterclockwise());
            assert!(p.holes().iter().all(|h| !h.is_counterclockwise()));
        }
        let m = r.measure(&TOL).unwrap();
        assert!((m.area - (100.0 - 9.0 * std::f64::consts::PI + 4.0 + 4.0)).abs() < 1e-9);
        // A composite of a segment and an elliptical arc.
        let e = Ellipse2::new(Point2::origin(), Vector2::new(2.0, 0.0), 0.5).unwrap();
        let half = crate::primitives::EllipticalArc2::new(e, 0.0, std::f64::consts::PI).unwrap();
        let comp = Composite2::new(
            vec![
                Curve2::EllipticalArc(half),
                Curve2::Segment(
                    Segment2::new(Point2::new(-2.0, 0.0), Point2::new(2.0, 0.0)).unwrap(),
                ),
            ],
            &TOL,
        )
        .unwrap();
        assert_eq!(
            build_region(&[comp], &TOL, &LIMITS).unwrap().parts().len(),
            1
        );
    }

    #[test]
    fn offending_curves_are_reported() {
        let open = Composite2::new(
            vec![Curve2::Segment(
                Segment2::new(Point2::origin(), Point2::new(1.0, 0.0)).unwrap(),
            )],
            &TOL,
        )
        .unwrap();
        assert_eq!(
            code(build_region(
                &[disk((0.0, 0.0), 1.0), open.clone(), open],
                &TOL,
                &LIMITS
            )),
            ("geom.region.loop_open", "Text(\"1,2\")".to_string())
        );
        let eight = closed(&[(0.0, 0.0), (2.0, 2.0), (2.0, 0.0), (0.0, 2.0)]);
        assert_eq!(
            code(build_region(
                &[disk((10.0, 0.0), 1.0), eight],
                &TOL,
                &LIMITS
            )),
            ("geom.region.self_intersecting", "Text(\"1\")".to_string())
        );
        // A polygon touching itself at a vertex.
        let pinched = closed(&[
            (0.0, 0.0),
            (4.0, 0.0),
            (2.0, 2.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (2.0, 2.0),
        ]);
        assert_eq!(
            code(build_region(&[pinched], &TOL, &LIMITS)).0,
            "geom.region.self_intersecting"
        );
        assert_eq!(
            code(build_region(
                &[
                    disk((0.0, 0.0), 1.0),
                    rect((9.0, 9.0), 1.0, true),
                    disk((1.5, 0.0), 1.0)
                ],
                &TOL,
                &LIMITS
            )),
            ("geom.region.curves_intersect", "Text(\"0,2\")".to_string())
        );
        // Touching from outside counts.
        assert_eq!(
            code(build_region(
                &[disk((0.0, 0.0), 1.0), disk((2.0, 0.0), 1.0)],
                &TOL,
                &LIMITS
            ))
            .0,
            "geom.region.curves_intersect"
        );
        let nurbs = Nurbs2::new(
            2,
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![
                Point2::origin(),
                Point2::new(1.0, 1.0),
                Point2::new(2.0, 0.0),
            ],
            vec![1.0; 3],
        )
        .unwrap();
        let closing =
            Curve2::Segment(Segment2::new(Point2::new(2.0, 0.0), Point2::origin()).unwrap());
        let lens = Composite2::new(vec![Curve2::Nurbs(nurbs), closing], &TOL).unwrap();
        assert_eq!(
            code(build_region(&[lens], &TOL, &LIMITS)),
            ("geom.curve.kind_unsupported", "Text(\"0\")".to_string())
        );
        assert_eq!(
            code(build_region(&[], &TOL, &LIMITS)).0,
            "geom.region.empty"
        );
        let tight = Limits {
            array_length: 2,
            ..LIMITS
        };
        assert_eq!(
            code(build_region(&[rect((0.0, 0.0), 1.0, true)], &TOL, &tight)).0,
            "geom.arrangement.too_large"
        );
    }

    /// Up to four groups of concentric shapes, each shape randomly a circle or a square and of
    /// either orientation, scaled by `s`.
    fn nested(s: f64) -> impl Strategy<Value = Vec<Composite2>> {
        prop::collection::vec(
            (
                0..4_i32,
                0..4_i32,
                prop::collection::vec((any::<bool>(), any::<bool>()), 1..5),
            ),
            1..5,
        )
        .prop_map(move |groups| {
            let mut out = Vec::new();
            let mut cells = std::collections::BTreeSet::new();
            for (i, j, shapes) in groups {
                if !cells.insert((i, j)) {
                    continue;
                }
                let c = (f64::from(i) * 30.0 * s, f64::from(j) * 30.0 * s);
                for ((round, ccw), k) in shapes.iter().zip(0_i32..) {
                    let r = 12.0 * 0.6_f64.powi(k) * s;
                    out.push(if *round {
                        let d = disk(c, r);
                        if *ccw { d } else { d.reversed() }
                    } else {
                        rect(c, r * 0.68, *ccw)
                    });
                }
            }
            out
        })
    }

    fn flat(c: &Composite2, s: f64) -> Vec<Point2> {
        use crate::flatten::CurveFlatten;
        c.curves()
            .iter()
            .flat_map(|k| k.flatten(1e-3 * s, 0.05, 100_000).unwrap())
            .collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(crate::arrange::tests::cases(24)))]

        /// Containment depth and orientation agree with a brute-force point-in-polygon
        /// reference, at extreme magnitudes too; a crossing curve is reported.
        #[test]
        fn construction_matches_brute_force_depth(
            (s, curves) in (-100.0..100.0_f64).prop_flat_map(|e| {
                let s = 10.0_f64.powf(e);
                (Just(s), nested(s))
            }),
            points in prop::collection::vec((-15.0..105.0_f64, -15.0..105.0_f64), 30),
            crossing in any::<prop::sample::Index>(),
        ) {
            let tol = Tolerance { linear: 1e-6 * s, ..TOL };
            let region = build_region(&curves, &tol, &LIMITS).unwrap();
            let loops: usize = region.parts().iter().map(|p| 1 + p.holes().len()).sum();
            prop_assert_eq!(loops, curves.len());
            for p in region.parts() {
                prop_assert!(p.outer().is_counterclockwise());
                prop_assert!(p.holes().iter().all(|h| !h.is_counterclockwise()));
            }
            let polygons: Vec<Vec<Point2>> = curves.iter().map(|c| flat(c, s)).collect();
            for (x, y) in points {
                let q = Point2::new(x * s, y * s);
                let (inside, distance) = crate::region::tests::reference(&polygons, q);
                if distance < 0.05 * s {
                    continue;
                }
                let got = region.classify(q, &tol).unwrap();
                prop_assert_eq!(got == Containment::Inside, inside, "at {:?}", q);
            }
            // A circle crossing one of the curves is reported with it.
            let k = crossing.index(curves.len());
            let first = curves[k].curves()[0].point_at(0.0);
            let mut bad = curves.clone();
            bad.push(disk((first.x, first.y), 0.05 * s));
            let (code, list) = code(build_region(&bad, &tol, &LIMITS));
            prop_assert_eq!(code, "geom.region.curves_intersect");
            prop_assert_eq!(list, format!("Text(\"{},{}\")", k, curves.len()));
        }
    }
}
