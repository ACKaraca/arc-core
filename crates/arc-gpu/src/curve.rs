//! Curve items: exact curves flattened for the current zoom (CORE-GPU-010), drawn with
//! polyline widths and dash patterns evaluated in the line shader (CORE-GPU-009).
//!
//! - A [`CurveItem`] holds exact `arc-geom` curves relative to its origin, in the plane
//!   `z = origin.z`. The scene keeps them and flattens them into line segments with a chord
//!   tolerance of `curve_tolerance_px / (2 · s₀)` item units, where `s₀` is the zoom in pixels
//!   per item unit they were flattened for. Each frame compares the item's current zoom `s`
//!   with `s₀`: the item is flattened again only when `s / s₀` leaves `[1/2, 2]` or the
//!   tolerance setting changed. Until then the chord error in pixels is at most
//!   `curve_tolerance_px · s / (2 · s₀) ≤ curve_tolerance_px`. Panning does not change the zoom
//!   of an orthographic view, so it never flattens again; only the flattened items' segments
//!   are written. Straight segments, points and polylines without bulges never depend on the
//!   zoom.
//! - Each flattened vertex carries its exact arc length along the curve (arcs and circles are
//!   divided into equal parts; ellipses are integrated between vertices), so a dash pattern's
//!   position at every vertex is that of the exact curve. The pattern position of a segment
//!   start is reduced to one period in `f64` before narrowing to `f32`.
//! - A [`DashPattern`] is evaluated per fragment: the position along a segment interpolates
//!   linearly between the positions of its ends in the image, so dashes continue across
//!   vertices and stay fixed in the document while the view pans (exact for orthographic
//!   views). Dash ends are cut flat; a zero-length interval is a dot, a disk of the line's
//!   half width.
//! - Polyline widths (in item units, linear along each segment) replace the style width of
//!   the segments that have them; such segments scale with the view and their free ends are
//!   cut flat.
//! - A clip rectangle (item coordinates) limits the item by a scissor rectangle when the view
//!   maps it to an image-aligned rectangle (orthographic views without rotation of the
//!   rectangle on screen); otherwise the item is drawn unclipped.

use std::f64::consts::{FRAC_PI_4, TAU};

use arc_base::math::{Point2, Point3, Vector2};
use arc_base::{Color, Error, Result, Tolerance};
use arc_geom::{Curve2, CurveEval, CurveFlatten, Ellipse2, PolylineSegment};

use crate::error::codes;
use crate::scene::{
    Encoded, LineStyle, Look, PathVertex, SceneLimits, SegmentExtra, check_style, color_array,
    finite_point,
};

/// The most drawn intervals (dashes and dots) of a [`DashPattern`] the shader evaluates.
pub const MAX_DASH_INTERVALS: usize = 4;
/// The most a flattened segment of an arc or ellipse turns, in radians.
pub const CURVE_ANGLE_STEP: f64 = FRAC_PI_4;
/// Curve items are flattened again when the zoom changes by more than this factor.
pub const REFLATTEN_FACTOR: f64 = 2.0;
/// How many times the chord tolerance is doubled when flattening needs more vertices than
/// the limit or a finer resolution than `f64` gives.
const CHORD_RETRIES: u32 = 64;

/// A dash pattern in item units, repeating along each path from its phase.
#[derive(Debug, Clone, PartialEq)]
pub struct DashPattern {
    /// The drawn intervals of one period as `(start, end)` with
    /// `0 ≤ start ≤ end ≤ period`, at most [`MAX_DASH_INTERVALS`]; `start == end` is a dot.
    pub intervals: Vec<[f64; 2]>,
    /// The length of one period; finite and positive.
    pub period: f64,
}

/// The geometry of a [`CurvePath`], relative to the item origin.
#[derive(Debug, Clone, PartialEq)]
pub enum CurveShape {
    /// A point, drawn as a dot.
    Point(Point2),
    /// A bounded curve: a segment, circle, arc, ellipse, elliptical arc or polyline (with
    /// widths). Rays and lines are rejected.
    Curve(Curve2),
}

