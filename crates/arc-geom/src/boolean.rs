//! Union, difference, intersection and exclusive-or of regions (CORE-GEOM-018).
//!
//! [`boolean`] puts the loops of both regions into one planar arrangement (see
//! `docs/crates/arc-geom.md`, section "Region booleans"), each loop curve carrying a winding
//! weight of 1 for its region in its direction (outer loops run counterclockwise and holes
//! clockwise, so the winding number of a region is 1 inside it and 0 outside). The winding
//! numbers of every face follow from the weights of the edges crossed from the outside of its
//! component, and the winding outside a component from the containment of one of its vertices
//! in the loops of the other components. A face belongs to the result when the operation holds
//! for its two memberships; the edges between a member face and a non-member face, oriented
//! with the member on their left, are walked into loops and grouped into a region.
//!
//! - **Exact arcs.** The arrangement never flattens a curve: every edge is a piece of an input
//!   segment, circular arc or elliptical arc, so an output edge on an input arc is that arc
//!   (a circular piece is rebuilt on the chord between its snapped end vertices with the same
//!   sweep, which moves it by at most twice the linear tolerance).
//! - **Coincidence.** Curves of the two regions that overlap within the linear tolerance
//!   (collinear segments, co-circular arcs) are cut at the ends of the overlap; their pieces
//!   have the same end vertices and middles within twice the linear tolerance, and are merged
//!   into one edge with the geometry of the first region's curve and the sum of the weights.
//!   Points within twice the linear tolerance are one vertex, so touching vertices and shared
//!   edges are decided once for both regions.
//! - **Result.** `None` when the result is empty. Loops of the result may touch each other at
//!   isolated vertices (two squares sharing only a corner), where the walk separates them.

use arc_base::{Limits, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::arrange::combine;
use crate::region::Region;

/// A boolean operation on two regions `a` and `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    /// The points in `a` or `b`.
    Union,
    /// The points in `a` and not in `b`.
    Difference,
    /// The points in both `a` and `b`.
    Intersection,
    /// The points in exactly one of `a` and `b`.
    ExclusiveOr,
}

impl BooleanOp {
    fn keeps(self, in_a: bool, in_b: bool) -> bool {
        match self {
            Self::Union => in_a || in_b,
            Self::Difference => in_a && !in_b,
            Self::Intersection => in_a && in_b,
            Self::ExclusiveOr => in_a != in_b,
        }
    }
}

