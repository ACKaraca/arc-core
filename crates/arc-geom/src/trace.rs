//! Boundary tracing: the region around a picked point (CORE-GEOM-017).
//!
//! [`trace_region`] builds the planar arrangement of arbitrary bounded curves (open or closed,
//! crossing each other; see `docs/crates/arc-geom.md`, section "Boundary tracing") and returns
//! the face that contains the picked point: the bounded face whose outer boundary cycle has the
//! least area among those containing the point. Curves that end freely (dangling ends) and
//! curves that connect two otherwise separate groups lie inside one face and do not bound it.
//! Gaps between curves are never bridged: an end that misses another curve by more than twice
//! the linear tolerance leaves the boundary open.
//!
//! Groups of curves that are not connected to the face's boundary but lie inside it are
//! islands; [`IslandMode`] says what happens to them.

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::arrange::{Arrangement, Input, check_tolerance, explode, max_count, too_large};
use crate::curve::Curve2;
use crate::error::codes;
use crate::region::{Containment, Loop2, Region};

/// What the islands inside the traced face become.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandMode {
    /// Islands are holes; faces nested inside an island are filled again, and so on in
    /// alternation: a face inside the traced face's boundary is filled when it is separated
    /// from the traced face by an even number of curves (the shortest path of faces crossing
    /// curves).
    Alternating,
    /// Islands are holes and nothing inside them is filled: the result is exactly the face that
    /// contains the point.
    Outermost,
    /// Islands are ignored: the result is the whole area inside the face's outer boundary.
    Ignore,
}

/// The region around `pick` bounded by `curves`; see the [module documentation](self).
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite pick point; `geom.tolerance.invalid` for an invalid
/// tolerance or a zero linear tolerance; `geom.curve.unbounded` for a ray or line and
/// `geom.curve.kind_unsupported` for a NURBS curve, with the detail `curve`;
/// `geom.trace.on_boundary` when the point lies within the linear tolerance of a curve that
/// bounds a face containing it; `geom.trace.not_enclosed` when no closed boundary encloses the
/// point; `geom.arrangement.too_large` when the elements, candidate pairs, intersections or
/// edges exceed `limits.array_length`; `geom.precision.insufficient` when the topology cannot
/// be decided in `f64`; as [`crate::intersect`].
pub fn trace_region(
    curves: &[Curve2],
    pick: Point2,
    islands: IslandMode,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Region> {
    crate::ensure_finite(&[pick.x, pick.y])?;
    check_tolerance(tolerance)?;
    let mut inputs = Vec::new();
    for (i, curve) in curves.iter().enumerate() {
        for curve in explode(curve, i)? {
            inputs.push(Input {
                curve,
                weight: [0, 0],
            });
        }
        if inputs.len() > max_count(limits) {
            return Err(too_large("elements"));
        }
    }
    let arrangement = Arrangement::build(&inputs, tolerance, limits)?;
    let faces = arrangement.faces()?;
    let count = faces.cycles.len();
    let mut loops: Vec<Option<Loop2>> = vec![None; count];
    let mut loop_of = |c: usize| -> Result<Option<Loop2>> {
        if let Some(Some(lp)) = loops.get(c) {
            return Ok(Some(lp.clone()));
        }
        let Some(cycle) = faces.cycles.get(c) else {
            return Ok(None);
        };
        let lp = arrangement.cycle_loop(cycle, tolerance)?;
        if let Some(slot) = loops.get_mut(c) {
            *slot = Some(lp.clone());
        }
        Ok(Some(lp))
    };
    // The bounded cycle of least area containing a point, skipping one component.
    let mut locate = |point: Point2, skip: Option<usize>| -> Result<Option<usize>> {
        let mut best: Option<usize> = None;
        for c in 0..count {
            let area = faces.area.get(c).copied().unwrap_or(0.0);
            if !faces.is_bounded(c)
                || skip.is_some_and(|s| faces.component.get(c) == Some(&s))
                || best.is_some_and(|b| faces.area.get(b).copied().unwrap_or(0.0) <= area)
            {
                continue;
            }
            let boxed = faces.cycles.get(c).map(|cy| arrangement.cycle_box(cy));
            if !boxed.is_some_and(|b| b.expanded(tolerance.linear).contains_point(&point)) {
                continue;
            }
            let Some(lp) = loop_of(c)? else { continue };
            match lp.classify(point, tolerance)? {
                Containment::Inside => best = Some(c),
                Containment::OnBoundary => {
                    return Err(Error::new(codes::TRACE_ON_BOUNDARY));
                }
                Containment::Outside => {}
            }
        }
        Ok(best)
    };
    let face = locate(pick, None)?.ok_or_else(|| Error::new(codes::TRACE_NOT_ENCLOSED))?;
    let face_loop = loop_of_cycle(&arrangement, &faces, face, tolerance)?;
    // The face containing each component inside the traced face's outer boundary.
    let members = arrangement.component_vertices();
    let mut parent: Vec<Option<usize>> = vec![None; arrangement.component_count()];
    for (comp, points) in members.iter().enumerate() {
        if faces.component.get(face) == Some(&comp) {
            continue;
        }
        let Some(point) = points.first() else {
            continue;
        };
        if face_loop.classify(*point, tolerance)? != Containment::Inside {
            continue;
        }
        let found = locate(*point, Some(comp)).map_err(|_| {
            Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "trace")
        })?;
        if let Some(slot) = parent.get_mut(comp) {
            *slot = found;
        }
    }
    let filled = filled_cycles(&faces, &parent, face, islands);
    arrangement
        .assemble(&faces, &filled, tolerance)?
        .ok_or_else(|| Error::new(codes::PRECISION_INSUFFICIENT).with_detail("operation", "trace"))
}

