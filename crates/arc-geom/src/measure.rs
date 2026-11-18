//! Area, perimeter, centroid and second moments of loops and regions (CORE-GEOM-020).
//!
//! The integrals over the enclosed area follow from Green's theorem as sums over the boundary
//! pieces. Each piece from `a` to `b` contributes the signed triangle of the reference point
//! `O` with `a` and `b`, plus, for a circular or elliptical arc, the signed segment between its
//! chord and the arc. The reference point is the center of the bounding box, so no large
//! coordinate is subtracted from another.
//!
//! - **Triangles** have the closed forms `A = ½·a×b`, `∫x dA = A·(a + b)/3` and
//!   `∫x x' dA = A/12·(2·a aᵀ + a bᵀ + b aᵀ + 2·b bᵀ)` (coordinates relative to `O`).
//! - **Segments.** An elliptical arc is the image of an arc of the unit circle under the linear
//!   map `M = [major, minor]` of its eccentric angle, and a circular arc under `M = r·I`. The
//!   segment of the unit circle with half-sweep `β` has, in the frame of its chord (origin at
//!   the chord's middle, `u` along the chord, `v` toward the arc), the area `a₀`, the first
//!   moment `s_v = ∫v` and the second moments `j_uu = ∫u²` and `j_vv = ∫v²`; `∫u` and `∫uv`
//!   vanish by symmetry. For `β ≥ 1` they are evaluated in closed form as the sector minus the
//!   triangle with the center: `a₀ = β − sin β cos β`, `∫v' = ⅔·sin³β`,
//!   `∫u'² = (β − sin β cos β)/4 − sin³β cos β/6` and
//!   `∫v'² = (β + sin β cos β)/4 − sin β cos³β/2` about the center, moved to the chord by the
//!   parallel-axis rule. Below `β = 1` these closed forms cancel (for a nearly straight arc of
//!   huge radius they lose every digit), and the same integrals are evaluated as boundary
//!   integrals over the arc, `a₀ = ∫ v du`, `s_v = ∫ v²/2 du`, `j_vv = ∫ v³/3 du`,
//!   `j_uu = ∫ u² v du` with `u = sin t`, `v = 2·sin((β + t)/2)·sin((β − t)/2)` (which does not
//!   cancel) and `du = cos t dt`, by 8-point Gauss–Legendre quadrature on panels of at most
//!   ¼ radian. The integrands are trigonometric polynomials of degree at most 4, for which the
//!   quadrature error on such panels is below 1e-20 relative to the result, far below the
//!   rounding of `f64`. The unit moments are mapped by `M` (area by `det M`) and moved to the
//!   chord's middle in world coordinates. A polyline arc takes its radius and bisector from its
//!   exact chord below a half turn; a plain arc uses its exact radius and angles.
//! - The perimeter is the sum of the curve lengths: exact for segments and circular arcs, by
//!   the adaptive quadrature of [`CurveEval::length`] for elliptical arcs.

use std::f64::consts::PI;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};
use serde::{Deserialize, Serialize};

use crate::bbox::Aabb2;
use crate::error::codes;
use crate::eval::{CurveEval, Piece, cross};
use crate::region::{Element, Loop2, Region};

/// The half-sweep from which the closed forms of a unit circular segment are used.
const CLOSED_FORM_HALF_SWEEP: f64 = 1.0;
/// The widest quadrature panel, in radians of half-sweep.
const PANEL: f64 = 0.25;
/// The 8-point Gauss–Legendre nodes on [−1, 1] (positive half) and their weights.
const GAUSS_NODES: [f64; 4] = [
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_3,
];
const GAUSS_WEIGHTS: [f64; 4] = [
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_5,
    0.101_228_536_290_376_3,
];

/// The second moments of an area about the axes of a frame: `xx = ∫x² dA`, `yy = ∫y² dA` and
/// `xy = ∫x·y dA`, with `x` and `y` the coordinates in that frame. The moment of inertia about
/// the frame's x axis is `yy`, about its y axis `xx`, and the polar moment about its origin
/// `xx + yy`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SecondMoments {
    /// `∫x² dA`.
    pub xx: f64,
    /// `∫y² dA`.
    pub yy: f64,
    /// `∫x·y dA`.
    pub xy: f64,
}

