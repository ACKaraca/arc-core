//! Exact 2D primitives (CORE-GEOM-001): segments, rays, infinite lines, circles, circular arcs,
//! ellipses and elliptical arcs.
//!
//! Constructors reject non-finite input and degenerate geometry; deserialization runs the same
//! constructors. Degeneracy is decided exactly (a zero length, a zero radius), not within a
//! tolerance, so the validity of a value never depends on the tolerance of a document.
//!
//! Arcs store a start angle and a signed sweep: a positive sweep runs counterclockwise, a
//! negative sweep clockwise, and a magnitude of 2π is a full turn. The start angle of an arc and
//! the start parameter of an elliptical arc are normalized into [0, 2π) on construction: a value
//! already in that interval is kept bit for bit, and any other value is reduced through its sine
//! and cosine, which use an exact argument reduction, so the normalized angle names the same
//! direction as the given one.
//!
//! Every finite nonzero direction and every finite positive length is accepted, however large
//! or small: lengths and unit directions are computed without squaring components.

use std::f64::consts::TAU;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::polyline::Chord;
use crate::{deserialize_through_new, ensure_finite};

/// The unit vector at `angle` radians from the positive x axis.
pub(crate) fn unit_at(angle: f64) -> Vector2 {
    let (sin, cos) = angle.sin_cos();
    Vector2::new(cos, sin)
}

/// The vector rotated a quarter turn counterclockwise.
pub(crate) fn perp(v: &Vector2) -> Vector2 {
    Vector2::new(-v.y, v.x)
}

/// `angle` reduced into the canonical interval [0, 2π).
///
/// A value already in [0, 2π) is returned unchanged, so normalizing is idempotent and a stored
/// angle round-trips bit for bit. Any other finite value is reduced through its sine and cosine,
/// which use an exact argument reduction, so the result names the same direction as the input
/// to within rounding of the result, however large the input is.
pub(crate) fn normalize_angle(angle: f64) -> f64 {
    if (0.0..TAU).contains(&angle) {
        return angle;
    }
    let (sin, cos) = angle.sin_cos();
    let reduced = sin.atan2(cos);
    let reduced = if reduced < 0.0 {
        reduced + TAU
    } else {
        reduced
    };
    // A tiny negative angle plus 2π rounds to 2π, which names the direction of 0.
    if reduced < TAU { reduced } else { 0.0 }
}

fn ensure_sweep(sweep: f64) -> Result<()> {
    if sweep == 0.0 || sweep.abs() > TAU {
        Err(Error::new(codes::SWEEP_INVALID).with_detail("sweep", sweep))
    } else {
        Ok(())
    }
}

/// A line segment from `start` to `end`. Parameter `t` in [0, 1], `t = 0` at the start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Segment2Raw")]
pub struct Segment2 {
    start: Point2,
    end: Point2,
}

deserialize_through_new!(
    Segment2,
    Segment2Raw {
        start: Point2,
        end: Point2
    }
);

impl Segment2 {
    /// Creates a segment.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite coordinate; `geom.segment.zero_length` when the
    /// points are equal.
    pub fn new(start: Point2, end: Point2) -> Result<Self> {
        ensure_finite(&[start.x, start.y, end.x, end.y])?;
        if start == end {
            return Err(Error::new(codes::SEGMENT_ZERO_LENGTH));
        }
        Ok(Self { start, end })
    }

    /// The start point.
    #[must_use]
    pub fn start(&self) -> Point2 {
        self.start
    }

    /// The end point.
    #[must_use]
    pub fn end(&self) -> Point2 {
        self.end
    }

    /// The point at parameter `t`, clamped to [0, 1]. The end points are returned exactly.
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        Point2::from(self.start.coords.lerp(&self.end.coords, t.clamp(0.0, 1.0)))
    }

    /// The same segment traversed from end to start.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            start: self.end,
            end: self.start,
        }
    }
}

/// A ray from `origin` along `direction`. Parameter `t >= 0` is the distance from the origin.
///
/// The direction is stored as given; only its direction matters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Ray2Raw")]
pub struct Ray2 {
    origin: Point2,
    direction: Vector2,
}

