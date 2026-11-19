//! Constrained cursor input: ortho, polar, snap step and grid points, tracking through acquired
//! points, the helper points "mid between" and "from", and the precedence of the constraints.
//!
//! Every function is pure. Angles are radians, measured counterclockwise from the x axis of the
//! [`Frame`] the caller passes. Non-finite points are refused with `geom.input.not_finite`.

use std::f64::consts::{PI, TAU};

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Result, Tolerance};
use arc_geom::Aabb2;
use arc_geom::error::codes as geom_codes;

use super::limit_error;
use crate::error::codes;

/// The most additional polar angles of one [`PolarSettings`].
pub const MAX_POLAR_ANGLES: usize = 32;
/// The most acquired points tracked at once.
pub const MAX_ACQUIRED: usize = 16;

fn is_finite(point: &Point2) -> bool {
    point.coords.iter().all(|c| c.is_finite())
}

fn check_point(point: &Point2) -> Result<()> {
    if is_finite(point) {
        Ok(())
    } else {
        Err(Error::new(geom_codes::INPUT_NOT_FINITE))
    }
}

fn invalid(field: &'static str) -> Error {
    Error::new(codes::GEOMETRY_INVALID).with_detail("field", field)
}

/// `angle` in `[0, TAU)`.
fn normalize_angle(angle: f64) -> f64 {
    let turned = angle.rem_euclid(TAU);
    if turned >= TAU { 0.0 } else { turned }
}

/// The signed difference `a - b` in `(-PI, PI]`.
fn angle_difference(a: f64, b: f64) -> f64 {
    let turned = (a - b).rem_euclid(TAU);
    if turned > PI { turned - TAU } else { turned }
}

fn unit(angle: f64) -> Vector2 {
    Vector2::new(angle.cos(), angle.sin())
}

/// The x axis of a coordinate frame: the current UCS, or the direction of the last segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    x: Vector2,
}

impl Frame {
    /// The frame whose x axis points along `x_axis`. The vector is normalized.
    ///
    /// # Errors
    ///
    /// `geom.input.not_finite` for a non-finite component; `draft.geometry.invalid` for the zero
    /// vector.
    pub fn new(x_axis: Vector2) -> Result<Self> {
        if !x_axis.iter().all(|c| c.is_finite()) {
            return Err(Error::new(geom_codes::INPUT_NOT_FINITE));
        }
        let length = x_axis.norm();
        if length == 0.0 {
            return Err(invalid("x_axis"));
        }
        Ok(Self { x: x_axis / length })
    }

    /// The unit x axis.
    #[must_use]
    pub fn x_axis(&self) -> Vector2 {
        self.x
    }

    /// The unit y axis: the x axis turned a quarter turn counterclockwise.
    #[must_use]
    pub fn y_axis(&self) -> Vector2 {
        Vector2::new(-self.x.y, self.x.x)
    }

    /// The angle of the x axis from the world x axis.
    #[must_use]
    pub fn angle(&self) -> f64 {
        self.x.y.atan2(self.x.x)
    }
}

/// Restricts the cursor to the axes of `frame` from `base`: the axis nearer to the cursor
/// direction wins, and the point is the projection of the cursor on it. Ties go to the x axis.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite point.
pub fn ortho(base: Point2, cursor: Point2, frame: &Frame) -> Result<Point2> {
    check_point(&base)?;
    check_point(&cursor)?;
    let delta = cursor - base;
    let (x, y) = (frame.x_axis(), frame.y_axis());
    let (along_x, along_y) = (delta.dot(&x), delta.dot(&y));
    Ok(if along_x.abs() >= along_y.abs() {
        base + x * along_x
    } else {
        base + y * along_y
    })
}

/// The polar alignment settings: the increment, the additional angles (relative to the frame,
/// radians) and the angular tolerance within which the cursor locks.
#[derive(Debug, Clone, PartialEq)]
pub struct PolarSettings {
    /// The increment of the alignment angles, in `(0, TAU]`.
    pub increment: f64,
    /// Further alignment angles, at most [`MAX_POLAR_ANGLES`].
    pub additional: Vec<f64>,
    /// How far the cursor direction may be from an alignment angle to lock to it, non-negative.
    pub tolerance: f64,
}