/// The measurements of a loop or a region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Measurements {
    /// The enclosed area, positive.
    pub area: f64,
    /// The total length of the boundary, holes included.
    pub perimeter: f64,
    /// The centroid of the area.
    pub centroid: Point2,
    /// The second moments about axes through the centroid parallel to the x and y axes.
    pub centroidal: SecondMoments,
}

impl Measurements {
    /// The second moments in the frame with origin `origin` and x axis along `x_axis` (its y
    /// axis a quarter turn counterclockwise from it), by the parallel-axis rule and rotation.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite origin or axis; `geom.direction.zero` for a
    /// zero axis.
    pub fn about_axes(&self, origin: Point2, x_axis: Vector2) -> Result<SecondMoments> {
        crate::ensure_finite(&[origin.x, origin.y, x_axis.x, x_axis.y])?;
        if x_axis.x == 0.0 && x_axis.y == 0.0 {
            return Err(Error::new(codes::DIRECTION_ZERO));
        }
        let e = crate::unit(&x_axis);
        let f = crate::primitives::perp(&e);
        let c = &self.centroidal;
        let d = self.centroid - origin;
        let (dx, dy) = (d.dot(&e), d.dot(&f));
        // ∫(p·e)² = eᵀ J e, with J the centroidal matrix [[xx, xy], [xy, yy]].
        let quadratic = |a: &Vector2, b: &Vector2| {
            c.xx * a.x * b.x + c.xy * (a.x * b.y + a.y * b.x) + c.yy * a.y * b.y
        };
        Ok(SecondMoments {
            xx: quadratic(&e, &e) + self.area * dx * dx,
            yy: quadratic(&f, &f) + self.area * dy * dy,
            xy: quadratic(&e, &f) + self.area * dx * dy,
        })
    }
}

/// Signed area integrals relative to a reference point: area, first moments and second
/// moments.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Integrals {
    pub(crate) area: f64,
    sx: f64,
    sy: f64,
    xx: f64,
    yy: f64,
    xy: f64,
}

impl Integrals {
    fn add(&mut self, other: &Self) {
        self.area += other.area;
        self.sx += other.sx;
        self.sy += other.sy;
        self.xx += other.xx;
        self.yy += other.yy;
        self.xy += other.xy;
    }

    fn negated(&self) -> Self {
        Self {
            area: -self.area,
            sx: -self.sx,
            sy: -self.sy,
            xx: -self.xx,
            yy: -self.yy,
            xy: -self.xy,
        }
    }

    /// The integrals of the triangle `(0, a, b)`.
    fn triangle(a: Vector2, b: Vector2) -> Self {
        let area = 0.5 * cross(&a, &b);
        let k = area / 12.0;
        Self {
            area,
            sx: area * (a.x + b.x) / 3.0,
            sy: area * (a.y + b.y) / 3.0,
            xx: 2.0 * k * (a.x * a.x + a.x * b.x + b.x * b.x),
            yy: 2.0 * k * (a.y * a.y + a.y * b.y + b.y * b.y),
            xy: k * (2.0 * a.x * a.y + a.x * b.y + b.x * a.y + 2.0 * b.x * b.y),
        }
    }

    /// The image of the unit segment with half-sweep `beta` placed with its chord middle at
    /// `mid`, its `u` axis mapped to `u` and its `v` axis to `v`, weighted by `w`.
    fn segment(beta: f64, mid: Vector2, u: Vector2, v: Vector2, w: f64) -> Self {
        let [a0, sv, juu, jvv] = unit_segment(beta);
        let area = w * a0;
        Self {
            area,
            sx: w * (a0 * mid.x + sv * v.x),
            sy: w * (a0 * mid.y + sv * v.y),
            xx: w
                * (a0 * mid.x * mid.x + 2.0 * sv * mid.x * v.x + juu * u.x * u.x + jvv * v.x * v.x),
            yy: w
                * (a0 * mid.y * mid.y + 2.0 * sv * mid.y * v.y + juu * u.y * u.y + jvv * v.y * v.y),
            xy: w
                * (a0 * mid.x * mid.y
                    + sv * (mid.x * v.y + v.x * mid.y)
                    + juu * u.x * u.y
                    + jvv * v.x * v.y),
        }
    }
}

