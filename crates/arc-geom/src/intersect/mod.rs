//! Curve intersections (CORE-GEOM-005).
//!
//! [`intersect`] returns every place where two curves of any kind meet: isolated points,
//! classified as crossing or tangent, and overlap intervals where the curves share a stretch.
//! Parameters follow the parameterization of [`Curve2`]. Every decision is made within the
//! [`Tolerance`] passed in; the rules are stated in `docs/crates/arc-geom.md`, section "Curve
//! intersections".
//!
//! The result is symmetric by construction: the two curves are put into a fixed canonical order
//! before computing, and the result for the other order is the same list with the roles of the
//! curves swapped.

mod piece;
mod trig;

use std::cmp::Ordering;

use arc_base::math::Point2;
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use piece::{Piece, Raw};

/// How two curves meet at an isolated intersection point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Contact {
    /// The curves meet transversally: their directions are not parallel.
    Crossing,
    /// The curves touch with parallel directions within the tolerance, including collinear or
    /// co-circular curves that meet only at an end point.
    Tangent,
}

/// Whether the two parameter intervals of an overlap run the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// The start of the interval on `a` meets the start of the interval on `b`.
    Same,
    /// The start of the interval on `a` meets the end of the interval on `b`.
    Opposite,
}

/// One intersection of two curves `a` and `b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intersection {
    /// The curves meet at an isolated point.
    Point {
        /// The intersection point: the midpoint of the points of `a` and `b` at the reported
        /// parameters.
        point: Point2,
        /// The parameter on `a`.
        a: f64,
        /// The parameter on `b`.
        b: f64,
        /// Whether the curves cross or touch.
        contact: Contact,
    },
    /// The curves share a stretch. Both intervals are ascending and may be unbounded for rays
    /// and lines.
    Overlap {
        /// The parameter interval on `a`.
        a: [f64; 2],
        /// The parameter interval on `b`.
        b: [f64; 2],
        /// How the intervals correspond.
        orientation: Orientation,
    },
}

impl Intersection {
    /// The same intersection with the roles of `a` and `b` exchanged.
    #[must_use]
    pub fn swapped(&self) -> Self {
        match *self {
            Self::Point {
                point,
                a,
                b,
                contact,
            } => Self::Point {
                point,
                a: b,
                b: a,
                contact,
            },
            Self::Overlap { a, b, orientation } => Self::Overlap {
                a: b,
                b: a,
                orientation,
            },
        }
    }

    /// The parameters at which the intersection starts on `a` and on `b`.
    fn start(&self) -> (f64, f64) {
        match self {
            Self::Point { a, b, .. } => (*a, *b),
            Self::Overlap { a, b, .. } => (a[0], b[0]),
        }
    }
}

/// Sorts by the start on `a`, then on `b`, then points before overlaps.
fn sort(result: &mut [Intersection]) {
    result.sort_by(|x, y| {
        let ((xa, xb), (ya, yb)) = (x.start(), y.start());
        xa.total_cmp(&ya).then(xb.total_cmp(&yb)).then(
            matches!(x, Intersection::Overlap { .. })
                .cmp(&matches!(y, Intersection::Overlap { .. })),
        )
    });
}

/// Every intersection of `a` and `b`, sorted by the parameter on `a`.
///
/// Points are classified as [`Contact::Crossing`] or [`Contact::Tangent`]; collinear lines,
/// co-circular arcs and coincident elliptical arcs that share a stretch longer than the linear
/// tolerance return an [`Intersection::Overlap`], never sampled points. A point within the
/// linear tolerance of an end point of a curve, or of the seam of a closed curve, is reported at
/// that end point; the seam of a closed curve reports the start of its range. Intersections of
/// adjacent polyline segments at their shared vertex are merged into one.
///
/// `intersect(b, a)` returns the same intersections with the parameters swapped.
///
/// # Errors
///
/// `geom.tolerance.invalid` when the linear tolerance is not positive and finite or the angular
/// tolerance is negative or not finite; `geom.precision.insufficient` when coordinates are so
/// large, or a construction so ill-conditioned, that the error bound of a result exceeds the
/// linear tolerance.
pub fn intersect(a: &Curve2, b: &Curve2, tolerance: &Tolerance) -> Result<Vec<Intersection>> {
    let valid = tolerance.linear.is_finite()
        && tolerance.linear > 0.0
        && tolerance.angular.is_finite()
        && tolerance.angular >= 0.0;
    if !valid {
        return Err(Error::new(codes::TOLERANCE_INVALID));
    }
    if canonical_order(a, b) == Ordering::Greater {
        let mut result: Vec<Intersection> = ordered(b, a, tolerance)?
            .iter()
            .map(Intersection::swapped)
            .collect();
        sort(&mut result);
        return Ok(result);
    }
    ordered(a, b, tolerance)
}

