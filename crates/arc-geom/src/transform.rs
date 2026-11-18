//! Rigid, similarity and affine transforms of every curve kind (CORE-GEOM-029).
//!
//! All three are [`Transform2`] values; the result depends on the transform only through its
//! effect, not through a declared class:
//!
//! - Points, segments, rays and lines map point by point.
//! - A circle or circular arc stays a circle or arc when its image is round within the linear
//!   tolerance (the image's semi-axes differ by at most `tolerance.linear`), and otherwise
//!   becomes an exact ellipse or elliptical arc. A mirror reverses the direction of an arc.
//! - An ellipse or elliptical arc stays one; its new axes are the principal axes of the
//!   transformed conjugate semi-diameters.
//! - A polyline maps vertex by vertex, with bulges negated under a mirror and widths scaled by
//!   the mean scale factor. This is exact only for a similarity, so a polyline with a nonzero
//!   bulge or width needs a transform whose two principal scale factors differ by at most
//!   `tolerance.linear` divided by its largest width or arc extent (half the chord times one
//!   plus the absolute bulge); otherwise the transform fails with
//!   `geom.transform.not_similarity`. A polyline without bulges or widths accepts any affine
//!   transform.
//!
//! For open arcs and elliptical arcs the parameter is preserved: the image of the point at `t`
//! is the point at `t` of the result. A closed circle or ellipse maps to a closed circle or
//! ellipse in its own canonical parameterization.
//!
//! The tolerance is checked first: a linear or angular tolerance that is NaN, infinite or
//! negative fails with `geom.tolerance.invalid`. A transform is singular when its linear 2×2
//! block, divided by its largest entry magnitude, is zero or has an exactly zero determinant;
//! scaling first keeps invertible blocks with very large or very small entries usable.
//! Lengths, directions and principal axes are computed from values scaled by their largest
//! component magnitude, so any input whose image is within the range of `f64` maps correctly;
//! when the image is not representable (it overflows, its axes or segments underflow until they
//! would degenerate, or an image ellipse has a subnormal minor-to-major ratio), the transform
//! fails with `geom.precision.insufficient`.

use arc_base::math::{Point2, Transform2, Vector2};
use arc_base::{Error, Result, Tolerance};

use crate::curve::Curve2;
use crate::error::codes;
use crate::polyline::{Polyline2, PolylineVertex};
use crate::primitives::{Arc2, Circle2, Ellipse2, EllipticalArc2, Line2, Ray2, Segment2, perp};
use crate::{largest_component, length};

/// Maps a failure to build a result value to `geom.precision.insufficient`: every input is
/// finite and valid by then, so a failure means that the result is out of the range of `f64`.
fn representable<T>(result: Result<T>) -> Result<T> {
    result.map_err(|error| Error::new(codes::PRECISION_INSUFFICIENT).with_source(error))
}

/// `a·d − b·c` to within two units in the last place of the result, and exactly zero only when
/// the exact value is zero (Kahan's algorithm with fused multiply-adds).
fn determinant(a: f64, b: f64, c: f64, d: f64) -> f64 {
    let w = b * c;
    // The rounding error of w, exactly.
    let error = (-b).mul_add(c, w);
    a.mul_add(d, -w) + error
}

/// The ellipse `factor · (p cos θ + q sin θ)` about a center, in principal form: the point at θ
/// is the point at eccentric angle `orientation · (θ − phase)` of the ellipse with major
/// semi-axis `major` and ratio `ratio()`.
///
/// Lengths are kept as `factor` times lengths of vectors whose largest component magnitude is
/// 1, so nothing is squared out of range. The minor length is the area `|p × q|` divided by the
/// major length, which the rotation to principal axes preserves; computed from an accurate
/// determinant it keeps its relative precision even for a very thin image, where rotating `p`
/// and `q` would leave only an absolute precision.
struct Principal {
    /// The major semi-axis at full size.
    major: Vector2,
    /// The major semi-axis divided by `factor`.
    scaled_major: Vector2,
    factor: f64,
    major_length: f64,
    minor_length: f64,
    phase: f64,
    orientation: f64,
}