/// `[a₀, s_v, j_uu, j_vv]` of the segment of the unit circle with half-sweep `beta` in (0, π],
/// in the frame of its chord.
fn unit_segment(beta: f64) -> [f64; 4] {
    if beta >= CLOSED_FORM_HALF_SWEEP {
        let (s, c) = beta.sin_cos();
        let a0 = beta - s * c;
        let sv_center = 2.0 / 3.0 * s * s * s;
        let juu = (beta - s * c) / 4.0 - s * s * s * c / 6.0;
        let jvv_center = (beta + s * c) / 4.0 - s * c * c * c / 2.0;
        // The chord lies at v' = cos β from the center.
        let sv = sv_center - c * a0;
        let jvv = jvv_center - 2.0 * c * sv_center + c * c * a0;
        return [a0, sv, juu, jvv];
    }
    // The integrands are even in t, so twice the integral over [0, β].
    let panels = (beta / PANEL).ceil().max(1.0);
    let width = beta / panels;
    let mut sums = [0.0; 4];
    let mut lo = 0.0;
    // At most four panels below the closed-form threshold.
    for _ in 0..4 {
        if lo >= beta {
            break;
        }
        let half = 0.5 * width;
        let center = lo + half;
        for (node, weight) in GAUSS_NODES.iter().zip(GAUSS_WEIGHTS) {
            for t in [center - half * node, center + half * node] {
                let (u, du) = t.sin_cos();
                let v = 2.0 * (0.5 * (beta + t)).sin() * (0.5 * (beta - t)).sin();
                let f = weight * half * du;
                sums[0] += f * v;
                sums[1] += f * v * v / 2.0;
                sums[2] += f * u * u * v;
                sums[3] += f * v * v * v / 3.0;
            }
        }
        lo += width;
    }
    [2.0 * sums[0], 2.0 * sums[1], 2.0 * sums[2], 2.0 * sums[3]]
}

/// The signed integrals of the area between `origin` and the piece from `start` to `end`.
pub(crate) fn piece_integrals(
    piece: &Piece,
    start: Point2,
    end: Point2,
    origin: Point2,
) -> Integrals {
    let (a, b) = (start - origin, end - origin);
    let mut total = Integrals::triangle(a, b);
    let mid = 0.5 * (a + b);
    let chord = end - start;
    match piece {
        Piece::Arc(arc) => {
            let sweep = arc.sweep();
            let beta = 0.5 * sweep.abs().min(2.0 * PI);
            // The arc of a polyline segment has exact end points, and below a half turn its
            // radius and bisector follow from its chord; a plain arc has an exact radius and
            // angles.
            let (radius, toward) =
                if arc.chord().is_some() && sweep.abs() < PI && chord != Vector2::zeros() {
                    let unit = crate::unit(&chord);
                    // A counterclockwise arc bulges to the right of its chord.
                    let right = Vector2::new(unit.y, -unit.x);
                    let toward = if sweep > 0.0 { right } else { -right };
                    (crate::length(&chord) / (2.0 * beta.sin()), toward)
                } else {
                    let angle = arc.angle_at(0.5 * sweep.abs());
                    (arc.radius(), Vector2::new(angle.cos(), angle.sin()))
                };
            let u = crate::primitives::perp(&toward) * radius;
            let v = toward * radius;
            total.add(&Integrals::segment(
                beta,
                mid,
                u,
                v,
                sweep.signum() * radius * radius,
            ));
        }
        Piece::Elliptical(arc) => {
            let sweep = arc.sweep();
            let beta = 0.5 * sweep.abs().min(2.0 * PI);
            let angle = arc.angle_at(0.5 * sweep.abs());
            let (sin, cos) = angle.sin_cos();
            let ellipse = arc.ellipse();
            let (major, minor) = (ellipse.major_axis(), ellipse.minor_axis());
            let u = minor * cos - major * sin;
            let v = major * cos + minor * sin;
            total.add(&Integrals::segment(
                beta,
                mid,
                u,
                v,
                sweep.signum() * cross(&major, &minor),
            ));
        }
        Piece::Segment(_) | Piece::Ray(_) | Piece::Line(_) => {}
    }
    total
}

