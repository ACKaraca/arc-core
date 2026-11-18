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
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::FRAC_PI_2;

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::index::SpatialIndex2;
use piece::{Piece, Raw};

/// Above this many piece pairs, candidate pairs are found with a spatial index of the pieces
/// of the second curve instead of by testing every pair.
const INDEXED_PAIRS: usize = 256;
/// Raw points and overlaps collected per allowed result before the limit is enforced: one
/// intersection at a vertex shared on both curves is found by up to four piece pairs.
const RAW_PER_RESULT: usize = 4;

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

/// Every intersection of `a` and `b`, sorted by the parameter on `a`, with the result limit of
/// [`Limits::DEFAULT`]; see [`intersect_with_limits`].
///
/// # Errors
///
/// As [`intersect_with_limits`].
pub fn intersect(a: &Curve2, b: &Curve2, tolerance: &Tolerance) -> Result<Vec<Intersection>> {
    intersect_with_limits(a, b, tolerance, &Limits::DEFAULT)
}

/// Every intersection of `a` and `b`, sorted by the parameter on `a`.
///
/// Points are classified as [`Contact::Crossing`] or [`Contact::Tangent`]; collinear lines,
/// co-circular arcs and coincident elliptical arcs that share a stretch longer than the linear
/// tolerance return an [`Intersection::Overlap`], never sampled points. A point within the
/// linear tolerance of an end point of a curve, or of the seam of a closed curve, is reported at
/// that end point; the seam of a closed curve reports the start of its range. Intersections of
/// adjacent polyline segments at their shared vertex are merged into one. An overlap that runs
/// across the seam of a closed curve is reported as two overlaps, one on each side of the seam.
/// `intersect(a, a)` returns the curve's overlap with itself, not its self-intersections.
///
/// `intersect(b, a)` returns the same intersections with the parameters swapped.
///
/// # Errors
///
/// `geom.tolerance.invalid` when the linear tolerance is not positive and finite or the angular
/// tolerance is negative, not finite or above a quarter turn; `geom.precision.insufficient`
/// when coordinates are so large, or a construction so ill-conditioned, that the error bound of
/// a result exceeds the linear tolerance; `geom.intersect.too_many_results` when there are more
/// than `limits.array_length` results; `geom.curve.kind_unsupported` when either curve is a
/// NURBS curve, whose intersections are not implemented yet.
pub fn intersect_with_limits(
    a: &Curve2,
    b: &Curve2,
    tolerance: &Tolerance,
    limits: &Limits,
) -> Result<Vec<Intersection>> {
    let valid = tolerance.linear.is_finite()
        && tolerance.linear > 0.0
        && tolerance.angular.is_finite()
        && tolerance.angular >= 0.0
        && tolerance.angular <= FRAC_PI_2;
    if !valid {
        return Err(Error::new(codes::TOLERANCE_INVALID));
    }
    if let Some(index) = [a, b].iter().position(|c| matches!(c, Curve2::Nurbs(_))) {
        return Err(Error::new(codes::CURVE_KIND_UNSUPPORTED)
            .with_detail("operation", "intersect")
            .with_detail("curve", if index == 0 { "a" } else { "b" }));
    }
    let max_results = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
    if canonical_order(a, b) == Ordering::Greater {
        let mut result: Vec<Intersection> = ordered(b, a, tolerance, max_results)?
            .iter()
            .map(Intersection::swapped)
            .collect();
        sort(&mut result);
        return Ok(result);
    }
    ordered(a, b, tolerance, max_results)
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
        Curve2::Nurbs(c) => (
            8,
            std::iter::once(c.degree() as f64)
                .chain(c.knots().iter().copied())
                .chain(
                    c.control_points()
                        .iter()
                        .zip(c.weights())
                        .flat_map(|(p, w)| [p.x, p.y, *w]),
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

/// One curve with its pieces.
struct Side<'c> {
    curve: &'c Curve2,
    pieces: Vec<Piece>,
    linear: f64,
}

impl<'c> Side<'c> {
    fn new(curve: &'c Curve2, linear: f64) -> Self {
        Self {
            curve,
            pieces: piece::pieces(curve, linear),
            linear,
        }
    }

    /// The length of the parameter range of a closed curve.
    fn period(&self) -> Option<f64> {
        self.curve
            .is_closed()
            .then(|| self.curve.parameter_range().1)
    }

    /// The seam of a closed curve is reported at the start of its range.
    fn seam(&self, t: f64) -> f64 {
        if self.curve.is_closed() && t >= self.curve.parameter_range().1 {
            0.0
        } else {
            t
        }
    }

    /// The pieces whose parameter range contains `t`: one, or two at a shared boundary.
    fn pieces_at(&self, t: f64) -> impl Iterator<Item = &Piece> {
        let first = self.pieces.partition_point(|p| p.range()[1] < t);
        self.pieces
            .get(first..)
            .unwrap_or_default()
            .iter()
            .take_while(move |p| p.range()[0] <= t)
    }

    /// The parameter distance around `t` within which moving along one piece covers at most the
    /// linear tolerance.
    fn window(&self, t: f64) -> f64 {
        self.pieces_at(t)
            .map(|p| self.linear / p.max_speed())
            .fold(0.0, f64::max)
    }

    /// Whether `t1` and `t2` name the same place of the curve: equal, or on one piece with at
    /// most the linear tolerance of curve between them, also across the seam of a closed curve.
    /// Distinct visits of a curve to one location are never the same place.
    fn same_place(&self, t1: f64, t2: f64) -> bool {
        self.same_place_on_range(t1, t2)
            || self.period().is_some_and(|period| {
                self.same_place_on_range(t1 + period, t2)
                    || self.same_place_on_range(t1, t2 + period)
            })
    }

    /// [`Self::same_place`] without crossing the seam.
    fn same_place_on_range(&self, t1: f64, t2: f64) -> bool {
        let near = |x: f64, y: f64| {
            x == y
                || self.pieces_at(x).any(|p| {
                    let [lo, hi] = p.range();
                    y >= lo && y <= hi && (x - y).abs() * p.max_speed() <= self.linear
                })
        };
        near(t1, t2)
    }

    /// Whether `t` lies in `interval` or at the same place as one of its ends.
    fn within(&self, t: f64, interval: [f64; 2]) -> bool {
        let inside = |t: f64| t >= interval[0] && t <= interval[1];
        inside(t)
            || self.same_place(t, interval[0])
            || self.same_place(t, interval[1])
            || self
                .period()
                .is_some_and(|period| inside(t + period) || inside(t - period))
    }
}

/// The index pairs of pieces whose boxes, one grown by `linear`, meet; unbounded pieces pair
/// with every piece. Sorted.
fn candidate_pairs(p: &[Piece], q: &[Piece], linear: f64) -> Result<Vec<(usize, usize)>> {
    let mut pairs = Vec::new();
    if p.len().saturating_mul(q.len()) <= INDEXED_PAIRS {
        for (i, x) in p.iter().enumerate() {
            for (j, y) in q.iter().enumerate() {
                let meet = match (x.bbox(), y.bbox()) {
                    (Some(bx), Some(by)) => bx.expanded(linear).intersects(&by),
                    _ => true,
                };
                if meet {
                    pairs.push((i, j));
                }
            }
        }
        return Ok(pairs);
    }
    let index = SpatialIndex2::from_entries(
        q.iter()
            .enumerate()
            .filter_map(|(j, y)| y.bbox().map(|boxed| (j, boxed))),
    )?;
    let unbounded: Vec<usize> = (0..q.len())
        .filter(|&j| q.get(j).is_some_and(|y| y.bbox().is_none()))
        .collect();
    for (i, x) in p.iter().enumerate() {
        let mut found = match x.bbox() {
            Some(boxed) => {
                let mut found = index.crossing(&boxed.expanded(linear))?;
                found.extend_from_slice(&unbounded);
                found.sort_unstable();
                found
            }
            None => (0..q.len()).collect(),
        };
        pairs.extend(found.drain(..).map(|j| (i, j)));
    }
    Ok(pairs)
}

fn too_many(limit: usize) -> Error {
    Error::new(codes::INTERSECT_TOO_MANY_RESULTS)
        .with_detail("limit", i64::try_from(limit).unwrap_or(i64::MAX))
}

fn ordered(
    a: &Curve2,
    b: &Curve2,
    tolerance: &Tolerance,
    max_results: usize,
) -> Result<Vec<Intersection>> {
    let linear = tolerance.linear;
    let (sa, sb) = (Side::new(a, linear), Side::new(b, linear));
    let point_at = |ta: f64, tb: f64| {
        let (pa, pb) = (a.point_at(ta), b.point_at(tb));
        pa + (pb - pa) * 0.5
    };
    let raw_limit = max_results.saturating_mul(RAW_PER_RESULT);
    let mut hits = Vec::new();
    let mut spans = Vec::new();
    for (i, j) in candidate_pairs(&sa.pieces, &sb.pieces, linear)? {
        let (Some(p), Some(q)) = (sa.pieces.get(i), sb.pieces.get(j)) else {
            continue;
        };
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
            if hits.len() + spans.len() > raw_limit {
                return Err(too_many(max_results));
            }
        }
    }
    let spans = join_spans(spans, &sa, &sb);
    let hits = drop_inside(merge_hits(hits, &sa, &sb, linear), &spans, &sa, &sb);
    if hits.len() + spans.len() > max_results {
        return Err(too_many(max_results));
    }
    let mut result: Vec<Intersection> = hits
        .into_iter()
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

/// The grid cell of side `linear` holding `p`. Coordinates are below `linear / ROUNDING` once
/// the pieces passed their magnitude check, so the cell indices fit.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the quotients are bounded by the magnitude check; saturation only coarsens cells"
)]
fn cell(p: &Point2, linear: f64) -> (i64, i64) {
    ((p.x / linear).floor() as i64, (p.y / linear).floor() as i64)
}