/// The kind rank and defining numbers of a curve, compared to fix the order of computation.
fn canonical_key(curve: &Curve2) -> (u8, Vec<f64>) {
    match curve {
        Curve2::Segment(c) => (0, vec![c.start().x, c.start().y, c.end().x, c.end().y]),
        Curve2::Ray(c) => (
            1,
            vec![c.origin().x, c.origin().y, c.direction().x, c.direction().y],
        ),
        Curve2::Line(c) => (
            2,
            vec![c.point().x, c.point().y, c.direction().x, c.direction().y],
        ),
        Curve2::Circle(c) => (3, vec![c.center().x, c.center().y, c.radius()]),
        Curve2::Arc(c) => (
            4,
            vec![
                c.center().x,
                c.center().y,
                c.radius(),
                c.start_angle(),
                c.sweep(),
            ],
        ),
        Curve2::Ellipse(c) => (
            5,
            vec![
                c.center().x,
                c.center().y,
                c.major_axis().x,
                c.major_axis().y,
                c.ratio(),
            ],
        ),
        Curve2::EllipticalArc(c) => {
            let e = c.ellipse();
            (
                6,
                vec![
                    e.center().x,
                    e.center().y,
                    e.major_axis().x,
                    e.major_axis().y,
                    e.ratio(),
                    c.start_parameter(),
                    c.sweep(),
                ],
            )
        }
        Curve2::Polyline(c) => (
            7,
            std::iter::once(if c.is_closed() { 1.0 } else { 0.0 })
                .chain(
                    c.vertices()
                        .iter()
                        .flat_map(|v| [v.point.x, v.point.y, v.bulge, v.start_width, v.end_width]),
                )
                .collect(),
        ),
    }
}

fn canonical_order(a: &Curve2, b: &Curve2) -> Ordering {
    let ((ka, va), (kb, vb)) = (canonical_key(a), canonical_key(b));
    ka.cmp(&kb).then_with(|| {
        va.iter()
            .zip(&vb)
            .map(|(x, y)| x.total_cmp(y))
            .find(|o| o.is_ne())
            .unwrap_or_else(|| va.len().cmp(&vb.len()))
    })
}

/// An isolated point in curve parameters, from the piece pair `pair`.
#[derive(Debug, Clone, Copy)]
struct Hit {
    a: f64,
    b: f64,
    tangent: bool,
    pair: (usize, usize),
    point: Point2,
}

/// An overlap in curve parameters.
#[derive(Debug, Clone, Copy)]
struct Span {
    a: [f64; 2],
    b: [f64; 2],
    same: bool,
}

/// One curve with its pieces and its parametric tolerance.
struct Side<'c> {
    curve: &'c Curve2,
    pieces: Vec<Piece>,
    /// The parameter distance that moves at most the linear tolerance along the curve.
    ptol: f64,
}

impl<'c> Side<'c> {
    fn new(curve: &'c Curve2, linear: f64) -> Self {
        let pieces = piece::pieces(curve);
        let speed = pieces
            .iter()
            .map(Piece::speed)
            .fold(f64::INFINITY, f64::min);
        Self {
            curve,
            pieces,
            ptol: linear / speed,
        }
    }

    /// The seam of a closed curve is reported at the start of its range.
    fn seam(&self, t: f64) -> f64 {
        if self.curve.is_closed() && t >= self.curve.parameter_range().1 {
            0.0
        } else {
            t
        }
    }

    /// Whether `t` lies in `interval` within the parametric tolerance, also one period later on
    /// a closed curve.
    fn within(&self, t: f64, interval: [f64; 2]) -> bool {
        let inside = |t: f64| t >= interval[0] - self.ptol && t <= interval[1] + self.ptol;
        inside(t) || (self.curve.is_closed() && inside(t + self.curve.parameter_range().1))
    }
}