fn loop_integrals(elements: &[Element], origin: Point2) -> Integrals {
    let mut total = Integrals::default();
    for e in elements {
        total.add(&piece_integrals(&e.piece, e.start, e.end, origin));
    }
    total
}

fn finish(total: &Integrals, origin: Point2, perimeter: f64) -> Result<Measurements> {
    let area = total.area;
    let values = [
        area, total.sx, total.sy, total.xx, total.yy, total.xy, perimeter,
    ];
    if values.iter().any(|v| !v.is_finite()) || area <= 0.0 {
        return Err(Error::new(codes::PRECISION_INSUFFICIENT).with_detail("area", area));
    }
    let (cx, cy) = (total.sx / area, total.sy / area);
    let centroid = origin + Vector2::new(cx, cy);
    crate::ensure_finite(&[centroid.x, centroid.y])?;
    Ok(Measurements {
        area,
        perimeter,
        centroid,
        centroidal: SecondMoments {
            xx: total.xx - total.sx * cx,
            yy: total.yy - total.sy * cy,
            xy: total.xy - total.sx * cy,
        },
    })
}

fn center(boxed: &Aabb2) -> Point2 {
    // Halves first, so the sum of two huge coordinates does not overflow.
    Point2::new(
        0.5 * boxed.min().x + 0.5 * boxed.max().x,
        0.5 * boxed.min().y + 0.5 * boxed.max().y,
    )
}

fn perimeter(lp: &Loop2, tolerance: &Tolerance) -> Result<f64> {
    lp.curves().iter().map(|c| c.length(tolerance)).sum()
}

impl Loop2 {
    /// The area, perimeter, centroid and centroidal second moments of the area the loop
    /// encloses, whatever its orientation; see the [module documentation](crate::measure).
    ///
    /// # Errors
    ///
    /// `geom.tolerance.invalid` for a tolerance whose linear part is not finite and positive
    /// (used for the length of elliptical arcs); `geom.precision.insufficient` when a result
    /// is not finite in `f64`.
    pub fn measure(&self, tolerance: &Tolerance) -> Result<Measurements> {
        crate::eval::ensure_tolerance("linear", tolerance.linear)?;
        let origin = center(&self.bounding_box());
        let total = loop_integrals(&self.elements(), origin);
        let total = if total.area < 0.0 {
            total.negated()
        } else {
            total
        };
        finish(&total, origin, perimeter(self, tolerance)?)
    }
}

impl Region {
    /// The area, perimeter (holes included), centroid and centroidal second moments of the
    /// region; see the [module documentation](crate::measure).
    ///
    /// # Errors
    ///
    /// As [`Loop2::measure`].
    pub fn measure(&self, tolerance: &Tolerance) -> Result<Measurements> {
        crate::eval::ensure_tolerance("linear", tolerance.linear)?;
        let boxed = self
            .parts()
            .iter()
            .fold(Aabb2::EMPTY, |b, p| b.union(&p.outer().bounding_box()));
        let origin = center(&boxed);
        let mut total = Integrals::default();
        let mut length = 0.0;
        for part in self.parts() {
            for lp in std::iter::once(part.outer()).chain(part.holes()) {
                total.add(&loop_integrals(&lp.elements(), origin));
                length += perimeter(lp, tolerance)?;
            }
        }
        finish(&total, origin, length)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use arc_base::math::Vector2;
    use proptest::prelude::*;

    use super::*;
    use crate::curve::Curve2;
    use crate::flatten::CurveFlatten;
    use crate::polyline::{Polyline2, PolylineVertex};
    use crate::primitives::{Arc2, Ellipse2, Segment2};
    use crate::region::RegionPart;
    use crate::region::tests::{TOL, circle, polygon, square};

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * a.abs().max(b.abs()).max(f64::MIN_POSITIVE)
    }

