//! Bulge polylines (CORE-GEOM-002).
//!
//! A bulged segment is evaluated relative to its chord: with chord `c` from vertex `A` to
//! vertex `B` and half included angle `α = 2·atan(bulge)`, the point at angle fraction `f` is
//!
//! ```text
//! P(f) = A + k(f) · (c · cos φ − perp(c) · sin φ),  k(f) = sin(f·α) / sin α,  φ = (1 − f)·α
//! ```
//!
//! because the chord from `A` to `P(f)` has length `|c|·k(f)` and leaves `A` turned by `−φ`
//! from the chord. `sin α` is evaluated as `2b / (1 + b²)`, which has no cancellation near a
//! half turn, and `k(f)` is `f` when `α` is so small that `sin α` and `α` are the same `f64`.
//! Nothing is computed from the center, so the accuracy does not degrade as the bulge goes to 0:
//! `P(0)` and `P(1)` are the vertices exactly, and every other point is accurate to a few units
//! in the last place of the larger of the chord length and the sagitta.

use std::f64::consts::TAU;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::bbox::Aabb2;
use crate::deserialize_through_new;
use crate::error::codes;
use crate::primitives::{Arc2, Segment2, perp};

/// The half included angle below which `sin` returns its argument unchanged in `f64`, so the
/// ratio `sin(f·α) / sin α` is `f` to rounding; it also keeps `f·α` away from subnormal values.
pub(crate) const SMALL_HALF_ANGLE: f64 = f64::MIN_POSITIVE / f64::EPSILON;

/// A bulged polyline segment in its chord frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Chord {
    start: Point2,
    end: Point2,
    bulge: f64,
}

impl Chord {
    /// The start vertex.
    pub(crate) fn start(&self) -> Point2 {
        self.start
    }

    /// The end vertex.
    pub(crate) fn end(&self) -> Point2 {
        self.end
    }

    /// The bulge.
    pub(crate) fn bulge(&self) -> f64 {
        self.bulge
    }

    /// The same chord traversed from end to start.
    pub(crate) fn reversed(self) -> Self {
        Self {
            start: self.end,
            end: self.start,
            bulge: -self.bulge,
        }
    }

    /// The signed half included angle `2·atan(bulge)`, in (−π, π).
    pub(crate) fn half_angle(&self) -> f64 {
        2.0 * self.bulge.atan()
    }

    /// `sin` of the half included angle, `2b / (1 + b²)`, without overflow.
    pub(crate) fn sin_half_angle(&self) -> f64 {
        let b = self.bulge;
        if b.abs() <= 1.0 {
            2.0 * b / (1.0 + b * b)
        } else {
            2.0 / (b + 1.0 / b)
        }
    }

    /// The point at angle fraction `f` in [0, 1]; the end points are the vertices exactly.
    pub(crate) fn point_at_fraction(&self, f: f64) -> Point2 {
        if f <= 0.0 {
            return self.start;
        }
        if f >= 1.0 {
            return self.end;
        }
        let half = self.half_angle();
        let k = if half.abs() < SMALL_HALF_ANGLE {
            f
        } else {
            (f * half).sin() / self.sin_half_angle()
        };
        let (sin, cos) = ((1.0 - f) * half).sin_cos();
        let c = self.end - self.start;
        self.start + (c * cos - perp(&c) * sin) * k
    }

    /// The tight bounding box: the end points and every axis extremum on the arc.
    pub(crate) fn bounding_box(&self) -> Aabb2 {
        // The tangent at fraction f is the chord direction turned by (2f − 1)·α, so it is
        // parallel to axis direction d where 2f − 1 = δ/α, δ being the signed angle from the
        // chord to d. Both angles are small together, so the quotient has no cancellation.
        let half = self.half_angle();
        let c = self.end - self.start;
        let mut boxed = Aabb2::from_corners(self.start, self.end);
        for d in [Vector2::x(), -Vector2::x(), Vector2::y(), -Vector2::y()] {
            let ratio = c.perp(&d).atan2(c.dot(&d)) / half;
            if ratio.abs() <= 1.0 {
                let p = self.point_at_fraction(0.5 + 0.5 * ratio);
                boxed = boxed.union(&Aabb2::from_corners(p, p));
            }
        }
        boxed
    }
}

