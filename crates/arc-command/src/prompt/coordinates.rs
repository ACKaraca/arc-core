//! Typed coordinate entry for point prompts (DB-SNAP-019, 021, 022 and 024 of the drafting
//! capability list).
//!
//! [`read`] recognizes the point forms of the grammar in `docs/crates/arc-command.md` and places
//! them: absolute and relative Cartesian, polar, cylindrical, spherical, the last point, direct
//! distance along a direction, and the direction lock. Lengths and angles are read by the solver first and then by
//! the notations of `arc_base::DisplayFormat`, so no second parser exists here.

use std::f64::consts::{FRAC_PI_2, TAU};

use arc_base::display::Notation;
use arc_base::{Dimension, DisplayFormat, Error, Quantity, Result, Unit};

use super::{Point, range_error};
use crate::context::Session;
use crate::error::codes;
use crate::line;

/// The longest typed point text, in bytes. Points are short; the bound keeps parsing small.
pub const MAX_POINT_BYTES: usize = 256;

/// The denominator the architectural notation is asked for. Parsing reads fractions of any
/// denominator up to 256; the value only has to be a valid denominator.
const ARCHITECTURAL_DENOMINATOR: u16 = 16;

/// A 2D affine map of the plane from the user coordinate system to world coordinates. Typed
/// coordinates other than world ones pass through it; heights are read unchanged.
// shortcut: the transform maps the plane only, so a user coordinate system that tilts the plane
// needs a 3D map; upgrade when a product sets non-vertical user coordinate systems.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneTransform {
    /// The linear part, as rows.
    matrix: [[f64; 2]; 2],
    /// The translation.
    offset: [f64; 2],
}

impl Default for PlaneTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl PlaneTransform {
    /// The identity: world and user coordinates are the same.
    pub const IDENTITY: Self = Self {
        matrix: [[1.0, 0.0], [0.0, 1.0]],
        offset: [0.0, 0.0],
    };

    /// A transform with the linear part `matrix` (rows) and the translation `offset`.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.range` with the reason `transform` when a value is not finite or the linear
    /// part is singular.
    pub fn new(matrix: [[f64; 2]; 2], offset: [f64; 2]) -> Result<Self> {
        let [[a, b], [c, d]] = matrix;
        let [e, f] = offset;
        let determinant = a * d - b * c;
        let finite = [a, b, c, d, e, f, determinant]
            .iter()
            .all(|v| v.is_finite());
        if finite && determinant != 0.0 {
            Ok(Self { matrix, offset })
        } else {
            Err(range_error("transform"))
        }
    }

    /// Maps a point of the user coordinate system to world coordinates.
    #[must_use]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        if *self == Self::IDENTITY {
            return (x, y);
        }
        let (px, py) = self.apply_vector(x, y);
        let [e, f] = self.offset;
        (px + e, py + f)
    }

    /// Maps a vector of the user coordinate system; the translation does not apply.
    #[must_use]
    pub fn apply_vector(&self, x: f64, y: f64) -> (f64, f64) {
        if *self == Self::IDENTITY {
            return (x, y);
        }
        let [[a, b], [c, d]] = self.matrix;
        (a * x + b * y, c * x + d * y)
    }
}

/// The state a point prompt reads besides its answers: the last point the host set, the host's
/// pointer direction, a direction locked by typing `<angle`, the default for typed coordinates
/// and the user coordinate system.
///
/// A host carries the entry from one prompt session to the next (see
/// [`super::PromptSession::carry`]). Its defaults are an absolute default, the identity transform
/// and no point, direction or lock.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PointEntry {
    last: Option<Point>,
    direction: Option<[f64; 2]>,
    locked: Option<[f64; 2]>,
    relative: bool,
    transform: PlaneTransform,
}

impl PointEntry {
    /// The defaults: no last point, no direction, absolute typed coordinates and the identity
    /// transform.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The last point the host set, used until a point answer is accepted. A session's last
    /// point is its latest accepted point; see [`super::PromptSession::last_point`].
    #[must_use]
    pub fn last_point(&self) -> Option<Point> {
        self.last
    }

    /// Sets or clears the last point the host supplies.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.range` with the reason `not_finite` when a coordinate is not finite.
    pub fn set_last_point(&mut self, point: Option<Point>) -> Result<()> {
        if let Some(p) = point
            && ![p.x, p.y, p.z.unwrap_or(0.0)].iter().all(|v| v.is_finite())
        {
            return Err(range_error("not_finite"));
        }
        self.last = point;
        Ok(())
    }

    /// The host's pointer direction: a unit vector in the plane, in world coordinates.
    #[must_use]
    pub fn direction(&self) -> Option<[f64; 2]> {
        self.direction
    }

    /// Sets or clears the host's pointer direction. The vector is normalized.
    ///
    /// # Errors
    ///
    /// `cmd.prompt.range` with the reason `direction` when a component is not finite or the
    /// vector has no length.
    pub fn set_direction(&mut self, direction: Option<[f64; 2]>) -> Result<()> {
        self.direction = direction.map(unit_vector).transpose()?;
        Ok(())
    }

    /// The direction a direct distance uses: the locked direction when `<angle` set one, else the
    /// host's pointer direction.
    #[must_use]
    pub fn effective_direction(&self) -> Option<[f64; 2]> {
        self.locked.or(self.direction)
    }

    /// The direction `<angle` locked for the next point answer, if any.
    #[must_use]
    pub fn locked_direction(&self) -> Option<[f64; 2]> {
        self.locked
    }

