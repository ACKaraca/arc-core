//! Linetype patterns and dash generation (CORE-DRAFT-018).
//!
//! [`dashes`] is a pure function from an `arc-geom` curve, a [`Pattern`] and [`DashOptions`] to
//! the drawn pieces of the curve, each given by its arc-length interval and the matching curve
//! parameters:
//!
//! - The pattern starts with its first element at the start of the curve (phase 0) and repeats
//!   until the end of the curve, where the last element is cut. Element lengths are multiplied
//!   by `global_scale · object_scale`.
//! - A dash draws its length; a gap, dot, text or shape element draws nothing over its length,
//!   and a dot also draws a piece of length zero at its start. Text and shape elements are kept
//!   as data; drawing them waits for the text units.
//! - A polyline runs one pattern over its whole length when `continuous` is set, so a dash may
//!   span a vertex; otherwise the pattern restarts at every vertex.
//! - Output is bounded: the walk visits at most `(⌈run length / scaled period⌉ + 1) · elements`
//!   pattern elements per run. When the sum over all runs exceeds `max_dashes`, or positions
//!   stop advancing in `f64` before a run ends, the result is [`Dashes::Continuous`], the whole
//!   curve drawn solid. Rays and construction lines are unbounded and are always
//!   continuous; dashing them waits for display-window clipping.

use arc_base::{Error, Result, Tolerance};
use arc_doc::Value;
use arc_geom::{Curve2, CurveEval};

use crate::error::codes;
use crate::types::{ElementKind, table};

/// The default bound on the pieces one call generates.
pub const DEFAULT_MAX_DASHES: usize = 100_000;

/// One element of a pattern: its kind and its length in pattern units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Element {
    /// The kind.
    pub kind: ElementKind,
    /// The length; finite and not negative.
    pub length: f64,
}

/// A validated linetype pattern: at least one element and a positive total length.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    elements: Vec<Element>,
    period: f64,
}

impl Pattern {
    /// A pattern of `elements`.
    ///
    /// # Errors
    ///
    /// `draft.linetype.pattern_empty` when a length is negative or not finite, or the total
    /// length is not a finite positive number.
    pub fn new(elements: Vec<Element>) -> Result<Self> {
        let period: f64 = elements.iter().map(|e| e.length).sum();
        let valid = elements
            .iter()
            .all(|e| e.length.is_finite() && e.length >= 0.0)
            && period.is_finite()
            && period > 0.0;
        if valid {
            Ok(Self { elements, period })
        } else {
            Err(Error::new(codes::LINETYPE_PATTERN_EMPTY))
        }
    }

