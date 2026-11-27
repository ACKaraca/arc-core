//! Profile definitions (CORE-IO-015): the loops of a profile, placed in the plane of the solid
//! that sweeps it. A profile is a list of parts, each an outer loop followed by its holes.
//!
//! Attributes are read by name through the schema tables, so the IFC2X3, IFC4 and IFC4X3 layouts
//! each read as declared. Supported:
//! - `IfcRectangleProfileDef`, `IfcRoundedRectangleProfileDef`, `IfcRectangleHollowProfileDef`,
//!   `IfcCircleProfileDef`, `IfcCircleHollowProfileDef`, `IfcEllipseProfileDef`;
//! - the shapes `IfcIShapeProfileDef`, `IfcLShapeProfileDef`, `IfcTShapeProfileDef`,
//!   `IfcUShapeProfileDef`, `IfcCShapeProfileDef`, `IfcZShapeProfileDef`,
//!   `IfcAsymmetricIShapeProfileDef` and `IfcTrapeziumProfileDef`, each with its origin at the
//!   center of its bounding box in the layouts below. The fillet and edge radii that the shape
//!   declares are rounded corners of the polygon. A flange, leg or web slope is not applied (the
//!   profile is the straight-sided shape) and is reported as approximated when it is not zero;
//! - `IfcArbitraryClosedProfileDef` and `IfcArbitraryProfileDefWithVoids` over any curve that
//!   [`Evaluator::curve_points`] evaluates;
//! - `IfcCompositeProfileDef` (the parts of its profiles, side by side; overlapping profiles give
//!   overlapping solids), `IfcDerivedProfileDef` (its parent transformed by the 2D operator) and
//!   `IfcMirroredProfileDef` (its parent with x negated);
//! - `IfcCenterLineProfileDef`: the strip of `Thickness` around a polyline curve, mitered at the
//!   corners; a closed curve gives a ring.
//!
//! Layouts (x to the right, y up, center of the bounding box at the origin): I, T, U, C and Z have
//! `Depth` along y; I has equal flanges at the bottom and top; T has its flange at the top; U and C
//! open toward +x with the web at -x; the Z has its top flange toward -x and its bottom flange toward
//! +x (`FlangeWidth` is measured from the center line of the web to the flange tip); L has its vertical leg at -x and its horizontal leg at the bottom; the trapezium has its bottom
//! edge centered and its top edge starting at `TopXOffset` from the bottom edge's left end.
//!
//! A circle is a regular polygon whose segment count is the smallest whose sagitta does not exceed the
//! chord tolerance, at least 3 and at most [`MAX_CIRCLE_SEGMENTS`]; a circle that needs more is
//! approximated and reported.

use crate::schema::access::known;
use std::f64::consts::{PI, TAU};

use arc_base::math::{Point3, Transform3};

use crate::error::reasons;
use crate::loss::Source;
use crate::model::{Parameter, Record};

use super::attr::{self, named, skip};
use super::curves::arc_segments;
use super::polygon::P2;
use super::{Evaluator, MAX_LOOP_VERTICES, Step, VERTEX_BYTES};

/// The most segments of a tessellated circle, whatever the tolerance asks for.
pub(crate) const MAX_CIRCLE_SEGMENTS: usize = 512;

/// The fewest segments of a tessellated circle: a triangle.
const MIN_CIRCLE_SEGMENTS: usize = 3;

/// The deepest nesting of composite and derived profiles.
const MAX_PROFILE_NESTING: u32 = 8;

/// The parts of a profile: each part is an outer loop and then its holes.
pub(crate) type Parts = Vec<Vec<Vec<P2>>>;

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
    let raw = PI / half;
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
fn positive(parameter: Option<&Parameter>, scale: f64, kind: &str) -> Step<f64> {
    let value = attr::real(parameter, scale, kind)?;
    if value > 0.0 {
        Ok(value)
    } else {
        Err(skip(reasons::GEOMETRY_DEGENERATE, kind))
    }
}

/// A corner of a polygon and the radius of its fillet (zero for a sharp corner).
type Corner = (P2, f64);

fn sharp(points: &[P2]) -> Vec<Corner> {
    points.iter().map(|&p| (p, 0.0)).collect()
}

