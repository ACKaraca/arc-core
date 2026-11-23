//! Merging of collinear segments (CORE-VIEW-011).

use std::f64::consts::PI;

use arc_base::Tolerance;
use arc_base::math::{Point2, Vector2};

/// A 2D segment with its sub-object key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Seg {
    pub a: Point2,
    pub b: Point2,
    pub key: Option<u32>,
}

struct Placed {
    seg: Seg,
    angle: f64,
}

/// Merges segments that lie on one line within the linear tolerance and overlap or touch into
/// one segment from the extreme endpoints; the key of the first piece along the line is kept.
///
/// Segments are grouped by direction angle in `[0, π)` (within the tolerance over the longest
/// segment; angles near π count as near 0), then by offset of the line, then merged along it.
/// Sorting makes the cost `O(n log n)`.
pub(crate) fn merge(segments: Vec<Seg>, tol: &Tolerance) -> Vec<Seg> {
    let longest = segments
        .iter()
        .map(|s| (s.b - s.a).norm())
        .fold(tol.linear, f64::max);
    let angle_tol = tol.linear / longest;
    let mut placed: Vec<Placed> = segments
        .into_iter()
        .map(|seg| {
            let d = seg.b - seg.a;
            let mut angle = d.y.atan2(d.x);
            if angle < 0.0 {
                angle += PI;
            }
            if angle >= PI - angle_tol {
                angle -= PI;
            }
            Placed { seg, angle }
        })
        .collect();
    placed.sort_by(|x, y| x.angle.total_cmp(&y.angle));
    let mut out = Vec::with_capacity(placed.len());
    for bundle in split(placed, |p| p.angle, angle_tol) {
        let Some(first) = bundle.first() else {
            continue;
        };
        let u = Vector2::new(first.angle.cos(), first.angle.sin());
        let n = Vector2::new(-u.y, u.x);
        let mut lines: Vec<(f64, Seg)> = bundle
            .iter()
            .map(|p| (n.dot(&p.seg.a.coords), p.seg))
            .collect();
        lines.sort_by(|x, y| x.0.total_cmp(&y.0));
        for line in split(lines, |l| l.0, tol.linear) {
            let mut runs: Vec<(f64, f64, Point2, Point2, Option<u32>)> = line
                .iter()
                .map(|(_, s)| {
                    let (sa, sb) = (u.dot(&s.a.coords), u.dot(&s.b.coords));
                    if sa <= sb {
                        (sa, sb, s.a, s.b, s.key)
                    } else {
                        (sb, sa, s.b, s.a, s.key)
                    }
                })
                .collect();
            runs.sort_by(|x, y| x.0.total_cmp(&y.0));
            let mut current: Option<(f64, f64, Point2, Point2, Option<u32>)> = None;
            for run in runs {
                current = match current {
                    Some(c) if run.0 <= c.1 + tol.linear => Some(if run.1 > c.1 {
                        (c.0, run.1, c.2, run.3, c.4)
                    } else {
                        c
                    }),
                    Some(c) => {
                        out.push(Seg {
                            a: c.2,
                            b: c.3,
                            key: c.4,
                        });
                        Some(run)
                    }
                    None => Some(run),
                };
            }
            if let Some(c) = current {
                out.push(Seg {
                    a: c.2,
                    b: c.3,
                    key: c.4,
                });
            }
        }
    }
    out
}

/// Splits sorted items where consecutive values differ by more than `gap`.
fn split<T>(items: Vec<T>, value: impl Fn(&T) -> f64, gap: f64) -> Vec<Vec<T>> {
    let mut out: Vec<Vec<T>> = Vec::new();
    let mut last = f64::NEG_INFINITY;
    for item in items {
        let v = value(&item);
        match out.last_mut() {
            Some(group) if v - last <= gap => group.push(item),
            _ => out.push(vec![item]),
        }
        last = v;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Seg, merge};
    use arc_base::Tolerance;
    use arc_base::math::Point2;

    fn seg(a: (f64, f64), b: (f64, f64)) -> Seg {
        Seg {
            a: Point2::new(a.0, a.1),
            b: Point2::new(b.0, b.1),
            key: None,
        }
    }

    #[test]
    fn collinear_overlapping_and_touching_pieces_merge() {
        let tol = Tolerance::DEFAULT;
        let out = merge(
            vec![
                seg((0.0, 0.0), (1.0, 0.0)),
                seg((2.0, 0.0), (1.0, 0.0)),
                seg((1.5, 0.0), (3.0, 0.0)),
                seg((5.0, 0.0), (6.0, 0.0)),
                seg((0.0, 1.0), (1.0, 1.0)),
                seg((0.0, 0.0), (0.0, 1.0)),
            ],
            &tol,
        );
        assert_eq!(out.len(), 4);
        assert!(out.contains(&seg((0.0, 0.0), (3.0, 0.0))));
        assert!(out.contains(&seg((5.0, 0.0), (6.0, 0.0))));
    }
}