impl Principal {
    /// `cross` is `p × q`, computed accurately by the caller.
    fn new(p: Vector2, q: Vector2, cross: f64, factor: f64) -> Result<Self> {
        let insufficient = || Error::new(codes::PRECISION_INSUFFICIENT);
        let components = [p.x, p.y, q.x, q.y];
        if !components
            .iter()
            .chain([&cross])
            .all(|value| value.is_finite())
        {
            return Err(insufficient());
        }
        let scale = components
            .iter()
            .fold(0.0_f64, |m, value| m.max(value.abs()));
        if scale == 0.0 {
            return Err(insufficient());
        }
        let (p, q, factor) = (p / scale, q / scale, factor * scale);
        // |p cos θ + q sin θ|² = (|p|² + |q|²)/2 + (|p|² − |q|²)/2·cos 2θ + p·q·sin 2θ is largest
        // at 2θ = atan2(2 p·q, |p|² − |q|²); a quarter turn later it is smallest.
        let phase = 0.5 * (2.0 * p.dot(&q)).atan2(p.norm_squared() - q.norm_squared());
        let (sin, cos) = phase.sin_cos();
        let scaled_major = p * cos + q * sin;
        // p cos θ + q sin θ = major cos(θ − phase) + minor sin(θ − phase) with minor = q cos θ −
        // p sin θ, and major × minor = p × q. When the minor axis points clockwise of the major
        // axis the eccentric angle runs backwards.
        let orientation = if cross >= 0.0 { 1.0 } else { -1.0 };
        let major_length = length(&scaled_major);
        let major = scaled_major * factor;
        let principal = Self {
            major,
            scaled_major,
            factor,
            major_length,
            minor_length: (cross / scale).abs() / scale / major_length,
            phase,
            orientation,
        };
        // Written so that a NaN fails the check. A subnormal ratio has lost its precision.
        let usable = factor.is_finite()
            && major.x.is_finite()
            && major.y.is_finite()
            && (major.x != 0.0 || major.y != 0.0)
            && principal.ratio() >= f64::MIN_POSITIVE;
        if usable {
            Ok(principal)
        } else {
            Err(insufficient())
        }
    }

    fn ratio(&self) -> f64 {
        (self.minor_length / self.major_length).min(1.0)
    }

    /// The eccentric angle of the result for angle `theta` of the input.
    fn angle(&self, theta: f64) -> f64 {
        self.orientation * (theta - self.phase)
    }

    /// The difference of the semi-axis lengths.
    fn anisotropy(&self) -> f64 {
        self.factor * (self.major_length - self.minor_length)
    }

    /// Whether the image is a circle within the linear tolerance.
    fn is_round(&self, tolerance: &Tolerance) -> bool {
        self.anisotropy() <= tolerance.linear
    }

    fn mean_radius(&self) -> f64 {
        self.factor * (0.5 * self.major_length + 0.5 * self.minor_length)
    }

    fn major_angle(&self) -> f64 {
        self.scaled_major.y.atan2(self.scaled_major.x)
    }
}

/// A validated transform.
struct Affine<'a> {
    transform: &'a Transform2,
    /// The linear 2×2 block, row by row, divided by `largest`.
    block: [f64; 4],
    /// The largest magnitude among the entries of the linear block.
    largest: f64,
    /// The determinant of `block`.
    determinant: f64,
}

impl<'a> Affine<'a> {
    fn new(transform: &'a Transform2) -> Result<Self> {
        let m = transform.matrix();
        crate::ensure_finite(m.as_slice())?;
        let entries = [m[(0, 0)], m[(0, 1)], m[(1, 0)], m[(1, 1)]];
        let largest = entries.iter().fold(0.0_f64, |l, entry| l.max(entry.abs()));
        if largest == 0.0 {
            return Err(Error::new(codes::TRANSFORM_SINGULAR));
        }
        // Scaling by the largest entry keeps the determinant of an invertible block in range.
        let block = entries.map(|entry| entry / largest);
        let [a, b, c, d] = block;
        let determinant = determinant(a, b, c, d);
        if determinant == 0.0 {
            return Err(Error::new(codes::TRANSFORM_SINGULAR));
        }
        Ok(Self {
            transform,
            block,
            largest,
            determinant,
        })
    }

