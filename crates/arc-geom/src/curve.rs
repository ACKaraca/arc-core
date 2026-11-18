//! The closed sum of the 2D curve kinds.

use std::f64::consts::TAU;

use arc_base::math::Point2;
use serde::{Deserialize, Serialize};

use crate::bbox::Aabb2;
use crate::nurbs::Nurbs2;
use crate::polyline::Polyline2;
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2};

/// A 2D curve of any kind.
///
/// Parameterization per kind, with `t` clamped to the range by [`Curve2::point_at`]:
///
/// | Kind | Range | Meaning of `t` |
/// |---|---|---|
/// | Segment | [0, 1] | Linear fraction from start to end |
/// | Ray | [0, +∞) | Distance from the origin |
/// | Line | (−∞, +∞) | Signed distance from the reference point |
/// | Circle | [0, 2π] | Angle from the positive x axis, counterclockwise |
/// | Arc | [0, \|sweep\|] | Angle traveled from the start, in the sweep direction |
/// | Ellipse | [0, 2π] | Eccentric angle from the major axis, counterclockwise |
/// | Elliptical arc | [0, \|sweep\|] | Eccentric angle traveled from the start |
/// | Polyline | [0, n] | Segment index plus the fraction along that segment |
/// | NURBS | [u_p, u_n] | The knot parameter, from the first to the last knot |
///
/// Serialized with a `kind` tag in snake case beside the fields of the kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Curve2 {
    /// A line segment.
    Segment(Segment2),
    /// A ray.
    Ray(Ray2),
    /// An infinite line.
    Line(Line2),
    /// A full circle.
    Circle(Circle2),
    /// A circular arc.
    Arc(Arc2),
    /// A full ellipse.
    Ellipse(Ellipse2),
    /// An elliptical arc.
    EllipticalArc(EllipticalArc2),
    /// A bulge polyline.
    Polyline(Polyline2),
    /// A NURBS curve.
    Nurbs(Nurbs2),
}

impl Curve2 {
    /// The parameter range as (start, end); infinite ends are `f64` infinities.
    #[must_use]
    pub fn parameter_range(&self) -> (f64, f64) {
        match self {
            Self::Segment(_) => (0.0, 1.0),
            Self::Ray(_) => (0.0, f64::INFINITY),
            Self::Line(_) => (f64::NEG_INFINITY, f64::INFINITY),
            Self::Circle(_) | Self::Ellipse(_) => (0.0, TAU),
            Self::Arc(arc) => (0.0, arc.sweep().abs()),
            Self::EllipticalArc(arc) => (0.0, arc.sweep().abs()),
            Self::Polyline(polyline) => (0.0, polyline.segment_count() as f64),
            Self::Nurbs(nurbs) => nurbs.parameter_range(),
        }
    }

    /// The point at parameter `t`, clamped to the parameter range.
    #[must_use]
    pub fn point_at(&self, t: f64) -> Point2 {
        match self {
            Self::Segment(c) => c.point_at(t),
            Self::Ray(c) => c.point_at(t),
            Self::Line(c) => c.point_at(t),
            Self::Circle(c) => c.point_at(t),
            Self::Arc(c) => c.point_at(t),
            Self::Ellipse(c) => c.point_at(t),
            Self::EllipticalArc(c) => c.point_at(t),
            Self::Polyline(c) => c.point_at(t),
            Self::Nurbs(c) => c.point_at(t),
        }
    }

    /// The point at the start of the parameter range, or `None` for a line.
    #[must_use]
    pub fn start_point(&self) -> Option<Point2> {
        match self {
            Self::Line(_) => None,
            Self::Ray(ray) => Some(ray.origin()),
            _ => Some(self.point_at(self.parameter_range().0)),
        }
    }

    /// The point at the end of the parameter range, or `None` for a ray or line. A closed
    /// curve ends at its start point.
    #[must_use]
    pub fn end_point(&self) -> Option<Point2> {
        match self {
            Self::Ray(_) | Self::Line(_) => None,
            Self::Segment(segment) => Some(segment.end()),
            _ if self.is_closed() => self.start_point(),
            _ => Some(self.point_at(self.parameter_range().1)),
        }
    }

