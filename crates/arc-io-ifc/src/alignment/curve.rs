//! Evaluation of placed curve segments and of the plan, gradient and cant of a curve.

use std::f64::consts::PI;

use arc_base::math::{Matrix4, Point3, Transform3, Vector3};
use arc_base::{Error, Result};

use super::parent::{Parent, Shape};
use crate::error::codes;

/// A curve segment: a parent curve trimmed from `s0` by `len` (negative reverses the sense), and
/// the placement of its start point and start tangent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seg {
    /// The entity type of the parent curve, or the layout segment type.
    pub(crate) kind: String,
    pub(crate) parent: Parent,
    pub(crate) s0: f64,
    pub(crate) len: f64,
    /// The location of the placement (x, y, z); z only for a 3D placement.
    pub(crate) loc: [f64; 3],
    /// The heading of the reference direction in the xy plane.
    pub(crate) phi: f64,
    /// The `Axis` of a 3D placement, `[0, 0, 1]` otherwise.
    pub(crate) axis: [f64; 3],
}

impl Seg {
    pub(crate) fn extent(&self) -> f64 {
        self.len.abs()
    }

    /// The point and heading at the distance `d` from the start, `0 <= d <= extent`, in the
    /// coordinates of the placement.
    pub(crate) fn local(&self, d: f64) -> ([f64; 2], f64) {
        let sigma = if self.len < 0.0 { -1.0 } else { 1.0 };
        let flip = if sigma < 0.0 { PI } else { 0.0 };
        let (_, theta0) = self.parent.delta(self.s0, self.s0);
        let (delta, theta) = self.parent.delta(self.s0, self.s0 + sigma * d);
        let alpha = self.phi - (theta0 + flip);
        let (sin, cos) = alpha.sin_cos();
        (
            [
                self.loc[0] + cos * delta[0] - sin * delta[1],
                self.loc[1] + sin * delta[0] + cos * delta[1],
            ],
            theta + flip + alpha,
        )
    }
}

fn out_of_range(distance: f64, length: f64) -> Error {
    Error::new(codes::IFC_DISTANCE_OUT_OF_RANGE)
        .with_detail("distance", distance)
        .with_detail("length", length)
}

/// The segments of a composite curve in the plane, with the distance at which each starts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    pub(crate) segs: Vec<Seg>,
    pub(crate) starts: Vec<f64>,
    pub(crate) total: f64,
}

impl Plan {
    pub(crate) fn new(segs: Vec<Seg>) -> Self {
        let mut starts = Vec::with_capacity(segs.len());
        let mut total = 0.0;
        for seg in &segs {
            starts.push(total);
            total += seg.extent();
        }
        Self {
            segs,
            starts,
            total,
        }
    }

    /// The point and heading at the distance `u`; `tol` is the slack allowed at the ends.
    pub(crate) fn eval(&self, u: f64, tol: f64) -> Result<([f64; 2], f64)> {
        if !(u >= -tol && u <= self.total + tol) {
            return Err(out_of_range(u, self.total));
        }
        let u = u.clamp(0.0, self.total);
        // The last segment that starts at or before `u` and has a length, else the last one.
        let after = self.starts.partition_point(|&s| s <= u);
        let mut i = after.saturating_sub(1);
        while i > 0 && self.segs.get(i).is_some_and(|s| s.extent() == 0.0) {
            i -= 1;
        }
        let seg = self
            .segs
            .get(i)
            .ok_or_else(|| Error::new(codes::IFC_CURVE_INVALID))?;
        let start = self.starts.get(i).copied().unwrap_or(0.0);
        Ok(seg.local((u - start).clamp(0.0, seg.extent())))
    }
}

/// One piece of a gradient: a segment in the plane (distance along, height), and the distances
/// along where it starts and ends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gradient {
    pub(crate) pieces: Vec<(f64, f64, Seg)>,
}