    fn point(&self, p: &Point2) -> Point2 {
        self.transform.transform_point(p)
    }

    /// The image of `v` under the scaled linear block.
    fn apply(&self, v: &Vector2) -> Vector2 {
        let [a, b, c, d] = self.block;
        Vector2::new(a * v.x + b * v.y, c * v.x + d * v.y)
    }

    /// A vector in the direction of the image of the nonzero vector `v`, computed from scaled
    /// values so that it neither overflows nor underflows.
    fn direction(&self, v: &Vector2) -> Vector2 {
        self.apply(&(v / largest_component(v)))
    }

    /// The image of the ellipse `center + size · (p cos θ + q sin θ)`, where the components of
    /// `p` and `q` are at most 1 in magnitude.
    fn conic(
        &self,
        center: &Point2,
        p: &Vector2,
        q: &Vector2,
        size: f64,
    ) -> Result<(Point2, Principal)> {
        // The image of p × q is the determinant times p × q.
        let cross = self.determinant * determinant(p.x, q.x, p.y, q.y);
        Ok((
            self.point(center),
            Principal::new(self.apply(p), self.apply(q), cross, size * self.largest)?,
        ))
    }

    fn circular(&self, center: &Point2, radius: f64) -> Result<(Point2, Principal)> {
        self.conic(center, &Vector2::x(), &Vector2::y(), radius)
    }

    fn circle(&self, circle: &Circle2, tolerance: &Tolerance) -> Result<Curve2> {
        let (center, image) = self.circular(&circle.center(), circle.radius())?;
        representable(if image.is_round(tolerance) {
            Circle2::new(center, image.mean_radius()).map(Curve2::Circle)
        } else {
            Ellipse2::new(center, image.major, image.ratio()).map(Curve2::Ellipse)
        })
    }

    fn arc(&self, arc: &Arc2, tolerance: &Tolerance) -> Result<Curve2> {
        let (center, image) = self.circular(&arc.center(), arc.radius())?;
        let start = image.angle(arc.start_angle());
        let sweep = image.orientation * arc.sweep();
        representable(if image.is_round(tolerance) {
            // On a round image the eccentric angle from the major axis is the polar angle less
            // the major axis angle.
            Arc2::new(
                center,
                image.mean_radius(),
                image.major_angle() + start,
                sweep,
            )
            .map(Curve2::Arc)
        } else {
            Ellipse2::new(center, image.major, image.ratio())
                .and_then(|ellipse| EllipticalArc2::new(ellipse, start, sweep))
                .map(Curve2::EllipticalArc)
        })
    }

    fn ellipse(&self, ellipse: &Ellipse2) -> Result<(Ellipse2, Principal)> {
        let major = ellipse.major_axis();
        let size = largest_component(&major);
        let p = major / size;
        let (center, image) =
            self.conic(&ellipse.center(), &p, &(perp(&p) * ellipse.ratio()), size)?;
        Ok((
            representable(Ellipse2::new(center, image.major, image.ratio()))?,
            image,
        ))
    }

    fn polyline(&self, polyline: &Polyline2, tolerance: &Tolerance) -> Result<Polyline2> {
        let unit = self.circular(&Point2::origin(), 1.0)?;
        let anisotropy = unit.1.anisotropy();
        let vertices = polyline.vertices();
        // Every point of a bulged segment lies within half its chord times (1 + |bulge|) of the
        // chord midpoint; the image departs from a circular arc by at most the anisotropy times
        // that extent.
        let largest = vertices
            .iter()
            .zip(vertices.iter().cycle().skip(1))
            .take(polyline.segment_count())
            .filter(|(start, _)| start.bulge != 0.0)
            .map(|(start, end)| {
                length(&(end.point.coords * 0.5 - start.point.coords * 0.5))
                    * (1.0 + start.bulge.abs())
            })
            .chain(vertices.iter().flat_map(|v| [v.start_width, v.end_width]))
            .fold(0.0, f64::max);
        // Written so that a NaN fails the check; an exact similarity passes at any extent.
        let similar = anisotropy == 0.0 || anisotropy * largest <= tolerance.linear;
        if !similar {
            return Err(
                Error::new(codes::TRANSFORM_NOT_SIMILARITY).with_detail("anisotropy", anisotropy)
            );
        }
        let scale = unit.1.mean_radius();
        let vertices = vertices
            .iter()
            .map(|v| PolylineVertex {
                point: self.point(&v.point),
                bulge: v.bulge * self.determinant.signum(),
                start_width: v.start_width * scale,
                end_width: v.end_width * scale,
            })
            .collect();
        representable(Polyline2::new(vertices, polyline.is_closed()))
    }
}