/// The region `a op b`, or `None` when it is empty; see the [module documentation](self).
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance or a zero linear tolerance;
/// `geom.curve.kind_unsupported` for a NURBS curve; `geom.arrangement.too_large` when the
/// curves, candidate pairs, intersections or edges exceed `limits.array_length`;
/// `geom.precision.insufficient` when coordinates are too large for the tolerance or the
/// topology cannot be decided in `f64`; as [`crate::intersect`].
pub fn boolean(
    a: &Region,
    b: &Region,
    op: BooleanOp,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Option<Region>> {
    let mut loops = Vec::new();
    for (operand, region) in [a, b].into_iter().enumerate() {
        for part in region.parts() {
            loops.push((part.outer().clone(), operand));
            loops.extend(part.holes().iter().map(|h| (h.clone(), operand)));
        }
    }
    combine(&loops, |w| op.keeps(w[0] > 0, w[1] > 0), tolerance, limits)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use arc_base::math::{Matrix3, Point2, Transform2};
    use proptest::prelude::*;

    use super::*;
    use crate::curve::Curve2;
    use crate::region::tests::{TOL, circle, polygon, region, square};
    use crate::region::{Containment, RegionPart};

    const LIMITS: Limits = Limits::DEFAULT;

    fn single(lp: crate::region::Loop2) -> Region {
        Region::new(vec![RegionPart::new(lp, vec![])]).unwrap()
    }

    fn area(r: &Option<Region>) -> f64 {
        r.as_ref().map_or(0.0, |r| r.measure(&TOL).unwrap().area)
    }

    fn run(a: &Region, b: &Region, op: BooleanOp) -> Option<Region> {
        boolean(a, b, op, &TOL, &LIMITS).unwrap()
    }

    #[test]
    fn union_of_overlapping_circles_keeps_exact_arcs() {
        let (a, b) = (single(circle(0.0, 0.0, 2.0)), single(circle(2.0, 0.5, 1.5)));
        let u = run(&a, &b, BooleanOp::Union).unwrap();
        assert_eq!(u.parts().len(), 1);
        assert!(u.parts()[0].holes().is_empty());
        for c in u.parts()[0].outer().curves() {
            let Curve2::Arc(arc) = c else {
                panic!("not an arc: {c:?}")
            };
            let r = arc.radius();
            assert!((r - 2.0).abs() < 1e-9 || (r - 1.5).abs() < 1e-9, "{r}");
        }
        let i = run(&a, &b, BooleanOp::Intersection);
        let total = 4.0 * PI + 2.25 * PI;
        assert!((area(&Some(u)) + area(&i) - total).abs() < 1e-9);
    }

    #[test]
    fn shared_edges_touching_vertices_and_coincident_boundaries() {
        let (a, b) = (single(square(0.0, 0.0, 1.0)), single(square(2.0, 0.0, 1.0)));
        // Squares sharing an edge merge into one rectangle.
        let u = run(&a, &b, BooleanOp::Union).unwrap();
        assert_eq!(u.parts().len(), 1);
        assert!((area(&Some(u)) - 8.0).abs() < 1e-12);
        assert!(run(&a, &b, BooleanOp::Intersection).is_none());
        assert!((area(&run(&a, &b, BooleanOp::Difference)) - 4.0).abs() < 1e-12);
        // Squares sharing a corner stay two loops.
        let c = single(square(2.0, 2.0, 1.0));
        let u = run(&a, &c, BooleanOp::Union).unwrap();
        assert_eq!(u.parts().len(), 2);
        // Identical regions.
        assert!(run(&a, &a, BooleanOp::ExclusiveOr).is_none());
        assert!(run(&a, &a, BooleanOp::Difference).is_none());
        assert!((area(&run(&a, &a, BooleanOp::Union)) - 4.0).abs() < 1e-12);
        let d = single(circle(0.0, 0.0, 1.0));
        assert!((area(&run(&d, &d, BooleanOp::Intersection)) - PI).abs() < 1e-12);
        // A hole made by a difference, and an island in it.
        let big = single(square(0.0, 0.0, 5.0));
        let ring = run(&big, &single(square(0.0, 0.0, 2.0)), BooleanOp::Difference).unwrap();
        assert_eq!(ring.parts()[0].holes().len(), 1);
        let island = run(&ring, &d, BooleanOp::Union).unwrap();
        assert_eq!(island.parts().len(), 2);
        assert!((area(&Some(island)) - (100.0 - 16.0 + PI)).abs() < 1e-9);
        // Disjoint.
        let far = single(circle(50.0, 0.0, 1.0));
        assert!(run(&a, &far, BooleanOp::Intersection).is_none());
        assert_eq!(run(&a, &far, BooleanOp::Union).unwrap().parts().len(), 2);
        // Internally and externally tangent circles.
        let (big, small) = (single(circle(0.0, 0.0, 2.0)), single(circle(1.0, 0.0, 1.0)));
        assert!((area(&run(&big, &small, BooleanOp::Union)) - 4.0 * PI).abs() < 1e-9);
        let crescent = run(&big, &small, BooleanOp::Difference).unwrap();
        assert!((area(&Some(crescent)) - 3.0 * PI).abs() < 1e-9);
        let beside = single(circle(3.0, 0.0, 1.0));
        assert_eq!(
            run(&big, &beside, BooleanOp::Union).unwrap().parts().len(),
            2
        );
        assert!(run(&big, &beside, BooleanOp::Intersection).is_none());
        // A circle tangent inside a square.
        let t = run(&a, &d, BooleanOp::Difference).unwrap();
        assert!((area(&Some(t)) - (4.0 - PI)).abs() < 1e-9);
    }

    #[test]
    fn hostile_input_is_rejected() {
        let a = single(square(0.0, 0.0, 1.0));
        let bad = Tolerance { linear: 0.0, ..TOL };
        assert_eq!(
            boolean(&a, &a, BooleanOp::Union, &bad, &LIMITS)
                .unwrap_err()
                .code(),
            "geom.tolerance.invalid"
        );
        let tight = Limits {
            array_length: 3,
            ..LIMITS
        };
        assert_eq!(
            boolean(&a, &a, BooleanOp::Union, &TOL, &tight)
                .unwrap_err()
                .code(),
            "geom.arrangement.too_large"
        );
    }

    fn moved(r: &Region, dx: f64, dy: f64, angle: f64) -> Region {
        let (s, c) = angle.sin_cos();
        let t = Transform2::from_matrix_unchecked(Matrix3::new(c, -s, dx, s, c, dy, 0.0, 0.0, 1.0));
        r.transformed(&t, &TOL).unwrap()
    }

    fn inside(r: &Option<Region>, p: Point2) -> Option<bool> {
        let probe = Tolerance {
            linear: 1e-3,
            ..TOL
        };
        match r {
            None => Some(false),
            Some(r) => match r.classify(p, &probe).unwrap() {
                Containment::Inside => Some(true),
                Containment::Outside => Some(false),
                Containment::OnBoundary => None,
            },
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(crate::arrange::tests::cases(12)))]

        /// Area identities and point membership of random regions with line, arc and
        /// elliptical boundaries, overlapping at random.
        #[test]
        fn booleans_satisfy_area_identities_and_membership(
            a in region(),
            b in region(),
            dx in -12.0..12.0_f64,
            dy in -12.0..12.0_f64,
            angle in -PI..PI,
            points in prop::collection::vec((-20.0..60.0_f64, -20.0..20.0_f64), 40),
        ) {
            let b = moved(&b, dx, dy, angle);
            let [u, d, i, x] = [
                BooleanOp::Union,
                BooleanOp::Difference,
                BooleanOp::Intersection,
                BooleanOp::ExclusiveOr,
            ]
            .map(|op| run(&a, &b, op));
            let (aa, ab) = (area(&Some(a.clone())), area(&Some(b.clone())));
            let (au, ad, ai, ax) = (area(&u), area(&d), area(&i), area(&x));
            let tol = 1e-9 * (aa + ab);
            prop_assert!((au - (aa + ab - ai)).abs() < tol, "{au} {aa} {ab} {ai}");
            prop_assert!((ax - (au - ai)).abs() < tol);
            prop_assert!((ad - (aa - ai)).abs() < tol);
            for (px, py) in points {
                let p = Point2::new(px, py);
                let (Some(ina), Some(inb)) = (inside(&Some(a.clone()), p), inside(&Some(b.clone()), p))
                else {
                    continue;
                };
                for (result, op) in [
                    (&u, BooleanOp::Union),
                    (&d, BooleanOp::Difference),
                    (&i, BooleanOp::Intersection),
                    (&x, BooleanOp::ExclusiveOr),
                ] {
                    if let Some(got) = inside(result, p) {
                        prop_assert_eq!(got, op.keeps(ina, inb), "{:?} at {:?}", op, p);
                    }
                }
            }
        }
    }

    /// A rectangle or circle on a small integer grid, scaled by `s`.
    fn grid_shape() -> impl Strategy<Value = (bool, [i32; 4])> {
        (any::<bool>(), 0..6_i32, 0..6_i32, 1..4_i32, 1..4_i32)
            .prop_map(|(round, x, y, w, h)| (round, [x, y, w, h]))
    }

    fn grid_region((round, [x, y, w, h]): (bool, [i32; 4]), s: f64) -> Region {
        let (x, y, w, h) = (
            f64::from(x) * s,
            f64::from(y) * s,
            f64::from(w) * s,
            f64::from(h) * s,
        );
        single(if round {
            circle(x, y, w)
        } else {
            polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], 0.0)
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(crate::arrange::tests::cases(24)))]

        /// Shapes on an integer grid share edges, vertices and tangencies often; scaled to
        /// extreme magnitudes with the tolerance, the area identities and membership hold.
        #[test]
        fn grid_booleans_hold_at_extreme_magnitudes(
            a in grid_shape(),
            b in grid_shape(),
            c in grid_shape(),
            exponent in -100.0..100.0_f64,
            points in prop::collection::vec((-1.0..9.0_f64, -1.0..9.0_f64), 30),
        ) {
            let s = 10.0_f64.powf(exponent);
            let tolerance = Tolerance { linear: 1e-6 * s, ..TOL };
            let run = |a: &Region, b: &Region, op| boolean(a, b, op, &tolerance, &LIMITS).unwrap();
            // Signed loop areas: the second moments of the measurements overflow above 1e75.
            let area = |r: &Option<Region>| {
                r.as_ref().map_or(0.0, |r| {
                    r.parts()
                        .iter()
                        .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
                        .map(crate::region::Loop2::signed_area)
                        .sum::<f64>()
                })
            };
            // A of two shapes, B of one.
            let (ra, rb) = (grid_region(a, s), grid_region(b, s));
            let a = run(&ra, &rb, BooleanOp::Union).unwrap();
            let b = grid_region(c, s);
            let [u, d, i, x] = [
                BooleanOp::Union,
                BooleanOp::Difference,
                BooleanOp::Intersection,
                BooleanOp::ExclusiveOr,
            ]
            .map(|op| run(&a, &b, op));
            let (aa, ab) = (area(&Some(a.clone())), area(&Some(b.clone())));
            let (au, ad, ai, ax) = (area(&u), area(&d), area(&i), area(&x));
            let tol = 1e-9 * (aa + ab);
            prop_assert!((au - (aa + ab - ai)).abs() < tol, "{au} {aa} {ab} {ai}");
            prop_assert!((ax - (au - ai)).abs() < tol);
            prop_assert!((ad - (aa - ai)).abs() < tol);
            let probe = Tolerance { linear: 1e-3 * s, ..TOL };
            let inside = |r: &Option<Region>, p: Point2| match r {
                None => Some(false),
                Some(r) => match r.classify(p, &probe).unwrap() {
                    Containment::Inside => Some(true),
                    Containment::Outside => Some(false),
                    Containment::OnBoundary => None,
                },
            };
            for (px, py) in points {
                let p = Point2::new(px * s, py * s);
                let (Some(ina), Some(inb)) = (inside(&Some(a.clone()), p), inside(&Some(b.clone()), p))
                else {
                    continue;
                };
                for (result, op) in [
                    (&u, BooleanOp::Union),
                    (&d, BooleanOp::Difference),
                    (&i, BooleanOp::Intersection),
                    (&x, BooleanOp::ExclusiveOr),
                ] {
                    if let Some(got) = inside(result, p) {
                        prop_assert_eq!(got, op.keeps(ina, inb), "{:?} at {:?}", op, p);
                    }
                }
            }
        }
    }
}