impl Gradient {
    /// The height and the slope `dz / du` at the distance along `u`.
    pub(crate) fn eval(&self, u: f64, tol: f64) -> Result<(f64, f64)> {
        let after = self.pieces.partition_point(|(x0, _, _)| *x0 <= u + tol);
        let Some((_, _, seg)) = after.checked_sub(1).and_then(|i| self.pieces.get(i)) else {
            return Err(out_of_range(u, 0.0));
        };
        let (mut lo, mut hi) = (0.0, seg.extent());
        let (end, _) = seg.local(hi);
        if end[0] <= u {
            lo = hi;
        } else {
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if seg.local(mid).0[0] < u {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        let (point, heading) = seg.local(0.5 * (lo + hi));
        let slope = heading.sin() / heading.cos().max(1e-12);
        // Past the end of the last piece the height continues along the end tangent.
        let past = if u > point[0] { u - point[0] } else { 0.0 };
        Ok((point[1] + slope * past, slope))
    }
}

/// How the cant of one piece is given.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CantKind {
    /// From an alignment layout: the centerline raise and the cross slope (right rail minus left
    /// rail) at the start and the end, changing with `shape`.
    Layout {
        shape: Shape,
        raise: [f64; 2],
        slope: [f64; 2],
        rail_head: f64,
    },
    /// From a segmented reference curve: the segment in (distance, deviating elevation), the
    /// tilt of its `Axis` at the start and at the end (the `Axis` of the next segment), and the
    /// deviating elevation at the end (the location of the next segment).
    Reference {
        seg: Seg,
        tilt: [f64; 2],
        dev_end: f64,
    },
}

/// One piece of a cant layout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CantPiece {
    pub(crate) x0: f64,
    pub(crate) x1: f64,
    pub(crate) kind: CantKind,
}

impl CantPiece {
    /// The deviating elevation and the tilt toward the left, at the distance along `u`.
    fn eval(&self, u: f64) -> (f64, f64) {
        let width = self.x1 - self.x0;
        let u = u.clamp(self.x0, self.x1);
        match &self.kind {
            CantKind::Layout {
                shape,
                raise,
                slope,
                rail_head,
            } => {
                let f = if width > 0.0 {
                    shape.value((u - self.x0) / width)
                } else {
                    1.0
                };
                let tilt = if *rail_head > 0.0 {
                    ((slope[0] + f * (slope[1] - slope[0])) / rail_head)
                        .clamp(-1.0, 1.0)
                        .asin()
                } else {
                    0.0
                };
                (raise[0] + f * (raise[1] - raise[0]), tilt)
            }
            CantKind::Reference { seg, tilt, dev_end } => {
                let len = seg.extent();
                let d = (u - self.x0).clamp(0.0, len);
                let linear = if len > 0.0 { d / len } else { 1.0 };
                let (dev, f) = match &seg.parent {
                    // A spiral gives the shape of the change: the curvature function, from its
                    // value at the start to its value at the end. Deviation and tilt change from
                    // their start values to their end values with that fraction.
                    Parent::Spiral(spiral) => {
                        let k = |s: f64| spiral.curvature(seg.s0 + seg.len.signum() * s);
                        let range = k(len) - k(0.0);
                        let f = if range.abs() > 1e-15 * (1.0 + k(0.0).abs()) {
                            (k(d) - k(0.0)) / range
                        } else {
                            linear
                        };
                        (seg.loc[1] + f * (dev_end - seg.loc[1]), f)
                    }
                    // A line or polynomial gives the deviation as its own height.
                    _ => {
                        let here = seg.local(d).0[1];
                        let (d0, d1) = (seg.local(0.0).0[1], seg.local(len).0[1]);
                        let f = if (d1 - d0).abs() > 1e-9 {
                            (here - d0) / (d1 - d0)
                        } else {
                            linear
                        };
                        (here, f)
                    }
                };
                (dev, tilt[0] + f * (tilt[1] - tilt[0]))
            }
        }
    }
}

/// The cant of a curve: pieces in order of distance along.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cant {
    pub(crate) pieces: Vec<CantPiece>,
}

impl Cant {
    /// The deviating elevation, its derivative along the distance and the tilt at `u`. The
    /// derivative is taken inside the piece that holds `u`, so a jump between pieces does not
    /// show in it. Before the first piece there is no cant, after the last the last value stays.
    fn eval(&self, u: f64, step: f64) -> (f64, f64, f64) {
        let after = self.pieces.partition_point(|p| p.x0 <= u);
        let Some(piece) = after.checked_sub(1).and_then(|i| self.pieces.get(i)) else {
            return (0.0, 0.0, 0.0);
        };
        let (dev, tilt) = piece.eval(u);
        let (lo, hi) = ((u - step).max(piece.x0), (u + step).min(piece.x1));
        let rate = if hi > lo {
            (piece.eval(hi).0 - piece.eval(lo).0) / (hi - lo)
        } else {
            0.0
        };
        (dev, rate, tilt)
    }
}

/// The segment types in effect at a distance along a curve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentKinds {
    /// The plan segment.
    pub plan: Option<String>,
    /// The gradient segment, for a curve with heights.
    pub gradient: Option<String>,
    /// The cant segment, for a curve with cant.
    pub cant: Option<String>,
}

/// A position and orientation on a curve, in meters.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    /// The point.
    pub position: Point3,
    /// The unit tangent in the direction of increasing distance.
    pub tangent: Vector3,
    /// The frame: X along the tangent, Y to the left, Z up (tilted by the cant), origin at the
    /// point.
    pub transform: Transform3,
}