/// A polyline vertex and the segment that starts at it.
///
/// The bulge is the tangent of a quarter of the included angle of the segment that starts at
/// this vertex: 0 is straight, positive bends counterclockwise, negative clockwise, and ±1 is a
/// half circle. The widths are the full widths at the start and end of that segment; 0 means
/// no width. The last vertex of an open polyline starts no segment; its bulge and widths are
/// stored as 0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PolylineVertex {
    /// The position.
    pub point: Point2,
    /// The bulge factor of the segment that starts here.
    pub bulge: f64,
    /// The width at the start of the segment that starts here.
    pub start_width: f64,
    /// The width at the end of the segment that starts here.
    pub end_width: f64,
}

impl PolylineVertex {
    /// A vertex with the given bulge and no width.
    #[must_use]
    pub fn new(point: Point2, bulge: f64) -> Self {
        Self {
            point,
            bulge,
            start_width: 0.0,
            end_width: 0.0,
        }
    }
}

/// One segment of a polyline, kept exact.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PolylineSegment {
    /// A straight segment (bulge 0).
    Line(Segment2),
    /// A circular arc (nonzero bulge).
    Arc(Arc2),
}

impl PolylineSegment {
    /// The point at fraction `f` in [0, 1] of the segment's parameter range. An arc from a
    /// polyline is evaluated relative to its chord, as described in the [module
    /// documentation](self).
    #[must_use]
    pub fn point_at_fraction(&self, f: f64) -> Point2 {
        match self {
            Self::Line(segment) => segment.point_at(f),
            Self::Arc(arc) => match arc.chord() {
                Some(chord) => chord.point_at_fraction(f.clamp(0.0, 1.0)),
                None => arc.point_at(f * arc.sweep().abs()),
            },
        }
    }

    /// The tight bounding box; for an arc from a polyline it is computed relative to the chord.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        match self {
            Self::Line(segment) => segment.bounding_box(),
            Self::Arc(arc) => arc.bounding_box(),
        }
    }
}

/// A polyline of straight and circular segments defined by bulge factors, open or closed.
///
/// Parameter `t` in [0, n], where n is the number of segments: segment `k = floor(t)` (the last
/// segment for `t = n`) at local fraction `t − k`. Straight segments are parameterized
/// linearly, arcs proportionally to angle and evaluated relative to their chord (see the
/// [module documentation](self)). A closed polyline has a segment from the last vertex back to
/// the first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Polyline2Raw")]
pub struct Polyline2 {
    vertices: Vec<PolylineVertex>,
    closed: bool,
}

deserialize_through_new!(
    Polyline2,
    Polyline2Raw {
        vertices: Vec<PolylineVertex>,
        closed: bool,
    }
);

/// The exact segment from `start` to `end` with `bulge`.
///
/// Fails with `geom.precision.insufficient` when the center-radius form of a bulged segment
/// cannot be represented: the chord, center, radius or bounding box is not finite, or the sweep
/// rounds to a full turn.
fn bulge_segment(start: Point2, end: Point2, bulge: f64) -> Result<PolylineSegment> {
    if bulge == 0.0 {
        return Segment2::new(start, end).map(PolylineSegment::Line);
    }
    let insufficient = || Error::new(codes::PRECISION_INSUFFICIENT).with_detail("bulge", bulge);
    // With half chord h and half included angle α = 2·atan(b), the center lies h·cot α =
    // h·(1/b − b)/2 to the left of the chord, and the radius is h/sin α = h·(1/b + b)/2.
    let chord = end - start;
    let quarter = 0.25 * crate::length(&chord);
    let center = start + chord * 0.5 + perp(&chord) * (0.25 * (1.0 / bulge - bulge));
    let radius = quarter * (1.0 / bulge.abs() + bulge.abs());
    let sweep = 4.0 * bulge.atan();
    // Written so that a NaN fails the check.
    let representable = [chord.x, chord.y, center.x, center.y, radius]
        .iter()
        .all(|value| value.is_finite())
        && radius > 0.0
        && sweep.abs() < TAU;
    if !representable {
        return Err(insufficient());
    }
    let from_center = start - center;
    let arc = Arc2::new(center, radius, from_center.y.atan2(from_center.x), sweep)
        .map_err(|error| insufficient().with_source(error))?
        .with_chord(Chord { start, end, bulge });
    let boxed = arc.bounding_box();
    if [boxed.min(), boxed.max()]
        .iter()
        .all(|corner| corner.x.is_finite() && corner.y.is_finite())
    {
        Ok(PolylineSegment::Arc(arc))
    } else {
        Err(insufficient())
    }
}

