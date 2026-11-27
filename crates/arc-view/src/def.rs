//! View definitions (CORE-VIEW-001, CORE-VIEW-003, CORE-VIEW-004, CORE-VIEW-006).

use arc_base::math::{Point2, Point3, Vector3};
use arc_base::{Error, Result, Tolerance};
use arc_geom::{Curve2, Region, RegionPart, Segment2};

use crate::error::codes;
use crate::source::Overrides;

/// The largest error a drawing may show on the sheet, in meters: 0.05 mm.
pub const SHEET_ERROR: f64 = 5e-5;

/// The direction a view looks in. The rest of the crate works in view space (x right, y up on
/// the sheet, z toward the viewer); a direction is the mapping between world and view space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum ViewDirection {
    /// Looking down the negative world z axis: world x and y are the sheet's x and y, and the
    /// world z is the height toward the viewer.
    #[default]
    Plan,
    /// Any orthographic frame (an elevation, a section, an axonometric view): view x runs along
    /// `right`, view y along `up` and view z along `right × up`, toward the viewer, all measured
    /// from `origin`. Build it with [`ViewDirection::frame`], which makes the axes orthonormal.
    Frame {
        /// The world point at the view-space origin.
        origin: Point3,
        /// The unit world direction of view x.
        right: Vector3,
        /// The unit world direction of view y, perpendicular to `right`.
        up: Vector3,
    },
}

impl ViewDirection {
    /// The orthographic frame at `origin` whose view x runs along `right` and whose view y runs
    /// along the part of `up` perpendicular to it; a vertical section or elevation has
    /// a horizontal `right` and `up` = +z.
    ///
    /// # Errors
    ///
    /// `view.definition.invalid` (`parameter` `direction`) for a point or axis that is not finite,
    /// an axis of no length or axes that are parallel.
    pub fn frame(origin: Point3, right: Vector3, up: Vector3) -> Result<Self> {
        let invalid = || Error::new(codes::DEFINITION_INVALID).with_arg("parameter", "direction");
        let finite = |v: &[f64]| v.iter().all(|c| c.is_finite());
        if !finite(origin.coords.as_slice()) || !finite(right.as_slice()) || !finite(up.as_slice())
        {
            return Err(invalid());
        }
        let right = right.try_normalize(f64::EPSILON).ok_or_else(invalid)?;
        let up = (up - right * up.dot(&right))
            .try_normalize(1e-9)
            .ok_or_else(invalid)?;
        Ok(Self::Frame { origin, right, up })
    }

    /// The view-space position of a world point.
    #[must_use]
    pub fn to_view(self, point: Point3) -> Point3 {
        match self {
            Self::Plan => point,
            Self::Frame { origin, right, up } => {
                let d = point - origin;
                Point3::new(d.dot(&right), d.dot(&up), d.dot(&right.cross(&up)))
            }
        }
    }

    /// The world position of a view-space point.
    #[must_use]
    pub fn to_world(self, point: Point3) -> Point3 {
        match self {
            Self::Plan => point,
            Self::Frame { origin, right, up } => {
                origin + right * point.x + up * point.y + right.cross(&up) * point.z
            }
        }
    }

    /// Whether the mapping is a rotation and translation: always for the plan direction; for a
    /// frame, finite unit axes that are perpendicular.
    fn is_rigid(self) -> bool {
        match self {
            Self::Plan => true,
            Self::Frame { origin, right, up } => {
                let unit = |v: Vector3| (v.norm() - 1.0).abs() < 1e-9;
                origin.coords.iter().all(|c| c.is_finite())
                    && unit(right)
                    && unit(up)
                    && right.dot(&up).abs() < 1e-9
            }
        }
    }
}

/// The depth ranges of a plan view, as heights in view space.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct PlanRange {
    /// The height of the cut plane.
    pub cut: f64,
    /// The view depth: nothing below this height is drawn. Below `cut`.
    pub bottom: f64,
    /// The top of the overhead range, above `cut`; `None` draws nothing above the cut.
    pub top: Option<f64>,
}

impl PlanRange {
    /// The ranges with the cut at `cut`, the view depth at `bottom` and the overhead range up to
    /// `top`.
    #[must_use]
    pub fn new(cut: f64, bottom: f64, top: Option<f64>) -> Self {
        Self { cut, bottom, top }
    }
}

/// The detail level passed to the view source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
#[non_exhaustive]
pub enum DetailLevel {
    /// Simplified geometry.
    Coarse,
    /// The usual geometry.
    #[default]
    Medium,
    /// Full geometry.
    Fine,
}

/// The bounds of one view build. Exceeding one fails with `view.limit.exceeded`, whose argument
/// `limit` names it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ViewLimits {
    /// Objects in scope (`objects`).
    pub max_objects: usize,
    /// Triangles of all objects in range (`triangles`).
    pub max_triangles: usize,
    /// Edges of all objects in range, feature and section edges (`edges`).
    pub max_edges: usize,
    /// Edge-against-occluder and edge-against-crop tests (`occlusion_tests`).
    pub max_occlusion_tests: u64,
    /// Primitives of the result (`primitives`).
    pub max_primitives: usize,
    /// Curves entering region unions when cut regions are joined (`join`).
    pub max_join_curves: u64,
    /// Lines and marks of one region's fill pattern; beyond it the region is filled solid.
    pub max_pattern_marks: usize,
    /// The largest absolute view-space coordinate of an object; an object beyond it is left out
    /// with the diagnostic `view.geometry.out_of_range`.
    pub max_coordinate: f64,
}

