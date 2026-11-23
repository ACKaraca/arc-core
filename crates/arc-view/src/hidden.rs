//! Polyhedral hidden-line removal in view space (CORE-VIEW-009).
//!
//! The occluders are triangles: every triangle of the objects below the cut, clipped to the cut
//! height, with its plane, and the triangles of the cut regions, which lie above everything
//! drawn below the cut. An edge segment is hidden where its projection is inside an occluder's
//! projection and the occluder is nearer the viewer by more than the linear tolerance. Both
//! conditions are linear in the segment parameter, so each occluder hides one exact interval;
//! the hidden part of an edge is the union of these intervals, and the result is exact for
//! polyhedral input up to rounding and the tolerance.
//!
//! "Inside" uses a margin per occluder edge: an edge on the outline of the occluding surface in
//! projection must be crossed by more than the tolerance (an edge on that outline is not hidden
//! by it), while an edge shared with a triangle that continues the surface on the other side,
//! such as a diagonal of a triangulated face, is widened by the tolerance so that nothing shows
//! through it. Cut regions are widened on every edge: edges on a cut boundary lie under the cut
//! line that is drawn there.
//!
//! Cost: an R*-tree over the occluder boxes gives the candidates of each edge; every candidate is
//! one constant-time test, counted against `ViewLimits::max_occlusion_tests`. The worst case is
//! the product of edges and occluders; the bound turns it into `view.limit.exceeded`.

use std::collections::HashMap;

use arc_base::math::{Point2, Point3};
use arc_base::{Result, Tolerance};
use arc_geom::predicates::{Sign, orient2d};
use arc_geom::{Aabb2, SpatialIndex2, TriangleMesh2};

use crate::mesh::{Prepared, too_many};

/// A depth plane `z = z0 + gx (x - x0) + gy (y - y0)`.
#[derive(Debug, Clone, Copy)]
struct Plane {
    origin: Point3,
    gx: f64,
    gy: f64,
}

impl Plane {
    fn z(&self, x: f64, y: f64) -> f64 {
        self.origin.z + self.gx * (x - self.origin.x) + self.gy * (y - self.origin.y)
    }
}

/// One occluding triangle, counterclockwise in projection, with the margin of each edge (edge
/// `i` runs from `points[i]` to `points[i + 1]`): positive is strict, negative widened.
#[derive(Debug, Clone, Copy)]
struct Occluder {
    points: [Point2; 3],
    margins: [f64; 3],
    /// `None` for a cut region, which is above every edge below the cut.
    plane: Option<Plane>,
}

/// The occluders of a view and their index.
#[derive(Default)]
pub(crate) struct Occlusion {
    occluders: Vec<Occluder>,
    index: SpatialIndex2<u32>,
}

/// The part of `[lo, hi]` where `f0 + t (f1 - f0) > 0`; `None` when either value is not finite,
/// so a degenerate occluder never hides anything.
fn positive(f0: f64, f1: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    if !(f0.is_finite() && f1.is_finite()) {
        return None;
    }
    let (lo, hi) = if f0 > 0.0 && f1 > 0.0 {
        (lo, hi)
    } else if f0 <= 0.0 && f1 <= 0.0 {
        return None;
    } else {
        let root = f0 / (f0 - f1);
        if f0 > 0.0 {
            (lo, hi.min(root))
        } else {
            (lo.max(root), hi)
        }
    };
    (lo < hi).then_some((lo, hi))
}

fn flat(p: Point3) -> Point2 {
    Point2::new(p.x, p.y)
}

fn cross(a: Point2, b: Point2, c: Point2) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