    /// Whether a typed coordinate without a prefix is relative (`false` by default: absolute).
    #[must_use]
    pub fn relative_default(&self) -> bool {
        self.relative
    }

    /// Sets whether a typed coordinate without a prefix is relative. `#` and `@` override it per
    /// answer.
    pub fn set_relative_default(&mut self, relative: bool) {
        self.relative = relative;
    }

    /// The user coordinate system transform.
    #[must_use]
    pub fn transform(&self) -> PlaneTransform {
        self.transform
    }

    /// Sets the user coordinate system transform.
    pub fn set_transform(&mut self, transform: PlaneTransform) {
        self.transform = transform;
    }

    /// Replaces the last point without validation; the caller passes accepted points only.
    pub(crate) fn with_last_point(mut self, point: Option<Point>) -> Self {
        self.last = point;
        self
    }

    /// Locks a direction for the next point answer.
    pub(crate) fn lock(&mut self, direction: [f64; 2]) {
        self.locked = Some(direction);
    }

    /// Releases a locked direction.
    pub(crate) fn unlock(&mut self) {
        self.locked = None;
    }
}

/// A unit vector of `[x, y]`.
fn unit_vector([x, y]: [f64; 2]) -> Result<[f64; 2]> {
    let length = x.hypot(y);
    if x.is_finite() && y.is_finite() && length.is_finite() && length > 0.0 {
        Ok([x / length, y / length])
    } else {
        Err(range_error("direction"))
    }
}

/// What typed point text means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Read {
    /// A point in world coordinates.
    Point(Point),
    /// A unit direction in world coordinates that the next direct distance uses.
    Lock([f64; 2]),
}

/// The prefix that chooses how a coordinate is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No prefix: the entry's default.
    Default,
    /// `#`: absolute.
    Absolute,
    /// `@`: relative to the last point.
    Relative,
}

/// The components of a Cartesian or polar form in the user coordinate system, in meters.
#[derive(Debug, Clone, Copy)]
struct Vector {
    x: f64,
    y: f64,
    z: Option<f64>,
}

/// Reads typed point text. `None` means the text has none of the point forms, so the accepted
/// kinds after it are tried; `Some(Err)` means a form was recognized and rejected.
pub(crate) fn read(
    text: &str,
    session: &Session,
    entry: &PointEntry,
    last: Option<Point>,
) -> Option<Result<Read>> {
    if text.len() > MAX_POINT_BYTES {
        return Some(Err(
            Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "point_bytes")
        ));
    }
    let (world, rest) = match text.strip_prefix('*') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text),
    };
    let (mode, body) = if let Some(rest) = rest.strip_prefix('#') {
        (Mode::Absolute, rest.trim_start())
    } else if let Some(rest) = rest.strip_prefix('@') {
        (Mode::Relative, rest.trim_start())
    } else {
        (Mode::Default, rest)
    };
    let body = body.trim_end();
    let framed = world || mode != Mode::Default;

    if body.is_empty() {
        // `@` alone is the last point; a prefix alone is not a point.
        return (mode == Mode::Relative).then(|| last.ok_or_else(no_base).map(Read::Point));
    }

    if let Some(angle_text) = body.strip_prefix('<') {
        // `<angle` locks a direction; it takes no prefix and no distance.
        let angle_text = angle_text.trim();
        if framed || angle_text.is_empty() {
            return None;
        }
        return Some(angle(angle_text, session).and_then(|radians| {
            let (x, y) = entry.transform.apply_vector(radians.cos(), radians.sin());
            unit_vector([x, y]).map(Read::Lock)
        }));
    }

    let parts = line::split_top_level(body, ',');
    if parts.len() > 1 && line::split_top_level(parts[0], '<').len() > 1 {
        // Cylindrical `distance<angle,z`: a polar form in the plane, then a height.
        let [planar, height] = parts.as_slice() else {
            return None;
        };
        let planar = polar_text(planar.trim(), session)?;
        return Some(
            planar
                .and_then(|(distance, radians)| {
                    length(height.trim(), session)
                        .map(|z| planar_vector(distance, radians, Some(z)))
                })
                .and_then(|vector| place(vector, mode, world, entry, last).map(Read::Point)),
        );
    }
    if parts.len() > 1 {
        if !(2..=3).contains(&parts.len()) {
            return None;
        }
        let values: Result<Vec<f64>> = parts
            .iter()
            .map(|part| length(part.trim(), session))
            .collect();
        return Some(values.and_then(|values| {
            let vector = match values.as_slice() {
                [x, y] => Vector {
                    x: *x,
                    y: *y,
                    z: None,
                },
                [x, y, z] => Vector {
                    x: *x,
                    y: *y,
                    z: Some(*z),
                },
                _ => return Err(range_error("dimensions")),
            };
            place(vector, mode, world, entry, last).map(Read::Point)
        }));
    }

    if line::split_top_level(body, '<').len() > 1 {
        return angled(body, session).map(|vector| {
            vector.and_then(|vector| place(vector, mode, world, entry, last).map(Read::Point))
        });
    }

    // A bare length: a distance along the direction from the last point.
    if framed {
        return None;
    }
    let distance = length(body, session).ok()?;
    Some(match (last, entry.effective_direction()) {
        (None, _) => Err(no_base()),
        (Some(_), None) => Err(Error::new(codes::PROMPT_NO_DIRECTION)),
        (Some(p), Some([dx, dy])) => Ok(Read::Point(Point {
            x: p.x + distance * dx,
            y: p.y + distance * dy,
            z: p.z,
        })),
    })
}