/// One path of a [`CurveItem`].
#[derive(Debug, Clone, PartialEq)]
pub struct CurvePath {
    /// The geometry.
    pub shape: CurveShape,
    /// The pattern position at the start of the path, in item units.
    pub phase: f64,
    /// Whether the pattern restarts at position 0 at every vertex of a polyline after the
    /// first; otherwise one pattern runs over the whole path.
    pub restart_at_vertices: bool,
}

impl CurvePath {
    /// A path of `curve` starting at phase 0 with one pattern over the whole curve.
    #[must_use]
    pub fn curve(curve: Curve2) -> Self {
        Self {
            shape: CurveShape::Curve(curve),
            phase: 0.0,
            restart_at_vertices: false,
        }
    }
}

/// Exact curves drawn with one style, color and pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveItem {
    /// The model-space point the curves are relative to.
    pub origin: Point3,
    /// The paths; at least one.
    pub paths: Vec<CurvePath>,
    /// The style; polyline widths replace its width where they are not zero.
    pub style: LineStyle,
    /// The color; alpha blends.
    pub color: Color,
    /// The dash pattern; `None` is solid.
    pub pattern: Option<DashPattern>,
    /// A clip rectangle `[min, max]` in item coordinates; `None` is unclipped.
    pub clip: Option<[Point2; 2]>,
}

/// The zoom and tolerance a curve item is flattened for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Flattened {
    /// Pixels per item unit.
    pub(crate) scale: f64,
    /// The chord tolerance in pixels.
    pub(crate) tolerance_px: f32,
}

/// The retained curves of a curve item.
#[derive(Debug)]
pub(crate) struct CurveState {
    pub(crate) item: CurveItem,
    pub(crate) flattened: Flattened,
    /// Whether any path has a curved part, so its flattening depends on the zoom.
    curved: bool,
}

fn curved(shape: &CurveShape) -> bool {
    match shape {
        CurveShape::Point(_) => false,
        CurveShape::Curve(Curve2::Polyline(p)) => p.vertices().iter().any(|v| v.bulge != 0.0),
        CurveShape::Curve(
            Curve2::Circle(_) | Curve2::Arc(_) | Curve2::Ellipse(_) | Curve2::EllipticalArc(_),
        ) => true,
        CurveShape::Curve(_) => false,
    }
}

impl CurveState {
    pub(crate) fn new(item: CurveItem, flattened: Flattened) -> Self {
        let curved = item.paths.iter().any(|p| curved(&p.shape));
        Self {
            item,
            flattened,
            curved,
        }
    }

    /// The flattening to use at zoom `scale`, or `None` while the current one holds: the zoom
    /// is within [`REFLATTEN_FACTOR`] of the flattened one and the tolerance is unchanged, or
    /// the item has nothing curved.
    pub(crate) fn refresh(&self, scale: f64, tolerance_px: f32) -> Option<Flattened> {
        if !(self.curved && scale.is_finite() && scale > 0.0) {
            return None;
        }
        let ratio = scale / self.flattened.scale;
        let stale = !(1.0 / REFLATTEN_FACTOR..=REFLATTEN_FACTOR).contains(&ratio)
            || tolerance_px != self.flattened.tolerance_px;
        stale.then_some(Flattened {
            scale,
            tolerance_px,
        })
    }
}