impl Occlusion {
    /// Adds the triangles of `mesh` clipped to `z <= cut`.
    pub(crate) fn add_mesh(&mut self, mesh: &Prepared, cut: f64, tol: &Tolerance) -> Result<()> {
        // The triangles of each welded edge with their third vertex.
        let mut sides: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        for t in &mesh.triangles {
            for i in 0..3 {
                let (Some(&u), Some(&w), Some(&x)) =
                    (t.get(i), t.get((i + 1) % 3), t.get((i + 2) % 3))
                else {
                    continue;
                };
                sides.entry((u.min(w), u.max(w))).or_default().push(x);
            }
        }
        let at = |v: u32| {
            usize::try_from(v)
                .ok()
                .and_then(|i| mesh.positions.get(i))
                .copied()
        };
        for t in &mesh.triangles {
            let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) else {
                continue;
            };
            if a.z.min(b.z).min(c.z) > cut {
                continue;
            }
            let (pa, pb, pc) = (flat(a), flat(b), flat(c));
            let area = cross(pa, pb, pc);
            let longest = (pb - pa).norm().max((pc - pb).norm()).max((pa - pc).norm());
            if area.abs() <= tol.linear * longest {
                continue; // edge-on: hides nothing
            }
            let gx = ((b.z - a.z) * (pc.y - pa.y) - (c.z - a.z) * (pb.y - pa.y)) / area;
            let gy = ((c.z - a.z) * (pb.x - pa.x) - (b.z - a.z) * (pc.x - pa.x)) / area;
            let plane = Plane { origin: a, gx, gy };
            let mut margins = [tol.linear; 3];
            for (i, margin) in margins.iter_mut().enumerate() {
                let (Some(&u), Some(&w), Some(&x)) =
                    (t.get(i), t.get((i + 1) % 3), t.get((i + 2) % 3))
                else {
                    continue;
                };
                let continues = match sides.get(&(u.min(w), u.max(w))).map(Vec::as_slice) {
                    Some(&[x1, x2]) => {
                        let other = if x1 == x { x2 } else { x1 };
                        match (at(u), at(w), at(x), at(other)) {
                            (Some(u), Some(w), Some(x), Some(o)) => {
                                let s1 = orient2d(&flat(u), &flat(w), &flat(x))?;
                                let s2 = orient2d(&flat(u), &flat(w), &flat(o))?;
                                s1 != Sign::Zero && s2 != Sign::Zero && s1 != s2
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                };
                if continues {
                    *margin = -tol.linear;
                }
            }
            self.add_clipped([a, b, c], margins, plane, cut, tol)?;
        }
        Ok(())
    }

    /// Clips a triangle to `z <= cut` (the new edge is strict) and adds it as a fan.
    fn add_clipped(
        &mut self,
        corners: [Point3; 3],
        margins: [f64; 3],
        plane: Plane,
        cut: f64,
        tol: &Tolerance,
    ) -> Result<()> {
        let mut polygon: Vec<(Point3, f64)> = Vec::with_capacity(4);
        for i in 0..3 {
            let (Some(&p), Some(&q), Some(&m)) =
                (corners.get(i), corners.get((i + 1) % 3), margins.get(i))
            else {
                continue;
            };
            let (inside_p, inside_q) = (p.z <= cut, q.z <= cut);
            if inside_p {
                polygon.push((p, m));
            }
            if inside_p != inside_q {
                let t = (cut - p.z) / (q.z - p.z);
                let x = p + (q - p) * t;
                polygon.push((x, if inside_p { tol.linear } else { m }));
            }
        }
        let Some(&(first, m0)) = polygon.first() else {
            return Ok(());
        };
        let fans = polygon.len().saturating_sub(2);
        for k in 0..fans {
            let (Some(&(b, mb)), Some(&(c, _))) = (polygon.get(k + 1), polygon.get(k + 2)) else {
                continue;
            };
            let first_margin = if k == 0 { m0 } else { -tol.linear };
            let last_margin = if k + 1 == fans {
                polygon.last().map_or(tol.linear, |l| l.1)
            } else {
                -tol.linear
            };
            self.push(
                [flat(first), flat(b), flat(c)],
                [first_margin, mb, last_margin],
                Some(plane),
                tol,
            )?;
        }
        Ok(())
    }

    /// Adds the triangles of a cut region, widened on every edge.
    pub(crate) fn add_cap(&mut self, mesh: &TriangleMesh2, tol: &Tolerance) -> Result<()> {
        let vertices = mesh.vertices();
        let at = |v: u32| {
            usize::try_from(v)
                .ok()
                .and_then(|i| vertices.get(i))
                .copied()
        };
        for t in mesh.triangles() {
            if let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) {
                self.push([a, b, c], [-tol.linear; 3], None, tol)?;
            }
        }
        Ok(())
    }

    fn push(
        &mut self,
        mut points: [Point2; 3],
        mut margins: [f64; 3],
        plane: Option<Plane>,
        tol: &Tolerance,
    ) -> Result<()> {
        let [a, b, c] = points;
        let area = cross(a, b, c);
        let longest = (b - a).norm().max((c - b).norm()).max((a - c).norm());
        // A triangle no wider than the tolerance hides nothing and has edges too short to
        // divide by (a clip at a vertex in the cut plane makes such fan triangles).
        if area.is_nan() || area.abs() <= tol.linear * longest {
            return Ok(());
        }
        if area < 0.0 {
            // Reverse: edges 0, 1, 2 become 2, 1, 0 of the reversed triangle.
            points.swap(1, 2);
            margins = [margins[2], margins[1], margins[0]];
        }
        let boxed = Aabb2::from_points(&points).expanded(tol.linear);
        let key = u32::try_from(self.occluders.len()).map_err(|_| too_many("triangles"))?;
        self.index.insert(key, boxed)?;
        self.occluders.push(Occluder {
            points,
            margins,
            plane,
        });
        Ok(())
    }

    /// The hidden parameter intervals of the segment `p`–`q`, sorted and disjoint, using at most
    /// `budget` occluder tests (decremented).
    pub(crate) fn hidden(
        &self,
        p: Point3,
        q: Point3,
        tol: &Tolerance,
        budget: &mut u64,
    ) -> Result<Vec<(f64, f64)>> {
        let (a, b) = (flat(p), flat(q));
        let query = Aabb2::from_points(&[a, b]).expanded(tol.linear);
        let mut hidden = Vec::new();
        for key in self.index.crossing(&query)? {
            *budget = budget
                .checked_sub(1)
                .ok_or_else(|| too_many("occlusion_tests"))?;
            let Some(o) = usize::try_from(key)
                .ok()
                .and_then(|k| self.occluders.get(k))
            else {
                continue;
            };
            let mut range = Some((0.0, 1.0));
            for i in 0..3 {
                let (Some(&s), Some(&e), Some(&m)) =
                    (o.points.get(i), o.points.get((i + 1) % 3), o.margins.get(i))
                else {
                    continue;
                };
                let edge = e - s;
                let len = edge.norm();
                if len.is_nan() || len <= 0.0 {
                    range = None;
                    continue;
                }
                let inward = |x: Point2| (edge.x * (x.y - s.y) - edge.y * (x.x - s.x)) / len - m;
                range = range.and_then(|(lo, hi)| positive(inward(a), inward(b), lo, hi));
            }
            if let (Some(plane), Some((lo, hi))) = (o.plane, range) {
                let depth = |x: Point3| plane.z(x.x, x.y) - x.z - tol.linear;
                range = positive(depth(p), depth(q), lo, hi);
            }
            if let Some(r) = range {
                hidden.push(r);
            }
        }
        hidden.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged: Vec<(f64, f64)> = Vec::with_capacity(hidden.len());
        for (lo, hi) in hidden {
            match merged.last_mut() {
                Some(last) if lo <= last.1 => last.1 = last.1.max(hi),
                _ => merged.push((lo, hi)),
            }
        }
        Ok(merged)
    }
}

/// The complement of sorted disjoint intervals in `[0, 1]`.
pub(crate) fn complement(hidden: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::with_capacity(hidden.len() + 1);
    let mut at = 0.0;
    for &(lo, hi) in hidden {
        if lo > at {
            out.push((at, lo));
        }
        at = at.max(hi);
    }
    if at < 1.0 {
        out.push((at, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::positive;

    #[test]
    fn positive_part_of_a_linear_function() {
        assert_eq!(positive(1.0, 1.0, 0.0, 1.0), Some((0.0, 1.0)));
        assert_eq!(positive(-1.0, -1.0, 0.0, 1.0), None);
        assert_eq!(positive(1.0, -1.0, 0.0, 1.0), Some((0.0, 0.5)));
        assert_eq!(positive(-1.0, 3.0, 0.0, 1.0), Some((0.25, 1.0)));
        assert_eq!(positive(f64::NAN, 1.0, 0.0, 1.0), None);
        assert_eq!(positive(1.0, f64::INFINITY, 0.0, 1.0), None);
    }
}