/// The plan, the gradient and the cant of a curve, in the length unit of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub(crate) plan: Plan,
    pub(crate) gradient: Option<Gradient>,
    pub(crate) cant: Option<Cant>,
    /// Meters per length unit.
    pub(crate) scale: f64,
}

impl Curve {
    /// The length of the curve in meters: the length of its plan, which is the range of the
    /// distance along for a curve with a gradient or cant.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.plan.total * self.scale
    }

    /// Whether the curve has a gradient (heights come from the vertical layout).
    #[must_use]
    pub fn has_gradient(&self) -> bool {
        self.gradient.is_some()
    }

    /// Whether the curve has a cant.
    #[must_use]
    pub fn has_cant(&self) -> bool {
        self.cant.is_some()
    }

    /// The kinds of the segments in effect at `distance` meters: the plan segment, the gradient
    /// segment and the cant segment, each by the entity type of its parent curve (representation)
    /// or its layout segment type (layout).
    #[must_use]
    pub fn segment_kinds(&self, distance: f64) -> SegmentKinds {
        let u = (distance / self.scale).clamp(0.0, self.plan.total);
        let after = self.plan.starts.partition_point(|&s| s <= u);
        let mut i = after.saturating_sub(1);
        while i > 0 && self.plan.segs.get(i).is_some_and(|s| s.extent() == 0.0) {
            i -= 1;
        }
        SegmentKinds {
            plan: self.plan.segs.get(i).map(|s| s.kind.clone()),
            gradient: self.gradient.as_ref().and_then(|g| {
                let k = g.pieces.partition_point(|(x0, _, _)| *x0 <= u);
                k.checked_sub(1)
                    .and_then(|k| g.pieces.get(k))
                    .map(|(_, _, s)| s.kind.clone())
            }),
            cant: self.cant.as_ref().and_then(|c| {
                let k = c.pieces.partition_point(|p| p.x0 <= u);
                k.checked_sub(1)
                    .and_then(|k| c.pieces.get(k))
                    .map(|p| match &p.kind {
                        CantKind::Layout { shape, .. } => format!("{shape:?}"),
                        CantKind::Reference { seg, .. } => seg.kind.clone(),
                    })
            }),
        }
    }

    /// The position, tangent and frame at `distance` meters along the curve. A distance outside
    /// `[0, length]` by no more than `tolerance` meters is clamped to the end.
    ///
    /// # Errors
    ///
    /// `io.ifc.distance_out_of_range` for a distance beyond the tolerance, or one that is not a
    /// number.
    pub fn evaluate(&self, distance: f64, tolerance: f64) -> Result<Pose> {
        self.evaluate_native(distance / self.scale, tolerance / self.scale)
    }

    /// As [`Self::evaluate`], the distance and tolerance in the length unit of the file.
    pub(crate) fn evaluate_native(&self, u: f64, tol: f64) -> Result<Pose> {
        let (xy, heading) = self.plan.eval(u, tol)?;
        let (z, slope) = match &self.gradient {
            Some(g) => g.eval(u.clamp(0.0, self.plan.total), tol)?,
            None => (0.0, 0.0),
        };
        let (sin, cos) = heading.sin_cos();
        let mut tangent = Vector3::new(cos, sin, slope);
        let mut point = Vector3::new(xy[0], xy[1], z);
        let mut tilt = 0.0;
        if let Some(cant) = &self.cant {
            let step = 1e-4 * self.plan.total.max(1.0);
            let (dev, rate, t) = cant.eval(u.clamp(0.0, self.plan.total), step);
            // The deviation is an elevation: it adds to the height, as the gradient does.
            point.z += dev;
            tangent.z += rate;
            tilt = t;
        }
        let tangent = tangent.normalize();
        let (left, up) = frame(&tangent);
        let z_axis = left * tilt.sin() + up * tilt.cos();
        let y_axis = z_axis.cross(&tangent);
        let origin = Point3::from(point * self.scale);
        let m = Matrix4::new(
            tangent.x, y_axis.x, z_axis.x, origin.x, //
            tangent.y, y_axis.y, z_axis.y, origin.y, //
            tangent.z, y_axis.z, z_axis.z, origin.z, //
            0.0, 0.0, 0.0, 1.0,
        );
        Ok(Pose {
            position: origin,
            tangent,
            transform: Transform3::from_matrix_unchecked(m),
        })
    }
}

/// The left and up directions of the frame of a unit tangent: left is horizontal, up is in the
/// vertical plane of the tangent.
fn frame(tangent: &Vector3) -> (Vector3, Vector3) {
    let horizontal = tangent.x.hypot(tangent.y);
    let left = if horizontal > 1e-12 {
        Vector3::new(-tangent.y / horizontal, tangent.x / horizontal, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    (left, tangent.cross(&left))
}
