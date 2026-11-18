//! Offsets of regions (CORE-GEOM-019).
//!
//! [`offset_region`] offsets every loop of a region with [`offset_composite`] to the side away
//! from the region for a positive distance (outward) and into it for a negative one: outer loops
//! run counterclockwise and holes clockwise, so this is the right side of every loop, at
//! distance `−distance` in the sign convention of curve offsets. The curve offset removes the
//! loops of each offset curve that come closer to it than the distance and keeps its
//! orientation; its chains close within four times the linear tolerance, slivers are dropped,
//! and a closed loop left as open chains is closed again, each chain followed by the one whose
//! start is nearest to its end. A loop whose offset collapses (an outer loop shrunk past its
//! inradius, a hole grown past it) contributes nothing. The remaining closed chains are combined by their
//! winding numbers in a planar arrangement, keeping the points with a positive winding number:
//! outer offsets that meet merge, a grown hole cuts away what it overlaps, and the result is
//! the set of points of the region at least `|distance|` inside its boundary (inward), or the
//! points within `distance` of the region (outward, with round joins). An inward offset larger
//! than the inradius gives `None`, the empty region.

use arc_base::{Error, Limits, Result, Tolerance};

use crate::arrange::combine;
use crate::curve::Curve2;
use crate::edit::Composite2;
use crate::error::codes;
use crate::offset::{JoinStyle, offset_composite};
use crate::primitives::Segment2;
use crate::region::{Loop2, Region};

/// The gap, in linear tolerances, within which [`offset_composite`] chains its pieces.
const OFFSET_CHAIN_TOLERANCES: f64 = 4.0;