impl Polyline2 {
    /// Creates a polyline. The bulge and widths of the last vertex of an open polyline are set
    /// to 0.
    ///
    /// # Errors
    ///
    /// `geom.polyline.too_few_vertices` for fewer than two vertices; `geom.input.not_finite`
    /// for a non-finite value; `geom.polyline.negative_width` for a negative width;
    /// `geom.polyline.zero_length_segment` when two consecutive vertices (including the last
    /// and the first of a closed polyline) are equal; `geom.precision.insufficient` when the
    /// center-radius form of a bulged segment cannot be represented in `f64`: its chord, center,
    /// radius or bounding box is not finite (a bulge so close to 0 that the center is out of
    /// range), or its sweep rounds to a full turn although the vertices differ (a bulge of
    /// about 1e16 or more in magnitude).
    ///
    /// A closed polyline of exactly two vertices is accepted, including with both bulges 0: it
    /// is a degenerate loop whose second segment retraces the first.
    pub fn new(mut vertices: Vec<PolylineVertex>, closed: bool) -> Result<Self> {
        if vertices.len() < 2 {
            return Err(Error::new(codes::POLYLINE_TOO_FEW_VERTICES));
        }
        if !closed && let Some(last) = vertices.last_mut() {
            *last = PolylineVertex::new(last.point, 0.0);
        }
        for v in &vertices {
            crate::ensure_finite(&[v.point.x, v.point.y, v.bulge, v.start_width, v.end_width])?;
            if v.start_width < 0.0 || v.end_width < 0.0 {
                return Err(Error::new(codes::POLYLINE_NEGATIVE_WIDTH));
            }
        }
        let polyline = Self { vertices, closed };
        for index in 0..polyline.segment_count() {
            if let Some((start, end, bulge)) = polyline.segment_ends(index) {
                if start == end {
                    return Err(Error::new(codes::POLYLINE_ZERO_LENGTH_SEGMENT)
                        .with_detail("segment", i64::try_from(index).unwrap_or(i64::MAX)));
                }
                bulge_segment(start, end, bulge)?;
            }
        }
        Ok(polyline)
    }

    /// The vertices.
    #[must_use]
    pub fn vertices(&self) -> &[PolylineVertex] {
        &self.vertices
    }

    /// Whether a segment joins the last vertex back to the first.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The number of segments: one less than the vertex count when open, equal when closed.
    #[must_use]
    pub fn segment_count(&self) -> usize {
        if self.closed {
            self.vertices.len()
        } else {
            self.vertices.len() - 1
        }
    }

    fn segment_ends(&self, index: usize) -> Option<(Point2, Point2, f64)> {
        if index >= self.segment_count() {
            return None;
        }
        let start = self.vertices.get(index)?;
        let end = self.vertices.get((index + 1) % self.vertices.len())?;
        Some((start.point, end.point, start.bulge))
    }

    /// Segment `index` as an exact segment or arc, or `None` past the last segment.
    #[must_use]
    pub fn segment(&self, index: usize) -> Option<PolylineSegment> {
        let (start, end, bulge) = self.segment_ends(index)?;
        bulge_segment(start, end, bulge).ok()
    }