    /// The pattern of the field values of a `core.draft.linetype` object.
    ///
    /// # Errors
    ///
    /// As [`Pattern::new`]; a missing element list is an empty pattern.
    pub fn from_values(values: &[Option<Value>]) -> Result<Self> {
        let elements = match values.get(table::LINETYPE_ELEMENTS) {
            Some(Some(Value::List(list))) => list
                .iter()
                .filter_map(|element| match element {
                    Value::Record(fields) => Some(Element {
                        kind: ElementKind::of(crate::types::enum_at(fields, 0)),
                        length: crate::types::f64_at(fields, 1).unwrap_or(0.0),
                    }),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Self::new(elements)
    }

    /// The elements.
    #[must_use]
    pub fn elements(&self) -> &[Element] {
        &self.elements
    }

    /// The total length of one repetition in pattern units.
    #[must_use]
    pub fn period(&self) -> f64 {
        self.period
    }
}

/// The parameters of [`dashes`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DashOptions {
    /// The document-wide linetype scale.
    pub global_scale: f64,
    /// The entity's `linetype_scale`.
    pub object_scale: f64,
    /// Whether a polyline runs one pattern across its vertices.
    pub continuous: bool,
    /// The most pattern elements to visit before falling back to [`Dashes::Continuous`]; it
    /// bounds the number of pieces as well.
    pub max_dashes: usize,
}

impl Default for DashOptions {
    fn default() -> Self {
        Self {
            global_scale: 1.0,
            object_scale: 1.0,
            continuous: false,
            max_dashes: DEFAULT_MAX_DASHES,
        }
    }
}

/// One drawn piece of a curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dash {
    /// The arc length from the start of the curve where the piece starts.
    pub start: f64,
    /// The arc length where it ends; equal to `start` for a dot.
    pub end: f64,
    /// The curve parameter at `start`.
    pub t0: f64,
    /// The curve parameter at `end`.
    pub t1: f64,
}

/// The result of [`dashes`].
#[derive(Debug, Clone, PartialEq)]
pub enum Dashes {
    /// The drawn pieces in order along the curve.
    Pieces(Vec<Dash>),
    /// The curve is drawn solid: unbounded, or over the output bound.
    Continuous,
}

/// The number of pattern elements a walk over a run of `length` visits at most, as `f64`.
fn steps_for(pattern: &Pattern, scale: f64, length: f64) -> f64 {
    let count = f64::from(u32::try_from(pattern.elements.len()).unwrap_or(u32::MAX));
    ((length / (pattern.period * scale)).ceil() + 1.0) * count
}

/// The arc-length intervals a pattern draws over `[from, to]`, starting at phase 0, appended
/// to `out`; `false` when the walk visits more than `step_limit` elements, which happens only
/// when positions stop advancing in `f64`.
fn walk(
    pattern: &Pattern,
    scale: f64,
    (from, to): (f64, f64),
    step_limit: f64,
    out: &mut Vec<(f64, f64)>,
) -> bool {
    let mut at = from;
    let mut steps = 0.0;
    for element in pattern.elements.iter().cycle() {
        if at >= to {
            return true;
        }
        steps += 1.0;
        if steps > step_limit {
            return false;
        }
        let end = (at + element.length * scale).min(to);
        match element.kind {
            ElementKind::Dash => out.push((at, end)),
            ElementKind::Dot => out.push((at, at)),
            ElementKind::Gap | ElementKind::Text | ElementKind::Shape => {}
        }
        at = end;
    }
    true
}

/// The drawn pieces of `curve` with `pattern`, as described in the [module
/// documentation](self).
///
/// # Errors
///
/// `draft.linetype.scale_invalid` when a scale or their product is not a finite positive
/// number; the length errors of [`CurveEval`] (`geom.tolerance.invalid`,
/// `geom.precision.insufficient`).
pub fn dashes(
    curve: &Curve2,
    pattern: &Pattern,
    options: &DashOptions,
    tolerance: &Tolerance,
) -> Result<Dashes> {
    let scale = options.global_scale * options.object_scale;
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !(positive(options.global_scale) && positive(options.object_scale) && positive(scale)) {
        return Err(Error::new(codes::LINETYPE_SCALE_INVALID)
            .with_detail("global_scale", options.global_scale)
            .with_detail("object_scale", options.object_scale));
    }
    let total = curve.length(tolerance)?;
    if !total.is_finite() {
        return Ok(Dashes::Continuous);
    }
    // The runs of the pattern: the whole curve, or each polyline segment.
    let mut runs = Vec::new();
    match curve {
        Curve2::Polyline(polyline) if !options.continuous => {
            let mut start = 0.0;
            for index in 1..=polyline.segment_count() {
                let end = if index == polyline.segment_count() {
                    total
                } else {
                    let t = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
                    curve.length_between(0.0, t, tolerance)?
                };
                runs.push((start, end.max(start)));
                start = end.max(start);
            }
        }
        _ => runs.push((0.0, total)),
    }
    let limits: Vec<f64> = runs
        .iter()
        .map(|(from, to)| steps_for(pattern, scale, to - from))
        .collect();
    let budget = f64::from(u32::try_from(options.max_dashes).unwrap_or(u32::MAX));
    let expected: f64 = limits.iter().sum();
    if expected.is_nan() || expected > budget {
        return Ok(Dashes::Continuous);
    }
    let mut intervals = Vec::new();
    for (run, limit) in runs.into_iter().zip(limits) {
        if !walk(pattern, scale, run, limit, &mut intervals) {
            return Ok(Dashes::Continuous);
        }
    }
    let parameter = |s: f64| curve.parameter_at_length(s.clamp(0.0, total), tolerance);
    let mut out = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        out.push(Dash {
            start,
            end,
            t0: parameter(start)?,
            t1: parameter(end)?,
        });
    }
    Ok(Dashes::Pieces(out))
}

