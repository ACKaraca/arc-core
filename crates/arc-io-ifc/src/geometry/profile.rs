//! Profile definitions (CORE-IO-015): the loops of a profile, placed in the plane of the solid
//! that sweeps it.
//!
//! Supported: `IfcRectangleProfileDef` (`Position` 2, `XDim` 3, `YDim` 4), `IfcCircleProfileDef`
//! (`Position` 2, `Radius` 3), `IfcArbitraryClosedProfileDef` (`OuterCurve` 2) and
//! `IfcArbitraryProfileDefWithVoids` (`OuterCurve` 2, `InnerCurves` 3). The curves are
//! `IfcPolyline` and `IfcIndexedPolyCurve` with line segments in the plane, read by
//! [`Evaluator::curve_points`]. A circle is a regular polygon whose segment count is the smallest
//! whose sagitta does not exceed the chord tolerance, at least 3 and at most
//! [`MAX_CIRCLE_SEGMENTS`]; a circle that needs more is approximated and reported.
//!
//! Other profile types are not evaluated.

use crate::schema::access::known;
use std::f64::consts::TAU;

use arc_base::math::{Point3, Transform3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::Record;

use super::attr::{self, skip};
use super::polygon::P2;
use super::{Evaluator, MAX_LOOP_VERTICES, Step, VERTEX_BYTES};

/// The most segments of a tessellated circle, whatever the tolerance asks for.
pub(crate) const MAX_CIRCLE_SEGMENTS: usize = 512;

/// The fewest segments of a tessellated circle: a triangle.
const MIN_CIRCLE_SEGMENTS: usize = 3;

/// The segment count of a circle of `radius` whose chord deviation is at most `chord`, and whether
/// the count was limited by [`MAX_CIRCLE_SEGMENTS`]. Both values must be positive and finite.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the count is a positive ceiling, bounded before the cast"
)]
pub(crate) fn circle_segments(radius: f64, chord: f64) -> (usize, bool) {
    let ratio = (1.0 - chord / radius).clamp(-1.0, 1.0);
    // The half angle of one segment whose sagitta is `chord`: sagitta = radius (1 - cos(half)).
    let half = ratio.acos();
    let raw = std::f64::consts::PI / half;
    if !raw.is_finite() || raw > MAX_CIRCLE_SEGMENTS as f64 {
        return (MAX_CIRCLE_SEGMENTS, true);
    }
    ((raw.ceil() as usize).max(MIN_CIRCLE_SEGMENTS), false)
}

/// The point of a 2D point in the plane of `placement`, which maps the plane `z = 0` to itself.
fn place(placement: &Transform3, p: P2) -> Step<P2> {
    let q = placement.transform_point(&Point3::new(p[0], p[1], 0.0));
    if q.x.is_finite() && q.y.is_finite() {
        Ok([q.x, q.y])
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, "IFCPROFILEDEF"))
    }
}

/// A positive length of a profile, scaled to meters.
fn positive(parameter: Option<&crate::model::Parameter>, scale: f64, kind: &str) -> Step<f64> {
    let value = attr::real(parameter, scale, kind)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
    }
}

impl Evaluator<'_> {
    /// The loops of a profile definition in the plane of the solid, outer loop first, in meters.
    pub(crate) fn profile(&mut self, name: u64) -> Step<Vec<Vec<P2>>> {
        let file = self.file;
        let Some(instance) = file.instance(name) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCPROFILEDEF"));
        };
        let Some(record) = instance.records.first() else {
            return Err(skip(reasons::TARGET_MISSING, "IFCPROFILEDEF"));
        };
        match record.type_name.as_str() {
            "IFCRECTANGLEPROFILEDEF" => self.rectangle(record),
            "IFCCIRCLEPROFILEDEF" => self.circle(name, record),
            "IFCARBITRARYCLOSEDPROFILEDEF" => {
                let outer = self.profile_loop(record)?;
                Ok(vec![outer])
            }
            "IFCARBITRARYPROFILEDEFWITHVOIDS" => {
                let mut loops = vec![self.profile_loop(record)?];
                let kind = "IFCARBITRARYPROFILEDEFWITHVOIDS";
                // A repeated inner curve is the same hole: it is read once.
                let (inner, _) = attr::distinct_references(
                    &mut self.budget,
                    self.file.attr(record, known::PROFILE_INNER_CURVES),
                    kind,
                    usize::MAX,
                )?;
                // The outer loop and every hole, each bridged twice, are capped in total.
                let mut total = loops.first().map_or(0, Vec::len);
                for curve in inner {
                    self.budget.step()?;
                    loops.push(self.profile_curve(curve)?);
                    let hole = loops.last().map_or(0, Vec::len);
                    total = total.saturating_add(hole).saturating_add(2);
                    if total > MAX_LOOP_VERTICES {
                        return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
                    }
                }
                Ok(loops)
            }
            other => Err(skip(reasons::GEOMETRY_NOT_EVALUATED, other)),
        }
    }

    fn rectangle(&self, record: &Record) -> Step<Vec<Vec<P2>>> {
        let kind = "IFCRECTANGLEPROFILEDEF";
        let placement = self.axis2(attr::opt_reference(
            self.file.attr(record, known::PROFILE_POSITION),
            kind,
        )?)?;
        let x = positive(self.file.attr(record, known::RECTANGLE_X), self.scale, kind)? / 2.0;
        let y = positive(self.file.attr(record, known::RECTANGLE_Y), self.scale, kind)? / 2.0;
        let corners = [[-x, -y], [x, -y], [x, y], [-x, y]];
        let loop_ = corners
            .iter()
            .map(|&c| place(&placement, c))
            .collect::<Step<Vec<P2>>>()?;
        Ok(vec![loop_])
    }

    fn circle(&mut self, name: u64, record: &Record) -> Step<Vec<Vec<P2>>> {
        let kind = "IFCCIRCLEPROFILEDEF";
        let placement = self.axis2(attr::opt_reference(
            self.file.attr(record, known::PROFILE_POSITION),
            kind,
        )?)?;
        let radius = positive(
            self.file.attr(record, known::CIRCLE_RADIUS),
            self.scale,
            kind,
        )?;
        let (count, limited) = circle_segments(radius, self.tol.tessellation_chord);
        if limited {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        self.budget.array(count)?;
        self.budget.charge(
            u64::try_from(count)
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        let mut loop_ = Vec::with_capacity(count);
        for k in 0..count {
            let angle = TAU * (k as f64) / (count as f64);
            loop_.push(place(
                &placement,
                [radius * angle.cos(), radius * angle.sin()],
            )?);
        }
        Ok(vec![loop_])
    }

    /// The outer curve of an arbitrary profile.
    fn profile_loop(&mut self, record: &Record) -> Step<Vec<P2>> {
        let curve = attr::reference(
            self.file.attr(record, known::PROFILE_OUTER_CURVE),
            &record.type_name,
        )?;
        self.profile_curve(curve)
    }

    /// A profile curve: its points in the plane, with every third coordinate zero.
    fn profile_curve(&mut self, name: u64) -> Step<Vec<P2>> {
        let points = self.curve_points(name, MAX_LOOP_VERTICES)?;
        if points.len() > MAX_LOOP_VERTICES {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, "IFCPOLYLINE"));
        }
        points
            .iter()
            .map(|p| {
                if p.z == 0.0 {
                    Ok([p.x, p.y])
                } else {
                    Err(attr::wrong("IFCPOLYLINE"))
                }
            })
            .collect()
    }
}