deserialize_through_new!(
    Ray2,
    Ray2Raw {
        origin: Point2,
        direction: Vector2
    }
);

impl Ray2 {
    /// Creates a ray.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.direction.zero` for a zero
    /// direction. Every other finite direction is accepted, however small or large.
    pub fn new(origin: Point2, direction: Vector2) -> Result<Self> {
        ensure_finite(&[origin.x, origin.y, direction.x, direction.y])?;
        if direction.x == 0.0 && direction.y == 0.0 {
            return Err(Error::new(codes::DIRECTION_ZERO));
        }
        Ok(Self { origin, direction })
    }

    /// The origin, at parameter 0.
    #[must_use]
    pub fn origin(&self) -> Point2 {
        self.origin
    }

    /// The direction as given to the constructor.
    #[must_use]
    pub fn direction(&self) -> Vector2 {
        self.direction
    }

    /// The direction scaled to unit length, computed without overflow or underflow for every
    /// accepted direction.
    #[must_use]
    pub fn unit_direction(&self) -> Vector2 {
        crate::unit(&self.direction)
    }

    /// The point at distance `t` from the origin; negative `t` is clamped to 0.
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.origin + self.unit_direction() * t.max(0.0)
    }
}

/// An infinite line through `point` along `direction`. Parameter `t` is any real number, the
/// signed distance from `point` along the direction.
///
/// The direction is stored as given; only its direction matters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Line2Raw")]
pub struct Line2 {
    point: Point2,
    direction: Vector2,
}

deserialize_through_new!(
    Line2,
    Line2Raw {
        point: Point2,
        direction: Vector2
    }
);

impl Line2 {
    /// Creates a line.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.direction.zero` for a zero
    /// direction. Every other finite direction is accepted, however small or large.
    pub fn new(point: Point2, direction: Vector2) -> Result<Self> {
        ensure_finite(&[point.x, point.y, direction.x, direction.y])?;
        if direction.x == 0.0 && direction.y == 0.0 {
            return Err(Error::new(codes::DIRECTION_ZERO));
        }
        Ok(Self { point, direction })
    }

    /// The point at parameter 0.
    #[must_use]
    pub fn point(&self) -> Point2 {
        self.point
    }

    /// The direction as given to the constructor.
    #[must_use]
    pub fn direction(&self) -> Vector2 {
        self.direction
    }

    /// The direction scaled to unit length, computed without overflow or underflow for every
    /// accepted direction.
    #[must_use]
    pub fn unit_direction(&self) -> Vector2 {
        crate::unit(&self.direction)
    }

    /// The point at signed distance `t` from [`Self::point`].
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.point + self.unit_direction() * t
    }

    /// The same line traversed in the opposite direction.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            point: self.point,
            direction: -self.direction,
        }
    }
}

/// A full circle, traversed counterclockwise. Parameter `t` in [0, 2π] is the angle from the
/// positive x axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Circle2Raw")]
pub struct Circle2 {
    center: Point2,
    radius: f64,
}

deserialize_through_new!(
    Circle2,
    Circle2Raw {
        center: Point2,
        radius: f64
    }
);

impl Circle2 {
    /// Creates a circle.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.radius.not_positive` for a radius
    /// that is zero or negative.
    pub fn new(center: Point2, radius: f64) -> Result<Self> {
        ensure_finite(&[center.x, center.y, radius])?;
        if radius <= 0.0 {
            return Err(Error::new(codes::RADIUS_NOT_POSITIVE).with_detail("radius", radius));
        }
        Ok(Self { center, radius })
    }

    /// The center.
    #[must_use]
    pub fn center(&self) -> Point2 {
        self.center
    }

    /// The radius.
    #[must_use]
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// The point at angle `t`, clamped to [0, 2π].
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.center + unit_at(t.clamp(0.0, TAU)) * self.radius
    }

    /// The circle as a full counterclockwise arc starting at angle 0, with the same
    /// parameterization.
    #[must_use]
    pub fn to_arc(&self) -> Arc2 {
        Arc2 {
            center: self.center,
            radius: self.radius,
            start_angle: 0.0,
            sweep: TAU,
            chord: None,
        }
    }
}