/// Places a vector given in the user coordinate system (or in world coordinates with `*`) as
/// an absolute point, or as an offset from the last point.
fn place(
    vector: Vector,
    mode: Mode,
    world: bool,
    entry: &PointEntry,
    last: Option<Point>,
) -> Result<Point> {
    let relative = match mode {
        Mode::Relative => true,
        Mode::Absolute => false,
        Mode::Default => entry.relative_default(),
    };
    let Vector { x, y, z } = vector;
    if relative {
        let base = last.ok_or_else(no_base)?;
        let (dx, dy) = if world {
            (x, y)
        } else {
            entry.transform.apply_vector(x, y)
        };
        let z = match z {
            Some(dz) => Some(base.z.unwrap_or(0.0) + dz),
            None => base.z,
        };
        Ok(Point {
            x: base.x + dx,
            y: base.y + dy,
            z,
        })
    } else {
        let (px, py) = if world {
            (x, y)
        } else {
            entry.transform.apply(x, y)
        };
        Ok(Point { x: px, y: py, z })
    }
}

/// The vector of a polar form: `distance` at `radians` counterclockwise from the X axis.
fn planar_vector(distance: f64, radians: f64, z: Option<f64>) -> Vector {
    Vector {
        x: distance * radians.cos(),
        y: distance * radians.sin(),
        z,
    }
}

/// The vector of a spherical form: `distance` from the origin, `elevation` above the plane and
/// `radians` counterclockwise from the X axis within the plane.
fn spherical_vector(distance: f64, radians: f64, elevation: f64) -> Vector {
    let horizontal = distance * elevation.cos();
    Vector {
        x: horizontal * radians.cos(),
        y: horizontal * radians.sin(),
        z: Some(distance * elevation.sin()),
    }
}

/// The vector of a polar `distance<angle` or a spherical `distance<angle<elevation` form, with the
/// angle from the plane last. Every piece is required; `None` when the text has neither shape.
fn angled(body: &str, session: &Session) -> Option<Result<Vector>> {
    let pieces = line::split_top_level(body, '<');
    if pieces.iter().any(|piece| piece.is_empty()) {
        return None;
    }
    match pieces.as_slice() {
        [distance, radians] => Some(length(distance, session).and_then(|distance| {
            angle(radians, session).map(|radians| planar_vector(distance, radians, None))
        })),
        [distance, radians, elevation] => Some(length(distance, session).and_then(|distance| {
            let radians = angle(radians, session)?;
            let elevation = angle(elevation, session)?;
            Ok(spherical_vector(distance, radians, elevation))
        })),
        _ => None,
    }
}

/// The distance and the angle of the planar part `distance<angle`; `None` when the text has no
/// such shape.
fn polar_text(text: &str, session: &Session) -> Option<Result<(f64, f64)>> {
    let pieces = line::split_top_level(text, '<');
    match pieces.as_slice() {
        [distance, radians] if !distance.is_empty() && !radians.is_empty() => Some(
            length(distance, session)
                .and_then(|distance| angle(radians, session).map(|radians| (distance, radians))),
        ),
        _ => None,
    }
}

fn no_base() -> Error {
    Error::new(codes::PROMPT_NO_BASE)
}

/// Reads one length typed as text, as a distance prompt reads a typed answer: the text is trimmed
/// and read with [`length`], in meters.
///
/// # Errors
///
/// `cmd.prompt.limit_exceeded` (detail `limit` = `line_bytes`) when the trimmed text is longer
/// than the command line's byte cap; otherwise the reader's error (`cmd.prompt.range`, or the
/// solver's error when the text is not a length).
pub fn parse_length(text: &str, session: &Session) -> Result<f64> {
    length(checked_line(text)?, session)
}

/// Reads one angle typed as text, as an angle prompt reads a typed answer: the text is trimmed and
/// read with [`angle`], in radians counterclockwise from the positive X axis.
///
/// # Errors
///
/// `cmd.prompt.limit_exceeded` (detail `limit` = `line_bytes`) when the trimmed text is longer
/// than the command line's byte cap; otherwise the reader's error (`cmd.prompt.range`, or the
/// solver's error when the text is not an angle).
pub fn parse_angle(text: &str, session: &Session) -> Result<f64> {
    angle(checked_line(text)?, session)
}

/// The trimmed text, refused when it is longer than the command line's byte cap.
fn checked_line(text: &str) -> Result<&str> {
    let text = text.trim();
    if text.len() > line::MAX_LINE_BYTES {
        return Err(Error::new(codes::PROMPT_LIMIT_EXCEEDED).with_detail("limit", "line_bytes"));
    }
    Ok(text)
}

/// A length in meters: the solver's reading, else the feet-and-inches notation. When both fail,
/// the solver's error is reported.
pub(crate) fn length(text: &str, session: &Session) -> Result<f64> {
    super::quantity(text, session, Dimension::LENGTH)
        .map(Quantity::value)
        .or_else(|error| architectural(text).ok_or(error))
}

fn architectural(text: &str) -> Option<f64> {
    let format = DisplayFormat::new(*Unit::by_name("inch").ok()?);
    format
        .parse_with(
            text,
            Notation::Architectural {
                denominator: ARCHITECTURAL_DENOMINATOR,
            },
            '.',
        )
        .ok()
        .map(Quantity::value)
}