    fn assert_close(a: f64, b: f64, rel: f64) {
        assert!(close(a, b, rel), "{a} != {b}");
    }

    /// The closed forms of the unit segment, evaluated directly.
    fn closed_form(beta: f64) -> [f64; 4] {
        let (s, c) = beta.sin_cos();
        let a0 = beta - s * c;
        let sv = 2.0 / 3.0 * s.powi(3) - c * a0;
        let juu = (beta - s * c) / 4.0 - s.powi(3) * c / 6.0;
        let jvv = (beta + s * c) / 4.0 - s * c.powi(3) / 2.0 - 2.0 * c * 2.0 / 3.0 * s.powi(3)
            + c * c * a0;
        [a0, sv, juu, jvv]
    }

    #[test]
    fn quadrature_matches_the_closed_forms_where_both_are_accurate() {
        for beta in [0.6, 0.8, 0.999] {
            let (q, c) = (unit_segment(beta), closed_form(beta));
            for (x, y) in q.iter().zip(c) {
                assert_close(*x, y, 1e-12);
            }
        }
        // Continuity across the switch at β = 1.
        let (below, above) = (unit_segment(1.0 - 1e-12), unit_segment(1.0));
        for (x, y) in below.iter().zip(above) {
            assert_close(*x, y, 1e-10);
        }
        // Small sweeps: the leading terms a₀ ≈ 4β³/3, s_v ≈ 4β⁵/15·… stay accurate.
        let tiny = unit_segment(1e-8);
        assert_close(tiny[0], 2.0 / 3.0 * 1e-24, 1e-12);
    }

    #[test]
    fn rectangles_disks_annuli_and_ellipses_match_closed_forms() {
        let rect = polygon(&[(1.0, 2.0), (5.0, 2.0), (5.0, 3.0), (1.0, 3.0)], 0.0);
        let m = rect.measure(&TOL).unwrap();
        assert_close(m.area, 4.0, 1e-15);
        assert_close(m.perimeter, 10.0, 1e-15);
        assert!((m.centroid - Point2::new(3.0, 2.5)).norm() < 1e-14);
        assert_close(m.centroidal.xx, 1.0 * 4.0_f64.powi(3) / 12.0, 1e-14);
        assert_close(m.centroidal.yy, 4.0 / 12.0, 1e-14);
        assert!(m.centroidal.xy.abs() < 1e-14);
        // The same rectangle clockwise measures the same.
        assert_eq!(rect.reversed().measure(&TOL).unwrap().area, m.area);

        let r = 3.0;
        let disk = circle(1.0, -2.0, r).measure(&TOL).unwrap();
        assert_close(disk.area, PI * r * r, 1e-15);
        assert_close(disk.perimeter, TAU * r, 1e-15);
        assert!((disk.centroid - Point2::new(1.0, -2.0)).norm() < 1e-14);
        assert_close(disk.centroidal.xx, PI * r.powi(4) / 4.0, 1e-14);
        assert_close(disk.centroidal.yy, PI * r.powi(4) / 4.0, 1e-14);

        let annulus = Region::new(vec![RegionPart::new(
            circle(0.0, 0.0, 2.0),
            vec![circle(0.0, 0.0, 1.0)],
        )])
        .unwrap()
        .measure(&TOL)
        .unwrap();
        assert_close(annulus.area, 3.0 * PI, 1e-15);
        assert_close(annulus.perimeter, 6.0 * PI, 1e-15);
        assert_close(annulus.centroidal.xx, PI * 15.0 / 4.0, 1e-14);
        // About a tangent line at y = 2: I = I_c + A·2².
        let about = annulus
            .about_axes(Point2::new(0.0, 2.0), Vector2::new(1.0, 0.0))
            .unwrap();
        assert_close(about.yy, PI * 15.0 / 4.0 + 3.0 * PI * 4.0, 1e-14);
        assert_close(about.xx, PI * 15.0 / 4.0, 1e-14);

        let (a, b) = (4.0, 1.5);
        let e =
            Ellipse2::new(Point2::new(2.0, 1.0), Vector2::new(a * 0.6, a * 0.8), b / a).unwrap();
        let lp = Loop2::new(vec![Curve2::Ellipse(e)], &TOL).unwrap();
        let m = lp.measure(&TOL).unwrap();
        assert_close(m.area, PI * a * b, 1e-14);
        assert!((m.centroid - Point2::new(2.0, 1.0)).norm() < 1e-13);
        let frame = m.about_axes(m.centroid, Vector2::new(0.6, 0.8)).unwrap();
        assert_close(frame.xx, PI * a.powi(3) * b / 4.0, 1e-13);
        assert_close(frame.yy, PI * a * b.powi(3) / 4.0, 1e-13);
        assert!(frame.xy.abs() < 1e-12);
    }