/// A 2D operator: the matrix columns and the origin.
struct Operator2 {
    x: P2,
    y: P2,
    origin: P2,
}

impl Operator2 {
    fn apply(&self, p: P2) -> P2 {
        [
            self.origin[0] + self.x[0] * p[0] + self.y[0] * p[1],
            self.origin[1] + self.x[1] * p[0] + self.y[1] * p[1],
        ]
    }
}

impl Evaluator<'_> {
    /// The parts of a profile definition in the plane of the solid, in meters.
    pub(crate) fn profile(&mut self, name: u64) -> Step<Parts> {
        self.profile_at(name, 0)
    }

    fn profile_at(&mut self, name: u64, depth: u32) -> Step<Parts> {
        let file = self.file;
        let Some(record) = file.instance(name).and_then(|i| i.records.first()) else {
            return Err(skip(reasons::TARGET_MISSING, "IFCPROFILEDEF"));
        };
        if depth > MAX_PROFILE_NESTING {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, &record.type_name));
        }
        let kind = record.type_name.as_str();
        match kind {
            "IFCARBITRARYCLOSEDPROFILEDEF" => {
                let outer = self.profile_loop(record)?;
                Ok(vec![vec![outer]])
            }
            "IFCARBITRARYPROFILEDEFWITHVOIDS" => self.with_voids(record),
            "IFCCOMPOSITEPROFILEDEF" => self.composite_profile(name, record, depth),
            "IFCDERIVEDPROFILEDEF" | "IFCMIRROREDPROFILEDEF" => self.derived(name, record, depth),
            "IFCCENTERLINEPROFILEDEF" => self.center_line(record),
            _ => {
                let loops = self.parameterized(name, record)?;
                Ok(vec![loops])
            }
        }
    }

    fn with_voids(&mut self, record: &Record) -> Step<Parts> {
        let kind = "IFCARBITRARYPROFILEDEFWITHVOIDS";
        let mut loops = vec![self.profile_loop(record)?];
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
        Ok(vec![loops])
    }

    fn composite_profile(&mut self, name: u64, record: &Record, depth: u32) -> Step<Parts> {
        let kind = "IFCCOMPOSITEPROFILEDEF";
        let (profiles, _) = attr::distinct_references(
            &mut self.budget,
            named(self.file, record, "Profiles", 2),
            kind,
            usize::MAX,
        )?;
        let mut parts: Parts = Vec::new();
        for profile in profiles {
            self.budget.step()?;
            parts.extend(self.profile_at(profile, depth + 1)?);
        }
        self.check_total(&parts, kind)?;
        if parts.is_empty() {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        if parts.len() > 1 {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        Ok(parts)
    }

    /// The 2D operator of a derived profile (`...2D` or its non-uniform form).
    fn operator2(&self, name: u64) -> Step<Operator2> {
        let file = self.file;
        let record = file
            .instance(name)
            .and_then(|i| i.records.first())
            .ok_or_else(|| {
                skip(
                    reasons::TARGET_MISSING,
                    "IFCCARTESIANTRANSFORMATIONOPERATOR2D",
                )
            })?;
        let kind = record.type_name.as_str();
        let non_uniform = match kind {
            "IFCCARTESIANTRANSFORMATIONOPERATOR2D" => false,
            "IFCCARTESIANTRANSFORMATIONOPERATOR2DNONUNIFORM" => true,
            _ => return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind)),
        };
        let planar = |reference: Option<u64>, default: P2| -> Step<P2> {
            match reference {
                Some(direction) => {
                    let d = super::place::unit(self.direction(direction)?, kind)?;
                    Ok([d.x, d.y])
                }
                None => Ok(default),
            }
        };
        let u1 = planar(
            attr::opt_reference(named(file, record, "Axis1", 0), kind)?,
            [1.0, 0.0],
        )?;
        let mut u2 = [-u1[1], u1[0]];
        if let Some(given) = attr::opt_reference(named(file, record, "Axis2", 1), kind)? {
            let d = planar(Some(given), u2)?;
            if d[0] * u2[0] + d[1] * u2[1] < 0.0 {
                u2 = [-u2[0], -u2[1]];
            }
        }
        let origin = match attr::opt_reference(named(file, record, "LocalOrigin", 2), kind)? {
            Some(point) => {
                let p = self.point(point)?;
                [p.x, p.y]
            }
            None => [0.0, 0.0],
        };
        let scale = attr::opt_real(named(file, record, "Scale", 3), 1.0, kind)?.unwrap_or(1.0);
        let scale2 = if non_uniform {
            attr::opt_real(named(file, record, "Scale2", 4), 1.0, kind)?.unwrap_or(scale)
        } else {
            scale
        };
        if !(scale > 0.0 && scale2 > 0.0) {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        Ok(Operator2 {
            x: [u1[0] * scale, u1[1] * scale],
            y: [u2[0] * scale2, u2[1] * scale2],
            origin,
        })
    }

    fn derived(&mut self, _name: u64, record: &Record, depth: u32) -> Step<Parts> {
        let kind = record.type_name.as_str();
        let file = self.file;
        let parent = attr::reference(named(file, record, "ParentProfile", 2), kind)?;
        let operator = if kind == "IFCMIRROREDPROFILEDEF" {
            // The mirror of the schema: Axis1 = (-1, 0), Axis2 = (0, 1).
            Operator2 {
                x: [-1.0, 0.0],
                y: [0.0, 1.0],
                origin: [0.0, 0.0],
            }
        } else {
            self.operator2(attr::reference(named(file, record, "Operator", 3), kind)?)?
        };
        let mut parts = self.profile_at(parent, depth + 1)?;
        for part in &mut parts {
            for loop_ in part.iter_mut() {
                for p in loop_.iter_mut() {
                    *p = operator.apply(*p);
                }
            }
        }
        Ok(parts)
    }

    /// The strip around a curve of a center line profile.
    fn center_line(&mut self, record: &Record) -> Step<Parts> {
        let kind = "IFCCENTERLINEPROFILEDEF";
        let file = self.file;
        let curve = attr::reference(named(file, record, "Curve", 2), kind)?;
        let half = positive(named(file, record, "Thickness", 3), self.scale, kind)? / 2.0;
        let mut points = self.profile_curve(curve)?;
        let closed = points.len() > 2 && points.first() == points.last();
        if closed {
            points.pop();
        }
        if points.len() < 2 {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        }
        let left = offset_polyline(&points, half, closed);
        let right = offset_polyline(&points, -half, closed);
        let (Some(left), Some(right)) = (left, right) else {
            return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
        };
        if closed {
            Ok(vec![vec![left, right]])
        } else {
            let mut outline = left;
            outline.extend(right.into_iter().rev());
            Ok(vec![vec![outline]])
        }
    }

    /// Refuses parts of more points than the loop limit.
    fn check_total(&self, parts: &Parts, kind: &str) -> Step<()> {
        let total: usize = parts.iter().flatten().map(Vec::len).sum();
        if total > MAX_LOOP_VERTICES {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        Ok(())
    }

    /// A length attribute of a parameterized profile, positive.
    fn dimension(&self, record: &Record, name: &str, position: usize) -> Step<f64> {
        positive(
            named(self.file, record, name, position),
            self.scale,
            &record.type_name,
        )
    }

    /// An optional non-negative length attribute (a radius), zero when absent.
    fn radius(&self, record: &Record, name: &str, position: usize) -> Step<f64> {
        let value = attr::opt_real(
            named(self.file, record, name, position),
            self.scale,
            &record.type_name,
        )?
        .unwrap_or(0.0);
        if value < 0.0 {
            Err(skip(reasons::GEOMETRY_DEGENERATE, &record.type_name))
        } else {
            Ok(value)
        }
    }

    /// Reports a non-zero slope attribute, which is not applied.
    fn slope(&mut self, name: u64, record: &Record, attribute: &str, position: usize) -> Step<()> {
        let value = attr::opt_real(
            named(self.file, record, attribute, position),
            1.0,
            "IFCPROFILEDEF",
        )?
        .unwrap_or(0.0);
        if value != 0.0 {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                &record.type_name,
                Some(Source::Decimal(name)),
                1,
            );
        }
        Ok(())
    }

    /// The loops of a parameterized profile, in the plane of the solid.
    #[allow(clippy::too_many_lines, reason = "one arm per profile type")]
    fn parameterized(&mut self, name: u64, record: &Record) -> Step<Vec<Vec<P2>>> {
        let kind = record.type_name.as_str();
        let file = self.file;
        if !matches!(
            kind,
            "IFCRECTANGLEPROFILEDEF"
                | "IFCROUNDEDRECTANGLEPROFILEDEF"
                | "IFCRECTANGLEHOLLOWPROFILEDEF"
                | "IFCCIRCLEPROFILEDEF"
                | "IFCCIRCLEHOLLOWPROFILEDEF"
                | "IFCELLIPSEPROFILEDEF"
                | "IFCISHAPEPROFILEDEF"
                | "IFCLSHAPEPROFILEDEF"
                | "IFCTSHAPEPROFILEDEF"
                | "IFCUSHAPEPROFILEDEF"
                | "IFCCSHAPEPROFILEDEF"
                | "IFCZSHAPEPROFILEDEF"
                | "IFCASYMMETRICISHAPEPROFILEDEF"
                | "IFCTRAPEZIUMPROFILEDEF"
        ) {
            return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
        }
        let placement = self.axis2(attr::opt_reference(
            file.attr(record, known::PROFILE_POSITION),
            kind,
        )?)?;
        let mut loops: Vec<Vec<Corner>> = Vec::new();
        match kind {
            "IFCRECTANGLEPROFILEDEF" => {
                let (x, y) = (
                    self.dimension(record, "XDim", 3)? / 2.0,
                    self.dimension(record, "YDim", 4)? / 2.0,
                );
                loops.push(sharp(&[[-x, -y], [x, -y], [x, y], [-x, y]]));
            }
            "IFCROUNDEDRECTANGLEPROFILEDEF" => {
                let (x, y) = (
                    self.dimension(record, "XDim", 3)? / 2.0,
                    self.dimension(record, "YDim", 4)? / 2.0,
                );
                let c = self.radius(record, "RoundingRadius", 5)?;
                loops.push(rect(x, y, c));
            }
            "IFCRECTANGLEHOLLOWPROFILEDEF" => {
                let (x, y) = (
                    self.dimension(record, "XDim", 3)? / 2.0,
                    self.dimension(record, "YDim", 4)? / 2.0,
                );
                let t = self.dimension(record, "WallThickness", 5)?;
                if t >= x.min(y) {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                loops.push(rect(x, y, self.radius(record, "OuterFilletRadius", 7)?));
                loops.push(rect(
                    x - t,
                    y - t,
                    self.radius(record, "InnerFilletRadius", 6)?,
                ));
            }
            "IFCCIRCLEPROFILEDEF" => {
                let radius = self.dimension(record, "Radius", 3)?;
                return self.circle_loops(name, kind, &placement, radius, None);
            }
            "IFCCIRCLEHOLLOWPROFILEDEF" => {
                let radius = self.dimension(record, "Radius", 3)?;
                let t = self.dimension(record, "WallThickness", 4)?;
                if t >= radius {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                return self.circle_loops(name, kind, &placement, radius, Some(radius - t));
            }
            "IFCELLIPSEPROFILEDEF" => {
                let (a, b) = (
                    self.dimension(record, "SemiAxis1", 3)?,
                    self.dimension(record, "SemiAxis2", 4)?,
                );
                let (count, limited) = arc_segments(a.max(b), TAU, self.tol.tessellation_chord);
                if limited {
                    self.log.add(
                        reasons::GEOMETRY_APPROXIMATED,
                        kind,
                        Some(Source::Decimal(name)),
                        1,
                    );
                }
                let count = count.max(MIN_CIRCLE_SEGMENTS);
                self.budget.array(count)?;
                self.budget.charge(
                    u64::try_from(count)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(VERTEX_BYTES),
                )?;
                let mut loop_ = Vec::with_capacity(count);
                for k in 0..count {
                    let t = TAU * (k as f64) / (count as f64);
                    loop_.push(place(&placement, [a * t.cos(), b * t.sin()])?);
                }
                return Ok(vec![loop_]);
            }
            "IFCISHAPEPROFILEDEF" => {
                let (w, h) = (
                    self.dimension(record, "OverallWidth", 3)? / 2.0,
                    self.dimension(record, "OverallDepth", 4)? / 2.0,
                );
                let (tw, tf) = (
                    self.dimension(record, "WebThickness", 5)? / 2.0,
                    self.dimension(record, "FlangeThickness", 6)?,
                );
                if tw >= w || tf >= h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                self.slope(name, record, "FlangeSlope", 9)?;
                let (fr, er) = (
                    self.radius(record, "FilletRadius", 7)?,
                    self.radius(record, "FlangeEdgeRadius", 8)?,
                );
                loops.push(vec![
                    ([-w, -h], 0.0),
                    ([w, -h], 0.0),
                    ([w, -h + tf], er),
                    ([tw, -h + tf], fr),
                    ([tw, h - tf], fr),
                    ([w, h - tf], er),
                    ([w, h], 0.0),
                    ([-w, h], 0.0),
                    ([-w, h - tf], er),
                    ([-tw, h - tf], fr),
                    ([-tw, -h + tf], fr),
                    ([-w, -h + tf], er),
                ]);
            }
            "IFCASYMMETRICISHAPEPROFILEDEF" => {
                let bottom = if self.has_attribute(record, "BottomFlangeWidth") {
                    (
                        "BottomFlangeWidth",
                        "BottomFlangeThickness",
                        "BottomFlangeFilletRadius",
                    )
                } else {
                    ("OverallWidth", "FlangeThickness", "FilletRadius")
                };
                let (bw, bt) = (
                    self.dimension(record, bottom.0, 3)? / 2.0,
                    self.dimension(record, bottom.1, 6)?,
                );
                let h = self.dimension(record, "OverallDepth", 4)? / 2.0;
                let tw = self.dimension(record, "WebThickness", 5)? / 2.0;
                let (uw, ut) = (
                    self.dimension(record, "TopFlangeWidth", 8)? / 2.0,
                    self.dimension(record, "TopFlangeThickness", 9)?,
                );
                if tw >= bw || tw >= uw || bt + ut >= 2.0 * h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                let (bf, uf) = (
                    self.radius(record, bottom.2, 7)?,
                    self.radius(record, "TopFlangeFilletRadius", 10)?,
                );
                let (be, ue) = (
                    self.radius(record, "BottomFlangeEdgeRadius", 11)?,
                    self.radius(record, "TopFlangeEdgeRadius", 13)?,
                );
                self.slope(name, record, "BottomFlangeSlope", 12)?;
                self.slope(name, record, "TopFlangeSlope", 14)?;
                loops.push(vec![
                    ([-bw, -h], 0.0),
                    ([bw, -h], 0.0),
                    ([bw, -h + bt], be),
                    ([tw, -h + bt], bf),
                    ([tw, h - ut], uf),
                    ([uw, h - ut], ue),
                    ([uw, h], 0.0),
                    ([-uw, h], 0.0),
                    ([-uw, h - ut], ue),
                    ([-tw, h - ut], uf),
                    ([-tw, -h + bt], bf),
                    ([-bw, -h + bt], be),
                ]);
            }
            "IFCLSHAPEPROFILEDEF" => {
                let depth = self.dimension(record, "Depth", 3)?;
                let width = attr::opt_real(named(file, record, "Width", 4), self.scale, kind)?
                    .unwrap_or(depth);
                let t = self.dimension(record, "Thickness", 5)?;
                if !(width > t && depth > t) {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                self.slope(name, record, "LegSlope", 8)?;
                let (w, h) = (width / 2.0, depth / 2.0);
                let (fr, er) = (
                    self.radius(record, "FilletRadius", 6)?,
                    self.radius(record, "EdgeRadius", 7)?,
                );
                loops.push(vec![
                    ([-w, -h], 0.0),
                    ([w, -h], 0.0),
                    ([w, -h + t], er),
                    ([-w + t, -h + t], fr),
                    ([-w + t, h], er),
                    ([-w, h], 0.0),
                ]);
            }
            "IFCTSHAPEPROFILEDEF" => {
                let h = self.dimension(record, "Depth", 3)? / 2.0;
                let w = self.dimension(record, "FlangeWidth", 4)? / 2.0;
                let (tw, tf) = (
                    self.dimension(record, "WebThickness", 5)? / 2.0,
                    self.dimension(record, "FlangeThickness", 6)?,
                );
                if tw >= w || tf >= 2.0 * h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                self.slope(name, record, "FlangeSlope", 11)?;
                self.slope(name, record, "WebSlope", 10)?;
                let (fr, fe, we) = (
                    self.radius(record, "FilletRadius", 7)?,
                    self.radius(record, "FlangeEdgeRadius", 8)?,
                    self.radius(record, "WebEdgeRadius", 9)?,
                );
                loops.push(vec![
                    ([-tw, -h], we),
                    ([tw, -h], we),
                    ([tw, h - tf], fr),
                    ([w, h - tf], fe),
                    ([w, h], 0.0),
                    ([-w, h], 0.0),
                    ([-w, h - tf], fe),
                    ([-tw, h - tf], fr),
                ]);
            }
            "IFCUSHAPEPROFILEDEF" => {
                let h = self.dimension(record, "Depth", 3)? / 2.0;
                let w = self.dimension(record, "FlangeWidth", 4)? / 2.0;
                let (tw, tf) = (
                    self.dimension(record, "WebThickness", 5)?,
                    self.dimension(record, "FlangeThickness", 6)?,
                );
                if tw >= 2.0 * w || tf >= h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                self.slope(name, record, "FlangeSlope", 9)?;
                let (fr, er) = (
                    self.radius(record, "FilletRadius", 7)?,
                    self.radius(record, "EdgeRadius", 8)?,
                );
                loops.push(vec![
                    ([-w, -h], 0.0),
                    ([w, -h], 0.0),
                    ([w, -h + tf], er),
                    ([-w + tw, -h + tf], fr),
                    ([-w + tw, h - tf], fr),
                    ([w, h - tf], er),
                    ([w, h], 0.0),
                    ([-w, h], 0.0),
                ]);
            }
            "IFCCSHAPEPROFILEDEF" => {
                let h = self.dimension(record, "Depth", 3)? / 2.0;
                let w = self.dimension(record, "Width", 4)? / 2.0;
                let (t, g) = (
                    self.dimension(record, "WallThickness", 5)?,
                    self.dimension(record, "Girth", 6)?,
                );
                if t >= w || t >= h || g <= t || g >= h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                let ir = self.radius(record, "InternalFilletRadius", 7)?;
                // The outer corners are concentric with the inner ones.
                let or = if ir > 0.0 { ir + t } else { 0.0 };
                loops.push(vec![
                    ([-w, -h], or),
                    ([w, -h], 0.0),
                    ([w, -h + g], 0.0),
                    ([w - t, -h + g], 0.0),
                    ([w - t, -h + t], ir),
                    ([-w + t, -h + t], ir),
                    ([-w + t, h - t], ir),
                    ([w - t, h - t], ir),
                    ([w - t, h - g], 0.0),
                    ([w, h - g], 0.0),
                    ([w, h], 0.0),
                    ([-w, h], or),
                ]);
            }
            "IFCZSHAPEPROFILEDEF" => {
                let h = self.dimension(record, "Depth", 3)? / 2.0;
                let fw = self.dimension(record, "FlangeWidth", 4)?;
                let (tw, tf) = (
                    self.dimension(record, "WebThickness", 5)?,
                    self.dimension(record, "FlangeThickness", 6)?,
                );
                if tw >= fw || tf >= h {
                    return Err(skip(reasons::GEOMETRY_DEGENERATE, kind));
                }
                let (fr, er) = (
                    self.radius(record, "FilletRadius", 7)?,
                    self.radius(record, "EdgeRadius", 8)?,
                );
                // The web is centered and each flange reaches `fw` from the center line of the web, so the
                // bounding box spans from -fw to fw.
                let (a, b) = (tw / 2.0, fw);
                loops.push(vec![
                    ([-a, -h], 0.0),
                    ([b, -h], 0.0),
                    ([b, -h + tf], er),
                    ([a, -h + tf], fr),
                    ([a, h], 0.0),
                    ([-b, h], 0.0),
                    ([-b, h - tf], er),
                    ([-a, h - tf], fr),
                ]);
            }
            _ => {
                // IFCTRAPEZIUMPROFILEDEF
                let (bottom, top) = (
                    self.dimension(record, "BottomXDim", 3)?,
                    self.dimension(record, "TopXDim", 4)?,
                );
                let h = self.dimension(record, "YDim", 5)? / 2.0;
                let offset = attr::real(named(file, record, "TopXOffset", 6), self.scale, kind)?;
                let left = -bottom / 2.0;
                loops.push(sharp(&[
                    [left, -h],
                    [-left, -h],
                    [left + offset + top, h],
                    [left + offset, h],
                ]));
            }
        }
        let mut out = Vec::with_capacity(loops.len());
        for corners in &loops {
            let polygon = self.fillet_polygon(name, kind, corners)?;
            let placed = polygon
                .iter()
                .map(|&p| place(&placement, p))
                .collect::<Step<Vec<P2>>>()?;
            out.push(placed);
        }
        let parts: Parts = vec![out.clone()];
        self.check_total(&parts, kind)?;
        Ok(out)
    }

    /// Whether the entity of `record` declares the attribute `name` (always true without tables, for
    /// the IFC4 layout).
    fn has_attribute(&self, record: &Record, name: &str) -> bool {
        match self.file.tables() {
            Some(tables) => tables
                .entity_index(&record.type_name)
                .is_some_and(|e| tables.attribute_position(e, name).is_some()),
            None => true,
        }
    }

    /// The outer circle of `radius` and, when `inner` is given, its hole, both placed.
    pub(crate) fn circle_loops(
        &mut self,
        name: u64,
        kind: &str,
        placement: &Transform3,
        radius: f64,
        inner: Option<f64>,
    ) -> Step<Vec<Vec<P2>>> {
        let (count, limited) = circle_segments(radius, self.tol.tessellation_chord);
        if limited {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        let loops = 1 + usize::from(inner.is_some());
        self.budget.array(count.saturating_mul(loops))?;
        self.budget.charge(
            u64::try_from(count.saturating_mul(loops))
                .unwrap_or(u64::MAX)
                .saturating_mul(VERTEX_BYTES),
        )?;
        let mut out = Vec::with_capacity(loops);
        for r in std::iter::once(radius).chain(inner) {
            let mut loop_ = Vec::with_capacity(count);
            for k in 0..count {
                let angle = TAU * (k as f64) / (count as f64);
                loop_.push(place(placement, [r * angle.cos(), r * angle.sin()])?);
            }
            out.push(loop_);
        }
        Ok(out)
    }

    /// The polygon of `corners` with each corner of a positive radius replaced by the tangent arc.
    /// A radius larger than the corner allows (half of the shorter adjacent edge) is reduced and the
    /// profile is reported as approximated.
    fn fillet_polygon(&mut self, name: u64, kind: &str, corners: &[Corner]) -> Step<Vec<P2>> {
        let n = corners.len();
        let mut out: Vec<P2> = Vec::with_capacity(n);
        let mut reduced = false;
        for (i, &(c, radius)) in corners.iter().enumerate() {
            let prev = corners.get((i + n - 1) % n).map_or(c, |k| k.0);
            let next = corners.get((i + 1) % n).map_or(c, |k| k.0);
            let to_prev = [prev[0] - c[0], prev[1] - c[1]];
            let to_next = [next[0] - c[0], next[1] - c[1]];
            let (lp, ln) = (norm(to_prev), norm(to_next));
            if radius <= 0.0 || lp <= 0.0 || ln <= 0.0 {
                out.push(c);
                continue;
            }
            let e1 = [to_prev[0] / lp, to_prev[1] / lp];
            let e2 = [to_next[0] / ln, to_next[1] / ln];
            let cos = (e1[0] * e2[0] + e1[1] * e2[1]).clamp(-1.0, 1.0);
            let theta = cos.acos();
            if !(1e-6..=PI - 1e-6).contains(&theta) {
                out.push(c);
                continue;
            }
            let tan_half = (theta / 2.0).tan();
            let mut distance = radius / tan_half;
            let limit = 0.5 * lp.min(ln);
            let mut r = radius;
            if distance > limit {
                distance = limit;
                r = distance * tan_half;
                reduced = true;
            }
            let t1 = [c[0] + e1[0] * distance, c[1] + e1[1] * distance];
            let t2 = [c[0] + e2[0] * distance, c[1] + e2[1] * distance];
            let bisector = [e1[0] + e2[0], e1[1] + e2[1]];
            let bl = norm(bisector);
            let to_center = r / (theta / 2.0).sin();
            let center = [
                c[0] + bisector[0] / bl * to_center,
                c[1] + bisector[1] / bl * to_center,
            ];
            let a1 = (t1[1] - center[1]).atan2(t1[0] - center[0]);
            let a2 = (t2[1] - center[1]).atan2(t2[0] - center[0]);
            let mut sweep = a2 - a1;
            while sweep > PI {
                sweep -= TAU;
            }
            while sweep < -PI {
                sweep += TAU;
            }
            let (count, limited) = arc_segments(r, sweep, self.tol.tessellation_chord);
            if limited {
                reduced = true;
            }
            self.budget.array(out.len().saturating_add(count + 1))?;
            if out.len().saturating_add(count + 1) > MAX_LOOP_VERTICES {
                return Err(skip(reasons::GEOMETRY_NOT_EVALUATED, kind));
            }
            self.budget.charge(
                u64::try_from(count + 1)
                    .unwrap_or(u64::MAX)
                    .saturating_mul(VERTEX_BYTES),
            )?;
            for k in 0..=count {
                let a = a1 + sweep * (k as f64) / (count as f64);
                out.push([center[0] + r * a.cos(), center[1] + r * a.sin()]);
            }
        }
        if reduced {
            self.log.add(
                reasons::GEOMETRY_APPROXIMATED,
                kind,
                Some(Source::Decimal(name)),
                1,
            );
        }
        Ok(out)
    }

    /// The outer curve of an arbitrary profile.
    fn profile_loop(&mut self, record: &Record) -> Step<Vec<P2>> {
        let curve = attr::reference(
            self.file.attr(record, known::PROFILE_OUTER_CURVE),
            &record.type_name,
        )?;
        self.profile_curve(curve)
    }

    /// A profile curve: its points in the plane, with every third coordinate zero. The closing
    /// repeat of the first point is kept as the curve gives it (the triangulation drops it).
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

fn norm(v: P2) -> f64 {
    v[0].hypot(v[1])
}

/// A rectangle of half sizes `x` and `y` with rounded corners of radius `r`.
fn rect(x: f64, y: f64, r: f64) -> Vec<Corner> {
    vec![([-x, -y], r), ([x, -y], r), ([x, y], r), ([-x, y], r)]
}

/// The polyline moved by `distance` to its left (counterclockwise normal), mitered at the interior
/// corners; `None` when two consecutive points coincide. A closed polyline wraps around.
fn offset_polyline(points: &[P2], distance: f64, closed: bool) -> Option<Vec<P2>> {
    let n = points.len();
    let segment = |i: usize| -> Option<P2> {
        let a = points.get(i)?;
        let b = points.get((i + 1) % n)?;
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = norm(d);
        if l > 0.0 {
            Some([d[0] / l, d[1] / l])
        } else {
            None
        }
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let previous = if i > 0 {
            segment(i - 1)
        } else if closed {
            segment(n - 1)
        } else {
            None
        };
        let next = if i + 1 < n || closed {
            segment(i)
        } else {
            None
        };
        let normal = |d: P2| [-d[1], d[0]];
        let p = *points.get(i)?;
        let shift = match (previous, next) {
            (Some(a), Some(b)) => {
                let (na, nb) = (normal(a), normal(b));
                let m = [na[0] + nb[0], na[1] + nb[1]];
                let denominator = 1.0 + na[0] * nb[0] + na[1] * nb[1];
                if denominator < 1e-6 {
                    return None;
                }
                [m[0] / denominator, m[1] / denominator]
            }
            (Some(d), None) | (None, Some(d)) => normal(d),
            (None, None) => return None,
        };
        out.push([p[0] + shift[0] * distance, p[1] + shift[1] * distance]);
    }
    Some(out)
}