impl PolarSettings {
    /// Validated settings.
    ///
    /// # Errors
    ///
    /// `draft.geometry.invalid` for an increment outside `(0, TAU]`, a non-finite additional
    /// angle or a negative or non-finite tolerance; `draft.snap.limit_exceeded` for more than
    /// [`MAX_POLAR_ANGLES`] additional angles.
    pub fn new(increment: f64, additional: Vec<f64>, tolerance: f64) -> Result<Self> {
        if !(increment.is_finite() && increment > 0.0 && increment <= TAU) {
            return Err(invalid("increment"));
        }
        if !(tolerance.is_finite() && tolerance >= 0.0) {
            return Err(invalid("tolerance"));
        }
        if additional.iter().any(|a| !a.is_finite()) {
            return Err(invalid("additional"));
        }
        if additional.len() > MAX_POLAR_ANGLES {
            return Err(limit_error(
                u64::try_from(MAX_POLAR_ANGLES).unwrap_or(u64::MAX),
            ));
        }
        Ok(Self {
            increment,
            additional,
            tolerance,
        })
    }

    /// The alignment angles nearest to `angle` (in `[0, TAU)`): the multiples of the increment
    /// on either side of it, and the additional angles, each in `[0, TAU)`.
    fn around(&self, angle: f64) -> Vec<f64> {
        let below = (angle / self.increment).floor();
        let mut angles = vec![
            normalize_angle(below * self.increment),
            normalize_angle((below + 1.0) * self.increment),
        ];
        angles.extend(self.additional.iter().map(|a| normalize_angle(*a)));
        angles.sort_by(f64::total_cmp);
        angles
    }

    /// The alignment angle nearest to `angle` and its distance; the smaller angle wins a tie.
    fn nearest(&self, angle: f64) -> (f64, f64) {
        let mut best = (0.0, f64::INFINITY);
        for candidate in self.around(angle) {
            let distance = angle_difference(angle, candidate).abs();
            if distance < best.1 {
                best = (candidate, distance);
            }
        }
        best
    }
}

/// A cursor locked to a polar alignment path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Polar {
    /// The point on the path, at the distance of the cursor's projection on it.
    pub point: Point2,
    /// The alignment angle, relative to the frame, in `[0, TAU)`.
    pub angle: f64,
}

/// Locks the cursor to the nearest polar alignment path from `base`, when its direction is within
/// the angular tolerance. A cursor at `base` locks to nothing.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite point.
pub fn polar(
    base: Point2,
    cursor: Point2,
    frame: &Frame,
    settings: &PolarSettings,
) -> Result<Option<Polar>> {
    check_point(&base)?;
    check_point(&cursor)?;
    let delta = cursor - base;
    if delta.norm() == 0.0 {
        return Ok(None);
    }
    let direction = normalize_angle(delta.y.atan2(delta.x) - frame.angle());
    let (angle, distance) = settings.nearest(direction);
    if distance > settings.tolerance {
        return Ok(None);
    }
    let path = unit(frame.angle() + angle);
    Ok(Some(Polar {
        point: base + path * delta.dot(&path),
        angle,
    }))
}

/// The snap step: a rectangular or polar grid of increments.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapStep {
    /// Points at multiples of `spacing` along the axes of a frame turned by `rotation` from
    /// `base`. Both spacings are positive.
    Rectangular {
        /// The origin of the grid.
        base: Point2,
        /// The spacing along the x and y axes of the grid.
        spacing: [f64; 2],
        /// The rotation of the grid, radians.
        rotation: f64,
    },
    /// Points at multiples of `distance` from `base`, in directions at multiples of `angle` from
    /// the rotated x axis. Both are positive.
    Polar {
        /// The origin of the grid.
        base: Point2,
        /// The radial spacing.
        distance: f64,
        /// The angular increment, radians.
        angle: f64,
        /// The rotation of the reference direction, radians.
        rotation: f64,
    },
}