impl Curve2 {
    /// The image of the curve under `transform`; the kind of the result can differ from the
    /// kind of the input, as described in the [module documentation](crate::transform).
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a linear or angular tolerance that is NaN, infinite or
    /// negative; `geom.input.not_finite` for a transform with a non-finite entry;
    /// `geom.transform.singular` for a transform that is not invertible;
    /// `geom.transform.not_similarity` for a polyline with arcs or widths under a transform that
    /// is not a similarity within the tolerance; `geom.precision.insufficient` when the result
    /// cannot be represented in `f64` (it overflows, or it underflows so far that it would
    /// degenerate).
    pub fn transformed(&self, transform: &Transform2, tolerance: &Tolerance) -> Result<Self> {
        crate::ensure_tolerance(tolerance)?;
        let map = Affine::new(transform)?;
        match self {
            Self::Segment(c) => representable(
                Segment2::new(map.point(&c.start()), map.point(&c.end())).map(Self::Segment),
            ),
            Self::Ray(c) => representable(
                Ray2::new(map.point(&c.origin()), map.direction(&c.direction())).map(Self::Ray),
            ),
            Self::Line(c) => representable(
                Line2::new(map.point(&c.point()), map.direction(&c.direction())).map(Self::Line),
            ),
            Self::Circle(c) => map.circle(c, tolerance),
            Self::Arc(c) => map.arc(c, tolerance),
            Self::Ellipse(c) => map.ellipse(c).map(|(e, _)| Self::Ellipse(e)),
            Self::EllipticalArc(c) => {
                let (ellipse, image) = map.ellipse(c.ellipse())?;
                representable(EllipticalArc2::new(
                    ellipse,
                    image.angle(c.start_parameter()),
                    image.orientation * c.sweep(),
                ))
                .map(Self::EllipticalArc)
            }
            Self::Polyline(c) => map.polyline(c, tolerance).map(Self::Polyline),
            Self::Nurbs(c) => representable(c.mapped(|p| map.point(p))).map(Self::Nurbs),
        }
    }
}

#[cfg(test)]
mod tests {
    use arc_base::math::Matrix3;
    use proptest::prelude::*;

    use super::*;
    use crate::curve::tests::{curve, extreme_curve, parameter};
    use crate::testing::{affine, similarity, similarity_with_scale};

    const CLOSE: f64 = 1e-7;

    fn matrix(a: f64, b: f64, c: f64, d: f64) -> Transform2 {
        Transform2::from_matrix_unchecked(Matrix3::new(a, b, 0.5, c, d, -2.0, 0.0, 0.0, 1.0))
    }

    /// Whether `q` lies on a circle, ellipse, ray or line; `None` for other kinds.
    fn lies_on(curve: &Curve2, q: &Point2) -> Option<bool> {
        Some(match curve {
            Curve2::Circle(c) => ((q - c.center()).norm() - c.radius()).abs() < CLOSE,
            Curve2::Ellipse(e) => {
                let d = q - e.center();
                let (u, v) = (e.major_axis(), e.minor_axis());
                let x = d.dot(&u) / u.norm_squared();
                let y = d.dot(&v) / v.norm_squared();
                ((x * x + y * y).sqrt() - 1.0).abs() * v.norm() < CLOSE
            }
            Curve2::Ray(r) => {
                let d = q - r.origin();
                r.unit_direction().perp(&d).abs() < CLOSE && r.unit_direction().dot(&d) > -CLOSE
            }
            Curve2::Line(l) => l.unit_direction().perp(&(q - l.point())).abs() < CLOSE,
            _ => return None,
        })
    }