/// A circular arc. Parameter `t` in [0, |sweep|] is the angle traveled from the start point,
/// in the direction of the sweep. The start angle is normalized into [0, 2π).
///
/// An arc taken from a bulged polyline segment ([`crate::Polyline2::segment`]) also keeps the
/// chord and bulge it was built from. Its center, radius, start angle and sweep are the rounded
/// center-radius form, whose end points are accurate only to the rounding of the radius (which
/// grows without bound as the bulge goes to 0); its [`Self::point_at`] and
/// [`Self::bounding_box`] instead evaluate relative to the chord, return the polyline vertices
/// exactly as end points and are accurate to a few units in the last place of the larger of the
/// chord length and the sagitta. Equality compares the kept chord too. Serialization stores only
/// the center-radius form, [`Self::reversed`] reverses the kept chord, and a transform builds an
/// arc without one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Arc2Raw")]
pub struct Arc2 {
    center: Point2,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    /// The polyline chord this arc was built from; not serialized.
    #[serde(skip)]
    chord: Option<Chord>,
}

deserialize_through_new!(
    Arc2,
    Arc2Raw {
        center: Point2,
        radius: f64,
        start_angle: f64,
        sweep: f64,
    }
);

impl Arc2 {
    /// Creates an arc from its center, radius, start angle and signed sweep in radians. The
    /// start angle is normalized into [0, 2π) as described in the [module
    /// documentation](self).
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.radius.not_positive` for a radius
    /// that is zero or negative; `geom.sweep.invalid` for a sweep that is zero or larger than
    /// 2π in magnitude.
    pub fn new(center: Point2, radius: f64, start_angle: f64, sweep: f64) -> Result<Self> {
        ensure_finite(&[center.x, center.y, radius, start_angle, sweep])?;
        if radius <= 0.0 {
            return Err(Error::new(codes::RADIUS_NOT_POSITIVE).with_detail("radius", radius));
        }
        ensure_sweep(sweep)?;
        Ok(Self {
            center,
            radius,
            start_angle: normalize_angle(start_angle),
            sweep,
            chord: None,
        })
    }

    /// The same arc, evaluated relative to `chord`.
    pub(crate) fn with_chord(self, chord: Chord) -> Self {
        Self {
            chord: Some(chord),
            ..self
        }
    }

    /// The polyline chord this arc was built from, if any.
    pub(crate) fn chord(&self) -> Option<&Chord> {
        self.chord.as_ref()
    }

    /// The center.
    #[must_use]
    pub fn center(&self) -> Point2 {
        self.center
    }

    /// The radius.
    #[must_use]
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// The angle of the start point from the positive x axis, in [0, 2π).
    #[must_use]
    pub fn start_angle(&self) -> f64 {
        self.start_angle
    }

    /// The signed sweep: positive counterclockwise, negative clockwise.
    #[must_use]
    pub fn sweep(&self) -> f64 {
        self.sweep
    }

    /// Whether the arc runs counterclockwise.
    #[must_use]
    pub fn is_counterclockwise(&self) -> bool {
        self.sweep > 0.0
    }

    /// The angle from the positive x axis at parameter `t`, clamped to [0, |sweep|].
    #[must_use]
    pub fn angle_at(&self, t: f64) -> f64 {
        self.start_angle + t.clamp(0.0, self.sweep.abs()) * self.sweep.signum()
    }

    /// The point at parameter `t`, clamped to [0, |sweep|]. An arc built from a polyline
    /// segment is evaluated relative to its chord.
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        match &self.chord {
            Some(chord) => {
                let sweep = self.sweep.abs();
                chord.point_at_fraction(t.clamp(0.0, sweep) / sweep)
            }
            None => self.center + unit_at(self.angle_at(t)) * self.radius,
        }
    }

    /// The same arc traversed from end to start, with its start angle normalized; a kept chord
    /// is reversed as well.
    #[must_use]
    pub fn reversed(&self) -> Self {
        // A chord arc is rebuilt from its reversed chord, so it equals the arc that a polyline
        // with the reversed segment gives and survives encoding.
        if let Some(chord) = self.chord
            && let Ok(Some(arc)) =
                crate::encoding::chord_arc(chord.end(), chord.start(), -chord.bulge())
        {
            return arc;
        }
        Self {
            start_angle: normalize_angle(self.start_angle + self.sweep),
            sweep: -self.sweep,
            chord: self.chord.map(|chord| chord.reversed()),
            ..*self
        }
    }
}

/// A full ellipse, traversed counterclockwise. Parameter `t` in [0, 2π] is the eccentric angle:
/// the point at `t` is `center + major_axis·cos t + minor_axis·sin t`, where the minor axis is
/// the major axis turned a quarter turn counterclockwise and scaled by `ratio`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Ellipse2Raw")]
pub struct Ellipse2 {
    center: Point2,
    major_axis: Vector2,
    ratio: f64,
}

deserialize_through_new!(
    Ellipse2,
    Ellipse2Raw {
        center: Point2,
        major_axis: Vector2,
        ratio: f64,
    }
);

impl Ellipse2 {
    /// Creates an ellipse from its center, its major semi-axis vector and the ratio of the minor
    /// to the major semi-axis length.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.ellipse.invalid_axis` for a zero
    /// major axis or a ratio outside (0, 1]. Every other finite major axis is accepted, however
    /// small or large.
    pub fn new(center: Point2, major_axis: Vector2, ratio: f64) -> Result<Self> {
        ensure_finite(&[center.x, center.y, major_axis.x, major_axis.y, ratio])?;
        if (major_axis.x == 0.0 && major_axis.y == 0.0) || ratio <= 0.0 || ratio > 1.0 {
            return Err(Error::new(codes::ELLIPSE_INVALID_AXIS).with_detail("ratio", ratio));
        }
        Ok(Self {
            center,
            major_axis,
            ratio,
        })
    }