/// Merges hits that are one intersection found by several piece pairs: within the linear
/// tolerance of each other and at the same place of each curve (see [`Side::same_place`]),
/// such as at a shared polyline vertex or at the seam. A merged hit keeps the parameters of its
/// first member, never an average, and is tangent when any member is, or when one piece pair
/// produced two of its members.
fn merge_hits(mut hits: Vec<Hit>, sa: &Side<'_>, sb: &Side<'_>, linear: f64) -> Vec<Hit> {
    hits.sort_by(|x, y| x.a.total_cmp(&y.a).then(x.b.total_cmp(&y.b)));
    let mut clusters: Vec<Vec<Hit>> = Vec::new();
    let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for hit in hits {
        let (cx, cy) = cell(&hit.point, linear);
        let joins = |other: &Hit| {
            (other.point - hit.point).norm() <= linear
                && sa.same_place(other.a, hit.a)
                && sb.same_place(other.b, hit.b)
        };
        let mut found: Option<usize> = None;
        for dx in -1..=1_i64 {
            for dy in -1..=1_i64 {
                let key = (cx.saturating_add(dx), cy.saturating_add(dy));
                for &k in cells.get(&key).map_or(&[][..], Vec::as_slice) {
                    if found.is_none_or(|f| k < f)
                        && clusters.get(k).is_some_and(|c| c.iter().any(joins))
                    {
                        found = Some(k);
                    }
                }
            }
        }
        let k = match found {
            Some(k) => k,
            None => {
                clusters.push(Vec::new());
                clusters.len() - 1
            }
        };
        if let Some(cluster) = clusters.get_mut(k) {
            cluster.push(hit);
        }
        let ids = cells.entry((cx, cy)).or_default();
        if !ids.contains(&k) {
            ids.push(k);
        }
    }
    clusters
        .into_iter()
        .filter_map(|members| {
            let first = *members.first()?;
            let tangent = members
                .iter()
                .enumerate()
                .any(|(k, m)| m.tangent || members.iter().skip(k + 1).any(|o| o.pair == m.pair));
            Some(Hit { tangent, ..first })
        })
        .collect()
}