    /// Checks `image` against the transformed samples of `original`: same parameter where the
    /// parameter is preserved, membership otherwise.
    fn agrees(original: &Curve2, image: &Curve2, map: impl Fn(Point2) -> Point2) -> bool {
        (0..=16).all(|i| {
            let t = parameter(original, f64::from(i) / 16.0);
            let q = map(original.point_at(t));
            lies_on(image, &q).unwrap_or_else(|| (image.point_at(t) - q).norm() < CLOSE)
        })
    }

    fn has_arcs_or_widths(curve: &Curve2) -> bool {
        matches!(curve, Curve2::Polyline(p) if p.vertices().iter().any(|v| {
            v.bulge != 0.0 || v.start_width != 0.0 || v.end_width != 0.0
        }))
    }

    /// Whether `q` lies on `image` (circles, ellipses, rays and lines) or is the point at the
    /// same parameter (other kinds), within `RELATIVE` of `size`. Every quantity is divided by
    /// `size` first, so nothing is squared out of range.
    fn agrees_relatively(
        original: &Curve2,
        image: &Curve2,
        map: impl Fn(Point2) -> Point2,
    ) -> bool {
        const RELATIVE: f64 = 1e-9;
        let samples: Vec<(f64, Point2)> = (0..=16)
            .map(|i| {
                let t = parameter(original, f64::from(i) / 16.0);
                (t, map(original.point_at(t)))
            })
            .collect();
        let size = samples
            .iter()
            .map(|(_, q)| q.coords.abs().max())
            .fold(f64::MIN_POSITIVE, f64::max);
        let rel = |p: Point2| p.coords / size;
        samples.iter().all(|(t, q)| {
            let q = rel(*q);
            let distance = match image {
                Curve2::Circle(c) => ((q - rel(c.center())).norm() - c.radius() / size).abs(),
                Curve2::Ellipse(e) => {
                    let d = q - rel(e.center());
                    // The minor axis is formed after scaling, where it cannot underflow.
                    let u = e.major_axis() / size;
                    let v = perp(&u) * e.ratio();
                    let (a, b) = (crate::length(&u), crate::length(&v));
                    let (x, y) = (d.dot(&u) / a, d.dot(&v) / b);
                    let rho = (x / a).hypot(y / b);
                    // The first-order distance, accurate near the curve, and the distances to
                    // the curve points straight across in each axis, upper bounds that stay
                    // tight for very thin ellipses.
                    let gradient = 2.0 * (x / (a * a)).hypot(y / (b * b));
                    let first_order = (rho * rho - 1.0).abs() / gradient;
                    let across = |s: f64, t: f64, s_axis: f64, t_axis: f64| {
                        if s.abs() <= s_axis {
                            (t.abs() - t_axis * (1.0 - (s / s_axis).powi(2)).sqrt()).abs()
                        } else {
                            f64::INFINITY
                        }
                    };
                    // |d| + a bounds the distance even when the axes vanish at this scale.
                    first_order
                        .min(across(x, y, a, b))
                        .min(across(y, x, b, a))
                        .min(crate::length(&d) + a)
                }
                Curve2::Ray(r) => {
                    let d = q - rel(r.origin());
                    let u = r.unit_direction();
                    u.perp(&d).abs().max(-u.dot(&d))
                }
                Curve2::Line(l) => l.unit_direction().perp(&(q - rel(l.point()))).abs(),
                _ => (rel(image.point_at(*t)) - q).norm(),
            };
            distance <= RELATIVE
        })
    }

    fn scale(a: f64, d: f64) -> Transform2 {
        Transform2::from_matrix_unchecked(Matrix3::new(a, 0.0, 0.0, 0.0, d, 0.0, 0.0, 0.0, 1.0))
    }