    #[test]
    fn circular_segments_and_rounded_rectangles_match_closed_forms() {
        for theta in [1e-6, 0.3, 1.0, 2.5, PI, 4.0, 6.0] {
            let r = 2.0;
            let arc = Arc2::new(Point2::origin(), r, -0.5 * theta, theta).unwrap();
            let (p, q) = (arc.point_at(0.0), arc.point_at(theta));
            let lp = Loop2::new(
                vec![
                    Curve2::Arc(arc),
                    Curve2::Segment(Segment2::new(q, p).unwrap()),
                ],
                &TOL,
            )
            .unwrap();
            let m = lp.measure(&TOL).unwrap();
            let area = 0.5 * r * r * crate::region::sweep_minus_sine(theta);
            assert_close(m.area, area, 1e-12);
            let distance = 4.0 * r * (0.5 * theta).sin().powi(3)
                / (3.0 * crate::region::sweep_minus_sine(theta));
            assert_close(m.centroid.x, distance, 1e-9);
            assert!(m.centroid.y.abs() < 1e-12 * r);
        }
        // Rounded rectangle w × h with corner radius c, as a bulge polyline.
        let (w, h, c) = (6.0, 4.0, 1.0);
        let k = (PI / 8.0).tan();
        let v = |x: f64, y: f64, b: f64| PolylineVertex::new(Point2::new(x, y), b);
        let rounded = Loop2::new(
            vec![Curve2::Polyline(
                Polyline2::new(
                    vec![
                        v(c, 0.0, 0.0),
                        v(w - c, 0.0, k),
                        v(w, c, 0.0),
                        v(w, h - c, k),
                        v(w - c, h, 0.0),
                        v(c, h, k),
                        v(0.0, h - c, 0.0),
                        v(0.0, c, k),
                    ],
                    true,
                )
                .unwrap(),
            )],
            &TOL,
        )
        .unwrap();
        let m = rounded.measure(&TOL).unwrap();
        assert_close(m.area, w * h - (4.0 - PI) * c * c, 1e-14);
        assert_close(m.perimeter, 2.0 * (w + h) - 8.0 * c + TAU * c, 1e-14);
        assert!((m.centroid - Point2::new(w / 2.0, h / 2.0)).norm() < 1e-13);
    }