    /// The center.
    #[must_use]
    pub fn center(&self) -> Point2 {
        self.center
    }

    /// The major semi-axis vector: the point at parameter 0 minus the center.
    #[must_use]
    pub fn major_axis(&self) -> Vector2 {
        self.major_axis
    }

    /// The minor semi-axis vector: the point at parameter π/2 minus the center.
    #[must_use]
    pub fn minor_axis(&self) -> Vector2 {
        perp(&self.major_axis) * self.ratio
    }

    /// The ratio of the minor to the major semi-axis length, in (0, 1].
    #[must_use]
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// The point at eccentric angle `t`, without clamping.
    pub(crate) fn point_at_angle(&self, t: f64) -> Point2 {
        let (sin, cos) = t.sin_cos();
        self.center + self.major_axis * cos + self.minor_axis() * sin
    }

    /// The point at parameter `t`, clamped to [0, 2π].
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.point_at_angle(t.clamp(0.0, TAU))
    }

    /// The ellipse as a full counterclockwise elliptical arc starting at parameter 0, with the
    /// same parameterization.
    #[must_use]
    pub fn to_arc(&self) -> EllipticalArc2 {
        EllipticalArc2 {
            ellipse: *self,
            start_parameter: 0.0,
            sweep: TAU,
        }
    }
}

/// An elliptical arc. The start and sweep are eccentric angles of the underlying ellipse;
/// parameter `t` in [0, |sweep|] is the eccentric angle traveled from the start point, in the
/// direction of the sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "EllipticalArc2Raw")]
pub struct EllipticalArc2 {
    ellipse: Ellipse2,
    start_parameter: f64,
    sweep: f64,
}

deserialize_through_new!(
    EllipticalArc2,
    EllipticalArc2Raw {
        ellipse: Ellipse2,
        start_parameter: f64,
        sweep: f64,
    }
);

impl EllipticalArc2 {
    /// Creates an elliptical arc on `ellipse` from a start eccentric angle and a signed sweep.
    /// The start parameter is normalized into [0, 2π) as described in the [module
    /// documentation](self).
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite value; `geom.sweep.invalid` for a sweep that is
    /// zero or larger than 2π in magnitude.
    pub fn new(ellipse: Ellipse2, start_parameter: f64, sweep: f64) -> Result<Self> {
        ensure_finite(&[start_parameter, sweep])?;
        ensure_sweep(sweep)?;
        Ok(Self {
            ellipse,
            start_parameter: normalize_angle(start_parameter),
            sweep,
        })
    }