    #[test]
    fn extreme_radii_and_ratios_survive_the_identity() {
        let identity = Transform2::identity();
        let tolerance = Tolerance::default();
        for r in [
            5e-324, 1e-300, 1e-200, 1e-150, 1e150, 1e155, 1e160, 1e200, 1e300,
        ] {
            let circle = Curve2::Circle(Circle2::new(Point2::origin(), r).unwrap());
            assert_eq!(circle.transformed(&identity, &tolerance).unwrap(), circle);
            let arc = Arc2::new(Point2::new(1.0, -2.0), r, 0.3, 2.0).unwrap();
            let Curve2::Arc(image) = Curve2::Arc(arc).transformed(&identity, &tolerance).unwrap()
            else {
                panic!("expected an arc for radius {r:e}");
            };
            assert_eq!(image.radius(), r);
            assert!((image.start_angle() - 0.3).abs() < 1e-15);
            let ellipse = Ellipse2::new(Point2::origin(), Vector2::new(r, 0.0), 0.5).unwrap();
            let Curve2::Ellipse(image) = Curve2::Ellipse(ellipse)
                .transformed(&identity, &tolerance)
                .unwrap()
            else {
                panic!("expected an ellipse");
            };
            assert_eq!(image.major_axis(), ellipse.major_axis());
            assert_eq!(image.ratio(), 0.5);
        }
        let thin = Ellipse2::new(Point2::origin(), Vector2::x(), 1e-300).unwrap();
        let Curve2::Ellipse(image) = Curve2::Ellipse(thin)
            .transformed(&identity, &tolerance)
            .unwrap()
        else {
            panic!("expected an ellipse");
        };
        assert!((image.ratio() / 1e-300 - 1.0).abs() < 1e-15);
    }

    #[test]
    fn invertible_transforms_with_extreme_entries_are_not_singular() {
        let tolerance = Tolerance::default();
        let arc = Curve2::Arc(Arc2::new(Point2::new(1.0, 2.0), 1.0, 0.3, 2.0).unwrap());
        for f in [1e160, 1e-150, 1e-160, 1e-300] {
            let t = scale(f, f);
            let image = arc.transformed(&t, &tolerance).unwrap();
            assert!(matches!(image, Curve2::Arc(_)));
            assert!(agrees_relatively(&arc, &image, |p| t.transform_point(&p)));
        }
        for t in [scale(1e-160, 1e160), scale(1e160, 1e-160)] {
            // Invertible, but the image ellipse's ratio 1e-320 is subnormal.
            assert_eq!(
                arc.transformed(&t, &tolerance).unwrap_err().code(),
                "geom.precision.insufficient"
            );
            let segment =
                Curve2::Segment(Segment2::new(Point2::origin(), Point2::new(1.0, 1.0)).unwrap());
            let image = segment.transformed(&t, &tolerance).unwrap();
            assert!(agrees_relatively(&segment, &image, |p| t.transform_point(&p)));
        }
        let t = scale(1e-10, 1e10);
        let image = arc.transformed(&t, &tolerance).unwrap();
        assert!(agrees_relatively(&arc, &image, |p| t.transform_point(&p)));
        let circle = Curve2::Circle(Circle2::new(Point2::origin(), 1.0).unwrap());
        assert_eq!(
            circle
                .transformed(&scale(0.0, 0.0), &tolerance)
                .unwrap_err()
                .code(),
            "geom.transform.singular"
        );
    }

    #[test]
    fn unrepresentable_images_report_insufficient_precision() {
        let tolerance = Tolerance::default();
        let code = |c: Curve2, t: Transform2| c.transformed(&t, &tolerance).unwrap_err().code();
        let far = Segment2::new(Point2::new(1e10, 0.0), Point2::new(2e10, 0.0)).unwrap();
        assert_eq!(
            code(Curve2::Segment(far), scale(1e300, 1e300)),
            "geom.precision.insufficient"
        );
        let tiny = Segment2::new(Point2::origin(), Point2::new(1e-300, 0.0)).unwrap();
        assert_eq!(
            code(Curve2::Segment(tiny), scale(1e-300, 1e-300)),
            "geom.precision.insufficient"
        );
        let big = Circle2::new(Point2::origin(), 1e300).unwrap();
        assert_eq!(
            code(Curve2::Circle(big), scale(1e10, 1e10)),
            "geom.precision.insufficient"
        );
        let small = Circle2::new(Point2::origin(), 1e-300).unwrap();
        assert_eq!(
            code(Curve2::Circle(small), scale(1e-300, 1e-300)),
            "geom.precision.insufficient"
        );
        let ray = Ray2::new(Point2::new(1e300, 0.0), Vector2::new(1e-200, 0.0)).unwrap();
        assert_eq!(
            code(Curve2::Ray(ray), scale(1e10, 1e10)),
            "geom.precision.insufficient"
        );
        let ray = Ray2::new(Point2::origin(), Vector2::new(1e-200, 0.0)).unwrap();
        assert!(
            Curve2::Ray(ray)
                .transformed(&scale(1e-200, 1e-200), &tolerance)
                .is_ok()
        );
    }

