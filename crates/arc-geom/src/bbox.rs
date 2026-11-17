//! Axis-aligned bounding boxes and the exact tight box of every bounded 2D curve kind.

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Point3, Vector2};
use arc_base::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::error::codes;
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Segment2};

macro_rules! aabb {
    ($name:ident, $corners:ident, $wire:tt, $point:ty, $dim:literal, [$($axis:ident),+]) => {
        #[doc = concat!("An axis-aligned box in ", $dim, "D, possibly empty.")]
        ///
        /// The empty box contains nothing, intersects nothing and is the identity of
        /// [`Self::union`]; its minimum corner is +∞ and its maximum corner −∞. Every operation
        /// returns the canonical empty box, never a box with crossed corners. A box serializes as
        /// `{ "min": …, "max": … }`, or `null` when empty.
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
        #[serde(try_from = $wire, into = $wire)]
        pub struct $name {
            min: $point,
            max: $point,
        }

        #[derive(Clone, Copy, Serialize, Deserialize)]
        struct $corners {
            min: $point,
            max: $point,
        }

        impl TryFrom<Option<$corners>> for $name {
            type Error = Error;

            fn try_from(corners: Option<$corners>) -> Result<Self> {
                let Some($corners { min, max }) = corners else {
                    return Ok(Self::EMPTY);
                };
                crate::ensure_finite(min.coords.as_slice())?;
                crate::ensure_finite(max.coords.as_slice())?;
                let boxed = Self::ordered(min, max);
                if boxed.is_empty() {
                    Err(Error::new(codes::BBOX_INVALID))
                } else {
                    Ok(boxed)
                }
            }
        }

        impl From<$name> for Option<$corners> {
            fn from(boxed: $name) -> Self {
                (!boxed.is_empty()).then_some($corners {
                    min: boxed.min,
                    max: boxed.max,
                })
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::EMPTY
            }
        }

        impl $name {
            /// The empty box.
            pub const EMPTY: Self = Self {
                min: <$point>::new($({ let $axis = f64::INFINITY; $axis }),+),
                max: <$point>::new($({ let $axis = f64::NEG_INFINITY; $axis }),+),
            };

            /// The box with `min` and `max` as corners, or the empty box when any coordinate of
            /// `min` exceeds that of `max` or is NaN.
            fn ordered(min: $point, max: $point) -> Self {
                if min.iter().zip(max.iter()).all(|(low, high)| low <= high) {
                    Self { min, max }
                } else {
                    Self::EMPTY
                }
            }

            /// The smallest box containing both corners, in any order.
            #[must_use]
            pub fn from_corners(a: $point, b: $point) -> Self {
                Self::ordered(a.inf(&b), a.sup(&b))
            }

            /// The smallest box containing the points; points with a non-finite coordinate are
            /// ignored.
            #[must_use]
            pub fn from_points<'a>(points: impl IntoIterator<Item = &'a $point>) -> Self {
                points
                    .into_iter()
                    .filter(|p| p.iter().all(|c| c.is_finite()))
                    .fold(Self::EMPTY, |boxed, p| boxed.union(&Self { min: *p, max: *p }))
            }

            /// Whether the box contains nothing.
            #[must_use]
            pub fn is_empty(&self) -> bool {
                // The canonical empty box is the only one with crossed corners.
                self.min.x > self.max.x
            }

            /// The minimum corner; +∞ in every coordinate for the empty box.
            #[must_use]
            pub fn min(&self) -> $point {
                self.min
            }

            /// The maximum corner; −∞ in every coordinate for the empty box.
            #[must_use]
            pub fn max(&self) -> $point {
                self.max
            }

            /// The smallest box containing both boxes.
            #[must_use]
            pub fn union(&self, other: &Self) -> Self {
                Self {
                    min: self.min.inf(&other.min),
                    max: self.max.sup(&other.max),
                }
            }

            /// Whether the boxes share at least one point; touching boxes intersect.
            #[must_use]
            pub fn intersects(&self, other: &Self) -> bool {
                !Self::ordered(self.min.sup(&other.min), self.max.inf(&other.max)).is_empty()
            }

            /// Whether the point lies in the box or on its boundary.
            #[must_use]
            pub fn contains_point(&self, point: &$point) -> bool {
                self.min
                    .iter()
                    .zip(self.max.iter())
                    .zip(point.iter())
                    .all(|((low, high), c)| low <= c && c <= high)
            }

            /// Whether `other` lies entirely in this box. Every box contains the empty box.
            #[must_use]
            pub fn contains(&self, other: &Self) -> bool {
                other.is_empty()
                    || (self.contains_point(&other.min) && self.contains_point(&other.max))
            }

            /// The box grown by `margin` on every side; a negative margin shrinks it, and a box
            /// that shrinks past nothing, or a NaN margin, yields the empty box.
            #[must_use]
            pub fn expanded(&self, margin: f64) -> Self {
                if self.is_empty() {
                    return Self::EMPTY;
                }
                Self::ordered(self.min.map(|c| c - margin), self.max.map(|c| c + margin))
            }
        }
    };
}