/// The point of `step` nearest to `cursor`.
///
/// # Errors
///
/// `draft.geometry.invalid` for a spacing or increment that is not finite and positive;
/// `geom.input.not_finite` for a non-finite point, rotation or result.
pub fn snap_step(cursor: Point2, step: &SnapStep) -> Result<Point2> {
    check_point(&cursor)?;
    let point = match *step {
        SnapStep::Rectangular {
            base,
            spacing: [sx, sy],
            rotation,
        } => {
            check_point(&base)?;
            if !(sx.is_finite() && sx > 0.0 && sy.is_finite() && sy > 0.0) {
                return Err(invalid("spacing"));
            }
            let frame = Frame::new(unit(rotation))?;
            let (x, y) = (frame.x_axis(), frame.y_axis());
            let delta = cursor - base;
            let along_x = (delta.dot(&x) / sx).round() * sx;
            let along_y = (delta.dot(&y) / sy).round() * sy;
            base + x * along_x + y * along_y
        }
        SnapStep::Polar {
            base,
            distance,
            angle,
            rotation,
        } => {
            check_point(&base)?;
            if !(distance.is_finite() && distance > 0.0 && angle.is_finite() && angle > 0.0) {
                return Err(invalid("increment"));
            }
            let delta = cursor - base;
            let radius = (delta.norm() / distance).round() * distance;
            let theta = normalize_angle(delta.y.atan2(delta.x) - rotation);
            let snapped = (theta / angle).round() * angle;
            base + unit(rotation + snapped) * radius
        }
    };
    check_point(&point)?;
    Ok(point)
}

/// A rectangular grid of points, for display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    /// The point the grid is anchored at.
    pub origin: Point2,
    /// The spacing along the grid's x and y axes, both positive.
    pub spacing: [f64; 2],
    /// The rotation of the grid, radians.
    pub rotation: f64,
}

/// The grid points inside `window`, row by row along the grid axes.
///
/// # Errors
///
/// `draft.geometry.invalid` for a spacing that is not positive; `geom.input.not_finite` for a
/// non-finite origin, rotation or window; `draft.snap.limit_exceeded` when more than
/// `max_points` grid points lie in the window's bounding range.
pub fn grid_points(window: &Aabb2, grid: &Grid, max_points: usize) -> Result<Vec<Point2>> {
    check_point(&grid.origin)?;
    check_point(&window.min())?;
    check_point(&window.max())?;
    if !grid.rotation.is_finite() {
        return Err(Error::new(geom_codes::INPUT_NOT_FINITE));
    }
    let [sx, sy] = grid.spacing;
    if !(sx.is_finite() && sx > 0.0 && sy.is_finite() && sy > 0.0) {
        return Err(invalid("spacing"));
    }
    if window.is_empty() {
        return Ok(Vec::new());
    }
    let frame = Frame::new(unit(grid.rotation))?;
    let (x, y) = (frame.x_axis(), frame.y_axis());
    let (min, max) = (window.min(), window.max());
    let corners = [
        min,
        Point2::new(max.x, min.y),
        max,
        Point2::new(min.x, max.y),
    ];
    let mut lo = (f64::INFINITY, f64::INFINITY);
    let mut hi = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for corner in corners {
        let delta = corner - grid.origin;
        let (u, v) = (delta.dot(&x), delta.dot(&y));
        lo = (lo.0.min(u), lo.1.min(v));
        hi = (hi.0.max(u), hi.1.max(v));
    }
    let first = ((lo.0 / sx).ceil(), (lo.1 / sy).ceil());
    let last = ((hi.0 / sx).floor(), (hi.1 / sy).floor());
    if last.0 < first.0 || last.1 < first.1 {
        return Ok(Vec::new());
    }
    let columns = last.0 - first.0 + 1.0;
    let rows = last.1 - first.1 + 1.0;
    if !(columns * rows).is_finite() || columns * rows > max_points as f64 {
        return Err(limit_error(u64::try_from(max_points).unwrap_or(u64::MAX)));
    }
    let mut points = Vec::new();
    let mut j = first.1;
    while j <= last.1 {
        let mut i = first.0;
        while i <= last.0 {
            let point = grid.origin + x * (i * sx) + y * (j * sy);
            if window.contains_point(&point) {
                points.push(point);
            }
            i += 1.0;
        }
        j += 1.0;
    }
    Ok(points)
}