fn loop_of_cycle(
    arrangement: &Arrangement,
    faces: &crate::arrange::Faces,
    cycle: usize,
    tolerance: &Tolerance,
) -> Result<Loop2> {
    let cycle = faces
        .cycles
        .get(cycle)
        .ok_or_else(|| Error::new(codes::PRECISION_INSUFFICIENT))?;
    arrangement.cycle_loop(cycle, tolerance)
}

/// Whether the face of each cycle is filled: the faces reached from the traced face `face`
/// without crossing its own boundary, by their depth and `islands`. `parent` is the face
/// containing each component inside the traced face.
fn filled_cycles(
    faces: &crate::arrange::Faces,
    parent: &[Option<usize>],
    face: usize,
    islands: IslandMode,
) -> Vec<bool> {
    let count = faces.cycles.len();
    // The face of each cycle: itself when bounded, else the face containing its component.
    let face_of = |c: usize| -> Option<usize> {
        if faces.is_bounded(c) {
            Some(c)
        } else {
            faces
                .component
                .get(c)
                .and_then(|comp| parent.get(*comp).copied().flatten())
        }
    };
    // Depth of each face reached from the traced face, crossing curves, without leaving it.
    let mut depth: Vec<Option<usize>> = vec![None; count];
    if let Some(d) = depth.get_mut(face) {
        *d = Some(0);
    }
    // The cycles bounding each face from inside: the outer cycles of its child components.
    let mut inner: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (comp, p) in parent.iter().enumerate() {
        if let (Some(p), Some(outer)) = (p, faces.outer.get(comp))
            && let Some(list) = inner.get_mut(*p)
        {
            list.push(*outer);
        }
    }
    let mut queue = std::collections::VecDeque::from([face]);
    while let Some(g) = queue.pop_front() {
        let d = depth.get(g).copied().flatten().unwrap_or(0);
        let own = if g == face { Vec::new() } else { vec![g] };
        let bounding = own
            .iter()
            .chain(inner.get(g).map_or(&[][..], Vec::as_slice));
        for c in bounding {
            for h in faces.cycles.get(*c).map_or(&[][..], Vec::as_slice) {
                let Some(other) = faces.cycle_of.get(h ^ 1).and_then(|t| face_of(*t)) else {
                    continue;
                };
                if let Some(slot @ None) = depth.get_mut(other) {
                    *slot = Some(d + 1);
                    queue.push_back(other);
                }
            }
        }
    }
    let filled: Vec<bool> = (0..count)
        .map(|c| {
            let d = face_of(c).and_then(|f| depth.get(f).copied().flatten());
            match (islands, d) {
                (_, None) => false,
                (IslandMode::Ignore, Some(_)) => true,
                (IslandMode::Outermost, Some(d)) => d == 0,
                (IslandMode::Alternating, Some(d)) => d.is_multiple_of(2),
            }
        })
        .collect();
    filled
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use arc_base::math::Vector2;
    use proptest::prelude::*;

    use super::*;
    use crate::closest::CurveProject;
    use crate::primitives::{Arc2, Circle2, Segment2};
    use crate::region::tests::TOL;

    const LIMITS: Limits = Limits::DEFAULT;

    fn seg(a: (f64, f64), b: (f64, f64)) -> Curve2 {
        Curve2::Segment(Segment2::new(Point2::new(a.0, a.1), Point2::new(b.0, b.1)).unwrap())
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Curve2> {
        vec![
            seg((x0, y0), (x1, y0)),
            seg((x1, y0), (x1, y1)),
            seg((x1, y1), (x0, y1)),
            seg((x0, y1), (x0, y0)),
        ]
    }

    fn area(r: &Region) -> f64 {
        r.measure(&TOL).unwrap().area
    }

    fn trace(curves: &[Curve2], x: f64, y: f64, mode: IslandMode) -> Result<Region> {
        trace_region(curves, Point2::new(x, y), mode, &TOL, &LIMITS)
    }

    #[test]
    fn crossing_lines_dangling_ends_and_open_gaps() {
        // A square cut by both diagonals, which overshoot its corners.
        let mut curves = rect(0.0, 0.0, 4.0, 4.0);
        curves.push(seg((-1.0, -1.0), (5.0, 5.0)));
        curves.push(seg((-1.0, 5.0), (5.0, -1.0)));
        // A dangling segment inside the bottom triangle.
        curves.push(seg((2.0, 0.0), (2.0, 1.0)));
        let r = trace(&curves, 2.5, 0.5, IslandMode::Alternating).unwrap();
        assert!((area(&r) - 4.0).abs() < 1e-12);
        assert_eq!(r.parts().len(), 1);
        assert!(r.parts()[0].holes().is_empty());
        assert_eq!(
            trace(&curves, 6.0, 0.0, IslandMode::Alternating)
                .unwrap_err()
                .code(),
            "geom.trace.not_enclosed"
        );
        assert_eq!(
            trace(&curves, 2.0, 0.5, IslandMode::Alternating)
                .unwrap_err()
                .code(),
            "geom.trace.on_boundary"
        );
        // An open gap of 1e-3 is not bridged.
        let open = vec![
            seg((0.0, 0.0), (4.0, 0.0)),
            seg((4.0, 0.0), (4.0, 4.0)),
            seg((4.0, 4.0), (0.0, 4.0)),
            seg((0.0, 4.0), (0.0, 1e-3)),
        ];
        assert_eq!(
            trace(&open, 2.0, 2.0, IslandMode::Alternating)
                .unwrap_err()
                .code(),
            "geom.trace.not_enclosed"
        );
        // Arcs: a circle cut by a chord gives the circular segment.
        let curves = vec![
            Curve2::Circle(Circle2::new(Point2::origin(), 2.0).unwrap()),
            seg((-3.0, 1.0), (3.0, 1.0)),
        ];
        let r = trace(&curves, 0.0, 1.5, IslandMode::Alternating).unwrap();
        let theta = 2.0 * (1.0_f64 / 2.0).acos();
        assert!((area(&r) - 2.0 * (theta - theta.sin())).abs() < 1e-12);
        assert!(
            r.parts()[0]
                .outer()
                .curves()
                .iter()
                .any(|c| matches!(c, Curve2::Arc(_)))
        );
    }

    #[test]
    fn island_modes() {
        let mut curves = rect(0.0, 0.0, 10.0, 10.0);
        curves.extend(rect(2.0, 2.0, 8.0, 8.0));
        curves.extend(rect(4.0, 4.0, 6.0, 6.0));
        // An island touching nothing, made of two crossing circles.
        curves.push(Curve2::Circle(
            Circle2::new(Point2::new(1.0, 9.0), 0.4).unwrap(),
        ));
        curves.push(Curve2::Circle(
            Circle2::new(Point2::new(1.3, 9.0), 0.4).unwrap(),
        ));
        let lens = {
            let d: f64 = 0.3;
            let r: f64 = 0.4;
            2.0 * r * r * (d / (2.0 * r)).acos() - 0.5 * d * (4.0 * r * r - d * d).sqrt()
        };
        let pair = 2.0 * PI * 0.16 - lens;
        let alt = trace(&curves, 1.0, 1.0, IslandMode::Alternating).unwrap();
        // Inside the double-circle island the lens is two curves deep and filled again.
        assert!(
            (area(&alt) - (100.0 - 36.0 + 4.0 - pair + lens)).abs() < 1e-9,
            "{}",
            area(&alt)
        );
        let outer = trace(&curves, 1.0, 1.0, IslandMode::Outermost).unwrap();
        assert!((area(&outer) - (100.0 - 36.0 - pair)).abs() < 1e-9);
        let all = trace(&curves, 1.0, 1.0, IslandMode::Ignore).unwrap();
        assert!((area(&all) - 100.0).abs() < 1e-12);
        assert_eq!(all.parts().len(), 1);
        // Picked in the middle square: islands of that face only.
        let inner = trace(&curves, 5.0, 5.0, IslandMode::Alternating).unwrap();
        assert!((area(&inner) - 4.0).abs() < 1e-12);
        let ring = trace(&curves, 3.0, 3.0, IslandMode::Alternating).unwrap();
        assert!((area(&ring) - 32.0).abs() < 1e-12);
    }

    #[test]
    fn hostile_input_is_rejected() {
        let curves = rect(0.0, 0.0, 1.0, 1.0);
        assert_eq!(
            trace(&curves, f64::NAN, 0.5, IslandMode::Ignore)
                .unwrap_err()
                .code(),
            "geom.input.not_finite"
        );
        let line = Curve2::Line(
            crate::primitives::Line2::new(Point2::origin(), Vector2::new(1.0, 0.0)).unwrap(),
        );
        assert_eq!(
            trace(&[line], 0.5, 0.5, IslandMode::Ignore)
                .unwrap_err()
                .code(),
            "geom.curve.unbounded"
        );
        let tight = Limits {
            array_length: 5,
            ..LIMITS
        };
        let mut many = curves.clone();
        many.extend(rect(0.2, 0.2, 0.8, 0.8));
        assert_eq!(
            trace_region(
                &many,
                Point2::new(0.1, 0.1),
                IslandMode::Ignore,
                &TOL,
                &tight
            )
            .unwrap_err()
            .code(),
            "geom.arrangement.too_large"
        );
        assert_eq!(
            trace(&[], 0.5, 0.5, IslandMode::Ignore).unwrap_err().code(),
            "geom.trace.not_enclosed"
        );
    }

    /// A random curve inside [-8, 8]²: a segment or an arc.
    fn random_curve() -> impl Strategy<Value = Curve2> {
        prop_oneof![
            (-8.0..8.0_f64, -8.0..8.0_f64, -8.0..8.0_f64, -8.0..8.0_f64)
                .prop_filter("long", |(a, b, c, d)| (a - c).hypot(b - d) > 1.0)
                .prop_map(|(a, b, c, d)| seg((a, b), (c, d))),
            (
                -5.0..5.0_f64,
                -5.0..5.0_f64,
                1.0..4.0_f64,
                -PI..PI,
                0.5..6.3_f64
            )
                .prop_map(|(x, y, r, s, w)| Curve2::Arc(
                    Arc2::new(Point2::new(x, y), r, s, w.min(2.0 * PI)).unwrap()
                )),
        ]
    }

    const GRID: usize = 48;
    const LOW: f64 = -10.0;
    const CELL: f64 = 20.0 / GRID as f64;

    fn cell_center(i: usize, j: usize) -> Point2 {
        Point2::new(LOW + (i as f64 + 0.5) * CELL, LOW + (j as f64 + 0.5) * CELL)
    }

    /// Flood fill of the raster cells not crossed by a curve, from the cell of `pick`; `None`
    /// when it reaches the border.
    fn flood(curves: &[Curve2], pick: Point2) -> (Option<Vec<bool>>, Vec<f64>) {
        let distance: Vec<f64> = (0..GRID * GRID)
            .map(|k| {
                let p = cell_center(k % GRID, k / GRID);
                curves
                    .iter()
                    .map(|c| c.closest_point(p, &TOL).unwrap().distance)
                    .fold(f64::INFINITY, f64::min)
            })
            .collect();
        // A cell is a wall when a curve passes within half its diagonal of its center.
        let wall = |k: usize| distance[k] <= CELL * 0.71;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let to_index = |v: f64| ((v - LOW) / CELL).floor() as usize;
        let start = to_index(pick.y) * GRID + to_index(pick.x);
        let mut seen = vec![false; GRID * GRID];
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(k) = stack.pop() {
            let (i, j) = (k % GRID, k / GRID);
            if i == 0 || j == 0 || i == GRID - 1 || j == GRID - 1 {
                return (None, distance);
            }
            for n in [k - 1, k + 1, k - GRID, k + GRID] {
                if !seen[n] && !wall(n) {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        (Some(seen), distance)
    }

    /// Whether the curves stay clear of near misses the raster cannot resolve: curves that do
    /// not meet, and free ends and the curves they do not reach, are more than three cells
    /// apart.
    fn resolvable(curves: &[Curve2]) -> bool {
        let gap = 3.0 * CELL;
        let distance = |c: &Curve2, p: Point2| c.closest_point(p, &TOL).unwrap().distance;
        for (i, a) in curves.iter().enumerate() {
            let (lo, hi) = a.parameter_range();
            let ends: Vec<Point2> = if a.is_closed() {
                Vec::new()
            } else {
                vec![a.point_at(lo), a.point_at(hi)]
            };
            if let [s, e] = ends.as_slice()
                && (s - e).norm() <= gap
            {
                return false;
            }
            for (j, b) in curves.iter().enumerate() {
                if i == j {
                    continue;
                }
                if ends.iter().any(|e| (1e-9..=gap).contains(&distance(b, *e))) {
                    return false;
                }
                if i < j && crate::intersect::intersect(a, b, &TOL).unwrap().is_empty() {
                    let near = (0..=64)
                        .map(|k| distance(b, a.point_at(lo + (hi - lo) * f64::from(k) / 64.0)))
                        .fold(f64::INFINITY, f64::min);
                    if near <= gap {
                        return false;
                    }
                }
            }
        }
        true
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: crate::arrange::tests::cases(24),
            max_global_rejects: 1 << 20,
            ..ProptestConfig::default()
        })]

        /// The traced face contains the pick point, lies on the input curves, and agrees with
        /// a rasterized flood fill away from the curves.
        #[test]
        fn tracing_agrees_with_a_flood_fill(
            boxed in any::<bool>(),
            mut curves in prop::collection::vec(random_curve(), 2..6),
            pick in (-7.0..7.0_f64, -7.0..7.0_f64),
        ) {
            if boxed {
                curves.extend(rect(-9.0, -9.0, 9.0, 9.0));
            }
            prop_assume!(resolvable(&curves));
            let pick = Point2::new(pick.0, pick.1);
            let traced = trace_region(&curves, pick, IslandMode::Outermost, &TOL, &LIMITS);
            let (filled, distance) = flood(&curves, pick);
            let start = curves.iter().map(|c| c.closest_point(pick, &TOL).unwrap().distance).fold(f64::INFINITY, f64::min);
            prop_assume!(start > CELL);
            match traced {
                Err(e) => {
                    prop_assert_eq!(e.code(), "geom.trace.not_enclosed");
                    if boxed {
                        prop_assert!(false, "a boxed pick is enclosed");
                    }
                }
                Ok(region) => {
                    prop_assert_eq!(region.classify(pick, &TOL).unwrap(), Containment::Inside);
                    let Some(filled) = filled else {
                        return Err(TestCaseError::fail("the flood escaped a closed face"));
                    };
                    for (k, f) in filled.iter().enumerate() {
                        // Cells near curves may be cut off by the walls' thickness.
                        if distance[k] < 2.0 * CELL {
                            continue;
                        }
                        let p = cell_center(k % GRID, k / GRID);
                        let inside = region.classify(p, &TOL).unwrap() == Containment::Inside;
                        prop_assert_eq!(inside, *f, "at {:?}", p);
                    }
                    for lp in region.parts().iter().flat_map(|p| std::iter::once(p.outer()).chain(p.holes())) {
                        for c in lp.curves() {
                            let (lo, hi) = c.parameter_range();
                            for k in 0..=4 {
                                let q = c.point_at(lo + (hi - lo) * f64::from(k) / 4.0);
                                let d = curves.iter().map(|c| c.closest_point(q, &TOL).unwrap().distance).fold(f64::INFINITY, f64::min);
                                prop_assert!(d < 1e-5, "boundary point {:?} is {} off", q, d);
                            }
                        }
                    }
                }
            }
        }
    }
}