aabb!(Aabb2, Corners2, "Option<Corners2>", Point2, 2, [x, y]);
aabb!(Aabb3, Corners3, "Option<Corners3>", Point3, 3, [x, y, z]);

/// Whether eccentric angle `theta` lies on the sweep that starts at `start`.
fn within_sweep(start: f64, sweep: f64, theta: f64) -> bool {
    let traveled = if sweep > 0.0 {
        (theta - start).rem_euclid(TAU)
    } else {
        (start - theta).rem_euclid(TAU)
    };
    traveled <= sweep.abs()
}

/// The tight box of `center + u cos θ + v sin θ` for θ on the sweep: the end points and every
/// axis extremum the sweep passes.
fn elliptical_box(center: Point2, u: Vector2, v: Vector2, start: f64, sweep: f64) -> Aabb2 {
    let at = |theta: f64| {
        let (sin, cos) = theta.sin_cos();
        center + u * cos + v * sin
    };
    // d/dθ of x is −u.x sin θ + v.x cos θ, zero at atan2(v.x, u.x) and that plus π; same for y.
    let x_extremum = v.x.atan2(u.x);
    let y_extremum = v.y.atan2(u.y);
    let mut boxed = Aabb2::from_corners(at(start), at(start + sweep));
    for theta in [x_extremum, x_extremum + PI, y_extremum, y_extremum + PI] {
        if within_sweep(start, sweep, theta) {
            let p = at(theta);
            boxed = boxed.union(&Aabb2::from_corners(p, p));
        }
    }
    boxed
}

impl Segment2 {
    /// The tight bounding box.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        Aabb2::from_corners(self.start(), self.end())
    }
}

impl Circle2 {
    /// The tight bounding box.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        let half = Vector2::repeat(self.radius());
        Aabb2::from_corners(self.center() - half, self.center() + half)
    }
}

impl Arc2 {
    /// The tight bounding box, including every axis extremum inside the sweep. An arc built
    /// from a polyline segment is bounded relative to its chord.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        if let Some(chord) = self.chord() {
            return chord.bounding_box();
        }
        let r = self.radius();
        elliptical_box(
            self.center(),
            Vector2::new(r, 0.0),
            Vector2::new(0.0, r),
            self.start_angle(),
            self.sweep(),
        )
    }
}

impl Ellipse2 {
    /// The tight bounding box.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        let (u, v) = (self.major_axis(), self.minor_axis());
        let half = Vector2::new(u.x.hypot(v.x), u.y.hypot(v.y));
        Aabb2::from_corners(self.center() - half, self.center() + half)
    }
}

impl EllipticalArc2 {
    /// The tight bounding box, including every axis extremum inside the sweep.
    #[must_use]
    pub fn bounding_box(&self) -> Aabb2 {
        let e = self.ellipse();
        elliptical_box(
            e.center(),
            e.major_axis(),
            e.minor_axis(),
            self.start_parameter(),
            self.sweep(),
        )
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

    /// Checks that `boxed` contains every sample and that every side is touched by a sample.
    fn contains_and_is_tight(boxed: &Aabb2, samples: &[Point2], slack: f64) -> bool {
        let sampled = Aabb2::from_points(samples);
        boxed.expanded(1e-9).contains(&sampled)
            && (boxed.min() - sampled.min()).norm() <= slack
            && (boxed.max() - sampled.max()).norm() <= slack
    }

    fn samples(f: impl Fn(f64) -> Point2, length: f64) -> Vec<Point2> {
        (0..=4000)
            .map(|i| f(length * f64::from(i) / 4000.0))
            .collect()
    }

    #[test]
    fn empty_box_is_the_identity_of_union() {
        let a = Aabb2::from_corners(Point2::new(1.0, 2.0), Point2::new(-1.0, 0.0));
        assert_eq!(a.min(), Point2::new(-1.0, 0.0));
        assert_eq!(Aabb2::EMPTY.union(&a), a);
        assert!(Aabb2::EMPTY.is_empty() && !a.is_empty());
        assert!(!Aabb2::EMPTY.intersects(&a) && !Aabb2::EMPTY.contains_point(&Point2::origin()));
        assert!(a.contains(&Aabb2::EMPTY) && !Aabb2::EMPTY.contains(&a));
        assert_eq!(a.expanded(-5.0), Aabb2::EMPTY);
        assert_eq!(a.expanded(f64::NAN), Aabb2::EMPTY);
        assert_eq!(Aabb2::EMPTY.expanded(3.0), Aabb2::EMPTY);
        assert_eq!(Aabb2::from_points([].iter()), Aabb2::EMPTY);
        let touching = Aabb2::from_corners(Point2::new(1.0, 2.0), Point2::new(3.0, 3.0));
        assert!(a.intersects(&touching));
        assert!(!a.intersects(&touching.expanded(-0.1)));
        let b = Aabb3::from_corners(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 1.0));
        assert!(b.contains_point(&Point3::new(0.5, 1.0, 0.0)));
        assert!(b.expanded(1.0).contains(&b));
    }