    /// The segments in order.
    pub fn segments(&self) -> impl Iterator<Item = PolylineSegment> + '_ {
        (0..self.segment_count()).filter_map(|index| self.segment(index))
    }

    /// The point at parameter `t`, clamped to [0, n].
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        let count = self.segment_count();
        let t = t.clamp(0.0, count as f64);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "t is clamped to [0, n] first"
        )]
        let index = (t.floor() as usize).min(count - 1);
        match self.segment(index) {
            Some(segment) => segment.point_at_fraction(t - index as f64),
            // Unreachable for a validated polyline: every index below the count has a segment.
            None => self.vertices.first().map_or(Point2::origin(), |v| v.point),
        }
    }

    /// The tight bounding box: the union of the boxes of the segments.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        self.segments().fold(Aabb2::EMPTY, |boxed, segment| {
            boxed.union(&segment.bounding_box())
        })
    }

    /// The same polyline traversed backwards: segment `k` of the result is segment `n − 1 − k`
    /// reversed, with its bulge negated and its widths swapped. A closed polyline keeps its first
    /// vertex as the start.
    #[must_use]
    pub fn reversed(&self) -> Self {
        let n = self.vertices.len();
        let count = self.segment_count();
        let vertices = (0..n)
            .map(|j| {
                // Vertex j of the result starts the reverse of original segment count − 1 − j,
                // which ends at original vertex (n − j) mod n for closed and n − 1 − j for open.
                let at = if self.closed { (n - j) % n } else { n - 1 - j };
                let point = self.vertices[at].point;
                match count.checked_sub(j + 1).and_then(|k| self.vertices.get(k)) {
                    Some(source) => PolylineVertex {
                        point,
                        bulge: -source.bulge,
                        start_width: source.end_width,
                        end_width: source.start_width,
                    },
                    None => PolylineVertex::new(point, 0.0),
                }
            })
            .collect();
        Self {
            vertices,
            closed: self.closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testing::point;

    fn vertex() -> impl Strategy<Value = PolylineVertex> {
        (point(), -3.0..3.0_f64, 0.0..2.0_f64, 0.0..2.0_f64).prop_map(|(point, bulge, s, e)| {
            PolylineVertex {
                point,
                bulge,
                start_width: s,
                end_width: e,
            }
        })
    }

    pub(crate) fn polyline() -> impl Strategy<Value = Polyline2> {
        (prop::collection::vec(vertex(), 2..8), any::<bool>())
            .prop_filter_map("valid", |(v, closed)| Polyline2::new(v, closed).ok())
    }

    #[test]
    fn invalid_polylines_are_rejected() {
        let v = |x: f64, y: f64| PolylineVertex::new(Point2::new(x, y), 0.0);
        let code = |r: Result<Polyline2>| r.unwrap_err().code();
        assert_eq!(
            code(Polyline2::new(vec![v(0.0, 0.0)], false)),
            "geom.polyline.too_few_vertices"
        );
        assert_eq!(
            code(Polyline2::new(
                vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 0.0)],
                false
            )),
            "geom.polyline.zero_length_segment"
        );
        assert_eq!(
            code(Polyline2::new(
                vec![v(0.0, 0.0), v(1.0, 0.0), v(0.0, 0.0)],
                true
            )),
            "geom.polyline.zero_length_segment"
        );
        let mut wide = v(0.0, 0.0);
        wide.end_width = -1.0;
        assert_eq!(
            code(Polyline2::new(vec![wide, v(1.0, 0.0)], false)),
            "geom.polyline.negative_width"
        );
        assert_eq!(
            code(Polyline2::new(vec![v(0.0, 0.0), v(f64::NAN, 0.0)], false)),
            "geom.input.not_finite"
        );
        let json = r#"{"vertices":[{"point":[0.0,0.0],"bulge":0.0,"start_width":0.0,"end_width":0.0}],"closed":false}"#;
        assert!(serde_json::from_str::<Polyline2>(json).is_err());
    }

    #[test]
    fn unit_bulge_is_a_half_circle() {
        let p = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::new(0.0, 0.0), 1.0),
                PolylineVertex::new(Point2::new(2.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let Some(PolylineSegment::Arc(arc)) = p.segment(0) else {
            panic!("expected an arc");
        };
        assert!((arc.center() - Point2::new(1.0, 0.0)).norm() < 1e-15);
        assert!((arc.radius() - 1.0).abs() < 1e-15);
        // Counterclockwise from (0, 0) to (2, 0) passes below the chord.
        assert!((p.point_at(0.5) - Point2::new(1.0, -1.0)).norm() < 1e-12);
        assert!(p.segment(1).is_none());
    }

    fn two_point(start: Point2, end: Point2, bulge: f64) -> Result<Polyline2> {
        Polyline2::new(
            vec![
                PolylineVertex::new(start, bulge),
                PolylineVertex::new(end, 0.0),
            ],
            false,
        )
    }

    /// The larger of the chord length and the sagitta: the scale of the accuracy guarantee.
    fn scale(start: Point2, end: Point2, bulge: f64) -> f64 {
        let chord = (end - start).norm();
        chord.max(0.5 * chord * bulge.abs())
    }

    #[test]
    fn tiny_bulges_keep_the_vertices_and_the_sagitta() {
        let (a, b) = (Point2::new(0.0, 0.0), Point2::new(1.0, 0.0));
        for bulge in [1e-6, 1e-8, 1e-10, 1e-12, 1e-14, 1e-100, 1e-200, 1e-300] {
            let p = two_point(a, b, bulge).unwrap();
            let segment = p.segment(0).unwrap();
            assert!(matches!(segment, PolylineSegment::Arc(_)));
            assert_eq!(segment.point_at_fraction(0.0), a);
            assert_eq!(segment.point_at_fraction(1.0), b);
            assert_eq!((p.point_at(0.0), p.point_at(1.0)), (a, b));
            // Counterclockwise from a to b passes below the chord by the sagitta b/2.
            let middle = p.point_at(0.5);
            assert!((middle.x - 0.5).abs() <= 4.0 * f64::EPSILON);
            assert!((middle.y + 0.5 * bulge).abs() <= 4.0 * f64::EPSILON * 0.5 * bulge);
            let boxed = p.bounding_box();
            assert_eq!(
                (boxed.min().x, boxed.max().x, boxed.max().y),
                (0.0, 1.0, 0.0)
            );
            assert!((boxed.min().y + 0.5 * bulge).abs() <= 4.0 * f64::EPSILON * 0.5 * bulge);
            assert_eq!(segment.bounding_box(), boxed);
        }
    }

    #[test]
    fn unrepresentable_bulges_are_rejected() {
        let (a, b) = (Point2::new(0.0, 0.0), Point2::new(1.0, 0.0));
        // The sweep rounds to a full turn although the vertices differ.
        for bulge in [1e16, 1e17, 1e300, f64::MAX, -1e300] {
            assert_eq!(
                two_point(a, b, bulge).unwrap_err().code(),
                "geom.precision.insufficient",
                "bulge {bulge:e}"
            );
        }
        // The center is out of range.
        for bulge in [5e-324, -1e-310] {
            assert_eq!(
                two_point(a, b, bulge).unwrap_err().code(),
                "geom.precision.insufficient"
            );
        }
        let far = Point2::new(1e10, 0.0);
        assert_eq!(
            two_point(a, far, 1e-300).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        let p = two_point(a, b, 1e8).unwrap();
        assert_eq!((p.point_at(0.0), p.point_at(1.0)), (a, b));
    }

    #[test]
    fn closed_two_vertex_polylines_are_accepted() {
        let straight = vec![
            PolylineVertex::new(Point2::origin(), 0.0),
            PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
        ];
        let p = Polyline2::new(straight, true).unwrap();
        assert_eq!(p.segment_count(), 2);
        assert_eq!(p.point_at(1.5), Point2::new(0.5, 0.0));
        assert_eq!(p.point_at(2.0), Point2::origin());
    }

    /// A bulge log-uniform over [1e-300, 1e300] in magnitude, of either sign.
    fn any_bulge() -> impl Strategy<Value = f64> {
        (crate::testing::log_uniform(-300.0, 300.0), any::<bool>())
            .prop_map(|(m, negative)| if negative { -m } else { m })
    }

    proptest! {
        #[test]
        fn bulged_segments_are_chord_accurate_for_every_bulge(
            a in point(), b in point(), bulge in any_bulge(),
        ) {
            prop_assume!(a != b);
            let p = match two_point(a, b, bulge) {
                Ok(p) => p,
                Err(error) => {
                    prop_assert_eq!(error.code(), "geom.precision.insufficient");
                    return Ok(());
                }
            };
            let segment = p.segment(0).unwrap();
            prop_assert_eq!(segment.point_at_fraction(0.0), a);
            prop_assert_eq!(segment.point_at_fraction(1.0), b);
            prop_assert_eq!(p.point_at(1.0), b);
            // Rounding grows with the coordinates' own size, not only with the chord.
            let scale = scale(a, b, bulge).max(a.coords.norm()).max(b.coords.norm());
            let close = 16.0 * f64::EPSILON * scale;
            // The middle of the arc lies the sagitta (half chord times bulge) off the chord.
            let c = b - a;
            let middle = Point2::from((a.coords + b.coords) * 0.5) - perp(&c) * (0.5 * bulge);
            prop_assert!((p.point_at(0.5) - middle).norm() <= close);
            // In the chord frame every point satisfies 2b·x(L − x) = y(2b·y − L(1 − b²)), whose
            // gradient has length L(1 + b²) on the circle.
            let l = c.norm();
            let (u, v) = (c / l, perp(&c) / l);
            let samples: Vec<Point2> = (0..=2000)
                .map(|i| p.point_at(f64::from(i) / 2000.0))
                .collect();
            for q in &samples {
                let (x, y) = ((q - a).dot(&u), (q - a).dot(&v));
                let g = 2.0 * bulge * x * (l - x) - y * (2.0 * bulge * y - l * (1.0 - bulge * bulge));
                prop_assert!(g.abs() / (l * (1.0 + bulge * bulge)) <= 64.0 * f64::EPSILON * scale);
            }
            let boxed = p.bounding_box();
            let sampled = Aabb2::from_points(&samples);
            prop_assert!(boxed.expanded(close).contains(&sampled));
            prop_assert!((boxed.min() - sampled.min()).norm() <= 1e-5 * scale);
            prop_assert!((boxed.max() - sampled.max()).norm() <= 1e-5 * scale);
        }

        #[test]
        fn bulge_segments_join_the_vertices(p in polyline()) {
            let n = p.segment_count();
            for k in 0..n {
                let segment = p.segment(k).unwrap();
                let start = p.vertices()[k].point;
                let end = p.vertices()[(k + 1) % p.vertices().len()].point;
                prop_assert!((segment.point_at_fraction(0.0) - start).norm() < 1e-9);
                prop_assert!((segment.point_at_fraction(1.0) - end).norm() < 1e-9);
                prop_assert!((p.point_at(k as f64) - start).norm() < 1e-9);
            }
        }

        #[test]
        fn reversed_polylines_retrace_the_curve(p in polyline(), f in 0.0..=1.0_f64) {
            let n = p.segment_count() as f64;
            let r = p.reversed();
            prop_assert_eq!(r.segment_count(), p.segment_count());
            prop_assert!((r.point_at(n * f) - p.point_at(n * (1.0 - f))).norm() < 1e-9);
            prop_assert_eq!(r.reversed(), p.clone());
            if p.is_closed() {
                prop_assert_eq!(r.vertices()[0].point, p.vertices()[0].point);
            }
        }

        #[test]
        fn polylines_round_trip_through_serde(p in polyline()) {
            let json = serde_json::to_string(&p).unwrap();
            prop_assert_eq!(serde_json::from_str::<Polyline2>(&json).unwrap(), p);
        }

        #[test]
        fn polyline_boxes_contain_samples(p in polyline()) {
            let boxed = p.bounding_box().expanded(1e-9);
            let n = p.segment_count() as f64;
            for i in 0..=1000 {
                prop_assert!(boxed.contains_point(&p.point_at(n * f64::from(i) / 1000.0)));
            }
        }
    }
}