/// An angle in radians, counterclockwise from the positive X axis of the user coordinate system:
/// the solver's reading, else degrees-minutes-seconds, else a surveyor bearing. When every reading
/// fails, the solver's error is reported.
pub(crate) fn angle(text: &str, session: &Session) -> Result<f64> {
    super::quantity(text, session, Dimension::ANGLE)
        .map(Quantity::value)
        .or_else(|error| notation_angle(text).ok_or(error))
}

fn notation_angle(text: &str) -> Option<f64> {
    let format = DisplayFormat::new(*Unit::by_name("degree").ok()?);
    if let Ok(quantity) = format.parse_with(text, Notation::DegreesMinutesSeconds, '.') {
        return Some(quantity.value());
    }
    // A bearing is an azimuth clockwise from north; the angle counts counterclockwise from east.
    let azimuth = format
        .parse_with(text, Notation::SurveyorBearing, '.')
        .ok()?
        .value();
    Some((FRAC_PI_2 - azimuth).rem_euclid(TAU))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, reason = "tests")]

    use super::*;

    #[test]
    fn identity_transform_leaves_values_unchanged() {
        assert_eq!(PlaneTransform::IDENTITY.apply(-1.5, 2.0), (-1.5, 2.0));
        assert_eq!(
            PlaneTransform::IDENTITY.apply_vector(-1.5, 2.0),
            (-1.5, 2.0)
        );
    }

    #[test]
    fn transform_rejects_singular_and_non_finite_parts() {
        assert!(PlaneTransform::new([[1.0, 2.0], [2.0, 4.0]], [0.0, 0.0]).is_err());
        assert!(PlaneTransform::new([[f64::NAN, 0.0], [0.0, 1.0]], [0.0, 0.0]).is_err());
        assert!(PlaneTransform::new([[1.0, 0.0], [0.0, 1.0]], [f64::INFINITY, 0.0]).is_err());
        assert!(PlaneTransform::new([[0.0, 1.0], [1.0, 0.0]], [1.0, 1.0]).is_ok());
    }

    #[test]
    fn unit_vectors_normalize_and_reject_degenerate_input() {
        assert_eq!(unit_vector([3.0, 4.0]).unwrap(), [0.6, 0.8]);
        assert!(unit_vector([0.0, 0.0]).is_err());
        assert!(unit_vector([f64::NAN, 1.0]).is_err());
    }

    #[test]
    fn a_rotated_transform_maps_the_plane() {
        // A quarter turn counterclockwise with a translation of (10, 0).
        let turn = PlaneTransform::new([[0.0, -1.0], [1.0, 0.0]], [10.0, 0.0]).unwrap();
        let (x, y) = turn.apply(1.0, 0.0);
        assert!((x - 10.0).abs() < 1e-12 && (y - 1.0).abs() < 1e-12);
        let (x, y) = turn.apply_vector(1.0, 0.0);
        assert!(x.abs() < 1e-12 && (y - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bearings_count_counterclockwise_from_east() {
        let north_east = notation_angle("N 45°0'0\" E").unwrap();
        assert!((north_east - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
        let south_east = notation_angle("S 45°0'0\" E").unwrap();
        assert!(
            (south_east + std::f64::consts::FRAC_PI_4)
                .rem_euclid(TAU)
                .abs()
                < 1e-12
        );
        let north_west = notation_angle("N 45°0'0\" W").unwrap();
        assert!((north_west - 3.0 * std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    }

    use std::sync::Arc;

    use arc_base::Unit;
    use arc_doc::{Snapshot, TypeRegistry};
    use serde_json::{Map, Value, json};

    use crate::prompt::{
        Accept, Feedback, PointDims, Prompt, PromptInput, PromptMachine, PromptSequence,
        PromptSession, PromptState, PromptStep, Step,
    };
    use crate::spec::{CommandSpec, Surface};

    #[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    struct At {
        at: Vec<f64>,
    }

    #[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    struct Scalar {
        at: f64,
    }

    #[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
    struct Two {
        a: Vec<f64>,
        b: Vec<f64>,
    }

    fn point_accept() -> Accept {
        Accept::Point {
            dims: PointDims::Any,
        }
    }

    fn session(units: &[&str]) -> Session {
        let mut session = Session::new(Surface::CommandLine, []);
        session.units = units
            .iter()
            .map(|name| *Unit::by_name(name).unwrap())
            .collect();
        session
    }

    fn snapshot() -> Snapshot {
        Snapshot::empty(Arc::new(TypeRegistry::new()))
    }

    fn entry_at(x: f64, y: f64) -> PointEntry {
        let mut entry = PointEntry::new();
        entry.set_last_point(Some(Point { x, y, z: None })).unwrap();
        entry
    }

    /// A session asking one point prompt `at` with the state `entry`.
    fn point_session(entry: PointEntry, units: &[&str]) -> (PromptSession, Snapshot) {
        let spec = CommandSpec::new("test.point.run", "l", "d", |_, _: At| Ok(()))
            .with_prompts(PromptSequence::new().step(PromptStep::new("at", "m", point_accept())));
        let snapshot = snapshot();
        let (session_state, first) = PromptSession::start_with_entry(
            Arc::new(spec),
            Map::new(),
            &session(units),
            &snapshot,
            entry,
        );
        assert!(matches!(first, Feedback::Ask(_)), "{first:?}");
        (session_state, snapshot)
    }

    /// A session asking one scalar prompt `at` with the accepted kind `accept`.
    fn scalar_session(accept: Accept, units: &[&str]) -> (PromptSession, Snapshot) {
        let spec = CommandSpec::new("test.scalar.run", "l", "d", |_, _: Scalar| Ok(()))
            .with_prompts(PromptSequence::new().step(PromptStep::new("at", "m", accept)));
        let snapshot = snapshot();
        let (session_state, first) =
            PromptSession::start(Arc::new(spec), Map::new(), &session(units), &snapshot);
        assert!(matches!(first, Feedback::Ask(_)), "{first:?}");
        (session_state, snapshot)
    }

    fn typed(text: &str, units: &[&str], entry: PointEntry) -> Feedback {
        let (mut prompts, snapshot) = point_session(entry, units);
        prompts.answer_text(text, &snapshot)
    }

    fn done_at(feedback: Feedback) -> Vec<f64> {
        let Feedback::Done(invocation) = feedback else {
            panic!("{feedback:?}")
        };
        invocation.params["at"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    }

    fn code_of(feedback: Feedback) -> String {
        match feedback {
            Feedback::Rejected { error, .. } | Feedback::Failed(error) => error.code().to_owned(),
            other => panic!("{other:?}"),
        }
    }

    fn close(actual: &[f64], expected: &[f64]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?} vs {expected:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-9, "{actual:?} vs {expected:?}");
        }
    }

    #[test]
    fn absolute_cartesian_keeps_its_meaning() {
        close(
            &done_at(typed("1,2", &["meter"], PointEntry::new())),
            &[1.0, 2.0],
        );
        close(
            &done_at(typed(" 1 , 2 , 3 ", &["meter"], PointEntry::new())),
            &[1.0, 2.0, 3.0],
        );
        // A last point does not make an absolute form relative.
        close(
            &done_at(typed("1,2", &["meter"], entry_at(10.0, 20.0))),
            &[1.0, 2.0],
        );
    }

    #[test]
    fn relative_cartesian_and_the_last_point_measure_from_the_last_point() {
        let e = entry_at(10.0, 20.0);
        close(
            &done_at(typed("@1,2", &["meter"], e.clone())),
            &[11.0, 22.0],
        );
        close(
            &done_at(typed("@1,2,3", &["meter"], e.clone())),
            &[11.0, 22.0, 3.0],
        );
        close(&done_at(typed("@", &["meter"], e.clone())), &[10.0, 20.0]);
        close(&done_at(typed("*@", &["meter"], e)), &[10.0, 20.0]);
    }

    #[test]
    fn relative_forms_without_a_last_point_are_typed_errors() {
        for text in ["@1,2", "@", "*@", "@5<30"] {
            assert_eq!(
                code_of(typed(text, &["meter", "degree"], PointEntry::new())),
                "cmd.prompt.no_base",
                "{text}"
            );
        }
    }

    #[test]
    fn the_default_for_typed_coordinates_is_absolute_and_can_be_relative() {
        let mut e = entry_at(10.0, 20.0);
        e.set_relative_default(true);
        close(&done_at(typed("1,2", &["meter"], e.clone())), &[11.0, 22.0]);
        close(&done_at(typed("#1,2", &["meter"], e.clone())), &[1.0, 2.0]);
        close(
            &done_at(typed("@1,2", &["meter"], e.clone())),
            &[11.0, 22.0],
        );
        close(&done_at(typed("*#1,2", &["meter"], e.clone())), &[1.0, 2.0]);
        close(
            &done_at(typed("5<0", &["meter", "degree"], e)),
            &[15.0, 20.0],
        );
    }

    #[test]
    fn polar_forms_are_counterclockwise_from_the_positive_x_axis() {
        let units = ["meter", "degree"];
        close(
            &done_at(typed("5<90", &units, PointEntry::new())),
            &[0.0, 5.0],
        );
        close(
            &done_at(typed("5 < 0", &units, PointEntry::new())),
            &[5.0, 0.0],
        );
        close(
            &done_at(typed("@5<180", &units, entry_at(10.0, 20.0))),
            &[5.0, 20.0],
        );
        // A bare angle needs the angle context unit, as the angle prompt does.
        assert!(matches!(
            typed("5<90", &["meter"], PointEntry::new()),
            Feedback::Rejected { .. }
        ));
    }

    #[test]
    fn world_prefix_and_the_user_coordinate_system() {
        let units = ["meter"];
        let mut e = PointEntry::new();
        // Quarter turn counterclockwise, then a translation of (100, 0).
        e.set_transform(PlaneTransform::new([[0.0, -1.0], [1.0, 0.0]], [100.0, 0.0]).unwrap());
        close(&done_at(typed("1,0", &units, e.clone())), &[100.0, 1.0]);
        close(&done_at(typed("*1,0", &units, e.clone())), &[1.0, 0.0]);
        close(
            &done_at(typed("1,0,5", &units, e.clone())),
            &[100.0, 1.0, 5.0],
        );

        let mut from_origin = e.clone();
        from_origin
            .set_last_point(Some(Point {
                x: 0.0,
                y: 0.0,
                z: None,
            }))
            .unwrap();
        // The offset of a relative form is mapped without the translation.
        close(
            &done_at(typed("@1,0", &units, from_origin.clone())),
            &[0.0, 1.0],
        );
        close(&done_at(typed("*@1,0", &units, from_origin)), &[1.0, 0.0]);
    }

    #[test]
    fn direct_distance_follows_the_host_direction_and_the_lock_overrides_it() {
        let mut e = entry_at(1.0, 2.0);
        e.set_direction(Some([0.0, 2.0])).unwrap();
        assert_eq!(e.direction(), Some([0.0, 1.0]));
        close(&done_at(typed("5", &["meter"], e.clone())), &[1.0, 7.0]);

        let mut no_direction = entry_at(1.0, 2.0);
        assert_eq!(
            code_of(typed("5", &["meter"], no_direction.clone())),
            "cmd.prompt.no_direction"
        );
        no_direction.set_direction(None).unwrap();
        assert_eq!(
            code_of(typed("5", &["meter"], PointEntry::new())),
            "cmd.prompt.no_base"
        );

        let mut locked = entry_at(0.0, 0.0);
        locked.set_direction(Some([1.0, 0.0])).unwrap();
        let (mut prompts, snapshot) = point_session(locked, &["meter", "degree"]);
        assert!(matches!(
            prompts.answer_text("<90", &snapshot),
            Feedback::Ask(_)
        ));
        close(
            &prompts.point_entry().locked_direction().unwrap(),
            &[0.0, 1.0],
        );
        close(&done_at(prompts.answer_text("5", &snapshot)), &[0.0, 5.0]);
        assert_eq!(prompts.point_entry().locked_direction(), None);
    }

    #[test]
    fn a_locked_direction_is_released_by_an_accepted_point() {
        let mut e = entry_at(0.0, 0.0);
        e.set_direction(Some([1.0, 0.0])).unwrap();
        let (mut prompts, snapshot) = point_session(e, &["meter", "degree"]);
        assert!(matches!(
            prompts.answer_text("<0", &snapshot),
            Feedback::Ask(_)
        ));
        assert!(prompts.point_entry().locked_direction().is_some());
        // A point typed after the lock is taken as typed and releases the lock.
        close(&done_at(prompts.answer_text("3,4", &snapshot)), &[3.0, 4.0]);
        assert_eq!(prompts.point_entry().locked_direction(), None);
    }

    #[test]
    fn lock_forms_are_read_only_on_point_prompts_without_a_prefix() {
        let (mut distance, snapshot) = scalar_session(Accept::distance(), &["meter"]);
        assert!(matches!(
            distance.answer_text("<90", &snapshot),
            Feedback::Rejected { .. }
        ));
        assert!(matches!(
            typed("@<30", &["degree"], entry_at(0.0, 0.0)),
            Feedback::Rejected { .. }
        ));
        assert_eq!(
            code_of(typed("@<30", &["degree"], entry_at(0.0, 0.0))),
            "cmd.prompt.unreadable"
        );
    }

    #[test]
    fn malformed_text_is_unreadable_on_point_prompts() {
        for text in ["5<", "<", "1,2,3,4", "@<30", "*", "#", "#@", "@1", "*5"] {
            assert_eq!(
                code_of(typed(text, &["meter", "degree"], entry_at(0.0, 0.0))),
                "cmd.prompt.unreadable",
                "{text}"
            );
        }
        // A recognized form with a bad component reports the solver's error.
        assert!(matches!(
            typed("1,", &["meter"], PointEntry::new()),
            Feedback::Rejected { .. }
        ));
    }

    #[test]
    fn whitespace_is_allowed_around_prefixes_and_separators() {
        let e = entry_at(10.0, 20.0);
        close(
            &done_at(typed("\t@\t1 ,\t2 ", &["meter"], e.clone())),
            &[11.0, 22.0],
        );
        close(&done_at(typed("@ 1 , 2", &["meter"], e)), &[11.0, 22.0]);
    }

    #[test]
    fn text_is_bounded_and_results_must_be_finite() {
        let long = "1".repeat(MAX_POINT_BYTES + 1);
        assert_eq!(
            code_of(typed(&long, &["meter"], PointEntry::new())),
            "cmd.prompt.limit_exceeded"
        );

        let mut e = PointEntry::new();
        assert!(
            e.set_last_point(Some(Point {
                x: f64::NAN,
                y: 0.0,
                z: None
            }))
            .is_err()
        );
        // The sum overflows to infinity, which a point prompt rejects.
        let huge = entry_at(f64::MAX, 0.0);
        assert_eq!(
            code_of(typed("@1e308,0", &["meter"], huge)),
            "cmd.prompt.range"
        );
    }

    #[test]
    fn distances_and_angles_read_the_existing_notations() {
        let units = session(&["meter", "degree"]);
        let inch = 0.0254;
        assert!((length("5'-3\"", &units).unwrap() - 63.0 * inch).abs() < 1e-12);
        assert!((length("3 1/2\"", &units).unwrap() - 3.5 * inch).abs() < 1e-12);
        assert!((length("1 1/2\"", &units).unwrap() - 1.5 * inch).abs() < 1e-12);
        assert!(
            (angle("45°30'15\"", &units).unwrap()
                - (45.0_f64 + 30.0 / 60.0 + 15.0 / 3600.0).to_radians())
            .abs()
                < 1e-12
        );
        assert!((angle("90°", &units).unwrap() - FRAC_PI_2).abs() < 1e-12);
        assert!((angle("100 gon", &units).unwrap() - FRAC_PI_2).abs() < 1e-12);
        assert!(
            (angle("N 45°0'0\" E", &units).unwrap() - std::f64::consts::FRAC_PI_4).abs() < 1e-12
        );
        assert!(length("abc", &units).is_err());
        assert!(angle("N 95°0'0\" E", &units).is_err());
    }

    #[test]
    fn distance_and_angle_prompts_keep_their_readings() {
        let (mut distance, snapshot) = scalar_session(Accept::distance(), &["meter"]);
        let Feedback::Done(invocation) = distance.answer_text("5", &snapshot) else {
            panic!()
        };
        assert_eq!(invocation.params["at"], json!(5.0));

        let (mut angle_prompt, snapshot) = scalar_session(Accept::angle(), &["degree"]);
        let Feedback::Done(invocation) = angle_prompt.answer_text("90", &snapshot) else {
            panic!()
        };
        let value = invocation.params["at"].as_f64().unwrap();
        assert!((value - FRAC_PI_2).abs() < 1e-12);

        // Point forms are not distances or angles.
        let (mut distance, snapshot) = scalar_session(Accept::distance(), &["meter"]);
        assert!(matches!(
            distance.answer_text("@1,2", &snapshot),
            Feedback::Rejected { .. }
        ));
        let (mut angle_prompt, snapshot) = scalar_session(Accept::angle(), &["degree"]);
        assert!(matches!(
            angle_prompt.answer_text("5<90", &snapshot),
            Feedback::Rejected { .. }
        ));
    }

    #[test]
    fn last_point_follows_accepted_points_and_back() {
        let spec = CommandSpec::new("test.two.run", "l", "d", |_, _: Two| Ok(())).with_prompts(
            PromptSequence::new()
                .step(PromptStep::new("a", "m", point_accept()))
                .step(PromptStep::new("b", "m", point_accept())),
        );
        let snapshot = snapshot();
        let (mut prompts, _) =
            PromptSession::start(Arc::new(spec), Map::new(), &session(&["meter"]), &snapshot);
        assert!(matches!(
            prompts.answer_text("1,1", &snapshot),
            Feedback::Ask(_)
        ));
        assert_eq!(
            prompts.last_point(),
            Some(Point {
                x: 1.0,
                y: 1.0,
                z: None
            })
        );
        let Feedback::Done(invocation) = prompts.answer_text("@1,0", &snapshot) else {
            panic!()
        };
        assert_eq!(invocation.params["b"], json!([2.0, 1.0]));

        // Back to the first prompt restores the entry's last point, which is none here.
        let (mut prompts, _) = PromptSession::start(
            Arc::new(
                CommandSpec::new("test.two.run", "l", "d", |_, _: Two| Ok(())).with_prompts(
                    PromptSequence::new()
                        .step(PromptStep::new("a", "m", point_accept()))
                        .step(PromptStep::new("b", "m", point_accept())),
                ),
            ),
            Map::new(),
            &session(&["meter"]),
            &snapshot,
        );
        prompts.answer_text("1,1", &snapshot);
        assert!(matches!(prompts.back(&snapshot), Feedback::Ask(_)));
        assert_eq!(prompts.last_point(), None);
    }

    #[test]
    fn carry_hands_the_last_point_and_settings_to_the_next_command() {
        let (mut first, snapshot) = point_session(PointEntry::new(), &["meter"]);
        close(&done_at(first.answer_text("4,5", &snapshot)), &[4.0, 5.0]);
        first.point_entry_mut().set_relative_default(true);
        first
            .point_entry_mut()
            .set_direction(Some([0.0, 1.0]))
            .unwrap();
        let carried = first.carry();
        assert_eq!(
            carried.last_point(),
            Some(Point {
                x: 4.0,
                y: 5.0,
                z: None
            })
        );
        assert!(carried.relative_default());
        assert_eq!(carried.direction(), Some([0.0, 1.0]));

        let (mut next, snapshot) = point_session(carried, &["meter"]);
        close(&done_at(next.answer_text("1,0", &snapshot)), &[5.0, 5.0]);
    }

    #[derive(Debug)]
    struct Rubber;

    impl PromptMachine for Rubber {
        fn next(&self, state: &PromptState<'_>) -> arc_base::Result<Step> {
            if state.answers.is_empty() {
                Ok(Step::Ask(Prompt::new("at", "m", point_accept())))
            } else {
                Ok(Step::Done(Map::new()))
            }
        }

        fn preview(&self, state: &PromptState<'_>, _: Option<&PromptInput>) -> Option<Value> {
            Some(json!({
                "base": state.base.map(|p| [p.x, p.y]),
                "direction": state.direction,
            }))
        }
    }

    #[test]
    fn the_prompt_state_exposes_the_base_point_and_direction() {
        let spec = CommandSpec::new("test.rubber.run", "l", "d", |_, _: At| Ok(()))
            .with_prompt(Arc::new(Rubber));
        let snapshot = snapshot();
        let mut e = entry_at(3.0, 4.0);
        e.set_direction(Some([0.0, 1.0])).unwrap();
        let (mut prompts, first) = PromptSession::start_with_entry(
            Arc::new(spec),
            Map::new(),
            &session(&["meter", "degree"]),
            &snapshot,
            e,
        );
        assert!(matches!(first, Feedback::Ask(_)));
        let preview = prompts.preview(&snapshot, None).unwrap();
        assert_eq!(preview["base"], json!([3.0, 4.0]));
        assert_eq!(preview["direction"], json!([0.0, 1.0]));
        // A locked direction takes the place of the host's.
        assert!(matches!(
            prompts.answer_text("<0", &snapshot),
            Feedback::Ask(_)
        ));
        let preview = prompts.preview(&snapshot, None).unwrap();
        assert_eq!(preview["direction"], json!([1.0, 0.0]));
    }

    #[test]
    fn cylindrical_forms_are_a_polar_form_in_the_plane_and_a_height() {
        let units = ["meter", "degree"];
        close(
            &done_at(typed("5<90,2", &units, PointEntry::new())),
            &[0.0, 5.0, 2.0],
        );
        close(
            &done_at(typed(" 5 < 0 , 2 ", &units, PointEntry::new())),
            &[5.0, 0.0, 2.0],
        );
        // A relative cylindrical form offsets the plane and the height from the last point.
        close(
            &done_at(typed("@5<0,1", &units, entry_at(10.0, 20.0))),
            &[15.0, 20.0, 1.0],
        );
        let mut elevated = PointEntry::new();
        elevated
            .set_last_point(Some(Point {
                x: 10.0,
                y: 20.0,
                z: Some(3.0),
            }))
            .unwrap();
        close(
            &done_at(typed("@5<0,1", &units, elevated.clone())),
            &[15.0, 20.0, 4.0],
        );
        close(
            &done_at(typed("#5<0,1", &units, elevated)),
            &[5.0, 0.0, 1.0],
        );
    }

    #[test]
    fn cylindrical_and_spherical_forms_honor_the_plane_transform_and_world_prefix() {
        let units = ["meter", "degree"];
        let mut e = PointEntry::new();
        // Quarter turn counterclockwise, then a translation of (100, 0).
        e.set_transform(PlaneTransform::new([[0.0, -1.0], [1.0, 0.0]], [100.0, 0.0]).unwrap());
        close(
            &done_at(typed("5<0,2", &units, e.clone())),
            &[100.0, 5.0, 2.0],
        );
        close(
            &done_at(typed("*5<0,2", &units, e.clone())),
            &[5.0, 0.0, 2.0],
        );
        close(
            &done_at(typed("*10<90<0", &units, e.clone())),
            &[0.0, 10.0, 0.0],
        );
        close(
            &done_at(typed("10<0<90", &units, e.clone())),
            &[100.0, 0.0, 10.0],
        );
    }

    #[test]
    fn spherical_forms_measure_the_elevation_from_the_plane() {
        let units = ["meter", "degree"];
        close(
            &done_at(typed("10<90<0", &units, PointEntry::new())),
            &[0.0, 10.0, 0.0],
        );
        close(
            &done_at(typed("10<0<90", &units, PointEntry::new())),
            &[0.0, 0.0, 10.0],
        );
        let half = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
        close(
            &done_at(typed("10<0<45", &units, PointEntry::new())),
            &[half, 0.0, half],
        );
        // Relative: the offset is measured from the last point, its height added to the base height.
        close(
            &done_at(typed("@10<0<45", &units, entry_at(10.0, 20.0))),
            &[10.0 + half, 20.0, half],
        );
        close(
            &done_at(typed("*10<0<45", &units, entry_at(10.0, 20.0))),
            &[half, 0.0, half],
        );
    }

    #[test]
    fn cylindrical_and_spherical_forms_reject_malformed_and_hostile_text() {
        let units = ["meter", "degree"];
        for text in ["5<30<", "5<<30", "5<30<20<10", "5<90,2,3", "5<,2"] {
            assert_eq!(
                code_of(typed(text, &units, entry_at(0.0, 0.0))),
                "cmd.prompt.unreadable",
                "{text}"
            );
        }
        // Each part reads its own notation: a missing unit and a bad number are the reader's errors.
        for text in ["5<90,2", "10<90<0", "5<90,", "<90,2"] {
            assert!(
                matches!(
                    typed(text, &["meter"], PointEntry::new()),
                    Feedback::Rejected { .. }
                ),
                "{text}"
            );
        }
        for text in ["NaN<0,1", "5<NaN,1", "10<0<NaN", "5<0,inf"] {
            assert!(
                matches!(
                    typed(text, &units, PointEntry::new()),
                    Feedback::Rejected { .. }
                ),
                "{text}"
            );
        }
        // Overflow of a sum or a product is a range error, as for Cartesian forms.
        let huge = entry_at(f64::MAX, 0.0);
        assert_eq!(
            code_of(typed("@1e308<0,0", &units, huge.clone())),
            "cmd.prompt.range"
        );
        assert_eq!(
            code_of(typed("@1e308<0<0", &units, huge)),
            "cmd.prompt.range"
        );
    }

    #[test]
    fn a_comparison_inside_parentheses_is_not_an_angle_separator() {
        let units = ["meter", "degree"];
        // The `<` sits inside a call or a group: the text is Cartesian, not polar or cylindrical.
        close(
            &done_at(typed("if(1 m<2 m,1 m,2 m),3 m", &units, PointEntry::new())),
            &[1.0, 3.0],
        );
        close(
            &done_at(typed(
                "if(2 m<3 m,1 m,2 m),2 m,3 m",
                &units,
                PointEntry::new(),
            )),
            &[1.0, 2.0, 3.0],
        );
        // A polar form whose distance holds a comparison reads the top-level `<` only.
        let half = 3.0_f64.sqrt() / 2.0;
        close(
            &done_at(typed("if(1<2,1,2)<30", &units, PointEntry::new())),
            &[half, 0.5],
        );
    }
}