    #[test]
    fn bulged_polylines_follow_extreme_uniform_scales() {
        let p = Curve2::Polyline(
            Polyline2::new(
                vec![
                    PolylineVertex::new(Point2::origin(), 0.5),
                    PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
                ],
                true,
            )
            .unwrap(),
        );
        for f in [1e-300, 1e-160, 1e160, 1e300] {
            let t = scale(f, f);
            let image = p.transformed(&t, &Tolerance::default()).unwrap();
            assert!(
                agrees_relatively(&p, &image, |q| t.transform_point(&q)),
                "{f:e}"
            );
        }
    }

    #[test]
    fn invalid_tolerances_are_rejected_before_anything_else() {
        let circle = Curve2::Circle(Circle2::new(Point2::origin(), 1.0).unwrap());
        let polyline = Curve2::Polyline(
            Polyline2::new(
                vec![
                    PolylineVertex::new(Point2::origin(), 0.9),
                    PolylineVertex::new(Point2::new(10.0, 0.0), 0.0),
                ],
                false,
            )
            .unwrap(),
        );
        let shear = matrix(1.0, 3.0, 0.0, 1.0);
        for bad in [f64::NAN, -1.0, f64::INFINITY, -0.5e-300] {
            for tolerance in [
                Tolerance {
                    linear: bad,
                    ..Tolerance::default()
                },
                Tolerance {
                    angular: bad,
                    ..Tolerance::default()
                },
            ] {
                for (curve, t) in [(&circle, Transform2::identity()), (&polyline, shear)] {
                    assert_eq!(
                        curve.transformed(&t, &tolerance).unwrap_err().code(),
                        "geom.tolerance.invalid"
                    );
                }
            }
        }
        // A huge but valid tolerance is accepted and keeps the round image a circle.
        let loose = Tolerance {
            linear: 10.0,
            ..Tolerance::default()
        };
        assert!(matches!(
            circle.transformed(&scale(5.0, 1.0), &loose).unwrap(),
            Curve2::Circle(_)
        ));
    }

    proptest! {
        #[test]
        fn widths_scale_by_the_mean_scale_factor_under_similarities(
            c in curve(), (t, factor) in similarity_with_scale(),
        ) {
            let Curve2::Polyline(p) = &c else { return Ok(()) };
            let Curve2::Polyline(image) = c.transformed(&t, &Tolerance::default()).unwrap() else {
                return Err(TestCaseError::fail("expected a polyline"));
            };
            for (v, w) in p.vertices().iter().zip(image.vertices()) {
                prop_assert!((w.start_width - v.start_width * factor).abs() <= 1e-12 * w.start_width);
                prop_assert!((w.end_width - v.end_width * factor).abs() <= 1e-12 * w.end_width);
            }
        }

        #[test]
        fn extreme_curves_survive_the_identity(c in extreme_curve()) {
            let image = c.transformed(&Transform2::identity(), &Tolerance::default()).unwrap();
            prop_assert_eq!(std::mem::discriminant(&image), std::mem::discriminant(&c));
            prop_assert!(agrees_relatively(&c, &image, |p| p));
        }

        #[test]
        fn extreme_curves_under_extreme_uniform_scales_are_right_or_insufficient(
            c in extreme_curve(), up in any::<bool>(),
        ) {
            let f = if up { 1e100 } else { 1e-100 };
            let t = scale(f, f);
            match c.transformed(&t, &Tolerance::default()) {
                Ok(image) => {
                    prop_assert_eq!(std::mem::discriminant(&image), std::mem::discriminant(&c));
                    prop_assert!(agrees_relatively(&c, &image, |p| t.transform_point(&p)));
                }
                Err(error) => prop_assert_eq!(error.code(), "geom.precision.insufficient"),
            }
        }

        #[test]
        fn affine_images_agree_with_transformed_samples(c in curve(), t in affine()) {
            let tolerance = Tolerance::default();
            match c.transformed(&t, &tolerance) {
                Ok(image) => prop_assert!(agrees(&c, &image, |p| t.transform_point(&p))),
                Err(error) => {
                    prop_assert_eq!(error.code(), "geom.transform.not_similarity");
                    prop_assert!(has_arcs_or_widths(&c));
                }
            }
        }

        #[test]
        fn similarity_images_keep_the_kind(c in curve(), t in similarity()) {
            let image = c.transformed(&t, &Tolerance::default()).unwrap();
            prop_assert_eq!(std::mem::discriminant(&image), std::mem::discriminant(&c));
            prop_assert!(agrees(&c, &image, |p| t.transform_point(&p)));
        }

        #[test]
        fn the_inverse_restores_the_curve(c in curve(), t in prop_oneof![affine(), similarity()]) {
            let tolerance = Tolerance::default();
            let Ok(image) = c.transformed(&t, &tolerance) else {
                return Ok(());
            };
            let back = image.transformed(&t.inverse(), &tolerance).unwrap();
            prop_assert!(agrees(&c, &back, |p| p));
        }
    }

