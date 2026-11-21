//! Triangulation of a filled overlay polygon in screen space: one outer ring and any number of
//! holes become triangles by joining each hole to the outer ring with a bridge and clipping ears.
//!
//! The input is whatever the camera projected, so it is treated as hostile: a ring that is not
//! finite or has no area, more vertices than the caller allows, and a polygon that stays stuck
//! (self-intersecting rings) or exceeds the work bound all give `None`, and the caller skips the
//! fill. Nothing here panics or allocates beyond the vertices it was given plus the bridges.

/// A screen position in logical points.
pub(crate) type P = [f64; 2];

/// Triangles of a polygon: the vertices and index triples into them. A bridge repeats vertices, so
/// the list holds a point once for each time a triangle needs it.
pub(crate) struct Triangles {
    pub vertices: Vec<P>,
    pub triangles: Vec<[usize; 3]>,
}

fn cross(o: P, a: P, b: P) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

fn signed_area(ring: &[P]) -> f64 {
    ring.iter()
        .zip(ring.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        / 2.0
}

/// A ring without repeated neighbors and without its closing point, `None` when it has fewer than
/// three points or no area, or a point is not finite.
fn clean(ring: &[P]) -> Option<Vec<P>> {
    if !ring.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    let mut out: Vec<P> = Vec::with_capacity(ring.len());
    for p in ring {
        if out.last() != Some(p) {
            out.push(*p);
        }
    }
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    (out.len() >= 3 && signed_area(&out) != 0.0).then_some(out)
}

/// Whether `p` lies in the closed triangle `a b c` of positive orientation.
fn in_triangle(p: P, a: P, b: P, c: P) -> bool {
    cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
}

/// The triangles of `rings[0]` minus the holes `rings[1..]`; the orientation of each ring is free.
/// `None` for an outer ring without area, more than `max_vertices` points in all, or a polygon
/// that cannot be clipped. A hole without area is ignored.
pub(crate) fn triangulate(rings: &[Vec<P>], max_vertices: usize) -> Option<Triangles> {
    if rings.iter().map(Vec::len).sum::<usize>() > max_vertices {
        return None;
    }
    let (outer, holes) = rings.split_first()?;
    let outer = clean(outer)?;
    let mut vertices: Vec<P> = Vec::new();
    // Positive area: counterclockwise in the axes of the numbers, whichever way the screen runs.
    let mut oriented = |ring: Vec<P>, positive: bool| -> Vec<usize> {
        let mut ring = ring;
        if (signed_area(&ring) > 0.0) != positive {
            ring.reverse();
        }
        let first = vertices.len();
        vertices.extend(ring);
        (first..vertices.len()).collect()
    };
    let mut polygon = oriented(outer, true);
    let mut holes: Vec<Vec<usize>> = holes
        .iter()
        .filter_map(|h| clean(h))
        .map(|h| oriented(h, false))
        .collect();
    let rightmost = |hole: &Vec<usize>, vertices: &[P]| -> (usize, f64) {
        hole.iter()
            .enumerate()
            .map(|(k, &i)| (k, vertices.get(i).map_or(f64::MIN, |p| p[0])))
            .fold((0, f64::MIN), |best, v| if v.1 > best.1 { v } else { best })
    };
    holes.sort_by(|a, b| {
        rightmost(b, &vertices)
            .1
            .total_cmp(&rightmost(a, &vertices).1)
    });
    for hole in &holes {
        let (start, _) = rightmost(hole, &vertices);
        bridge(&mut polygon, hole, start, &mut vertices)?;
    }
    clip(&polygon, &vertices)
}

/// The nearest edge crossed by the ray from `m` to +x: the x of the crossing and the polygon
/// position of the edge's endpoint with the larger x.
fn crossing(polygon: &[usize], vertices: &[P], m: P) -> Option<(f64, usize)> {
    let mut best: Option<(f64, usize)> = None;
    for k in 0..polygon.len() {
        let a = *vertices.get(*polygon.get(k)?)?;
        let next = (k + 1) % polygon.len();
        let b = *vertices.get(*polygon.get(next)?)?;
        if (a[1] > m[1]) == (b[1] > m[1]) && a[1] != m[1] && b[1] != m[1] {
            continue;
        }
        if a[1] == b[1] {
            // A horizontal edge on the ray: its nearer end at or right of `m`.
            for (pos, p) in [(k, a), (next, b)] {
                if a[1] == m[1] && p[0] >= m[0] && best.is_none_or(|(x, _)| p[0] < x) {
                    best = Some((p[0], pos));
                }
            }
            continue;
        }
        let x = a[0] + (b[0] - a[0]) * (m[1] - a[1]) / (b[1] - a[1]);
        if x >= m[0] && best.is_none_or(|(bx, _)| x < bx) {
            best = Some((x, if a[0] > b[0] { k } else { next }));
        }
    }
    best
}

/// Joins `hole` (starting at its rightmost vertex, position `start`) to `polygon`: finds the
/// polygon vertex that the hole's rightmost vertex sees along the positive x axis and splices the
/// hole in after it, repeating the two vertices of the bridge.
fn bridge(
    polygon: &mut Vec<usize>,
    hole: &[usize],
    start: usize,
    vertices: &mut Vec<P>,
) -> Option<()> {
    let m = *vertices.get(*hole.get(start)?)?;
    let (hit, mut at) = crossing(polygon, vertices, m)?;
    // A reflex vertex inside the triangle m, crossing, candidate blocks the view; the one that
    // makes the smallest angle with the ray is seen first.
    let candidate = *vertices.get(*polygon.get(at)?)?;
    if candidate[1] != m[1] {
        let crossing = [hit, m[1]];
        let (a, b, c) = if cross(m, crossing, candidate) > 0.0 {
            (m, crossing, candidate)
        } else {
            (m, candidate, crossing)
        };
        let mut nearest: Option<(f64, usize)> = None;
        for k in 0..polygon.len() {
            let prev = *vertices.get(*polygon.get((k + polygon.len() - 1) % polygon.len())?)?;
            let p = *vertices.get(*polygon.get(k)?)?;
            let next = *vertices.get(*polygon.get((k + 1) % polygon.len())?)?;
            if k == at || p == candidate || p == m || cross(prev, p, next) > 0.0 {
                continue;
            }
            if in_triangle(p, a, b, c) {
                let (dx, dy) = (p[0] - m[0], (p[1] - m[1]).abs());
                let tangent = if dx > 0.0 { dy / dx } else { f64::MAX };
                if nearest.is_none_or(|(t, _)| tangent < t) {
                    nearest = Some((tangent, k));
                }
            }
        }
        if let Some((_, k)) = nearest {
            at = k;
        }
    }
    let seen = *polygon.get(at)?;
    let mut spliced: Vec<usize> = Vec::with_capacity(polygon.len() + hole.len() + 2);
    spliced.extend(polygon.get(..=at)?);
    for k in 0..=hole.len() {
        spliced.push(*hole.get((start + k) % hole.len())?);
    }
    // The bridge visits the hole's start and the seen vertex twice: copies keep every polygon
    // vertex a distinct index, so a triangle never refers to the wrong occurrence.
    let (copy_m, copy_seen) = (vertices.len(), vertices.len() + 1);
    vertices.push(m);
    vertices.push(*vertices.get(seen)?);
    if let Some(last) = spliced.last_mut() {
        *last = copy_m;
    }
    spliced.push(copy_seen);
    spliced.extend(polygon.get(at + 1..)?);
    *polygon = spliced;
    Some(())
}

/// Ear clipping of a counterclockwise polygon, which may touch itself along bridges. Collinear
/// vertices are dropped without a triangle. `None` when no ear is left before the polygon is
/// used up, or the work bound is exceeded.
fn clip(polygon: &[usize], vertices: &[P]) -> Option<Triangles> {
    let at = |i: usize| vertices.get(i).copied();
    let mut ring: Vec<usize> = polygon.to_vec();
    let mut triangles = Vec::with_capacity(ring.len().saturating_sub(2));
    let mut work = 0_usize;
    let bound = 64 * ring.len() * ring.len() + 1024;
    while ring.len() > 3 {
        let n = ring.len();
        let mut clipped = false;
        for k in 0..n {
            let (ia, ib, ic) = (
                *ring.get((k + n - 1) % n)?,
                *ring.get(k)?,
                *ring.get((k + 1) % n)?,
            );
            let (a, b, c) = (at(ia)?, at(ib)?, at(ic)?);
            let turn = cross(a, b, c);
            if turn == 0.0 {
                ring.remove(k);
                clipped = true;
                break;
            }
            if turn < 0.0 {
                continue;
            }
            let mut blocked = false;
            for &other in &ring {
                work += 1;
                let p = at(other)?;
                if other == ia || other == ib || other == ic || p == a || p == b || p == c {
                    continue;
                }
                if in_triangle(p, a, b, c) {
                    blocked = true;
                    break;
                }
            }
            if work > bound {
                return None;
            }
            if !blocked {
                triangles.push([ia, ib, ic]);
                ring.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            return None;
        }
    }
    if let [a, b, c] = ring[..]
        && cross(at(a)?, at(b)?, at(c)?) > 0.0
    {
        triangles.push([a, b, c]);
    }
    (!triangles.is_empty()).then(|| Triangles {
        vertices: vertices.to_vec(),
        triangles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(t: &Triangles) -> f64 {
        t.triangles
            .iter()
            .map(|&[a, b, c]| cross(t.vertices[a], t.vertices[b], t.vertices[c]) / 2.0)
            .sum()
    }

    /// Whether the triangle centroids are all inside the polygon and outside the holes.
    fn inside(rings: &[Vec<P>], p: P) -> bool {
        let mut count = 0;
        for ring in rings {
            for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
                if (a[1] > p[1]) != (b[1] > p[1])
                    && p[0] < a[0] + (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1])
                {
                    count += 1;
                }
            }
        }
        count % 2 == 1
    }

    #[test]
    fn a_concave_polygon_with_a_hole_is_filled_exactly() {
        // An L of area 12 with a 1 x 1 hole in its corner block, in either ring direction.
        let l = vec![
            [0.0, 0.0],
            [4.0, 0.0],
            [4.0, 2.0],
            [2.0, 2.0],
            [2.0, 4.0],
            [0.0, 4.0],
        ];
        let hole = vec![[0.5, 0.5], [0.5, 1.5], [1.5, 1.5], [1.5, 0.5]];
        for flip in [false, true] {
            let mut rings = vec![l.clone(), hole.clone()];
            if flip {
                rings[0].reverse();
                rings[1].reverse();
            }
            let t = triangulate(&rings, 64).unwrap();
            assert!((area(&t) - 11.0).abs() < 1e-9, "area {}", area(&t));
            for &[a, b, c] in &t.triangles {
                let (a, b, c) = (t.vertices[a], t.vertices[b], t.vertices[c]);
                let centroid = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
                assert!(inside(&rings, centroid), "{centroid:?} is outside");
            }
        }
    }

    #[test]
    fn two_holes_and_a_closing_point_are_handled() {
        let outer = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
        let rings = vec![
            outer,
            vec![[1.0, 1.0], [3.0, 1.0], [3.0, 3.0], [1.0, 3.0]],
            vec![[5.0, 1.0], [7.0, 1.0], [7.0, 3.0], [5.0, 3.0]],
        ];
        let t = triangulate(&rings, 64).unwrap();
        assert!((area(&t) - 32.0).abs() < 1e-9);
    }

    #[test]
    fn hostile_rings_give_nothing() {
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(
            triangulate(std::slice::from_ref(&square), 3).is_none(),
            "over the cap"
        );
        assert!(triangulate(&[vec![[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0]]], 64).is_none());
        assert!(triangulate(&[vec![[0.0, 0.0], [1.0, 1.0]]], 64).is_none());
        assert!(triangulate(&[vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]], 64).is_none());
        assert!(triangulate(&[], 64).is_none());
        // A hole without area is ignored.
        let t = triangulate(&[square, vec![[0.2, 0.2], [0.4, 0.2], [0.6, 0.2]]], 64).unwrap();
        assert!((area(&t) - 1.0).abs() < 1e-9);
    }
}