/// The alignment direction of a tracking path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackPathKind {
    /// Along the x axis of the frame.
    Horizontal,
    /// Along the y axis of the frame.
    Vertical,
    /// Along a polar alignment angle, relative to the frame.
    Polar {
        /// The alignment angle, radians.
        angle: f64,
    },
}

/// An alignment path through an acquired point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPath {
    /// The index of the acquired point the path goes through.
    pub acquired: usize,
    /// The acquired point.
    pub origin: Point2,
    /// The unit direction of the path.
    pub direction: Vector2,
    /// What kind of path it is.
    pub kind: TrackPathKind,
}

/// The point a tracking query offers: the intersection of two paths, or the projection of the
/// cursor on one path.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackingHit {
    /// The offered point.
    pub point: Point2,
    /// The paths that define it: two for an intersection, one for a projection.
    pub paths: Vec<TrackPath>,
}

/// Tracking: the alignment paths through the acquired points that pass within `aperture` of the
/// cursor, and the point the cursor is offered. Paths are horizontal and vertical from each
/// point and, when `polar` is given, the polar angles nearest to the direction from the point to
/// the cursor. An intersection of two paths of different points within the aperture wins over a
/// projection on one path; among intersections, the nearest to the cursor wins, and among
/// projections the nearest. Ties go to the earlier path.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite point; `draft.snap.limit_exceeded` for more than
/// [`MAX_ACQUIRED`] acquired points.
pub fn track(
    cursor: Point2,
    acquired: &[Point2],
    frame: &Frame,
    polar: Option<&PolarSettings>,
    aperture: f64,
    tolerance: &Tolerance,
) -> Result<Option<TrackingHit>> {
    check_point(&cursor)?;
    if acquired.len() > MAX_ACQUIRED {
        return Err(limit_error(u64::try_from(MAX_ACQUIRED).unwrap_or(u64::MAX)));
    }
    if !(aperture.is_finite() && aperture > 0.0) {
        return Ok(None);
    }
    let (x, y) = (frame.x_axis(), frame.y_axis());
    let mut paths = Vec::new();
    for (index, origin) in acquired.iter().enumerate() {
        check_point(origin)?;
        let mut add = |direction: Vector2, kind: TrackPathKind| {
            paths.push(TrackPath {
                acquired: index,
                origin: *origin,
                direction,
                kind,
            });
        };
        add(x, TrackPathKind::Horizontal);
        add(y, TrackPathKind::Vertical);
        let delta = cursor - *origin;
        if let (Some(settings), true) = (polar, delta.norm() > 0.0) {
            let direction = normalize_angle(delta.y.atan2(delta.x) - frame.angle());
            for angle in settings.around(direction) {
                add(unit(frame.angle() + angle), TrackPathKind::Polar { angle });
            }
        }
    }
    // A path is near when the cursor is within the aperture of its line.
    let near: Vec<&TrackPath> = paths
        .iter()
        .filter(|path| (cursor - path.origin).dot(&perp(path.direction)).abs() <= aperture)
        .collect();
    let mut best: Option<(f64, TrackingHit)> = None;
    for (i, a) in near.iter().enumerate() {
        for b in &near[i + 1..] {
            if a.acquired == b.acquired {
                continue;
            }
            let cross = a.direction.x * b.direction.y - a.direction.y * b.direction.x;
            if cross.abs() <= tolerance.angular {
                continue;
            }
            let w = b.origin - a.origin;
            let s = (w.x * b.direction.y - w.y * b.direction.x) / cross;
            let point = a.origin + a.direction * s;
            if !is_finite(&point) {
                continue;
            }
            let distance = (point - cursor).norm();
            if distance <= aperture && best.as_ref().is_none_or(|(d, _)| distance < *d) {
                best = Some((
                    distance,
                    TrackingHit {
                        point,
                        paths: vec![**a, **b],
                    },
                ));
            }
        }
    }
    if let Some((_, hit)) = best {
        return Ok(Some(hit));
    }
    for path in near {
        let point = path.origin + path.direction * (cursor - path.origin).dot(&path.direction);
        let distance = (point - cursor).norm();
        if distance <= aperture && best.as_ref().is_none_or(|(d, _)| distance < *d) {
            best = Some((
                distance,
                TrackingHit {
                    point,
                    paths: vec![*path],
                },
            ));
        }
    }
    Ok(best.map(|(_, hit)| hit))
}

