//! Planar polygon triangulation with holes (CORE-IO-015).
//!
//! The triangulation keeps the input vertices: every triangle corner is one of the loop points, and
//! every loop point is a corner of at least one triangle. Neighbouring faces and caps that share an
//! edge therefore share its vertices. This is why the module does not use the flattening
//! triangulator of `arc-geom`, which drops segments shorter than its chord tolerance and would open
//! cracks between the faces of a closed shell.
//!
//! [`prepare`] removes repeated consecutive points and the closing repeat of a loop, and orients the
//! outer loop counterclockwise and every hole clockwise, so the material is on the left of each edge.
//! Collinear middle points are removed only when the caller asks for it (extrusion profiles); a face
//! keeps them, so that a neighbouring face that keeps the same point still meets it.
//!
//! [`triangulate`] refuses a region that is not valid: every hole vertex must lie strictly inside the
//! outer loop and in no other hole, and no edge of one loop may touch an edge of another loop. It then
//! bridges the holes to the outer loop in order of decreasing maximum x, each from its rightmost
//! vertex, and clips ears. A ring of `n` points gives `n - 2` counterclockwise triangles, which the
//! output is checked against. Not checked: a loop that crosses itself, which is not detected.

use arc_base::Result;

use super::budget::Budget;

/// A 2D point.
pub(crate) type P2 = [f64; 2];

/// Bytes charged for one point of a triangulated ring: its 2D copy, its 3D copy and its index.
pub(crate) const RING_POINT_BYTES: u64 = 48;
/// Bytes charged for one triangle of a triangulation.
pub(crate) const TRIANGLE_OUTPUT_BYTES: u64 = 48;

/// The point at `index`, or NaN for an index outside the points (never reached by valid loops).
fn at(points: &[P2], index: usize) -> P2 {
    points.get(index).copied().unwrap_or([f64::NAN, f64::NAN])
}

/// The point index at position `k` of a ring, or 0 outside the ring.
fn index_at(ring: &[usize], k: usize) -> usize {
    ring.get(k).copied().unwrap_or(0)
}

/// The point at position `k` of a ring, wrapped around its length.
fn vertex(points: &[P2], ring: &[usize], k: usize) -> P2 {
    let n = ring.len().max(1);
    at(points, index_at(ring, k % n))
}

/// The cross product of `a - o` and `b - o`: positive for a counterclockwise turn.
fn cross(o: P2, a: P2, b: P2) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// The signed area of a loop (positive when counterclockwise).
fn signed_area(points: &[P2], ring: &[usize]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|k| {
            let a = vertex(points, ring, k);
            let b = vertex(points, ring, k + 1);
            (a[0] * b[1] - b[0] * a[1]) / 2.0
        })
        .sum()
}

/// Whether a point is inside the triangle `a, b, c` or on its boundary, for either orientation.
fn inside(p: P2, a: P2, b: P2, c: P2) -> bool {
    let d1 = cross(a, b, p);
    let d2 = cross(b, c, p);
    let d3 = cross(c, a, p);
    let negative = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let positive = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(negative && positive)
}

/// Whether `r` lies on the closed segment `p, q`, given that the three points are collinear.
fn within(p: P2, q: P2, r: P2) -> bool {
    r[0] >= p[0].min(q[0])
        && r[0] <= p[0].max(q[0])
        && r[1] >= p[1].min(q[1])
        && r[1] <= p[1].max(q[1])
}

/// Whether two numbers have strictly opposite signs.
fn opposite(x: f64, y: f64) -> bool {
    (x > 0.0 && y < 0.0) || (x < 0.0 && y > 0.0)
}

/// Whether the closed segments `a, b` and `c, d` share a point.
fn touch(a: P2, b: P2, c: P2, d: P2) -> bool {
    let d1 = cross(c, d, a);
    let d2 = cross(c, d, b);
    let d3 = cross(a, b, c);
    let d4 = cross(a, b, d);
    (opposite(d1, d2) && opposite(d3, d4))
        || (d1 == 0.0 && within(c, d, a))
        || (d2 == 0.0 && within(c, d, b))
        || (d3 == 0.0 && within(a, b, c))
        || (d4 == 0.0 && within(a, b, d))
}

/// Where a point lies relative to a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Inside,
    Boundary,
    Outside,
}

