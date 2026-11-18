//! What dimensions, leaders and multi-leaders share: arrowheads, the parts of their drawn
//! geometry, and annotation scaling (CORE-DRAFT-014).
//!
//! An annotative object has the field `annotative` set and a list `scales` of
//! [`Representation`]s, one per supported annotation scale. Its sizes are paper sizes: in the
//! representation for the scale `s` (paper length per model length) they are divided by `s`, so
//! they measure their paper size in a viewport of annotation scale `s`. Its display list holds
//! the primitives of every representation, each tagged with the scale; assembly keeps those of
//! the current scale and the untagged ones. A non-annotative object multiplies its sizes by the
//! style's overall scale and is shown at every scale.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result};
use arc_doc::{Bound, FieldDecl, Finding, Value, ValueType};
use arc_geom::{Arc2, Curve2, Polyline2, PolylineVertex, Segment2};

use crate::error::codes;
use crate::types::{finding, flag};

/// The most representations of one annotative object.
pub const MAX_SCALES: u64 = 256;

/// An arrowhead shape, with the enum code of the style field `arrow`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Arrow {
    /// A filled triangle: tip, and two corners one size back and a sixth of the size aside.
    #[default]
    ClosedFilled = 0,
    /// The same triangle as a closed outline.
    Closed = 1,
    /// The two sides of the triangle without its base.
    Open = 2,
    /// A filled disc of a quarter size radius centered on the tip.
    Dot = 3,
    /// A stroke of one size through the tip at 45 degrees counterclockwise from the direction.
    Tick = 4,
    /// Nothing.
    None = 5,
}

impl Arrow {
    /// The arrow of an enum code; unknown codes read as [`Arrow::ClosedFilled`].
    #[must_use]
    pub fn of(code: u32) -> Self {
        match code {
            1 => Self::Closed,
            2 => Self::Open,
            3 => Self::Dot,
            4 => Self::Tick,
            5 => Self::None,
            _ => Self::ClosedFilled,
        }
    }
}

/// The part of an annotation a primitive draws: the `part` of its sub-object key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The dimension line or arc, a leader line, a radial or ordinate leg.
    Line = 0,
    /// An extension line.
    Extension = 1,
    /// An arrowhead.
    Arrow = 2,
    /// Text.
    Text = 3,
    /// A box of a tolerance frame.
    Frame = 4,
    /// The landing of a leader.
    Landing = 5,
    /// Block content of a multi-leader.
    Content = 6,
}

/// Where a text block goes: its center and its rotation in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextPlacement {
    /// The center of the text block.
    pub center: Point2,
    /// The direction of the baseline.
    pub rotation: f64,
}

/// The drawn geometry of an annotation in the coordinates of its container.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drawn {
    /// Strokes with their parts.
    pub strokes: Vec<(Part, Curve2)>,
    /// Filled regions (filled arrowheads) with their parts.
    pub fills: Vec<(Part, Polyline2)>,
    /// The text block, if any.
    pub text: Option<TextPlacement>,
}

impl Drawn {
    /// Adds the segment `a`–`b`; nothing when the ends coincide.
    ///
    /// # Errors
    ///
    /// The `arc-geom` errors of a segment with ends that are not finite.
    pub fn line(&mut self, part: Part, a: Point2, b: Point2) -> Result<()> {
        if a != b {
            self.strokes
                .push((part, Curve2::Segment(Segment2::new(a, b)?)));
        }
        Ok(())
    }

    /// Adds the counterclockwise arc about `center` of `radius` from `start` through `sweep`.
    ///
    /// # Errors
    ///
    /// The `arc-geom` errors of the arc.
    pub fn arc(&mut self, center: Point2, radius: f64, start: f64, sweep: f64) -> Result<()> {
        self.strokes.push((
            Part::Line,
            Curve2::Arc(Arc2::new(center, radius, start, sweep)?),
        ));
        Ok(())
    }

    /// Adds an open polyline through `points`, with repeated consecutive points dropped;
    /// nothing for fewer than two distinct points.
    ///
    /// # Errors
    ///
    /// The `arc-geom` errors of the polyline.
    pub fn polyline(&mut self, part: Part, points: &[Point2]) -> Result<()> {
        let mut vertices: Vec<PolylineVertex> = Vec::with_capacity(points.len());
        for p in points {
            if vertices.last().is_none_or(|v| v.point != *p) {
                vertices.push(PolylineVertex::new(*p, 0.0));
            }
        }
        if vertices.len() >= 2 {
            self.strokes
                .push((part, Curve2::Polyline(Polyline2::new(vertices, false)?)));
        }
        Ok(())
    }
}