    /// The underlying ellipse.
    #[must_use]
    pub fn ellipse(&self) -> &Ellipse2 {
        &self.ellipse
    }

    /// The eccentric angle of the start point, in [0, 2π).
    #[must_use]
    pub fn start_parameter(&self) -> f64 {
        self.start_parameter
    }

    /// The signed sweep of the eccentric angle: positive counterclockwise, negative clockwise.
    #[must_use]
    pub fn sweep(&self) -> f64 {
        self.sweep
    }

    /// The eccentric angle at parameter `t`, clamped to [0, |sweep|].
    #[must_use]
    pub fn angle_at(&self, t: f64) -> f64 {
        self.start_parameter + t.clamp(0.0, self.sweep.abs()) * self.sweep.signum()
    }

    /// The point at parameter `t`, clamped to [0, |sweep|].
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        self.ellipse.point_at_angle(self.angle_at(t))
    }

    /// The same arc traversed from end to start, with its start parameter normalized.
    #[must_use]
    pub fn reversed(&self) -> Self {
        Self {
            start_parameter: normalize_angle(self.start_parameter + self.sweep),
            sweep: -self.sweep,
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testing::{
        angle, extreme_angle, extreme_length, extreme_point, extreme_vector, log_uniform,
        nonzero_vector, point, sweep,
    };

    fn code<T: std::fmt::Debug>(result: Result<T>) -> &'static str {
        result.unwrap_err().code()
    }

    #[test]
    fn degenerate_input_is_rejected_with_stable_codes() {
        let o = Point2::origin();
        let x = Vector2::x();
        assert_eq!(code(Segment2::new(o, o)), "geom.segment.zero_length");
        assert_eq!(code(Ray2::new(o, Vector2::zeros())), "geom.direction.zero");
        assert_eq!(code(Line2::new(o, Vector2::zeros())), "geom.direction.zero");
        assert_eq!(code(Circle2::new(o, 0.0)), "geom.radius.not_positive");
        assert_eq!(code(Circle2::new(o, -1.0)), "geom.radius.not_positive");
        assert_eq!(code(Arc2::new(o, 1.0, 0.0, 0.0)), "geom.sweep.invalid");
        assert_eq!(code(Arc2::new(o, 1.0, 0.0, 7.0)), "geom.sweep.invalid");
        assert_eq!(
            code(Arc2::new(o, 0.0, 0.0, 1.0)),
            "geom.radius.not_positive"
        );
        assert_eq!(
            code(Ellipse2::new(o, Vector2::zeros(), 0.5)),
            "geom.ellipse.invalid_axis"
        );
        for ratio in [0.0, -0.5, 1.5] {
            assert_eq!(
                code(Ellipse2::new(o, x, ratio)),
                "geom.ellipse.invalid_axis"
            );
        }
        let ellipse = Ellipse2::new(o, x, 0.5).unwrap();
        assert_eq!(
            code(EllipticalArc2::new(ellipse, 0.0, 0.0)),
            "geom.sweep.invalid"
        );
        assert!(Arc2::new(o, 1.0, 0.0, -TAU).is_ok());
        assert!(Ellipse2::new(o, x, 1.0).is_ok());
    }

    #[test]
    fn invalid_serialized_values_are_rejected() {
        for json in [
            r#"{"start":[0.0,0.0],"end":[0.0,0.0]}"#,
            r#"{"start":[0.0,0.0],"end":[1.0]}"#,
        ] {
            assert!(serde_json::from_str::<Segment2>(json).is_err());
        }
        let err = serde_json::from_str::<Arc2>(
            r#"{"center":[0.0,0.0],"radius":-1.0,"start_angle":0.0,"sweep":1.0}"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("geom.radius.not_positive"));
        assert!(
            serde_json::from_str::<EllipticalArc2>(
                r#"{"ellipse":{"center":[0.0,0.0],"major_axis":[1.0,0.0],"ratio":2.0},
                    "start_parameter":0.0,"sweep":1.0}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<Ray2>(r#"{"origin":[0.0,0.0],"direction":[0.0,0.0]}"#).is_err()
        );
    }

    proptest! {
        #[test]
        fn non_finite_input_is_rejected(bad in prop_oneof![Just(f64::NAN), Just(f64::INFINITY), Just(f64::NEG_INFINITY)]) {
            let o = Point2::origin();
            let p = Point2::new(bad, 0.0);
            prop_assert_eq!(code(Segment2::new(o, p)), "geom.input.not_finite");
            prop_assert_eq!(code(Ray2::new(p, Vector2::x())), "geom.input.not_finite");
            prop_assert_eq!(code(Line2::new(o, Vector2::new(0.0, bad))), "geom.input.not_finite");
            prop_assert_eq!(code(Circle2::new(o, bad)), "geom.input.not_finite");
            prop_assert_eq!(code(Arc2::new(o, 1.0, bad, 1.0)), "geom.input.not_finite");
            prop_assert_eq!(code(Ellipse2::new(o, Vector2::x(), bad)), "geom.input.not_finite");
        }

        #[test]
        fn extreme_values_are_accepted_and_evaluate_to_finite_points(
            c in extreme_point(), d in extreme_vector(), r in extreme_length(),
            a in extreme_angle(), s in sweep(), k in log_uniform(-150.0, 0.0), f in 0.0..=1.0_f64,
        ) {
            let finite = |p: Point2| p.x.is_finite() && p.y.is_finite();
            let ray = Ray2::new(c, d).unwrap();
            prop_assert!(finite(ray.point_at(f)));
            prop_assert!((ray.unit_direction().norm() - 1.0).abs() < 4.0 * f64::EPSILON);
            prop_assert!(finite(Line2::new(c, d).unwrap().point_at(-f)));
            prop_assert!(finite(Circle2::new(c, r).unwrap().point_at(f * TAU)));
            let arc = Arc2::new(c, r, a, s).unwrap();
            prop_assert!((0.0..TAU).contains(&arc.start_angle()));
            prop_assert!(finite(arc.point_at(f * s.abs())));
            let ellipse = Ellipse2::new(c, d, k.min(1.0)).unwrap();
            prop_assert!(finite(ellipse.point_at(f * TAU)));
            let e = EllipticalArc2::new(ellipse, a, s).unwrap();
            prop_assert!(finite(e.point_at(f * s.abs())));
            let json = serde_json::to_string(&arc).unwrap();
            prop_assert_eq!(serde_json::from_str::<Arc2>(&json).unwrap(), arc);
            prop_assert_eq!(Arc2::new(c, r, arc.start_angle(), s).unwrap(), arc);
        }

        #[test]
        fn arcs_round_trip_through_serde(
            c in point(), r in 0.1..50.0_f64, a in angle(), s in sweep(), u in nonzero_vector(), k in 0.05..=1.0_f64,
        ) {
            let arc = Arc2::new(c, r, a, s).unwrap();
            let json = serde_json::to_string(&arc).unwrap();
            prop_assert_eq!(serde_json::from_str::<Arc2>(&json).unwrap(), arc);
            let ellipse = EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap();
            let json = serde_json::to_string(&ellipse).unwrap();
            prop_assert_eq!(serde_json::from_str::<EllipticalArc2>(&json).unwrap(), ellipse);
        }

        #[test]
        fn reversed_arcs_retrace_the_curve(
            c in point(), r in 0.1..50.0_f64, a in angle(), s in sweep(), u in nonzero_vector(), k in 0.05..=1.0_f64, f in 0.0..=1.0_f64,
        ) {
            let arc = Arc2::new(c, r, a, s).unwrap();
            let t = f * s.abs();
            prop_assert!((arc.reversed().point_at(s.abs() - t) - arc.point_at(t)).norm() < 1e-9);
            prop_assert!((arc.reversed().reversed().point_at(t) - arc.point_at(t)).norm() < 1e-9);
            let e = EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap();
            prop_assert!((e.reversed().point_at(s.abs() - t) - e.point_at(t)).norm() < 1e-9);
        }
    }

    #[test]
    fn large_start_angles_are_normalized_to_the_same_direction() {
        for a in [
            1e6,
            1e10,
            1e15,
            1e17,
            1e300,
            -1e300,
            f64::MAX,
            -7.0,
            TAU,
            -1e-20,
        ] {
            let arc = Arc2::new(Point2::origin(), 1.0, a, 1.0).unwrap();
            let start = arc.start_angle();
            assert!((0.0..TAU).contains(&start), "start {a:e} -> {start}");
            assert!((arc.point_at(0.0) - Point2::from(unit_at(a))).norm() < 1e-15);
            let chord = (arc.point_at(1.0) - arc.point_at(0.0)).norm();
            assert!((chord - 2.0 * 0.5_f64.sin()).abs() < 1e-15);
            assert_eq!(Arc2::new(Point2::origin(), 1.0, start, 1.0).unwrap(), arc);
            let ellipse = Ellipse2::new(Point2::origin(), Vector2::x(), 0.5).unwrap();
            let e = EllipticalArc2::new(ellipse, a, -1.0).unwrap();
            assert!((0.0..TAU).contains(&e.start_parameter()));
            assert!((e.point_at(0.0) - ellipse.point_at_angle(a)).norm() < 1e-15);
            let r = arc.reversed();
            assert!((0.0..TAU).contains(&r.start_angle()));
            assert!((r.point_at(1.0) - arc.point_at(0.0)).norm() < 1e-15);
        }
    }

    #[test]
    fn extreme_directions_and_axes_are_accepted_and_usable() {
        let o = Point2::origin();
        for m in [
            5e-324,
            1e-300,
            1e-200,
            1e-160,
            1.0,
            1e155,
            1e200,
            1e300,
            f64::MAX,
        ] {
            for d in [
                Vector2::new(m, 0.0),
                Vector2::new(m, m),
                Vector2::new(-m, 0.5 * m),
            ] {
                let ray = Ray2::new(o, d).unwrap();
                let unit = ray.unit_direction();
                assert!((unit.norm() - 1.0).abs() < 4.0 * f64::EPSILON, "{d:?}");
                let w = d / m;
                assert!(unit.perp(&w).abs() < 4.0 * f64::EPSILON && unit.dot(&w) > 0.0);
                let line = Line2::new(o, d).unwrap();
                assert!((line.point_at(2.0) - o).norm() > 1.9);
                let ellipse = Ellipse2::new(o, d, 1.0).unwrap();
                assert!(ellipse.point_at(1.0).x.is_finite());
                // The box of the largest ellipses is wider than f64 can hold.
                if m <= 1e300 {
                    let boxed = ellipse.bounding_box();
                    assert!(boxed.max().x.is_finite() && boxed.max().x > 0.0);
                }
            }
        }
    }

    #[test]
    fn arc_end_points_follow_the_sweep_direction() {
        let arc = Arc2::new(Point2::origin(), 2.0, 0.0, -std::f64::consts::FRAC_PI_2).unwrap();
        assert!(!arc.is_counterclockwise());
        assert!(
            (arc.point_at(std::f64::consts::FRAC_PI_2) - Point2::new(0.0, -2.0)).norm() < 1e-12
        );
        let ellipse = Ellipse2::new(Point2::origin(), Vector2::new(2.0, 0.0), 0.5).unwrap();
        assert!(
            (ellipse.point_at(std::f64::consts::FRAC_PI_2) - Point2::new(0.0, 1.0)).norm() < 1e-12
        );
        let segment = Segment2::new(Point2::new(0.1, 0.2), Point2::new(0.7, 0.3)).unwrap();
        assert_eq!(segment.point_at(1.0), segment.end());
        let ray = Ray2::new(Point2::origin(), Vector2::new(0.0, 3.0)).unwrap();
        assert_eq!(ray.point_at(2.0), Point2::new(0.0, 2.0));
    }
}