/// The unit direction turned a quarter turn counterclockwise.
fn perp(direction: Vector2) -> Vector2 {
    Vector2::new(-direction.y, direction.x)
}

/// What decided a constrained point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConstraintSource {
    /// No constraint applied: the cursor itself.
    Raw,
    /// An object snap.
    ObjectSnap,
    /// A tracking point.
    Tracking,
    /// Ortho.
    Ortho,
    /// A polar alignment path at `angle`.
    Polar {
        /// The alignment angle relative to the frame, radians.
        angle: f64,
    },
    /// The snap step.
    Step,
}

/// The inputs of [`constrain`] for one pick.
#[derive(Debug, Clone)]
pub struct ConstraintContext<'a> {
    /// The frame of ortho and polar.
    pub frame: Frame,
    /// The previous point of the running command; ortho and polar need it.
    pub base: Option<Point2>,
    /// The point of the object snap under the cursor, if any (from [`super::snap_candidates`]).
    pub snapped: Option<Point2>,
    /// The point of tracking, if any (from [`track`]).
    pub tracked: Option<Point2>,
    /// Whether ortho is on for this pick (the host resolves an inverting key).
    pub ortho: bool,
    /// The polar settings when polar tracking is on.
    pub polar: Option<&'a PolarSettings>,
    /// The snap step when cursor snap is on.
    pub step: Option<SnapStep>,
}

/// A constrained point and what decided it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Constrained {
    /// The point.
    pub point: Point2,
    /// What decided it.
    pub source: ConstraintSource,
}

/// The constrained point for a cursor position. The precedence is: an object snap, then a
/// tracking point, then ortho, and only when ortho is off the polar alignment, then the snap
/// step, and otherwise the cursor. Ortho and polar apply only with a previous point.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite cursor, snapped or tracked point, and the errors of
/// the constraint that applies.
pub fn constrain(cursor: Point2, context: &ConstraintContext<'_>) -> Result<Constrained> {
    check_point(&cursor)?;
    if let Some(point) = context.snapped {
        check_point(&point)?;
        return Ok(Constrained {
            point,
            source: ConstraintSource::ObjectSnap,
        });
    }
    if let Some(point) = context.tracked {
        check_point(&point)?;
        return Ok(Constrained {
            point,
            source: ConstraintSource::Tracking,
        });
    }
    if let Some(base) = context.base {
        if context.ortho {
            return Ok(Constrained {
                point: ortho(base, cursor, &context.frame)?,
                source: ConstraintSource::Ortho,
            });
        }
        let locked = match context.polar {
            Some(settings) => polar(base, cursor, &context.frame, settings)?,
            None => None,
        };
        if let Some(hit) = locked {
            return Ok(Constrained {
                point: hit.point,
                source: ConstraintSource::Polar { angle: hit.angle },
            });
        }
    }
    if let Some(step) = &context.step {
        return Ok(Constrained {
            point: snap_step(cursor, step)?,
            source: ConstraintSource::Step,
        });
    }
    Ok(Constrained {
        point: cursor,
        source: ConstraintSource::Raw,
    })
}

/// The point halfway between two points.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite point or a result that overflows.
pub fn mid_between(a: Point2, b: Point2) -> Result<Point2> {
    check_point(&a)?;
    check_point(&b)?;
    let middle = Point2::from(a.coords * 0.5 + b.coords * 0.5);
    check_point(&middle)?;
    Ok(middle)
}