    #[test]
    fn boxes_round_trip_through_serde_and_reject_crossed_corners() {
        let a = Aabb3::from_corners(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 2.0, 3.0));
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Aabb3>(&json).unwrap(), a);
        assert_eq!(serde_json::to_string(&Aabb2::EMPTY).unwrap(), "null");
        assert_eq!(serde_json::from_str::<Aabb2>("null").unwrap(), Aabb2::EMPTY);
        let err =
            serde_json::from_str::<Aabb2>(r#"{"min":[1.0,0.0],"max":[0.0,0.0]}"#).unwrap_err();
        assert!(err.to_string().contains("geom.bbox.invalid"));
    }

    #[test]
    fn quarter_arc_box_includes_the_axis_extremum() {
        let arc = Arc2::new(Point2::origin(), 1.0, 0.25, PI / 2.0).unwrap();
        let boxed = arc.bounding_box();
        assert!((boxed.max().y - 1.0).abs() < 1e-15);
        assert!((boxed.min().x + 1.0).abs() > 0.01);
        let full = Arc2::new(Point2::new(1.0, 1.0), 2.0, 0.3, -TAU).unwrap();
        let circle = Circle2::new(Point2::new(1.0, 1.0), 2.0).unwrap();
        assert!((full.bounding_box().min() - circle.bounding_box().min()).norm() < 1e-12);
    }

    proptest! {
        #[test]
        fn arc_boxes_contain_samples_and_are_tight(
            c in point(), r in 0.1..50.0_f64, a in angle(), s in sweep(),
        ) {
            let arc = Arc2::new(c, r, a, s).unwrap();
            let pts = samples(|t| arc.point_at(t), s.abs());
            // 4000 samples of at most a full turn are within r·(π/4000)²/2 of every extremum.
            prop_assert!(contains_and_is_tight(&arc.bounding_box(), &pts, r * 1e-6));
            let circle = Circle2::new(c, r).unwrap();
            let pts = samples(|t| circle.point_at(t), TAU);
            prop_assert!(contains_and_is_tight(&circle.bounding_box(), &pts, r * 1e-6));
        }

        #[test]
        fn boxes_of_arcs_with_huge_start_angles_are_tight(
            c in point(), r in 0.1..50.0_f64, a in -1e15..1e15_f64, s in sweep(),
            u in nonzero_vector(), k in 0.05..=1.0_f64,
        ) {
            let arc = Arc2::new(c, r, a, s).unwrap();
            let pts = samples(|t| arc.point_at(t), s.abs());
            prop_assert!(contains_and_is_tight(&arc.bounding_box(), &pts, r * 1e-6));
            let ellipse = Ellipse2::new(c, u, k).unwrap();
            let e = EllipticalArc2::new(ellipse, a, s).unwrap();
            let pts = samples(|t| e.point_at(t), s.abs());
            prop_assert!(contains_and_is_tight(&e.bounding_box(), &pts, u.norm() * 1e-6));
        }

        #[test]
        fn extreme_curves_have_boxes_that_contain_their_samples(
            c in extreme_point(), d in extreme_vector(), r in extreme_length(),
            a in extreme_angle(), s in sweep(), k in log_uniform(-150.0, 0.0),
        ) {
            let arc = Arc2::new(c, r, a, s).unwrap();
            let size = c.coords.abs().max().max(r);
            let pts = samples(|t| arc.point_at(t), s.abs());
            prop_assert!(arc.bounding_box().expanded(size * 1e-12).contains(&Aabb2::from_points(&pts)));
            let e = EllipticalArc2::new(Ellipse2::new(c, d, k.min(1.0)).unwrap(), a, s).unwrap();
            let size = c.coords.abs().max().max(d.abs().max());
            let pts = samples(|t| e.point_at(t), s.abs());
            prop_assert!(e.bounding_box().expanded(size * 1e-12).contains(&Aabb2::from_points(&pts)));
        }

        #[test]
        fn elliptical_boxes_contain_samples_and_are_tight(
            c in point(), u in nonzero_vector(), k in 0.05..=1.0_f64, a in angle(), s in sweep(),
        ) {
            let ellipse = Ellipse2::new(c, u, k).unwrap();
            let size = u.norm();
            let pts = samples(|t| ellipse.point_at(t), TAU);
            prop_assert!(contains_and_is_tight(&ellipse.bounding_box(), &pts, size * 1e-6));
            let arc = EllipticalArc2::new(ellipse, a, s).unwrap();
            let pts = samples(|t| arc.point_at(t), s.abs());
            prop_assert!(contains_and_is_tight(&arc.bounding_box(), &pts, size * 1e-6));
        }
    }
}