/// Drops the hits that lie inside an overlap on both curves.
fn drop_inside(hits: Vec<Hit>, spans: &[Span], sa: &Side<'_>, sb: &Side<'_>) -> Vec<Hit> {
    if spans.is_empty() {
        return hits;
    }
    hits.into_iter()
        .filter(|h| {
            !spans
                .iter()
                .any(|s| sa.within(h.a, s.a) && sb.within(h.b, s.b))
        })
        .collect()
}

/// A key that orders like [`f64::total_cmp`].
fn order_key(x: f64) -> u64 {
    let bits = x.to_bits();
    if bits >> 63 == 1 {
        !bits
    } else {
        bits | (1 << 63)
    }
}

/// Joins overlaps whose intervals continue each other on both curves: the end of one and the
/// start of the next on `a` are at the same place, and so are the corresponding ends on `b`.
/// Overlaps are processed by their start on `a`, and each joins any earlier joined overlap it
/// continues, so chains are joined whatever other overlaps lie between them. Overlaps are not
/// joined across the seam of a closed curve.
fn join_spans(mut spans: Vec<Span>, sa: &Side<'_>, sb: &Side<'_>) -> Vec<Span> {
    spans.sort_by(|x, y| x.a[0].total_cmp(&y.a[0]).then(x.b[0].total_cmp(&y.b[0])));
    let mut joined: Vec<Span> = Vec::new();
    // The joined overlaps by the order key of their end on `a`.
    let mut by_end: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for span in spans {
        let w = sa.window(span.a[0]);
        let (lo, hi) = (span.a[0] - w, span.a[0] + w);
        let keys: Vec<u64> = if lo <= hi {
            by_end
                .range(order_key(lo)..=order_key(hi))
                .map(|(key, _)| *key)
                .collect()
        } else {
            vec![order_key(span.a[0])]
        };
        let continues = |last: &Span| {
            last.same == span.same
                && sa.same_place_on_range(last.a[1], span.a[0])
                && if span.same {
                    sb.same_place_on_range(last.b[1], span.b[0])
                } else {
                    sb.same_place_on_range(span.b[1], last.b[0])
                }
        };
        let target = keys
            .iter()
            .filter_map(|key| by_end.get(key).map(|ids| (*key, ids)))
            .flat_map(|(key, ids)| ids.iter().map(move |id| (key, *id)))
            .filter(|(_, id)| joined.get(*id).is_some_and(continues))
            .min_by_key(|(_, id)| *id);
        match target {
            Some((key, id)) => {
                if let Some(ids) = by_end.get_mut(&key) {
                    ids.retain(|x| *x != id);
                    if ids.is_empty() {
                        by_end.remove(&key);
                    }
                }
                if let Some(last) = joined.get_mut(id) {
                    last.a[1] = span.a[1];
                    if span.same {
                        last.b[1] = span.b[1];
                    } else {
                        last.b[0] = span.b[0];
                    }
                    by_end.entry(order_key(last.a[1])).or_default().push(id);
                }
            }
            None => {
                by_end
                    .entry(order_key(span.a[1]))
                    .or_default()
                    .push(joined.len());
                joined.push(span);
            }
        }
    }
    joined
}

#[cfg(test)]
mod tests;