/// Whether a point is strictly inside the loop, on its boundary or outside it, by the crossings of
/// a ray in +x (exact comparisons).
fn locate(points: &[P2], ring: &[usize], p: P2) -> Side {
    let mut inside = false;
    for k in 0..ring.len() {
        let a = vertex(points, ring, k);
        let b = vertex(points, ring, k + 1);
        if cross(a, b, p) == 0.0 && within(a, b, p) {
            return Side::Boundary;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    if inside { Side::Inside } else { Side::Outside }
}

/// Removes repeated consecutive points (cyclically), the closing repeat of a loop and, when
/// `drop_collinear` is set, the collinear middle points.
fn simplify(points: &[P2], ring: &[usize], drop_collinear: bool) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(ring.len());
    for &index in ring {
        if out
            .last()
            .is_none_or(|&last| at(points, last) != at(points, index))
        {
            out.push(index);
        }
    }
    while out.len() > 1
        && at(points, index_at(&out, 0)) == at(points, index_at(&out, out.len() - 1))
    {
        out.pop();
    }
    if !drop_collinear {
        return out;
    }
    while out.len() >= 3 {
        let n = out.len();
        let collinear = (0..n).find(|&k| {
            let prev = vertex(points, &out, k + n - 1);
            let next = vertex(points, &out, k + 1);
            cross(prev, vertex(points, &out, k), next) == 0.0
        });
        match collinear {
            Some(k) => {
                out.remove(k);
            }
            None => break,
        }
    }
    out
}

/// Prepares loops for [`triangulate`]: the first loop is the outer boundary and is oriented
/// counterclockwise, every other loop is a hole and is oriented clockwise. Returns `None` for a loop
/// with fewer than three points or with zero area. `drop_collinear` removes the collinear middle
/// points; a face keeps them so that its edges meet those of its neighbours.
pub(crate) fn prepare(
    points: &[P2],
    loops: Vec<Vec<usize>>,
    drop_collinear: bool,
) -> Option<Vec<Vec<usize>>> {
    let mut out = Vec::with_capacity(loops.len());
    for (k, ring) in loops.iter().enumerate() {
        let mut ring = simplify(points, ring, drop_collinear);
        if ring.len() < 3 {
            return None;
        }
        let area = signed_area(points, &ring);
        if area == 0.0 || !area.is_finite() {
            return None;
        }
        if (area > 0.0) != (k == 0) {
            ring.reverse();
        }
        out.push(ring);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// The number of points a triangulation of loops of these lengths works on: every loop point and
/// two copies of the bridge point of each hole. `None` when it overflows.
pub(crate) fn ring_vertices(lengths: &[usize]) -> Option<usize> {
    let holes = lengths.len().saturating_sub(1);
    lengths
        .iter()
        .try_fold(0_usize, |sum, &n| sum.checked_add(n))?
        .checked_add(holes.checked_mul(2)?)
}

/// Charges the copies of the points of a ring of `ring` points and the triangles of its
/// triangulation, before the triangulation is made.
pub(crate) fn charge_triangulation(budget: &mut Budget, ring: usize) -> Result<()> {
    let points = u64::try_from(ring).unwrap_or(u64::MAX);
    let triangles = u64::try_from(ring.saturating_sub(2)).unwrap_or(u64::MAX);
    budget.charge(
        points
            .saturating_mul(RING_POINT_BYTES)
            .saturating_add(triangles.saturating_mul(TRIANGLE_OUTPUT_BYTES)),
    )
}

/// The index within a hole of its vertex with the largest x (the lowest y among ties).
fn hole_start(points: &[P2], hole: &[usize]) -> usize {
    let mut best = 0;
    for (k, &index) in hole.iter().enumerate() {
        let p = at(points, index);
        let q = at(points, index_at(hole, best));
        if p[0] > q[0] || (p[0] == q[0] && p[1] < q[1]) {
            best = k;
        }
    }
    best
}

/// The largest x of a loop.
fn max_x(points: &[P2], ring: &[usize]) -> f64 {
    ring.iter()
        .map(|&index| at(points, index)[0])
        .fold(f64::NEG_INFINITY, f64::max)
}

/// The position in `ring` of the vertex a hole is bridged to: the endpoint of the nearest edge that
/// the ray from the hole's start, cast in +x, crosses, or a vertex inside the triangle this forms
/// with the ray, the one at the smallest angle to the ray.
fn bridge(points: &[P2], ring: &[usize], hole: &[usize]) -> Option<usize> {
    let m = at(points, *hole.get(hole_start(points, hole))?);
    let n = ring.len();
    let mut best: Option<(usize, f64)> = None;
    for i in 0..n {
        let a = vertex(points, ring, i);
        let b = vertex(points, ring, i + 1);
        if (a[1] > m[1]) == (b[1] > m[1]) {
            continue;
        }
        let x = a[0] + (m[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
        if x >= m[0] && best.is_none_or(|(_, bx)| x < bx) {
            let endpoint = if a[0] > b[0] { i } else { (i + 1) % n };
            best = Some((endpoint, x));
        }
    }
    let (position, x) = best?;
    let p = vertex(points, ring, position);
    let corner = [x, m[1]];
    let mut choice = position;
    let mut best_angle = f64::INFINITY;
    let mut best_distance = f64::INFINITY;
    for (j, &index) in ring.iter().enumerate() {
        let c = at(points, index);
        if j == position || c[0] < m[0] || !inside(c, m, corner, p) {
            continue;
        }
        let angle = (c[1] - m[1]).abs().atan2(c[0] - m[0]);
        let distance = (c[0] - m[0]).hypot(c[1] - m[1]);
        if angle < best_angle || (angle == best_angle && distance < best_distance) {
            choice = j;
            best_angle = angle;
            best_distance = distance;
        }
    }
    Some(choice)
}

/// The ring with the hole spliced in at `position`: the outer walk to the bridge, the hole walked
/// once from its start and back to it, then the bridge point again.
fn merge(ring: &[usize], position: usize, hole: &[usize], start: usize) -> Vec<usize> {
    let h = hole.len();
    let mut out = Vec::with_capacity(ring.len() + h + 2);
    out.extend_from_slice(ring.get(..=position).unwrap_or(&[]));
    for k in 0..=h {
        out.push(index_at(hole, (start + k) % h));
    }
    out.push(index_at(ring, position));
    out.extend_from_slice(ring.get(position + 1..).unwrap_or(&[]));
    out
}

/// Whether a point of the ring other than the ear's corners lies in the ear `a, b, c`.
fn blocked(points: &[P2], ring: &[usize], corners: [usize; 3]) -> bool {
    let [a, b, c] = corners.map(|k| vertex(points, ring, k));
    ring.iter().enumerate().any(|(j, &index)| {
        if corners.contains(&j) {
            return false;
        }
        let x = at(points, index);
        x != a && x != b && x != c && inside(x, a, b, c)
    })
}

/// Whether the holes are valid in the outer loop: every hole vertex is strictly inside the outer
/// loop and in no other hole, and no edge of one loop touches an edge of another. The edge test is
/// quadratic in the vertex count, and it steps the budget per edge.
fn holes_valid(points: &[P2], loops: &[Vec<usize>], budget: &mut Budget) -> Result<bool> {
    let Some((outer, holes)) = loops.split_first() else {
        return Ok(false);
    };
    for (h, hole) in holes.iter().enumerate() {
        for &index in hole {
            budget.step()?;
            let p = at(points, index);
            if locate(points, outer, p) != Side::Inside {
                return Ok(false);
            }
            for (g, other) in holes.iter().enumerate() {
                if g != h && locate(points, other, p) != Side::Outside {
                    return Ok(false);
                }
            }
        }
    }
    for (i, a) in loops.iter().enumerate() {
        for b in loops.iter().skip(i + 1) {
            for k in 0..a.len() {
                budget.step()?;
                let (p, q) = (vertex(points, a, k), vertex(points, a, k + 1));
                for l in 0..b.len() {
                    if touch(p, q, vertex(points, b, l), vertex(points, b, l + 1)) {
                        return Ok(false);
                    }
                }
            }
        }
    }
    Ok(true)
}

/// Triangulates the prepared loops (see [`prepare`]): the outer loop with its holes. Returns the
/// triangles as triples of point indices, counterclockwise seen from the normal of the outer loop,
/// or `None` when the loops are not a valid region (see the module documentation). Every loop point
/// is a corner of some triangle. The caller charges the memory of the triangulation with
/// [`charge_triangulation`] before the call.
///
/// # Errors
///
/// `base.cancelled` when the token was cancelled during the search.
pub(crate) fn triangulate(
    points: &[P2],
    loops: &[Vec<usize>],
    budget: &mut Budget,
) -> Result<Option<Vec<[usize; 3]>>> {
    let Some((outer, holes)) = loops.split_first() else {
        return Ok(None);
    };
    if loops.iter().any(|l| l.len() < 3) || !holes_valid(points, loops, budget)? {
        return Ok(None);
    }
    let mut ring = outer.clone();
    let mut order: Vec<&Vec<usize>> = holes.iter().collect();
    order.sort_by(|a, b| max_x(points, b).total_cmp(&max_x(points, a)));
    for hole in order {
        budget.step()?;
        let Some(position) = bridge(points, &ring, hole) else {
            return Ok(None);
        };
        ring = merge(&ring, position, hole, hole_start(points, hole));
    }
    let n = ring.len();
    let mut triangles: Vec<[usize; 3]> = Vec::with_capacity(n.saturating_sub(2));
    let mut i = 0;
    let mut stalled = 0_usize;
    while ring.len() > 3 {
        budget.step()?;
        let m = ring.len();
        let p = (i + m - 1) % m;
        let q = (i + 1) % m;
        let (ip, ii, iq) = (index_at(&ring, p), index_at(&ring, i), index_at(&ring, q));
        let turn = cross(at(points, ip), at(points, ii), at(points, iq));
        if turn > 0.0 && !blocked(points, &ring, [p, i, q]) {
            triangles.push([ip, ii, iq]);
            if i < ring.len() {
                ring.remove(i);
            }
            i = if i == 0 {
                ring.len() - 1
            } else {
                i.saturating_sub(1)
            };
            stalled = 0;
        } else {
            i = (i + 1) % m;
            stalled += 1;
            if stalled > m {
                return Ok(None);
            }
        }
    }
    if ring.len() != 3 {
        return Ok(None);
    }
    triangles.push([index_at(&ring, 0), index_at(&ring, 1), index_at(&ring, 2)]);
    // The output check: n - 2 triangles, each counterclockwise (positive area) in the polygon's
    // orientation.
    let positive = triangles
        .iter()
        .all(|t| cross(at(points, t[0]), at(points, t[1]), at(points, t[2])) > 0.0);
    if triangles.len() + 2 != n || !positive {
        return Ok(None);
    }
    Ok(Some(triangles))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Vec<P2> {
        vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]]
    }

    fn run(points: &[P2], loops: Vec<Vec<usize>>) -> Option<Vec<[usize; 3]>> {
        let prepared = prepare(points, loops, true)?;
        let mut budget = Budget::new(&arc_base::Limits::CONSTRAINED, 0, &arc_base::Cancel::new());
        triangulate(points, &prepared, &mut budget).ok()?
    }

    fn area_of(points: &[P2], triangles: &[[usize; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| cross(at(points, t[0]), at(points, t[1]), at(points, t[2])) / 2.0)
            .sum()
    }

    fn centroid(points: &[P2], t: &[usize; 3]) -> P2 {
        let [a, b, c] = t.map(|k| at(points, k));
        [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0]
    }

    #[test]
    fn convex_square_gives_two_triangles() {
        let points = square(2.0);
        let triangles = run(&points, vec![vec![0, 1, 2, 3]]).expect("triangulated");
        assert_eq!(triangles.len(), 2);
        assert!((area_of(&points, &triangles) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn clockwise_input_is_reoriented_and_triangles_are_counterclockwise() {
        let points = square(1.0);
        let triangles = run(&points, vec![vec![0, 3, 2, 1]]).expect("triangulated");
        assert!(triangles.iter().all(|t| area_of(&points, &[*t]) > 0.0));
    }

    #[test]
    fn concave_l_shape_is_triangulated_with_its_area() {
        let points = vec![
            [0.0, 0.0],
            [2.0, 0.0],
            [2.0, 1.0],
            [1.0, 1.0],
            [1.0, 2.0],
            [0.0, 2.0],
        ];
        let triangles = run(&points, vec![vec![0, 1, 2, 3, 4, 5]]).expect("triangulated");
        assert_eq!(triangles.len(), 4);
        assert!((area_of(&points, &triangles) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn square_with_a_hole_keeps_the_area_difference() {
        let mut points = square(4.0);
        points.extend([[1.0, 1.0], [1.0, 3.0], [3.0, 3.0], [3.0, 1.0]]);
        let triangles =
            run(&points, vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7]]).expect("triangulated");
        assert!((area_of(&points, &triangles) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn a_hole_outside_its_boundary_gives_no_triangles() {
        let mut points = square(1.0);
        points.extend([[5.0, 5.0], [6.0, 5.0], [6.0, 6.0]]);
        assert!(run(&points, vec![vec![0, 1, 2, 3], vec![4, 5, 6]]).is_none());
    }

    #[test]
    fn a_hole_touching_the_outer_boundary_gives_no_triangles() {
        let mut points = square(4.0);
        points.extend([[0.0, 1.0], [2.0, 1.0], [2.0, 2.0]]);
        assert!(run(&points, vec![vec![0, 1, 2, 3], vec![4, 5, 6]]).is_none());
    }

    /// Outer 0..5 square, hole A x 1..2 y 1.8..3.2 and hole B x 3..4 y 1.5..2.5. Their areas are 1.4
    /// and 1.0, so the region has 25 - 2.4 = 22.6.
    fn two_holes() -> (Vec<P2>, Vec<Vec<usize>>) {
        let mut points = square(5.0);
        points.extend([[1.0, 1.8], [2.0, 1.8], [2.0, 3.2], [1.0, 3.2]]);
        points.extend([[3.0, 1.5], [4.0, 1.5], [4.0, 2.5], [3.0, 2.5]]);
        (
            points,
            vec![vec![0, 1, 2, 3], vec![4, 5, 6, 7], vec![8, 9, 10, 11]],
        )
    }

    #[test]
    fn two_holes_leave_the_area_and_each_triangle_is_in_the_region() {
        let (points, loops) = two_holes();
        let prepared = prepare(&points, loops.clone(), true).expect("prepared");
        let triangles = run(&points, loops.clone()).expect("triangulated");
        assert!((area_of(&points, &triangles) - 22.6).abs() < 1e-9);
        for t in &triangles {
            let c = centroid(&points, t);
            assert_eq!(locate(&points, &prepared[0], c), Side::Inside);
            assert_eq!(locate(&points, &prepared[1], c), Side::Outside);
            assert_eq!(locate(&points, &prepared[2], c), Side::Outside);
        }
    }

    #[test]
    fn two_holes_in_either_input_order_give_the_same_area() {
        let (points, loops) = two_holes();
        let reversed = vec![loops[0].clone(), loops[2].clone(), loops[1].clone()];
        let a = area_of(&points, &run(&points, loops).expect("triangulated"));
        let b = area_of(&points, &run(&points, reversed).expect("triangulated"));
        assert!((a - b).abs() < 1e-9);
        assert!((b - 22.6).abs() < 1e-9);
    }

    /// A keyhole written as one loop that reuses two points: the outer square, a bridge to the hole
    /// and the hole walked back to the bridge. Its region is 16 - 4 = 12.
    #[test]
    fn keyhole_loop_triangulates_to_outer_minus_hole() {
        let points = vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 4.0],
            [0.0, 4.0],
            [1.0, 1.0],
            [1.0, 3.0],
            [3.0, 3.0],
            [3.0, 1.0],
        ];
        let keyhole = vec![0, 1, 2, 3, 0, 4, 5, 6, 7, 4];
        let triangles = run(&points, vec![keyhole]).expect("triangulated");
        assert!((area_of(&points, &triangles) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn collinear_points_of_a_face_are_triangle_corners() {
        let points = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        let prepared = prepare(&points, vec![vec![0, 1, 2, 3, 4]], false).expect("prepared");
        let triangles = triangulate(
            &points,
            &prepared,
            &mut Budget::new(&arc_base::Limits::CONSTRAINED, 0, &arc_base::Cancel::new()),
        )
        .expect("not cancelled")
        .expect("triangulated");
        assert_eq!(triangles.len(), 3);
        assert!((area_of(&points, &triangles) - 4.0).abs() < 1e-12);
        assert!(triangles.iter().any(|t| t.contains(&1)));
    }

    #[test]
    fn degenerate_loops_are_refused() {
        let points = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
        assert!(prepare(&points, vec![vec![0, 1, 2]], true).is_none());
        assert!(prepare(&points, vec![vec![0, 1]], true).is_none());
    }

    #[test]
    fn repeated_and_collinear_points_are_removed() {
        let points = vec![
            [0.0, 0.0],
            [0.0, 0.0],
            [1.0, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [0.0, 2.0],
        ];
        let prepared = prepare(&points, vec![vec![0, 1, 2, 3, 4, 5]], true).expect("prepared");
        assert_eq!(prepared[0].len(), 4);
    }

    #[test]
    fn ring_size_counts_each_bridge_twice() {
        assert_eq!(ring_vertices(&[4, 4, 4]), Some(16));
        assert_eq!(ring_vertices(&[usize::MAX, 1]), None);
    }
}