/// The zoom a curve item is first flattened for when no frame has been drawn: its extent
/// over 1000 pixels.
pub(crate) fn initial_scale(item: &CurveItem) -> f64 {
    let extent = item
        .paths
        .iter()
        .filter_map(|p| match &p.shape {
            CurveShape::Point(q) => Some(q.coords.amax()),
            CurveShape::Curve(c) => c
                .bounding_box()
                .map(|b| b.min().coords.amax().max(b.max().coords.amax())),
        })
        .fold(0.0, f64::max);
    let scale = 1000.0 / extent;
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn invalid() -> Error {
    Error::new(codes::ITEM_INVALID)
}

/// Validates a curve item, including that its coarsest flattening fits the segment limit.
pub(crate) fn validate(item: &CurveItem, limits: &SceneLimits) -> Result<()> {
    finite_point(&item.origin)?;
    check_style(&item.style, item.color, limits)?;
    if item.paths.is_empty() {
        return Err(invalid());
    }
    for path in &item.paths {
        let finite = match &path.shape {
            CurveShape::Point(p) => p.coords.iter().all(|v| v.is_finite()),
            CurveShape::Curve(Curve2::Ray(_) | Curve2::Line(_)) => return Err(invalid()),
            CurveShape::Curve(_) => true,
        };
        if !(finite && path.phase.is_finite()) {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
    }
    if let Some(pattern) = &item.pattern {
        let period = pattern.period;
        let valid = period.is_finite()
            && period > 0.0
            && pattern.intervals.len() <= MAX_DASH_INTERVALS
            && pattern
                .intervals
                .iter()
                .all(|[a, b]| 0.0 <= *a && a <= b && *b <= period);
        if !valid {
            return Err(invalid());
        }
    }
    if let Some([low, high]) = item.clip {
        let finite = low
            .coords
            .iter()
            .chain(high.coords.iter())
            .all(|v| v.is_finite());
        if !finite {
            return Err(Error::new(codes::VALUE_NOT_FINITE));
        }
        if !(low.x < high.x && low.y < high.y) {
            return Err(invalid());
        }
    }
    // The coarsest flattening: no chord bound, only the angle step.
    encode_at(item, f64::MAX.sqrt(), limits, 0).map(|_| ())
}

/// The segments of a curve item flattened for `flattened`. When the chord tolerance needs
/// more segments than the limit or a finer resolution than `f64` gives at the curve, it is
/// doubled until the flattening succeeds, so at extreme zooms the error may exceed the
/// tolerance instead of failing the frame.
pub(crate) fn encode(
    item: &CurveItem,
    flattened: Flattened,
    limits: &SceneLimits,
    slot: u32,
) -> Result<Encoded> {
    let mut chord = f64::from(flattened.tolerance_px) / (REFLATTEN_FACTOR * flattened.scale);
    let mut last = Error::new(codes::ITEM_INVALID);
    for _ in 0..CHORD_RETRIES {
        if !(chord.is_finite() && chord > 0.0) {
            break;
        }
        match encode_at(item, chord, limits, slot) {
            Ok(encoded) => return Ok(encoded),
            Err(e)
                if matches!(
                    e.code(),
                    "geom.precision.insufficient"
                        | "geom.flatten.too_many_vertices"
                        | "gpu.limit.exceeded"
                ) =>
            {
                last = e;
                chord *= 2.0;
            }
            Err(e) => return Err(e),
        }
    }
    // The coarsest flattening fits whenever the item was validated.
    encode_at(item, f64::MAX.sqrt(), limits, slot).map_err(|_| last)
}

fn pattern_floats(pattern: &DashPattern) -> [f32; 9] {
    let mut out = crate::scene::SOLID;
    out[0] = narrow(pattern.period);
    for (k, [a, b]) in pattern
        .intervals
        .iter()
        .take(MAX_DASH_INTERVALS)
        .enumerate()
    {
        if let Some(slot) = out.get_mut(1 + 2 * k) {
            *slot = narrow(*a);
        }
        if let Some(slot) = out.get_mut(2 + 2 * k) {
            *slot = narrow(*b);
        }
    }
    out
}

fn narrow(value: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // Narrowing to the GPU's precision is the intent.
    let narrowed = value as f32;
    narrowed
}

fn encode_at(item: &CurveItem, chord: f64, limits: &SceneLimits, slot: u32) -> Result<Encoded> {
    let look = Look {
        style: &item.style,
        color: color_array(item.color),
        pattern: item.pattern.as_ref().map(pattern_floats),
        slot,
    };
    let period = item.pattern.as_ref().map(|p| p.period);
    let max_vertices = usize::try_from(limits.max_segments_per_item)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    let mut out = Encoded::default();
    for path in &item.paths {
        let (vertices, closed) = flatten_path(path, chord, period, max_vertices)?;
        out.path(vertices, closed, &look, limits)?;
    }
    Ok(out)
}

/// A flattened vertex: its point, its arc length from the start of its piece and its
/// fraction of the piece's parameter range.
type Sample = (Point2, f64, f64);

/// The eccentric angle of a point of `ellipse`, in (−π, π].
fn eccentric(ellipse: &Ellipse2, p: Point2) -> f64 {
    let d = p - ellipse.center();
    let along = |axis: Vector2| {
        let n = axis.norm();
        d.dot(&(axis / n)) / n
    };
    along(ellipse.minor_axis()).atan2(along(ellipse.major_axis()))
}

/// The flattened vertices of a curve other than a polyline, with exact arc lengths when
/// `lengths` is set.
fn flatten_piece(
    curve: &Curve2,
    chord: f64,
    max_vertices: usize,
    lengths: bool,
) -> Result<Vec<Sample>> {
    let tolerance = Tolerance::DEFAULT;
    if let Curve2::Segment(s) = curve {
        let length = crate::scene::length(&arc_base::math::Vector3::new(
            s.end().x - s.start().x,
            s.end().y - s.start().y,
            0.0,
        ));
        return Ok(vec![(s.start(), 0.0, 0.0), (s.end(), length, 1.0)]);
    }
    let points = curve.flatten(chord, CURVE_ANGLE_STEP, max_vertices)?;
    let last = points.len().saturating_sub(1).max(1);
    #[allow(clippy::cast_precision_loss)] // Vertex counts are far below 2^52.
    let fraction = |i: usize| i as f64 / last as f64;
    let (_, end) = curve.parameter_range();
    match curve {
        // Equal parts of the angle are equal parts of the length.
        Curve2::Circle(_) | Curve2::Arc(_) => {
            let total = if lengths {
                curve.length(&tolerance)?
            } else {
                0.0
            };
            Ok(points
                .into_iter()
                .enumerate()
                .map(|(i, p)| (p, total * fraction(i), fraction(i)))
                .collect())
        }
        Curve2::Ellipse(_) | Curve2::EllipticalArc(_) => {
            let (ellipse, start, sign) = match curve {
                Curve2::EllipticalArc(a) => (*a.ellipse(), a.start_parameter(), a.sweep().signum()),
                Curve2::Ellipse(e) => (*e, 0.0, 1.0),
                _ => return Err(invalid()),
            };
            let mut out = Vec::with_capacity(points.len());
            let (mut t_prev, mut s) = (0.0, 0.0);
            for (i, p) in points.into_iter().enumerate() {
                let t = if i == 0 {
                    0.0
                } else if i == last {
                    end
                } else {
                    ((eccentric(&ellipse, p) - start) * sign).rem_euclid(TAU)
                };
                if lengths && i > 0 {
                    s += curve.length_between(t_prev, t, &tolerance)?;
                }
                t_prev = t;
                out.push((p, s, t / end));
            }
            Ok(out)
        }
        _ => Err(invalid()),
    }
}

/// The vertices of a path with the extra data of each segment, and whether it is closed.
fn flatten_path(
    path: &CurvePath,
    chord: f64,
    period: Option<f64>,
    max_vertices: usize,
) -> Result<(Vec<PathVertex>, bool)> {
    let curve = match &path.shape {
        CurveShape::Point(p) => {
            return Ok((
                vec![PathVertex {
                    point: point3(*p),
                    out: SegmentExtra::default(),
                }],
                false,
            ));
        }
        CurveShape::Curve(c) => c,
    };
    // Pieces with their widths at both ends.
    let pieces: Vec<(Curve2, [f64; 2])> = match curve {
        Curve2::Polyline(p) => p
            .segments()
            .zip(p.vertices())
            .map(|(segment, v)| {
                let piece = match segment {
                    PolylineSegment::Line(s) => Curve2::Segment(s),
                    PolylineSegment::Arc(a) => Curve2::Arc(a),
                };
                (piece, [v.start_width, v.end_width])
            })
            .collect(),
        other => vec![(other.clone(), [0.0, 0.0])],
    };
    let closed = curve.is_closed();
    let mut vertices: Vec<PathVertex> = Vec::new();
    let (mut base, mut start_phase) = (0.0, path.phase);
    for (k, (piece, [w0, w1])) in pieces.iter().enumerate() {
        if path.restart_at_vertices && k > 0 {
            (base, start_phase) = (0.0, 0.0);
        }
        let samples = flatten_piece(piece, chord, max_vertices, period.is_some())?;
        if vertices.len().saturating_add(samples.len()) > max_vertices.saturating_add(pieces.len())
        {
            return Err(Error::new(
                arc_geom::error::codes::FLATTEN_TOO_MANY_VERTICES,
            ));
        }
        let wide = *w0 > 0.0 || *w1 > 0.0;
        let width = |f: f64| (w1 - w0).mul_add(f, *w0);
        for pair in samples.windows(2) {
            let [(p, s0, f0), (_, s1, f1)] = pair else {
                continue;
            };
            let phase = match period {
                Some(period) => {
                    let start = (start_phase + base + s0).rem_euclid(period);
                    [narrow(start), narrow(start + (s1 - s0))]
                }
                None => [0.0, 0.0],
            };
            let out = SegmentExtra {
                widths: wide.then(|| [narrow(width(*f0)), narrow(width(*f1))]),
                phase,
            };
            // The first sample of a piece repeats the last vertex: it takes the new segment.
            match vertices.last_mut() {
                Some(last) if last.point == point3(*p) => last.out = out,
                _ => vertices.push(PathVertex {
                    point: point3(*p),
                    out,
                }),
            }
        }
        if let Some((p, s, _)) = samples.last() {
            vertices.push(PathVertex {
                point: point3(*p),
                out: SegmentExtra::default(),
            });
            base += s;
        }
    }
    Ok((vertices, closed))
}

fn point3(p: Point2) -> [f32; 3] {
    [narrow(p.x), narrow(p.y), 0.0]
}

#[cfg(test)]
mod tests {
    use arc_geom::{Arc2, Polyline2, PolylineVertex, Segment2};

    use super::*;

    fn item(paths: Vec<CurvePath>) -> CurveItem {
        CurveItem {
            origin: Point3::origin(),
            paths,
            style: LineStyle::round(crate::LineWidth::Screen(1.0)),
            color: Color::WHITE,
            pattern: None,
            clip: None,
        }
    }

    #[test]
    fn refresh_follows_the_factor_of_two_rule() {
        let arc = Curve2::Arc(Arc2::new(Point2::origin(), 1.0, 0.0, 1.0).unwrap());
        let f = Flattened {
            scale: 10.0,
            tolerance_px: 0.5,
        };
        let state = CurveState::new(item(vec![CurvePath::curve(arc)]), f);
        assert_eq!(state.refresh(19.9, 0.5), None);
        assert_eq!(state.refresh(5.1, 0.5), None);
        assert!(state.refresh(20.1, 0.5).is_some());
        assert!(state.refresh(4.9, 0.5).is_some());
        assert!(state.refresh(10.0, 0.25).is_some());
        let line = Curve2::Segment(Segment2::new(Point2::origin(), Point2::new(1.0, 0.0)).unwrap());
        let straight = CurveState::new(item(vec![CurvePath::curve(line)]), f);
        assert_eq!(straight.refresh(1e6, 0.5), None);
    }

    #[test]
    fn arc_lengths_and_widths_follow_the_exact_curve() {
        // A half circle of radius 1 then a straight segment, with widths 0.2 → 0.4 on the arc.
        let polyline = Polyline2::new(
            vec![
                PolylineVertex {
                    point: Point2::new(0.0, 0.0),
                    bulge: 1.0,
                    start_width: 0.2,
                    end_width: 0.4,
                },
                PolylineVertex::new(Point2::new(2.0, 0.0), 0.0),
                PolylineVertex::new(Point2::new(5.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let path = CurvePath::curve(Curve2::Polyline(polyline));
        let (vertices, closed) = flatten_path(&path, 1e-3, Some(100.0), 10_000).unwrap();
        assert!(!closed);
        let last_arc = vertices
            .iter()
            .rposition(|v| v.out.widths.is_some())
            .unwrap();
        let arc = vertices[last_arc].out;
        assert!((f64::from(arc.phase[1]) - std::f64::consts::PI).abs() < 1e-5);
        let widths = arc.widths.unwrap();
        assert!((widths[1] - 0.4).abs() < 1e-6, "{widths:?}");
        let line = vertices[last_arc + 1].out;
        assert_eq!(line.widths, None);
        assert!((f64::from(line.phase[1]) - (std::f64::consts::PI + 3.0)).abs() < 1e-5);
    }

    proptest::proptest! {
        /// CORE-GPU-010: an arc flattened for zoom `s₀` stays within the pixel tolerance at
        /// every zoom up to `2 · s₀`, for radii and zooms over twelve orders of magnitude.
        #[test]
        fn arc_chord_error_stays_below_the_tolerance_within_a_factor_of_two(
            radius in 1e-3_f64..1e6,
            start in -3.0_f64..3.0,
            sweep in 0.01_f64..6.25,
            exponent in -6.0_f64..6.0,
            tolerance_px in 0.05_f32..4.0,
        ) {
            let scale = 10_f64.powf(exponent) / radius;
            let arc = Arc2::new(Point2::new(radius, -radius), radius, start, sweep).unwrap();
            let chord = f64::from(tolerance_px) / (REFLATTEN_FACTOR * scale);
            let samples = flatten_piece(&Curve2::Arc(arc), chord, 1_000_001, true).unwrap();
            let center = Point2::new(radius, -radius);
            for pair in samples.windows(2) {
                let mid = Point2::from((pair[0].0.coords + pair[1].0.coords) / 2.0);
                let sagitta = radius - (mid - center).norm();
                let error_px = sagitta * REFLATTEN_FACTOR * scale;
                proptest::prop_assert!(
                    error_px <= f64::from(tolerance_px) * (1.0 + 1e-6) + 1e-9 * radius * scale,
                    "{error_px} px"
                );
            }
            // The lengths are those of the exact arc.
            let total = samples.last().unwrap().1;
            proptest::prop_assert!((total - radius * sweep).abs() <= 1e-9 * radius * sweep);
        }
    }

    #[test]
    fn invalid_items_are_rejected() {
        let ray = Curve2::Ray(arc_geom::Ray2::new(Point2::origin(), Vector2::x()).unwrap());
        let limits = SceneLimits::DEFAULT;
        assert_eq!(
            validate(&item(vec![CurvePath::curve(ray)]), &limits)
                .unwrap_err()
                .code(),
            "gpu.item.invalid"
        );
        let seg = || {
            CurvePath::curve(Curve2::Segment(
                Segment2::new(Point2::origin(), Point2::new(1.0, 0.0)).unwrap(),
            ))
        };
        let mut dashed = item(vec![seg()]);
        dashed.pattern = Some(DashPattern {
            intervals: vec![[0.0, 2.0]],
            period: 1.0,
        });
        assert_eq!(
            validate(&dashed, &limits).unwrap_err().code(),
            "gpu.item.invalid"
        );
        let mut clipped = item(vec![seg()]);
        clipped.clip = Some([Point2::new(1.0, 0.0), Point2::new(0.0, 1.0)]);
        assert_eq!(
            validate(&clipped, &limits).unwrap_err().code(),
            "gpu.item.invalid"
        );
        assert!(validate(&item(vec![]), &limits).is_err());
        let mut nan = item(vec![seg()]);
        nan.paths[0].phase = f64::NAN;
        assert_eq!(
            validate(&nan, &limits).unwrap_err().code(),
            "gpu.value.not_finite"
        );
    }
}
