//! Planar polygon triangulation with holes (CORE-IO-015).
//!
//! The triangulation keeps the input vertices: every triangle corner is one of the loop points, so
//! neighbouring faces and caps that share an edge share its vertices. This is why the module does
//! not use the flattening triangulator of `arc-geom`, which drops segments shorter than its chord
//! tolerance and would open cracks between the faces of a closed shell.
//!
//! [`prepare`] removes repeated and collinear points and orients the outer loop counterclockwise and
//! every hole clockwise, so the material is on the left of each edge. [`triangulate`] bridges each
//! hole to the outer loop (the visibility search of the bridge, then ear clipping) and checks that
//! the triangle areas add up to the area of the loops, so an overlapping or misplaced hole gives
//! no triangles rather than a wrong mesh.

use arc_base::Result;

use super::budget::Budget;

/// A 2D point.
pub(crate) type P2 = [f64; 2];

/// The relative error allowed between the sum of the triangle areas and the area of the loops: the
/// rounding of the sums, far below any geometric tolerance.
const AREA_RELATIVE_TOLERANCE: f64 = 1e-9;

fn at(points: &[P2], index: usize) -> P2 {
    points.get(index).copied().unwrap_or([f64::NAN, f64::NAN])
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
            let a = at(points, ring.get(k).copied().unwrap_or(0));
            let b = at(points, ring.get((k + 1) % n).copied().unwrap_or(0));
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

/// Removes repeated consecutive points (cyclically) and collinear middle points.
fn simplify(points: &[P2], ring: &[usize]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::with_capacity(ring.len());
    for &index in ring {
        if out
            .last()
            .is_none_or(|&last| at(points, last) != at(points, index))
        {
            out.push(index);
        }
    }
    while out.len() > 1 && at(points, out[0]) == at(points, out[out.len() - 1]) {
        out.pop();
    }
    while out.len() >= 3 {
        let n = out.len();
        let collinear = (0..n).find(|&k| {
            let prev = at(points, out[(k + n - 1) % n]);
            let next = at(points, out[(k + 1) % n]);
            cross(prev, at(points, out[k]), next) == 0.0
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
/// with fewer than three points or with zero area.
pub(crate) fn prepare(points: &[P2], loops: Vec<Vec<usize>>) -> Option<Vec<Vec<usize>>> {
    let mut out = Vec::with_capacity(loops.len());
    for (k, ring) in loops.iter().enumerate() {
        let mut ring = simplify(points, ring);
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

/// The index within a hole of its vertex with the largest x (the lowest y among ties).
fn hole_start(points: &[P2], hole: &[usize]) -> usize {
    let mut best = 0;
    for (k, &index) in hole.iter().enumerate() {
        let p = at(points, index);
        let q = at(points, hole.get(best).copied().unwrap_or(0));
        if p[0] > q[0] || (p[0] == q[0] && p[1] < q[1]) {
            best = k;
        }
    }
    best
}

/// The position in `ring` of the vertex a hole is bridged to: the endpoint of the nearest edge that
/// the ray from the hole's start, cast in +x, crosses, or a vertex inside the triangle this forms
/// with the ray, the one at the smallest angle to the ray.
fn bridge(points: &[P2], ring: &[usize], hole: &[usize]) -> Option<usize> {
    let m = at(points, hole.get(hole_start(points, hole)).copied()?);
    let n = ring.len();
    let mut best: Option<(usize, f64)> = None;
    for i in 0..n {
        let a = at(points, ring[i]);
        let b = at(points, ring[(i + 1) % n]);
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
    let p = at(points, ring[position]);
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
        out.push(hole.get((start + k) % h).copied().unwrap_or(0));
    }
    out.push(ring.get(position).copied().unwrap_or(0));
    out.extend_from_slice(ring.get(position + 1..).unwrap_or(&[]));
    out
}

/// Whether a point of the ring other than the ear's corners lies in the ear `a, b, c`.
fn blocked(points: &[P2], ring: &[usize], corners: [usize; 3]) -> bool {
    let [a, b, c] = corners.map(|k| at(points, ring.get(k).copied().unwrap_or(0)));
    ring.iter().enumerate().any(|(j, &index)| {
        if corners.contains(&j) {
            return false;
        }
        let x = at(points, index);
        x != a && x != b && x != c && inside(x, a, b, c)
    })
}

/// Triangulates the prepared loops (see [`prepare`]): the outer loop with its holes. Returns the
/// triangles as triples of point indices, counterclockwise seen from the normal of the outer loop,
/// or `None` when the loops are not a valid region (no ear, a hole outside its boundary, or
/// overlapping holes).
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
    let mut ring = outer.clone();
    let mut order: Vec<&Vec<usize>> = holes.iter().collect();
    order.sort_by(|a, b| min_x(points, a).total_cmp(&min_x(points, b)));
    for hole in order {
        budget.step()?;
        let Some(position) = bridge(points, &ring, hole) else {
            return Ok(None);
        };
        ring = merge(&ring, position, hole, hole_start(points, hole));
    }
    let mut triangles = Vec::new();
    let mut i = 0;
    let mut stalled = 0_usize;
    while ring.len() > 3 {
        budget.step()?;
        let n = ring.len();
        let p = (i + n - 1) % n;
        let q = (i + 1) % n;
        let (a, b, c) = (
            at(points, ring[p]),
            at(points, ring[i]),
            at(points, ring[q]),
        );
        let turn = cross(a, b, c);
        if turn == 0.0 {
            ring.remove(i);
            i = if i == 0 { ring.len() - 1 } else { i - 1 };
            stalled = 0;
        } else if turn > 0.0 && !blocked(points, &ring, [p, i, q]) {
            triangles.push([ring[p], ring[i], ring[q]]);
            ring.remove(i);
            i = if i == 0 { ring.len() - 1 } else { i - 1 };
            stalled = 0;
        } else {
            i = (i + 1) % n;
            stalled += 1;
            if stalled > n {
                return Ok(None);
            }
        }
    }
    if ring.len() == 3 {
        let turn = cross(
            at(points, ring[0]),
            at(points, ring[1]),
            at(points, ring[2]),
        );
        if turn < 0.0 {
            return Ok(None);
        }
        if turn > 0.0 {
            triangles.push([ring[0], ring[1], ring[2]]);
        }
    }
    let expected: f64 = loops.iter().map(|l| signed_area(points, l)).sum();
    let mut total = 0.0;
    let mut magnitude = 0.0;
    for t in &triangles {
        let area = cross(at(points, t[0]), at(points, t[1]), at(points, t[2])) / 2.0;
        total += area;
        magnitude += area.abs();
    }
    let scale = (magnitude + expected.abs()).max(f64::MIN_POSITIVE);
    if triangles.is_empty() || (total - expected).abs() > AREA_RELATIVE_TOLERANCE * scale {
        return Ok(None);
    }
    Ok(Some(triangles))
}

fn min_x(points: &[P2], ring: &[usize]) -> f64 {
    ring.iter()
        .map(|&index| at(points, index)[0])
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(size: f64) -> Vec<P2> {
        vec![[0.0, 0.0], [size, 0.0], [size, size], [0.0, size]]
    }

    fn run(points: &[P2], loops: Vec<Vec<usize>>) -> Option<Vec<[usize; 3]>> {
        let prepared = prepare(points, loops)?;
        let mut budget = Budget::new(&arc_base::Limits::CONSTRAINED, 0, &arc_base::Cancel::new());
        triangulate(points, &prepared, &mut budget).ok()?
    }

    fn area_of(points: &[P2], triangles: &[[usize; 3]]) -> f64 {
        triangles
            .iter()
            .map(|t| cross(at(points, t[0]), at(points, t[1]), at(points, t[2])) / 2.0)
            .sum()
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
    fn degenerate_loops_are_refused() {
        let points = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
        assert!(prepare(&points, vec![vec![0, 1, 2]]).is_none());
        assert!(prepare(&points, vec![vec![0, 1]]).is_none());
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
        let prepared = prepare(&points, vec![vec![0, 1, 2, 3, 4, 5]]).expect("prepared");
        assert_eq!(prepared[0].len(), 4);
    }
}