/// The point `offset` from `base`: the base of a "from" pick plus a relative offset.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite base, offset or result.
pub fn from_offset(base: Point2, offset: Vector2) -> Result<Point2> {
    check_point(&base)?;
    if !offset.iter().all(|c| c.is_finite()) {
        return Err(Error::new(geom_codes::INPUT_NOT_FINITE));
    }
    let point = base + offset;
    check_point(&point)?;
    Ok(point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn ortho_picks_the_nearer_axis_and_ties_go_to_x() {
        let frame = Frame::new(Vector2::x()).unwrap();
        let base = p(1.0, 1.0);
        assert_eq!(ortho(base, p(5.0, 2.0), &frame).unwrap(), p(5.0, 1.0));
        assert_eq!(ortho(base, p(2.0, 5.0), &frame).unwrap(), p(1.0, 5.0));
        assert_eq!(ortho(base, p(3.0, 3.0), &frame).unwrap(), p(3.0, 1.0));
    }

    #[test]
    fn polar_locks_only_within_the_tolerance_and_wraps_at_the_turn() {
        let frame = Frame::new(Vector2::x()).unwrap();
        let settings = PolarSettings::new(FRAC_PI_2, vec![], 0.01).unwrap();
        let base = p(0.0, 0.0);
        let hit = polar(base, p(4.0, 0.001), &frame, &settings)
            .unwrap()
            .unwrap();
        assert_eq!(hit.angle, 0.0);
        assert!((hit.point.x - 4.0).abs() < 1e-12 && hit.point.y == 0.0);
        // Just below a full turn is nearest to the zero angle.
        let below = polar(base, p(4.0, -0.001), &frame, &settings)
            .unwrap()
            .unwrap();
        assert_eq!(below.angle, 0.0);
        assert!(
            polar(base, p(4.0, 2.0), &frame, &settings)
                .unwrap()
                .is_none()
        );
        assert!(polar(base, base, &frame, &settings).unwrap().is_none());
    }

    #[test]
    fn polar_uses_additional_angles_and_the_frame() {
        let frame = Frame::new(Vector2::new(0.0, 1.0)).unwrap();
        let settings = PolarSettings::new(FRAC_PI_2, vec![PI / 4.0], 1e-6).unwrap();
        // The additional angle is relative to the frame: world 135 degrees is 45 degrees here.
        let hit = polar(p(0.0, 0.0), p(-1.0, 1.0), &frame, &settings)
            .unwrap()
            .unwrap();
        assert!((hit.angle - PI / 4.0).abs() < 1e-12);
    }

    #[test]
    fn polar_settings_refuse_bad_values() {
        assert!(PolarSettings::new(0.0, vec![], 0.0).is_err());
        assert!(PolarSettings::new(f64::NAN, vec![], 0.0).is_err());
        assert!(PolarSettings::new(1.0, vec![f64::INFINITY], 0.0).is_err());
        assert!(PolarSettings::new(1.0, vec![0.0; MAX_POLAR_ANGLES + 1], 0.0).is_err());
    }

    #[test]
    fn snap_step_rounds_in_the_grid_frame() {
        let step = SnapStep::Rectangular {
            base: p(0.0, 0.0),
            spacing: [1.0, 2.0],
            rotation: 0.0,
        };
        assert_eq!(snap_step(p(1.4, 2.9), &step).unwrap(), p(1.0, 2.0));
        assert_eq!(snap_step(p(-0.6, -1.1), &step).unwrap(), p(-1.0, -2.0));
        let polar_step = SnapStep::Polar {
            base: p(0.0, 0.0),
            distance: 1.0,
            angle: FRAC_PI_2,
            rotation: 0.0,
        };
        let snapped = snap_step(p(0.1, 2.2), &polar_step).unwrap();
        assert!(snapped.x.abs() < 1e-12 && (snapped.y - 2.0).abs() < 1e-12);
    }

    #[test]
    fn grid_points_cover_the_window_and_respect_the_bound() {
        let grid = Grid {
            origin: p(0.0, 0.0),
            spacing: [1.0, 1.0],
            rotation: 0.0,
        };
        let window = Aabb2::from_corners(p(-0.5, -0.5), p(1.5, 0.5));
        let points = grid_points(&window, &grid, 100).unwrap();
        assert_eq!(points, vec![p(0.0, 0.0), p(1.0, 0.0)]);
        assert!(grid_points(&window, &grid, 1).is_err());
    }

    #[test]
    fn tracking_intersects_horizontal_and_vertical_paths() {
        let frame = Frame::new(Vector2::x()).unwrap();
        let tolerance = Tolerance::DEFAULT;
        let acquired = [p(0.0, 5.0), p(7.0, 0.0)];
        let hit = track(p(7.2, 5.1), &acquired, &frame, None, 0.5, &tolerance)
            .unwrap()
            .unwrap();
        assert_eq!(hit.point, p(7.0, 5.0));
        assert_eq!(hit.paths.len(), 2);
        let single = track(p(3.0, 5.2), &acquired, &frame, None, 0.5, &tolerance)
            .unwrap()
            .unwrap();
        assert_eq!(single.paths.len(), 1);
        assert!((single.point.y - 5.0).abs() < 1e-12 && single.point.x == 3.0);
        assert!(
            track(p(3.0, 9.0), &acquired, &frame, None, 0.5, &tolerance)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn constraint_precedence_follows_the_documented_order() {
        let frame = Frame::new(Vector2::x()).unwrap();
        let settings = PolarSettings::new(FRAC_PI_2, vec![], 0.01).unwrap();
        let step = SnapStep::Rectangular {
            base: p(0.0, 0.0),
            spacing: [10.0, 10.0],
            rotation: 0.0,
        };
        let mut context = ConstraintContext {
            frame,
            base: Some(p(0.0, 0.0)),
            snapped: Some(p(9.0, 9.0)),
            tracked: Some(p(8.0, 8.0)),
            ortho: true,
            polar: Some(&settings),
            step: Some(step),
        };
        let cursor = p(3.2, 4.1);
        assert_eq!(
            constrain(cursor, &context).unwrap().source,
            ConstraintSource::ObjectSnap
        );
        context.snapped = None;
        assert_eq!(
            constrain(cursor, &context).unwrap().source,
            ConstraintSource::Tracking
        );
        context.tracked = None;
        let ortho_hit = constrain(cursor, &context).unwrap();
        assert_eq!(ortho_hit.source, ConstraintSource::Ortho);
        // The vertical component is the larger one, so the vertical axis wins.
        assert_eq!(ortho_hit.point, p(0.0, 4.1));
        context.ortho = false;
        assert_eq!(
            constrain(p(4.0, 0.02), &context).unwrap().source,
            ConstraintSource::Polar { angle: 0.0 }
        );
        assert_eq!(
            constrain(cursor, &context).unwrap().source,
            ConstraintSource::Step
        );
        context.step = None;
        assert_eq!(
            constrain(cursor, &context).unwrap(),
            Constrained {
                point: cursor,
                source: ConstraintSource::Raw
            }
        );
    }

    #[test]
    fn helper_points_are_checked() {
        assert_eq!(mid_between(p(0.0, 0.0), p(2.0, 4.0)).unwrap(), p(1.0, 2.0));
        assert_eq!(
            from_offset(p(1.0, 1.0), Vector2::new(2.0, -1.0)).unwrap(),
            p(3.0, 0.0)
        );
        assert!(mid_between(p(f64::NAN, 0.0), p(0.0, 0.0)).is_err());
        assert!(from_offset(p(0.0, 0.0), Vector2::new(f64::INFINITY, 0.0)).is_err());
    }

    #[test]
    fn frame_refuses_zero_and_non_finite_axes() {
        assert!(Frame::new(Vector2::zeros()).is_err());
        assert!(Frame::new(Vector2::new(f64::NAN, 1.0)).is_err());
    }
}