#[cfg(test)]
mod tests {
    use arc_base::math::{Point2, Vector2};
    use arc_geom::{Arc2, Polyline2, PolylineVertex, Ray2, Segment2};
    use proptest::prelude::*;

    use super::*;

    fn tol() -> Tolerance {
        Tolerance::DEFAULT
    }

    fn el(kind: ElementKind, length: f64) -> Element {
        Element { kind, length }
    }

    fn dash_gap(d: f64, g: f64) -> Pattern {
        Pattern::new(vec![el(ElementKind::Dash, d), el(ElementKind::Gap, g)]).unwrap()
    }

    fn segment(length: f64) -> Curve2 {
        Curve2::Segment(Segment2::new(Point2::new(0.0, 0.0), Point2::new(length, 0.0)).unwrap())
    }

    fn pieces(result: Dashes) -> Vec<(f64, f64)> {
        match result {
            Dashes::Pieces(p) => p.iter().map(|d| (d.start, d.end)).collect(),
            Dashes::Continuous => panic!("continuous"),
        }
    }

    fn close(a: &[(f64, f64)], b: &[(f64, f64)], eps: f64) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| (x.0 - y.0).abs() <= eps && (x.1 - y.1).abs() <= eps)
    }

    #[test]
    fn a_segment_gets_whole_dashes_and_a_cut_last_one() {
        let pattern = Pattern::new(vec![
            el(ElementKind::Dash, 2.0),
            el(ElementKind::Gap, 1.0),
            el(ElementKind::Dot, 0.0),
            el(ElementKind::Text, 1.0),
        ])
        .unwrap();
        let got = pieces(dashes(&segment(9.0), &pattern, &DashOptions::default(), &tol()).unwrap());
        assert_eq!(
            got,
            [(0.0, 2.0), (3.0, 3.0), (4.0, 6.0), (7.0, 7.0), (8.0, 9.0)]
        );
    }

    #[test]
    fn invalid_patterns_scales_and_bounds() {
        assert!(Pattern::new(vec![]).is_err());
        assert!(Pattern::new(vec![el(ElementKind::Dot, 0.0)]).is_err());
        assert!(
            Pattern::new(vec![el(ElementKind::Dash, -1.0), el(ElementKind::Gap, 2.0)]).is_err()
        );
        assert!(
            Pattern::new(vec![
                el(ElementKind::Dash, f64::MAX),
                el(ElementKind::Gap, f64::MAX)
            ])
            .is_err()
        );
        let p = dash_gap(1.0, 1.0);
        let bad = DashOptions {
            object_scale: 0.0,
            ..DashOptions::default()
        };
        assert_eq!(
            dashes(&segment(1.0), &p, &bad, &tol()).unwrap_err().code(),
            "draft.linetype.scale_invalid"
        );
        let over = DashOptions {
            global_scale: 1e300,
            object_scale: 1e300,
            ..DashOptions::default()
        };
        assert!(dashes(&segment(1.0), &p, &over, &tol()).is_err());
        let many = DashOptions {
            max_dashes: 10,
            ..DashOptions::default()
        };
        assert_eq!(
            dashes(&segment(100.0), &p, &many, &tol()).unwrap(),
            Dashes::Continuous
        );
        assert_eq!(
            dashes(&segment(1e300), &p, &DashOptions::default(), &tol()).unwrap(),
            Dashes::Continuous
        );
        let ray = Curve2::Ray(Ray2::new(Point2::new(0.0, 0.0), Vector2::new(1.0, 0.0)).unwrap());
        assert_eq!(
            dashes(&ray, &p, &DashOptions::default(), &tol()).unwrap(),
            Dashes::Continuous
        );
        // A dash far below the precision of the position still terminates.
        let tiny = dash_gap(1e-300, 1.0);
        let long =
            Curve2::Segment(Segment2::new(Point2::new(0.0, 0.0), Point2::new(5.0, 0.0)).unwrap());
        assert_eq!(
            pieces(dashes(&long, &tiny, &DashOptions::default(), &tol()).unwrap()).len(),
            5
        );
    }

    /// A staircase polyline whose segments have `lengths`, turning at every vertex.
    fn polyline(lengths: &[f64]) -> Curve2 {
        let mut at = Point2::new(0.0, 0.0);
        let mut vertices = vec![PolylineVertex::new(at, 0.0)];
        for (i, l) in lengths.iter().enumerate() {
            at = if i % 2 == 0 {
                Point2::new(at.x + l, at.y)
            } else {
                Point2::new(at.x, at.y + l)
            };
            vertices.push(PolylineVertex::new(at, 0.0));
        }
        Curve2::Polyline(Polyline2::new(vertices, false).unwrap())
    }

    proptest! {
        #[test]
        fn dashes_and_gaps_add_up_to_the_curve_length(
            d in 1e-3_f64..10.0, g in 0.0_f64..10.0, length in 1e-3_f64..1000.0,
            exp in -100_i32..100,
        ) {
            let k = 10f64.powi(exp);
            let curve = segment(length * k);
            let options = DashOptions { object_scale: k, ..DashOptions::default() };
            let result = dashes(&curve, &dash_gap(d, g), &options, &tol()).unwrap();
            let Dashes::Pieces(p) = result else { return Ok(()); };
            let total = length * k;
            let mut drawn = 0.0;
            let mut blank = 0.0;
            let mut at = 0.0;
            for dash in &p {
                prop_assert!(dash.start >= at && dash.end >= dash.start && dash.end <= total);
                prop_assert!(dash.end - dash.start <= d * k * (1.0 + 1e-12));
                blank += dash.start - at;
                drawn += dash.end - dash.start;
                at = dash.end;
            }
            blank += total - at;
            prop_assert!((drawn + blank - total).abs() <= 1e-9 * total);
        }

        #[test]
        fn a_continuous_polyline_dashes_like_a_straight_line_of_its_length(
            lengths in prop::collection::vec(0.5_f64..20.0, 1..6),
            d in 0.1_f64..5.0, g in 0.1_f64..5.0,
        ) {
            let pl = polyline(&lengths);
            let total = pl.length(&tol()).unwrap();
            let pattern = dash_gap(d, g);
            let on = DashOptions { continuous: true, ..DashOptions::default() };
            let a = pieces(dashes(&pl, &pattern, &on, &tol()).unwrap());
            let b = pieces(dashes(&segment(total), &pattern, &on, &tol()).unwrap());
            prop_assert!(close(&a, &b, 1e-9 * total));
            // Without continuity every segment restarts the pattern.
            let off = pieces(dashes(&pl, &pattern, &DashOptions::default(), &tol()).unwrap());
            let mut expected = Vec::new();
            let mut start = 0.0;
            for l in &lengths {
                let seg = pieces(dashes(&segment(*l), &pattern, &on, &tol()).unwrap());
                expected.extend(seg.iter().map(|(s, e)| (s + start, e + start)));
                start += l;
            }
            prop_assert!(close(&off, &expected, 1e-9 * total));
        }

        #[test]
        fn scaling_curve_and_pattern_together_scales_the_dashes(
            radius in 0.1_f64..100.0, sweep in 0.1_f64..6.0, k in 1e-6_f64..1e6,
            d in 0.05_f64..2.0, g in 0.05_f64..2.0,
        ) {
            let arc = |r: f64| Curve2::Arc(Arc2::new(Point2::new(0.0, 0.0), r, 0.3, sweep).unwrap());
            let pattern = dash_gap(d, g);
            let base = dashes(&arc(radius), &pattern, &DashOptions::default(), &tol()).unwrap();
            let scaled_options = DashOptions { global_scale: k, ..DashOptions::default() };
            let scaled = dashes(&arc(radius * k), &pattern, &scaled_options, &tol()).unwrap();
            let base = pieces(base);
            let expected: Vec<_> = base.iter().map(|(s, e)| (s * k, e * k)).collect();
            prop_assert!(close(&pieces(scaled), &expected, 1e-9 * radius * k * 7.0));
        }
    }
}