impl ViewLimits {
    /// The default bounds.
    pub const DEFAULT: Self = Self {
        max_objects: 1_000_000,
        max_triangles: 20_000_000,
        max_edges: 20_000_000,
        max_occlusion_tests: 50_000_000,
        max_primitives: 20_000_000,
        max_join_curves: 2_000_000,
        max_pattern_marks: 1_000_000,
        max_coordinate: 1.0e7,
    };
}

impl Default for ViewLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A view definition: direction, ranges, crop, detail level, crease angle, scale, overrides and
/// bounds.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ViewDef {
    /// The view direction.
    pub direction: ViewDirection,
    /// The cut plane and the depth ranges.
    pub range: PlanRange,
    /// The crop boundary in view coordinates; `None` draws everything.
    pub crop: Option<Region>,
    /// The detail level passed to the source.
    pub detail: DetailLevel,
    /// Edges between faces whose normals differ by more than this angle, in radians, are
    /// drawn; in [0, π].
    pub crease_angle: f64,
    /// Model meters per sheet meter (100 for 1:100); finite and positive.
    pub scale: f64,
    /// Graphic overrides by object and category.
    pub overrides: Overrides,
    /// The bounds of the build.
    pub limits: ViewLimits,
}

impl ViewDef {
    /// A plan view with `range`, no crop, medium detail, a crease angle of 30 degrees, scale
    /// 1:100, no overrides and the default bounds.
    #[must_use]
    pub fn plan(range: PlanRange) -> Self {
        Self {
            direction: ViewDirection::Plan,
            range,
            crop: None,
            detail: DetailLevel::Medium,
            crease_angle: std::f64::consts::FRAC_PI_6,
            scale: 100.0,
            overrides: Overrides::default(),
            limits: ViewLimits::DEFAULT,
        }
    }

    /// The chord a source should tessellate curved geometry with so the drawing stays within
    /// [`SHEET_ERROR`] on the sheet.
    #[must_use]
    pub fn chord(&self) -> f64 {
        SHEET_ERROR * self.scale
    }

    /// Checks the definition.
    ///
    /// # Errors
    ///
    /// `view.definition.invalid` with the argument `parameter` for a direction frame whose axes
    /// are not perpendicular unit vectors, a tolerance with a linear value, chord or angle that is
    /// not finite and positive or an angular value that is not finite and not negative, ranges that are not finite or not ordered `bottom < cut < top`, a
    /// crease angle outside [0, π], a scale that is not finite and positive, or a coordinate
    /// bound that is not finite and positive.
    pub fn validate(&self, tolerance: &Tolerance) -> Result<()> {
        let invalid = |name: &'static str| {
            Err(Error::new(codes::DEFINITION_INVALID).with_arg("parameter", name))
        };
        let positive = |v: f64| v.is_finite() && v > 0.0;
        let valid_tolerance = positive(tolerance.linear)
            && tolerance.angular.is_finite()
            && tolerance.angular >= 0.0
            && positive(tolerance.tessellation_chord)
            && positive(tolerance.tessellation_angle);
        if !valid_tolerance {
            return invalid("tolerance");
        }
        let PlanRange { cut, bottom, top } = self.range;
        if !(cut.is_finite() && bottom.is_finite() && bottom < cut) {
            return invalid("range");
        }
        if let Some(top) = top
            && !(top.is_finite() && top > cut)
        {
            return invalid("range");
        }
        if !self.direction.is_rigid() {
            return invalid("direction");
        }
        if !(0.0..=std::f64::consts::PI).contains(&self.crease_angle) {
            return invalid("crease_angle");
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            return invalid("scale");
        }
        let bound = self.limits.max_coordinate;
        if !(bound.is_finite() && bound > 0.0) {
            return invalid("max_coordinate");
        }
        Ok(())
    }
}

/// The rectangular crop boundary with corners `a` and `b`.
///
/// # Errors
///
/// `view.definition.invalid` (`parameter` `crop`) when the corners are not finite or the
/// rectangle has no area at the linear tolerance; the errors of [`arc_geom::Loop2::new`].
pub fn crop_rectangle(a: Point2, b: Point2, tolerance: &Tolerance) -> Result<Region> {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    let sized = x1 - x0 > tolerance.linear && y1 - y0 > tolerance.linear;
    if !(sized && [x0, x1, y0, y1].iter().all(|v| v.is_finite())) {
        return Err(Error::new(codes::DEFINITION_INVALID).with_arg("parameter", "crop"));
    }
    crop_polygon(
        &[
            Point2::new(x0, y0),
            Point2::new(x1, y0),
            Point2::new(x1, y1),
            Point2::new(x0, y1),
        ],
        tolerance,
    )
}

/// The polygonal crop boundary through `points`, in order.
///
/// # Errors
///
/// The errors of [`Segment2::new`] for coincident consecutive points and of
/// [`arc_geom::Loop2::new`] and [`Region::new`].
pub fn crop_polygon(points: &[Point2], tolerance: &Tolerance) -> Result<Region> {
    let curves = points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| Segment2::new(*a, *b).map(Curve2::Segment))
        .collect::<Result<Vec<_>>>()?;
    Region::new(vec![RegionPart::new(
        arc_geom::Loop2::new(curves, tolerance)?,
        Vec::new(),
    )])
}