    #[test]
    fn nearly_straight_arcs_and_extreme_magnitudes_keep_their_digits() {
        // A bulge of 1e-10 on a unit chord: a segment of area ⅔·1·(sagitta) on a radius of 2.5e9.
        for bulge in [1e-10, 1e-5] {
            let lp = polygon(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], 0.0);
            let bent = Loop2::new(
                vec![Curve2::Polyline(
                    Polyline2::new(
                        vec![
                            PolylineVertex::new(Point2::new(0.0, 0.0), bulge),
                            PolylineVertex::new(Point2::new(1.0, 0.0), 0.0),
                            PolylineVertex::new(Point2::new(1.0, 1.0), 0.0),
                            PolylineVertex::new(Point2::new(0.0, 1.0), 0.0),
                        ],
                        true,
                    )
                    .unwrap(),
                )],
                &TOL,
            )
            .unwrap();
            let (m, flat) = (bent.measure(&TOL).unwrap(), lp.measure(&TOL).unwrap());
            // A positive bulge bulges to the right of the bottom edge: outward, adding area.
            let sagitta = bulge / 2.0;
            assert_close(m.area - flat.area, 2.0 / 3.0 * sagitta, 1e-6);
        }
        for scale in [1e-70, 1e-30, 1e30, 1e70] {
            let m = square(3.0 * scale, -scale, scale).measure(&TOL).unwrap();
            assert_close(m.area, 4.0 * scale * scale, 1e-14);
            assert_close(m.centroidal.xx, 16.0 / 12.0 * scale.powi(4), 1e-13);
        }
        assert_eq!(
            square(0.0, 0.0, 1e100).measure(&TOL).unwrap_err().code(),
            "geom.precision.insufficient"
        );
        let m = square(0.0, 0.0, 1.0).measure(&TOL).unwrap();
        assert_eq!(
            m.about_axes(Point2::origin(), Vector2::zeros())
                .unwrap_err()
                .code(),
            "geom.direction.zero"
        );
        assert_eq!(
            m.about_axes(Point2::new(f64::NAN, 0.0), Vector2::new(1.0, 0.0))
                .unwrap_err()
                .code(),
            "geom.input.not_finite"
        );
        let bad = Tolerance {
            linear: f64::NAN,
            ..TOL
        };
        assert_eq!(
            square(0.0, 0.0, 1.0).measure(&bad).unwrap_err().code(),
            "geom.tolerance.invalid"
        );
    }

    /// The integrals of a closed polygon by its triangle fan.
    fn polygon_integrals(points: &[Point2], origin: Point2) -> Integrals {
        let mut total = Integrals::default();
        for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
            total.add(&Integrals::triangle(a - origin, b - origin));
        }
        total
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(crate::arrange::tests::cases(16)))]

        /// Moments of random regions with line, arc and elliptical boundaries agree with those of
        /// their dense flattening, also scaled to extreme magnitudes.
        #[test]
        fn moments_match_the_flattened_boundary(
            r in crate::region::tests::region(),
            exponent in -70.0..70.0_f64,
        ) {
            let chord = 1e-4;
            let exact = r.measure(&TOL).unwrap();
            let origin = exact.centroid;
            let mut total = Integrals::default();
            for lp in r.parts().iter().flat_map(|p| std::iter::once(p.outer()).chain(p.holes())) {
                let points: Vec<Point2> = lp
                    .curves()
                    .iter()
                    .flat_map(|c| c.flatten(chord, 0.002, 1_000_000).unwrap())
                    .collect();
                total.add(&polygon_integrals(&points, origin));
            }
            // A chord error of ε changes the area by about perimeter·ε.
            let rel = 1e-5;
            prop_assert!(close(exact.area, total.area, rel));
            let size = exact.area.sqrt();
            prop_assert!((total.sx / total.area).abs() < rel * size);
            prop_assert!((total.sy / total.area).abs() < rel * size);
            let scale = exact.area * exact.area;
            prop_assert!((exact.centroidal.xx - total.xx).abs() < rel * scale);
            prop_assert!((exact.centroidal.yy - total.yy).abs() < rel * scale);
            prop_assert!((exact.centroidal.xy - total.xy).abs() < rel * scale);
            // Scaling by s multiplies area by s², second moments by s⁴.
            let s = 10.0_f64.powf(exponent);
            let tolerance = Tolerance { linear: 1e-6 * s, ..TOL };
            let scaled = r
                .transformed(
                    &arc_base::math::Transform2::from_matrix_unchecked(
                        arc_base::math::Matrix3::new(s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0),
                    ),
                    // Loop2::transformed compares the length of a source joint with the
                    // tolerance of the image, so shrinking images keep the default.
                    &Tolerance { linear: 1e-6 * s.max(1.0), ..TOL },
                )
                .unwrap()
                .measure(&tolerance)
                .unwrap();
            prop_assert!(close(scaled.area, exact.area * s * s, 1e-12));
            prop_assert!(close(scaled.perimeter, exact.perimeter * s, 1e-9));
            prop_assert!(close(scaled.centroidal.xx, exact.centroidal.xx * s.powi(4), 1e-9));
        }
    }
}