fn ordered(a: &Curve2, b: &Curve2, tolerance: &Tolerance) -> Result<Vec<Intersection>> {
    let linear = tolerance.linear;
    let (sa, sb) = (Side::new(a, linear), Side::new(b, linear));
    let point_at = |ta: f64, tb: f64| {
        let (pa, pb) = (a.point_at(ta), b.point_at(tb));
        pa + (pb - pa) * 0.5
    };
    let mut hits = Vec::new();
    let mut spans = Vec::new();
    for (i, p) in sa.pieces.iter().enumerate() {
        for (j, q) in sb.pieces.iter().enumerate() {
            if !p.may_meet(q, linear) {
                continue;
            }
            for raw in piece::intersect_pieces(p, q, tolerance)? {
                match raw {
                    Raw::Point { s, t, tangent } => {
                        let (ta, tb) = (sa.seam(p.param(s)), sb.seam(q.param(t)));
                        hits.push(Hit {
                            a: ta,
                            b: tb,
                            tangent,
                            pair: (i, j),
                            point: point_at(ta, tb),
                        });
                    }
                    Raw::Overlap { s, t, same } => spans.push(Span {
                        a: [p.param(s[0]), p.param(s[1])],
                        b: [q.param(t[0]), q.param(t[1])],
                        same,
                    }),
                }
            }
        }
    }
    let spans = join_spans(spans, &sa, &sb);
    let mut result: Vec<Intersection> = merge_hits(hits, &sa, &sb, linear)
        .into_iter()
        .filter(|h| {
            !spans
                .iter()
                .any(|s| sa.within(h.a, s.a) && sb.within(h.b, s.b))
        })
        .map(|h| Intersection::Point {
            point: point_at(h.a, h.b),
            a: h.a,
            b: h.b,
            contact: if h.tangent {
                Contact::Tangent
            } else {
                Contact::Crossing
            },
        })
        .chain(spans.iter().map(|s| Intersection::Overlap {
            a: s.a,
            b: s.b,
            orientation: if s.same {
                Orientation::Same
            } else {
                Orientation::Opposite
            },
        }))
        .collect();
    sort(&mut result);
    Ok(result)
}

/// Merges hits that are the same intersection found by adjacent pieces: within the linear
/// tolerance in position and within the parametric tolerance on both curves. The merged hit has
/// the mean parameters and is tangent when any member is, or when one piece pair produced two
/// of its members.
fn merge_hits(mut hits: Vec<Hit>, sa: &Side<'_>, sb: &Side<'_>, linear: f64) -> Vec<Hit> {
    hits.sort_by(|x, y| x.a.total_cmp(&y.a).then(x.b.total_cmp(&y.b)));
    let mut clusters: Vec<Vec<Hit>> = Vec::new();
    for hit in hits {
        let close = |other: &Hit| {
            (other.b - hit.b).abs() <= sb.ptol && (other.point - hit.point).norm() <= linear
        };
        let found = clusters
            .iter_mut()
            .rev()
            .take_while(|c| c.last().is_some_and(|last| hit.a - last.a <= sa.ptol))
            .find(|c| c.iter().any(close));
        match found {
            Some(cluster) => cluster.push(hit),
            None => clusters.push(vec![hit]),
        }
    }
    clusters
        .into_iter()
        .filter_map(|members| {
            let first = *members.first()?;
            let n = members.len() as f64;
            let tangent = members
                .iter()
                .enumerate()
                .any(|(k, m)| m.tangent || members.iter().skip(k + 1).any(|o| o.pair == m.pair));
            Some(Hit {
                a: members.iter().map(|m| m.a).sum::<f64>() / n,
                b: members.iter().map(|m| m.b).sum::<f64>() / n,
                tangent,
                ..first
            })
        })
        .collect()
}

/// Joins overlaps found by adjacent pieces whose intervals continue each other on both curves.
fn join_spans(mut spans: Vec<Span>, sa: &Side<'_>, sb: &Side<'_>) -> Vec<Span> {
    spans.sort_by(|x, y| x.a[0].total_cmp(&y.a[0]).then(x.b[0].total_cmp(&y.b[0])));
    let mut joined: Vec<Span> = Vec::new();
    for span in spans {
        if let Some(last) = joined.last_mut()
            && last.same == span.same
            && (span.a[0] - last.a[1]).abs() <= sa.ptol
        {
            if span.same && (span.b[0] - last.b[1]).abs() <= sb.ptol {
                last.a[1] = span.a[1];
                last.b[1] = span.b[1];
                continue;
            }
            if !span.same && (span.b[1] - last.b[0]).abs() <= sb.ptol {
                last.a[1] = span.a[1];
                last.b[0] = span.b[0];
                continue;
            }
        }
        joined.push(span);
    }
    joined
}

#[cfg(test)]
mod tests;