    #[test]
    fn arc_under_non_uniform_scale_is_an_exact_elliptical_arc() {
        let arc = Curve2::Arc(Arc2::new(Point2::new(1.0, 1.0), 2.0, 0.3, 2.0).unwrap());
        let stretch = matrix(3.0, 0.0, 0.0, 1.0);
        let image = arc.transformed(&stretch, &Tolerance::default()).unwrap();
        let Curve2::EllipticalArc(e) = &image else {
            panic!("expected an elliptical arc, got {image:?}");
        };
        assert!((e.ellipse().major_axis().norm() - 6.0).abs() < 1e-12);
        assert!((e.ellipse().ratio() - 1.0 / 3.0).abs() < 1e-12);
        assert!(agrees(&arc, &image, |p| stretch.transform_point(&p)));
    }

    #[test]
    fn a_mirror_reverses_arcs_and_bulges() {
        let mirror = matrix(-1.0, 0.0, 0.0, 1.0);
        let arc = Curve2::Arc(Arc2::new(Point2::origin(), 1.0, 0.0, 1.0).unwrap());
        let Ok(Curve2::Arc(image)) = arc.transformed(&mirror, &Tolerance::default()) else {
            panic!("expected an arc");
        };
        assert!(image.sweep() < 0.0);
        let polyline = Polyline2::new(
            vec![
                PolylineVertex::new(Point2::origin(), 0.5),
                PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
            ],
            false,
        )
        .unwrap();
        let Ok(Curve2::Polyline(image)) =
            Curve2::Polyline(polyline).transformed(&mirror, &Tolerance::default())
        else {
            panic!("expected a polyline");
        };
        assert_eq!(image.vertices()[0].bulge, -0.5);
    }

    #[test]
    fn bad_transforms_and_bulged_polylines_under_shear_are_rejected() {
        let tolerance = Tolerance::default();
        let circle = Curve2::Circle(Circle2::new(Point2::origin(), 1.0).unwrap());
        let code = |t: &Transform2, c: &Curve2| c.transformed(t, &tolerance).unwrap_err().code();
        assert_eq!(
            code(&matrix(1.0, 2.0, 2.0, 4.0), &circle),
            "geom.transform.singular"
        );
        assert_eq!(
            code(&matrix(f64::NAN, 0.0, 0.0, 1.0), &circle),
            "geom.input.not_finite"
        );
        let shear = matrix(1.0, 0.5, 0.0, 1.0);
        let straight = vec![
            PolylineVertex::new(Point2::origin(), 0.0),
            PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
        ];
        let mut bulged = straight.clone();
        bulged[0].bulge = 0.2;
        let bulged = Curve2::Polyline(Polyline2::new(bulged, false).unwrap());
        assert_eq!(code(&shear, &bulged), "geom.transform.not_similarity");
        let straight = Curve2::Polyline(Polyline2::new(straight, false).unwrap());
        assert!(straight.transformed(&shear, &tolerance).is_ok());
    }
}