/// The vector of length 1 along `v`, scaled to avoid overflow; `None` for a zero or
/// non-finite vector.
#[must_use]
pub fn unit(v: Vector2) -> Option<Vector2> {
    let length = v.x.hypot(v.y);
    (length > 0.0 && length.is_finite()).then(|| v / length)
}

/// The unit vector at `angle` radians.
#[must_use]
pub fn direction(angle: f64) -> Vector2 {
    let (sin, cos) = angle.sin_cos();
    Vector2::new(cos, sin)
}

/// `v` turned a quarter counterclockwise.
#[must_use]
pub fn perp(v: Vector2) -> Vector2 {
    Vector2::new(-v.y, v.x)
}

/// The angle of `v` from the positive x axis.
#[must_use]
pub fn angle_of(v: Vector2) -> f64 {
    v.y.atan2(v.x)
}

/// `angle` reduced to [0, 2π).
#[must_use]
pub fn normalized(angle: f64) -> f64 {
    let a = angle.rem_euclid(TAU);
    if a >= TAU { 0.0 } else { a }
}

/// The text direction closest to `angle` that reads from left to right or from bottom to
/// top: `angle` reduced to (−π/2, π/2].
#[must_use]
pub fn readable(angle: f64) -> f64 {
    let a = normalized(angle);
    if a <= FRAC_PI_2 {
        a
    } else if a <= 3.0 * FRAC_PI_2 {
        a - PI
    } else {
        a - TAU
    }
}

/// Adds the arrowhead `arrow` of `size` with its tip at `tip`, pointing along the unit vector
/// `toward` (from its body to its tip).
///
/// # Errors
///
/// The `arc-geom` errors of shapes with values that are not finite.
pub fn arrowhead(
    drawn: &mut Drawn,
    arrow: Arrow,
    tip: Point2,
    toward: Vector2,
    size: f64,
) -> Result<()> {
    if size <= 0.0 {
        return Ok(());
    }
    let side = perp(toward) * (size / 6.0);
    let back = tip - toward * size;
    let vertex = |p: Point2| PolylineVertex::new(p, 0.0);
    match arrow {
        Arrow::ClosedFilled | Arrow::Closed => {
            let outline = Polyline2::new(
                vec![vertex(tip), vertex(back + side), vertex(back - side)],
                true,
            )?;
            if arrow == Arrow::Closed {
                drawn.strokes.push((Part::Arrow, Curve2::Polyline(outline)));
            } else {
                drawn.fills.push((Part::Arrow, outline));
            }
        }
        Arrow::Open => drawn.polyline(Part::Arrow, &[back + side, tip, back - side])?,
        Arrow::Dot => {
            let r = toward * (size / 4.0);
            let disc = Polyline2::new(
                vec![
                    PolylineVertex::new(tip - r, 1.0),
                    PolylineVertex::new(tip + r, 1.0),
                ],
                true,
            )?;
            drawn.fills.push((Part::Arrow, disc));
        }
        Arrow::Tick => {
            let d = unit(toward + perp(toward)).unwrap_or(toward) * (size / 2.0);
            drawn.line(Part::Arrow, tip - d, tip + d)?;
        }
        Arrow::None => {}
    }
    Ok(())
}

/// The representation of an annotative object at one annotation scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Representation {
    /// The annotation scale: paper length per model length, positive.
    pub scale: f64,
    /// The position at this scale: a dimension's line position or a multi-leader's content
    /// position; absent keeps the object's.
    pub position: Option<Point2>,
    /// The text position at this scale; absent keeps the object's.
    pub text_position: Option<Point2>,
}

impl Representation {
    /// The representation of a stored record.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(fields) = value else {
            return None;
        };
        let point = |i: usize| match fields.get(i) {
            Some(Some(Value::Point2(p))) => Some(*p),
            _ => None,
        };
        match fields.first() {
            Some(Some(Value::F64(scale))) => Some(Self {
                scale: *scale,
                position: point(1),
                text_position: point(2),
            }),
            _ => None,
        }
    }

    /// The stored record.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Record(vec![
            Some(Value::F64(self.scale)),
            self.position.map(Value::Point2),
            self.text_position.map(Value::Point2),
        ])
    }
}