    /// Whether the curve ends where it starts: circles, ellipses, arcs of a full turn, closed
    /// polylines and NURBS curves whose first and last control points are equal.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        match self {
            Self::Circle(_) | Self::Ellipse(_) => true,
            Self::Arc(arc) => arc.sweep().abs() == TAU,
            Self::EllipticalArc(arc) => arc.sweep().abs() == TAU,
            Self::Polyline(polyline) => polyline.is_closed(),
            Self::Nurbs(nurbs) => nurbs.is_closed(),
            Self::Segment(_) | Self::Ray(_) | Self::Line(_) => false,
        }
    }

    /// The same point set traversed in the opposite direction, with
    /// `reversed.point_at(a + b − t) = point_at(t)` for range (a, b); a line maps `t` to `−t`.
    /// A circle or ellipse becomes a clockwise full arc. A NURBS curve maps `t` to `−t`: its
    /// knots are negated, which keeps them exact. `None` for a ray, which has no end to start
    /// from.
    #[must_use]
    pub fn reversed(&self) -> Option<Self> {
        Some(match self {
            Self::Segment(c) => Self::Segment(c.reversed()),
            Self::Ray(_) => return None,
            Self::Line(c) => Self::Line(c.reversed()),
            Self::Circle(c) => Self::Arc(c.to_arc().reversed()),
            Self::Arc(c) => Self::Arc(c.reversed()),
            Self::Ellipse(c) => Self::EllipticalArc(c.to_arc().reversed()),
            Self::EllipticalArc(c) => Self::EllipticalArc(c.reversed()),
            Self::Polyline(c) => Self::Polyline(c.reversed()),
            Self::Nurbs(c) => Self::Nurbs(c.reversed()),
        })
    }

    /// The exact tight bounding box, or `None` for a ray or line, which are unbounded.
    #[must_use]
    pub fn bounding_box(&self) -> Option<Aabb2> {
        match self {
            Self::Ray(_) | Self::Line(_) => None,
            Self::Segment(c) => Some(c.bounding_box()),
            Self::Circle(c) => Some(c.bounding_box()),
            Self::Arc(c) => Some(c.bounding_box()),
            Self::Ellipse(c) => Some(c.bounding_box()),
            Self::EllipticalArc(c) => Some(c.bounding_box()),
            Self::Polyline(c) => Some(c.bounding_box()),
            Self::Nurbs(c) => Some(c.bounding_box()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use arc_base::math::Vector2;
    use proptest::prelude::*;

    use super::*;
    use crate::polyline::PolylineVertex;
    use crate::testing::{
        angle, extreme_angle, extreme_length, extreme_point, extreme_vector, log_uniform,
        nonzero_vector, point, sweep, wide_bulge, wide_ratio,
    };

    /// Any curve kind with moderate coordinates.
    pub(crate) fn curve() -> impl Strategy<Value = Curve2> {
        let polyline = (
            prop::collection::vec((point(), -3.0..3.0_f64, 0.0..2.0_f64), 2..6),
            any::<bool>(),
        )
            .prop_filter_map("valid polyline", |(v, closed)| {
                let vertices = v
                    .into_iter()
                    .map(|(p, bulge, w)| PolylineVertex {
                        point: p,
                        bulge,
                        start_width: w,
                        end_width: w,
                    })
                    .collect();
                Polyline2::new(vertices, closed).ok()
            });
        prop_oneof![
            (point(), point()).prop_filter_map("segment", |(a, b)| Segment2::new(a, b)
                .ok()
                .map(Curve2::Segment)),
            (point(), nonzero_vector()).prop_map(|(p, d)| Curve2::Ray(Ray2::new(p, d).unwrap())),
            (point(), nonzero_vector()).prop_map(|(p, d)| Curve2::Line(Line2::new(p, d).unwrap())),
            (point(), 0.1..50.0_f64).prop_map(|(c, r)| Curve2::Circle(Circle2::new(c, r).unwrap())),
            (point(), 0.1..50.0_f64, angle(), sweep())
                .prop_map(|(c, r, a, s)| Curve2::Arc(Arc2::new(c, r, a, s).unwrap())),
            (point(), nonzero_vector(), 0.05..=1.0_f64)
                .prop_map(|(c, u, k)| Curve2::Ellipse(Ellipse2::new(c, u, k).unwrap())),
            (point(), nonzero_vector(), 0.05..=1.0_f64, angle(), sweep()).prop_map(
                |(c, u, k, a, s)| Curve2::EllipticalArc(
                    EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap()
                )
            ),
            polyline.prop_map(Curve2::Polyline),
        ]
    }

    /// Moderate coordinates with the hard shapes: polylines with bulges from 1e-300 to 1e15 and
    /// ellipses with ratios down to 1e-12.
    pub(crate) fn wide_curve() -> impl Strategy<Value = Curve2> {
        let polyline = (
            prop::collection::vec((point(), wide_bulge()), 2..5),
            any::<bool>(),
        )
            .prop_filter_map("valid polyline", |(v, closed)| {
                let vertices = v
                    .into_iter()
                    .map(|(p, bulge)| PolylineVertex::new(p, bulge))
                    .collect();
                Polyline2::new(vertices, closed).ok()
            });
        prop_oneof![
            1 => (point(), 0.1..50.0_f64).prop_map(|(c, r)| Curve2::Circle(Circle2::new(c, r).unwrap())),
            1 => (point(), nonzero_vector(), wide_ratio())
                .prop_map(|(c, u, k)| Curve2::Ellipse(Ellipse2::new(c, u, k).unwrap())),
            1 => (point(), nonzero_vector(), wide_ratio(), angle(), sweep()).prop_map(
                |(c, u, k, a, s)| Curve2::EllipticalArc(
                    EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap()
                )
            ),
            3 => polyline.prop_map(Curve2::Polyline),
        ]
    }

    /// Any curve kind with extreme-magnitude coordinates, lengths and angles.
    pub(crate) fn extreme_curve() -> impl Strategy<Value = Curve2> {
        let polyline = (
            prop::collection::vec(
                (
                    extreme_point(),
                    -3.0..3.0_f64,
                    prop_oneof![Just(0.0), extreme_length()],
                ),
                2..5,
            ),
            any::<bool>(),
        )
            .prop_filter_map("valid polyline", |(v, closed)| {
                let vertices = v
                    .into_iter()
                    .map(|(p, bulge, w)| PolylineVertex {
                        point: p,
                        bulge,
                        start_width: w,
                        end_width: w,
                    })
                    .collect();
                Polyline2::new(vertices, closed).ok()
            });
        let ratio = || log_uniform(-150.0, 0.0).prop_map(|k| k.min(1.0));
        prop_oneof![
            (extreme_point(), extreme_point()).prop_filter_map("segment", |(a, b)| {
                Segment2::new(a, b).ok().map(Curve2::Segment)
            }),
            (extreme_point(), extreme_vector())
                .prop_map(|(p, d)| Curve2::Ray(Ray2::new(p, d).unwrap())),
            (extreme_point(), extreme_vector())
                .prop_map(|(p, d)| Curve2::Line(Line2::new(p, d).unwrap())),
            (extreme_point(), extreme_length())
                .prop_map(|(c, r)| Curve2::Circle(Circle2::new(c, r).unwrap())),
            (extreme_point(), extreme_length(), extreme_angle(), sweep())
                .prop_map(|(c, r, a, s)| Curve2::Arc(Arc2::new(c, r, a, s).unwrap())),
            (extreme_point(), extreme_vector(), ratio())
                .prop_map(|(c, u, k)| Curve2::Ellipse(Ellipse2::new(c, u, k).unwrap())),
            (
                extreme_point(),
                extreme_vector(),
                ratio(),
                extreme_angle(),
                sweep()
            )
                .prop_map(|(c, u, k, a, s)| Curve2::EllipticalArc(
                    EllipticalArc2::new(Ellipse2::new(c, u, k).unwrap(), a, s).unwrap()
                )),
            polyline.prop_map(Curve2::Polyline),
        ]
    }

    /// A finite parameter inside the range at fraction `f`; lines and rays use [−50, 50].
    pub(crate) fn parameter(curve: &Curve2, f: f64) -> f64 {
        let (a, b) = curve.parameter_range();
        let (a, b) = (a.max(-50.0), b.min(50.0));
        a + (b - a) * f
    }

    #[test]
    fn closed_kinds_and_end_points() {
        let circle = Curve2::Circle(Circle2::new(Point2::new(1.0, 0.0), 2.0).unwrap());
        assert!(circle.is_closed());
        assert_eq!(circle.start_point(), circle.end_point());
        let ray = Curve2::Ray(Ray2::new(Point2::origin(), Vector2::x()).unwrap());
        assert_eq!(ray.start_point(), Some(Point2::origin()));
        assert_eq!(ray.end_point(), None);
        assert_eq!(ray.reversed(), None);
        assert_eq!(ray.bounding_box(), None);
        let full = Curve2::Arc(Arc2::new(Point2::origin(), 1.0, 0.0, -TAU).unwrap());
        assert!(full.is_closed());
    }

    proptest! {
        #[test]
        fn reversed_curves_retrace_the_curve(c in curve(), f in 0.0..=1.0_f64) {
            let Some(r) = c.reversed() else {
                prop_assert!(matches!(c, Curve2::Ray(_)));
                return Ok(());
            };
            let t = parameter(&c, f);
            let (a, b) = c.parameter_range();
            let mirrored = if a.is_finite() && !matches!(c, Curve2::Nurbs(_)) { a + b - t } else { -t };
            prop_assert!((r.point_at(mirrored) - c.point_at(t)).norm() < 1e-9);
            if let (Some(start), Some(end)) = (r.start_point(), c.end_point()) {
                prop_assert!((start - end).norm() < 1e-9);
            }
        }

        #[test]
        fn curves_round_trip_through_serde(c in curve()) {
            let json = serde_json::to_string(&c).unwrap();
            prop_assert_eq!(serde_json::from_str::<Curve2>(&json).unwrap(), c);
        }
    }

    #[test]
    fn deserialized_extreme_values_are_normalized_and_usable() {
        let arc: Curve2 = serde_json::from_str(
            r#"{"kind":"arc","center":[0.0,0.0],"radius":1.0,"start_angle":1e300,"sweep":6.283185307179586}"#,
        )
        .unwrap();
        let Curve2::Arc(inner) = &arc else {
            panic!("expected an arc");
        };
        assert!((0.0..TAU).contains(&inner.start_angle()));
        let json = serde_json::to_string(&arc).unwrap();
        assert_eq!(serde_json::from_str::<Curve2>(&json).unwrap(), arc);
        let polyline: Curve2 = serde_json::from_str(
            r#"{"kind":"polyline","vertices":[{"point":[0.0,0.0],"bulge":1e-300,"start_width":0.0,"end_width":0.0},{"point":[1.0,0.0],"bulge":0.0,"start_width":0.0,"end_width":0.0}],"closed":false}"#,
        )
        .unwrap();
        let boxed = polyline.bounding_box().unwrap();
        assert_eq!(
            (boxed.min().x, boxed.max().x, boxed.max().y),
            (0.0, 1.0, 0.0)
        );
        assert!((boxed.min().y + 0.5e-300).abs() < 1e-315);
    }

    #[test]
    fn invalid_tagged_curves_are_rejected() {
        let ok = r#"{"kind":"circle","center":[0.0,0.0],"radius":1.0}"#;
        assert!(serde_json::from_str::<Curve2>(ok).is_ok());
        let bad = r#"{"kind":"circle","center":[0.0,0.0],"radius":0.0}"#;
        assert!(serde_json::from_str::<Curve2>(bad).is_err());
        let unknown = r#"{"kind":"spiral","length":1.0}"#;
        assert!(serde_json::from_str::<Curve2>(unknown).is_err());
    }
}