/// The region offset by `distance`, outward when positive and inward when negative, or `None`
/// when nothing remains; see the [module documentation](self). `distance = 0` returns the
/// region.
///
/// # Errors
///
/// `geom.tolerance.invalid` for an invalid tolerance or a zero linear tolerance;
/// `geom.input.not_finite` for a non-finite distance; `geom.offset.join_invalid` for an invalid
/// miter limit; `geom.curve.kind_unsupported` for a loop with an elliptical arc, whose offset is
/// a NURBS curve; `geom.precision.insufficient` when an offset chain does not close; as
/// [`offset_composite`] and [`crate::boolean`].
pub fn offset_region(
    region: &Region,
    distance: f64,
    join: JoinStyle,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Option<Region>> {
    crate::arrange::check_tolerance(tolerance)?;
    crate::ensure_finite(&[distance])?;
    if distance == 0.0 {
        return Ok(Some(region.clone()));
    }
    // Curve offsets chain their pieces within four times the linear tolerance.
    let loose = Tolerance {
        linear: OFFSET_CHAIN_TOLERANCES * tolerance.linear,
        ..*tolerance
    };
    let mut loops = Vec::new();
    for part in region.parts() {
        for lp in std::iter::once(part.outer()).chain(part.holes()) {
            let source = Composite2::new(lp.curves().to_vec(), tolerance)?;
            let chains = match offset_composite(&source, -distance, join, tolerance, limits) {
                Ok(chains) => chains,
                Err(error) if error.code() == codes::OFFSET_COLLAPSED.code => continue,
                Err(error) => return Err(error),
            };
            let mut open = Vec::new();
            for chain in chains {
                let boxed = chain.bounding_box();
                // A sliver left at a corner by the loop removal encloses nothing.
                if crate::length(&(boxed.max() - boxed.min())) <= 2.0 * loose.linear {
                    continue;
                }
                if chain.is_closed() {
                    push_loop(&mut loops, chain.curves().to_vec(), &loose)?;
                } else {
                    open.push(chain.curves().to_vec());
                }
            }
            for curves in close_chains(open, distance.abs())? {
                push_loop(&mut loops, curves, &loose)?;
            }
        }
    }
    combine(&loops, |w| w[0] > 0, tolerance, limits)
}

/// Adds the loop of `curves`, closing gaps between consecutive curves with segments; a loop of
/// no area is skipped.
fn push_loop(
    loops: &mut Vec<(Loop2, usize)>,
    curves: Vec<Curve2>,
    tolerance: &Tolerance,
) -> Result<()> {
    let mut joined: Vec<Curve2> = Vec::with_capacity(curves.len());
    let count = curves.len();
    for (k, curve) in curves.iter().enumerate() {
        joined.push(curve.clone());
        let next = curves.get((k + 1) % count).and_then(Curve2::start_point);
        if let (Some(end), Some(start)) = (curve.end_point(), next)
            && crate::length(&(start - end)) > tolerance.linear
        {
            joined.push(Curve2::Segment(Segment2::new(end, start)?));
        }
    }
    match Loop2::new(joined, tolerance) {
        Ok(lp) => loops.push((lp, 0)),
        Err(error) if error.code() == codes::REGION_LOOP_ZERO_AREA.code => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Closes open offset chains into loops, each chain followed by the chain whose start is
/// nearest to its end. Where the loop removal of the curve offset leaves the ends of two chains
/// overshooting their crossing by a little (the band of the linear tolerance along a shallow
/// crossing), the joint between them makes a small reversed lobe, whose winding number is not
/// positive, so the arrangement removes it.
///
/// # Errors
///
/// `geom.precision.insufficient` when the nearest start is farther than `reach`.
fn close_chains(open: Vec<Vec<Curve2>>, reach: f64) -> Result<Vec<Vec<Curve2>>> {
    let start = |c: &Vec<Curve2>| c.first().and_then(Curve2::start_point);
    let end = |c: &Vec<Curve2>| c.last().and_then(Curve2::end_point);
    let mut used = vec![false; open.len()];
    let mut out = Vec::new();
    for seed in 0..open.len() {
        if used.get(seed).copied().unwrap_or(true) {
            continue;
        }
        let mut curves = Vec::new();
        let mut current = seed;
        // Each chain is used once, so this ends within the number of chains.
        for _ in 0..open.len() {
            if let Some(u) = used.get_mut(current) {
                *u = true;
            }
            let Some(chain) = open.get(current) else {
                break;
            };
            curves.extend(chain.iter().cloned());
            let tip = end(chain);
            let nearest = (0..open.len())
                .filter(|j| *j == seed || !used.get(*j).copied().unwrap_or(true))
                .filter_map(|j| {
                    let gap = crate::length(&(start(open.get(j)?)? - tip?));
                    Some((gap, j))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            match nearest {
                Some((gap, j)) if gap <= reach => {
                    if j == seed {
                        break;
                    }
                    current = j;
                }
                _ => {
                    return Err(Error::new(codes::PRECISION_INSUFFICIENT)
                        .with_detail("operation", "offset"));
                }
            }
        }
        out.push(curves);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use arc_base::math::{Point2, Vector2};
    use proptest::prelude::*;

    use super::*;
    use crate::closest::CurveProject;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::Circle2;
    use crate::region::RegionPart;
    use crate::region::tests::{TOL, circle, ellipse, square};

    const LIMITS: Limits = Limits::DEFAULT;

    fn single(lp: Loop2) -> Region {
        Region::new(vec![RegionPart::new(lp, vec![])]).unwrap()
    }

    fn area(r: &Option<Region>) -> f64 {
        r.as_ref().map_or(0.0, |r| r.measure(&TOL).unwrap().area)
    }

    fn run(r: &Region, d: f64, join: JoinStyle) -> Option<Region> {
        offset_region(r, d, join, &TOL, &LIMITS).unwrap()
    }

    #[test]
    fn squares_with_every_join_style() {
        let (s, d) = (2.0, 0.5);
        let sq = single(square(0.0, 0.0, s / 2.0));
        let round = run(&sq, d, JoinStyle::Round);
        assert!((area(&round) - (s * s + 4.0 * s * d + PI * d * d)).abs() < 1e-9);
        let miter = run(&sq, d, JoinStyle::Miter { limit: 2.0 });
        assert!((area(&miter) - (s + 2.0 * d).powi(2)).abs() < 1e-9);
        // A miter limit below √2 bevels the corners.
        let beveled = run(&sq, d, JoinStyle::Miter { limit: 1.2 });
        assert!((area(&beveled) - ((s + 2.0 * d).powi(2) - 2.0 * d * d)).abs() < 1e-9);
        let squared = run(&sq, d, JoinStyle::Square);
        let cut = (2.0_f64.sqrt() - 1.0).powi(2) * d * d;
        assert!((area(&squared) - ((s + 2.0 * d).powi(2) - 4.0 * cut)).abs() < 1e-9);
        let inward = run(&sq, -d, JoinStyle::Round);
        assert!((area(&inward) - (s - 2.0 * d).powi(2)).abs() < 1e-9);
        // Past the inradius nothing remains.
        assert!(run(&sq, -1.0, JoinStyle::Round).is_none());
        assert!(run(&sq, -5.0, JoinStyle::Miter { limit: 4.0 }).is_none());
        assert_eq!(run(&sq, 0.0, JoinStyle::Round), Some(sq.clone()));
    }

    #[test]
    fn loops_merge_and_vanish() {
        // Two squares 1 apart merge when offset by more than ½.
        let two = Region::new(vec![
            RegionPart::new(square(0.0, 0.0, 1.0), vec![]),
            RegionPart::new(square(3.0, 0.0, 1.0), vec![]),
        ])
        .unwrap();
        assert_eq!(run(&two, 0.4, JoinStyle::Round).unwrap().parts().len(), 2);
        let merged = run(&two, 0.6, JoinStyle::Round).unwrap();
        assert_eq!(merged.parts().len(), 1);
        // A hole of radius 1 closes when offset outward by more than 1.
        let ring = Region::new(vec![RegionPart::new(
            square(0.0, 0.0, 5.0),
            vec![circle(0.0, 0.0, 1.0)],
        )])
        .unwrap();
        let grown = run(&ring, 0.5, JoinStyle::Round).unwrap();
        assert_eq!(grown.parts()[0].holes().len(), 1);
        assert!((area(&Some(grown)) - (11.0 * 11.0 - (4.0 - PI) * 0.25 - PI * 0.25)).abs() < 1e-9);
        let closed = run(&ring, 1.5, JoinStyle::Round).unwrap();
        assert!(closed.parts()[0].holes().is_empty());
        // Inward, a grown hole meets the shrinking outer loop and cuts it.
        let thin = Region::new(vec![RegionPart::new(
            square(0.0, 0.0, 2.0),
            vec![circle(1.0, 0.0, 0.5)],
        )])
        .unwrap();
        let cut = run(&thin, -0.4, JoinStyle::Round).unwrap();
        assert!(cut.parts()[0].holes().is_empty());
        // An annulus offset inward by more than half its width vanishes.
        let annulus = Region::new(vec![RegionPart::new(
            circle(0.0, 0.0, 2.0),
            vec![circle(0.0, 0.0, 1.0)],
        )])
        .unwrap();
        assert!(run(&annulus, -0.6, JoinStyle::Round).is_none());
        assert!(
            (area(&run(&annulus, -0.25, JoinStyle::Round)) - PI * (1.75 * 1.75 - 1.25 * 1.25))
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn rejected_input() {
        let sq = single(square(0.0, 0.0, 1.0));
        assert_eq!(
            offset_region(&sq, f64::NAN, JoinStyle::Round, &TOL, &LIMITS)
                .unwrap_err()
                .code(),
            "geom.input.not_finite"
        );
        assert_eq!(
            offset_region(&sq, 1.0, JoinStyle::Miter { limit: 0.5 }, &TOL, &LIMITS)
                .unwrap_err()
                .code(),
            "geom.offset.join_invalid"
        );
        let oval = single(ellipse(Point2::origin(), Vector2::new(2.0, 0.0), 0.5));
        assert_eq!(
            offset_region(&oval, 0.1, JoinStyle::Round, &TOL, &LIMITS)
                .unwrap_err()
                .code(),
            "geom.curve.kind_unsupported"
        );
    }

    /// A star-shaped bulge polygon around the origin, scaled by `s`.
    fn star(s: f64) -> impl Strategy<Value = Loop2> {
        (
            4_usize..9,
            prop::collection::vec((-0.2..0.2_f64, 2.0..5.0_f64, -0.5..0.5_f64), 9),
        )
            .prop_map(move |(n, v)| {
                let vertices = v
                    .iter()
                    .take(n)
                    .enumerate()
                    .map(|(k, (jitter, radius, bulge))| {
                        let angle = TAU * k as f64 / n as f64 + jitter;
                        PolylineVertex::new(
                            Point2::new(angle.cos(), angle.sin()) * (*radius * s),
                            *bulge,
                        )
                    })
                    .collect();
                Loop2::new(
                    vec![Curve2::Polyline(Polyline2::new(vertices, true).unwrap())],
                    &scaled(s),
                )
                .unwrap()
            })
            .prop_filter("simple", |lp| lp.is_counterclockwise())
    }

    fn scaled(s: f64) -> Tolerance {
        Tolerance {
            linear: 1e-6 * s,
            ..TOL
        }
    }

    fn distance_to(region: &Region, p: Point2, tolerance: &Tolerance) -> f64 {
        region
            .parts()
            .iter()
            .flat_map(|q| std::iter::once(q.outer()).chain(q.holes()))
            .flat_map(|lp| lp.curves())
            .map(|c| c.closest_point(p, tolerance).unwrap().distance)
            .fold(f64::INFINITY, f64::min)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(crate::arrange::tests::cases(12)))]

        /// Every boundary point of a round offset lies at the offset distance from the source
        /// boundary, and the area grows with the distance.
        #[test]
        fn offsets_keep_their_distance_and_order_areas(
            (s, outer) in (-50.0..50.0_f64).prop_flat_map(|e| {
                let s = 10.0_f64.powf(e);
                (Just(s), star(s))
            }),
            hole in prop::option::of(0.2..0.8_f64),
            d in prop::collection::vec(-2.5..2.5_f64, 3),
        ) {
            let tol = scaled(s);
            let holes = hole
                .map(|r| {
                    Loop2::new(
                        vec![Curve2::Circle(Circle2::new(Point2::new(0.3 * s, 0.0), r * s).unwrap())],
                        &tol,
                    )
                    .unwrap()
                })
                .into_iter()
                .collect();
            let Ok(source) = crate::region_build::build_region(
                &[outer, ]
                    .into_iter()
                    .chain::<Vec<Loop2>>(holes)
                    .map(|lp| crate::edit::Composite2::new(lp.curves().to_vec(), &tol).unwrap())
                    .collect::<Vec<_>>(),
                &tol,
                &LIMITS,
            ) else {
                return Err(TestCaseError::reject("self-intersecting star"));
            };
            let mut d: Vec<f64> = d.iter().map(|d| d * s).collect();
            d.sort_by(f64::total_cmp);
            let mut previous = 0.0;
            for (k, distance) in d.iter().enumerate() {
                let result = offset_region(&source, *distance, JoinStyle::Round, &tol, &LIMITS).unwrap();
                let a = result.as_ref().map_or(0.0, |r| {
                    r.parts()
                        .iter()
                        .flat_map(|p| std::iter::once(p.outer()).chain(p.holes()))
                        .map(Loop2::signed_area)
                        .sum::<f64>()
                });
                if k > 0 {
                    prop_assert!(a >= previous - 1e-9 * s * s, "area {} after {}", a, previous);
                }
                previous = a;
                let Some(result) = result else { continue };
                for lp in result.parts().iter().flat_map(|p| std::iter::once(p.outer()).chain(p.holes())) {
                    for c in lp.curves() {
                        let (lo, hi) = c.parameter_range();
                        for j in 0..=6 {
                            let p = c.point_at(lo + (hi - lo) * f64::from(j) / 6.0);
                            let off = distance_to(&source, p, &tol);
                            prop_assert!((off - distance.abs()).abs() < 1e-5 * s, "{} vs {}", off, distance);
                        }
                    }
                }
            }
        }
    }
}