fn representation_type() -> ValueType {
    ValueType::Record(vec![
        FieldDecl::new("scale", ValueType::F64).with_bound(Bound::Real {
            min: f64::MIN_POSITIVE,
            max: f64::MAX,
        }),
        FieldDecl::new("position", ValueType::Point2).optional(),
        FieldDecl::new("text_position", ValueType::Point2).optional(),
    ])
}

/// The fields `annotative` and `scales` every annotative type ends with.
pub(crate) fn fields() -> [FieldDecl; 2] {
    [
        flag("annotative", false),
        FieldDecl::new("scales", ValueType::List(Box::new(representation_type())))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_SCALES }),
    ]
}

/// The representations of a `scales` value.
#[must_use]
pub fn representations(value: Option<&Value>) -> Vec<Representation> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(Representation::from_value)
            .collect(),
        _ => Vec::new(),
    }
}

/// `draft.annotation.scale_invalid` when a scale of the `scales` value at `index` is listed
/// twice.
pub(crate) fn scale_findings(values: &[Option<Value>], index: usize) -> Vec<Finding> {
    let mut scales: Vec<f64> = representations(values.get(index).and_then(Option::as_ref))
        .iter()
        .map(|r| r.scale)
        .collect();
    scales.sort_by(f64::total_cmp);
    if scales.windows(2).any(|w| w.first() == w.get(1)) {
        vec![finding(Error::new(codes::ANNOTATION_SCALE_INVALID))]
    } else {
        Vec::new()
    }
}

/// One way an object is drawn: the annotation scale it is tagged with (`None`: every scale),
/// the factor of its style sizes, and the position overrides.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rendition {
    /// The annotation scale of the primitives, `None` for a non-annotative object.
    pub scale: Option<f64>,
    /// The factor of style sizes: 1/scale, or the overall scale.
    pub factor: f64,
    /// The position override.
    pub position: Option<Point2>,
    /// The text position override.
    pub text_position: Option<Point2>,
}

/// The renditions of an object: one per representation when `annotative`, otherwise one with
/// the factor `overall`.
#[must_use]
pub fn renditions(annotative: bool, scales: &[Representation], overall: f64) -> Vec<Rendition> {
    if annotative {
        scales
            .iter()
            .map(|r| Rendition {
                scale: Some(r.scale),
                factor: 1.0 / r.scale,
                position: r.position,
                text_position: r.text_position,
            })
            .collect()
    } else {
        vec![Rendition {
            scale: None,
            factor: overall,
            position: None,
            text_position: None,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_angles_never_point_left_or_down() {
        assert!((readable(PI) - 0.0).abs() < 1e-12);
        assert!((readable(-FRAC_PI_2) - FRAC_PI_2).abs() < 1e-12);
        assert!((readable(3.0) - (3.0 - PI)).abs() < 1e-12);
        assert!((readable(-0.5) + 0.5).abs() < 1e-12);
    }

    #[test]
    fn arrowheads_put_their_tip_on_the_point() {
        let tip = Point2::new(1.0, 2.0);
        for code in 0..6 {
            let mut d = Drawn::default();
            arrowhead(&mut d, Arrow::of(code), tip, Vector2::new(1.0, 0.0), 3.0).unwrap();
            let shapes = d.strokes.len() + d.fills.len();
            assert_eq!(shapes, usize::from(code != 5), "{code}");
            if let Some((_, f)) = d.fills.first() {
                let bb = f.bounding_box();
                assert!(bb.max().x <= tip.x + 0.75 + 1e-12);
            }
        }
        let mut d = Drawn::default();
        arrowhead(
            &mut d,
            Arrow::ClosedFilled,
            tip,
            Vector2::new(1.0, 0.0),
            3.0,
        )
        .unwrap();
        assert_eq!(d.fills[0].1.vertices()[0].point, tip);
        assert_eq!(d.fills[0].1.vertices()[1].point, Point2::new(-2.0, 2.5));
    }

    #[test]
    fn duplicate_scales_are_refused() {
        let reps = Value::List(vec![
            Representation {
                scale: 0.5,
                position: None,
                text_position: None,
            }
            .to_value(),
            Representation {
                scale: 0.5,
                position: None,
                text_position: None,
            }
            .to_value(),
        ]);
        assert_eq!(scale_findings(&[Some(reps)], 0).len(), 1);
        assert_eq!(renditions(false, &[], 2.0)[0].factor, 2.0);
    }
}
