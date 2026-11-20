//! Object snaps: the candidate points a cursor can snap to, computed headless from a document
//! snapshot.
//!
//! [`snap_candidates`] collects the curves and points that draw in a space (through
//! [`crate::display::entity_list`], so block references are followed through their transform),
//! keeps those near the cursor, and computes the snap points of the enabled [`SnapKind`]s. The
//! constraint functions of [`constrain`] (ortho, polar, snap step, tracking and their precedence)
//! are pure functions over points and frames; the host passes their results back in.
//!
//! [`snap_candidates`] rebuilds the display lists of the space on every call. A host that queries
//! on every cursor move builds a [`SnapIndex`] once per document revision and calls
//! [`snap_candidates_indexed`]; both return the same candidates for the same query.

mod constrain;
mod geometry;

pub use constrain::{
    Constrained, ConstraintContext, ConstraintSource, Frame, Grid, MAX_ACQUIRED, MAX_POLAR_ANGLES,
    Polar, PolarSettings, SnapStep, TrackPath, TrackPathKind, TrackingHit, constrain, from_offset,
    grid_points, mid_between, ortho, polar, snap_step, track,
};

use std::cmp::Ordering;
use std::fmt;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_doc::{Snapshot, Value};
use arc_geom::{Aabb2, Curve2, PolylineSegment, SpatialIndex2};

use crate::display::{self, DisplayOptions, Primitive, Shape};
use crate::error::codes;
use crate::text::{RICH_TEXT, TEXT};
use crate::types::{
    ARC, BLOCK_REF, CIRCLE, CONSTRUCTION_LINE, ELLIPSE, ELLIPTICAL_ARC, LINE, POINT, POLYLINE, RAY,
    SPLINE, layer,
};
use crate::validate::DEFAULT_MAX_NESTING;

/// The most candidate pairs of one query whose intersections are computed. Pairs are counted
/// among the curves near the cursor; more fail with `draft.snap.limit_exceeded`.
pub const MAX_INTERSECTION_PAIRS: usize = 1 << 16;

/// What a candidate point is: the snap kinds, in the order that ranks them.
///
/// The names are those of the snap markers of the shared UI kit, extended by the kinds that have
/// no marker there yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SnapKind {
    /// An end of a line, ray, arc, spline or polyline segment.
    Endpoint,
    /// The middle of a line, arc or polyline segment (parameter midpoint for elliptical arcs).
    Midpoint,
    /// The center of a circle, arc, ellipse or elliptical arc, or of a polyline arc segment.
    Center,
    /// The area centroid of a closed polyline or closed spline.
    GeometricCenter,
    /// A point object.
    Node,
    /// The insertion point of a block reference or a text entity at the top of a space.
    Insertion,
    /// A 0, 90, 180 or 270 degree point of a circle, arc or ellipse in the UCS.
    Quadrant,
    /// Where two curves meet. With [`SnapCandidate::extended`] set, where their natural
    /// extensions meet outside the curves.
    Intersection,
    /// A point on the extension path of an end, hovered by the cursor.
    Extension,
    /// A point on the path through the previous point parallel to a line the cursor hovers.
    Parallel,
    /// The foot of the perpendicular from the previous point.
    Perpendicular,
    /// A point where a curve touches a line through the previous point.
    Tangent,
    /// The closest point of a curve to the cursor.
    Nearest,
}

impl SnapKind {
    /// Every kind, in ranking order.
    pub const ALL: [Self; 13] = [
        Self::Endpoint,
        Self::Midpoint,
        Self::Center,
        Self::GeometricCenter,
        Self::Node,
        Self::Insertion,
        Self::Quadrant,
        Self::Intersection,
        Self::Extension,
        Self::Parallel,
        Self::Perpendicular,
        Self::Tangent,
        Self::Nearest,
    ];

    const fn bit(self) -> u16 {
        1 << (self as u16)
    }
}

/// A set of enabled snap kinds, the running set of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SnapModes(u16);

impl SnapModes {
    /// No kind.
    pub const NONE: Self = Self(0);
    /// Every kind.
    pub const ALL: Self = Self(0x1FFF);

    /// The set of `kinds`.
    #[must_use]
    pub fn of(kinds: &[SnapKind]) -> Self {
        kinds.iter().fold(Self::NONE, |set, kind| set.with(*kind))
    }

    /// Whether `kind` is in the set.
    #[must_use]
    pub fn contains(self, kind: SnapKind) -> bool {
        self.0 & kind.bit() != 0
    }

    /// The set with `kind` added.
    #[must_use]
    pub fn with(self, kind: SnapKind) -> Self {
        Self(self.0 | kind.bit())
    }

    /// The set with `kind` removed.
    #[must_use]
    pub fn without(self, kind: SnapKind) -> Self {
        Self(self.0 & !kind.bit())
    }
}

/// The entity a candidate came from, and the block references it is inserted through.
///
/// `entity` is the drawing entity that draws the geometry. `block_path` lists the block
/// references that insert it, outermost first; it is empty for an entity of the space itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapSource {
    /// The drawing entity.
    pub entity: ObjectId,
    /// The block references the entity is inserted through, outermost first.
    pub block_path: Vec<ObjectId>,
}

/// A point the cursor can snap to.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapCandidate {
    /// The kind of the snap.
    pub kind: SnapKind,
    /// The snapped point, in the coordinates of the space.
    pub point: Point2,
    /// The distance from the point to the cursor.
    pub distance: f64,
    /// Whether an intersection is an extended one (only for [`SnapKind::Intersection`]).
    pub extended: bool,
    /// The entities the point comes from: one, or two for an intersection.
    pub sources: Vec<SnapSource>,
}

/// The parameters of one [`snap_candidates`] query.
///
/// Every length is in drawing units. `from` is the previous point of the running command; the
/// perpendicular, tangent and parallel snaps need it and produce nothing without it. `ucs_x` is
/// the x axis of the current UCS, which defines the quadrant points.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapQuery {
    /// The cursor position.
    pub cursor: Point2,
    /// The pick aperture: the largest distance from the cursor to a candidate. Zero or less
    /// yields no candidates.
    pub aperture: f64,
    /// The enabled kinds.
    pub modes: SnapModes,
    /// The previous point of the running command.
    pub from: Option<Point2>,
    /// The x axis of the current UCS; need not be unit length, must not be zero.
    pub ucs_x: Vector2,
    /// The tolerances of the computations.
    pub tolerance: Tolerance,
    /// The bounds: `record_count` is the most top-level entities examined, `array_length` the
    /// most results of one intersection.
    pub limits: Limits,
    /// The deepest nesting of block references followed.
    pub max_depth: u32,
}

impl SnapQuery {
    /// A query with no previous point, the UCS of the world, the default limits and the default
    /// nesting bound.
    #[must_use]
    pub fn new(cursor: Point2, aperture: f64, modes: SnapModes, tolerance: Tolerance) -> Self {
        Self {
            cursor,
            aperture,
            modes,
            from: None,
            ucs_x: Vector2::x(),
            tolerance,
            limits: Limits::DEFAULT,
            max_depth: DEFAULT_MAX_NESTING,
        }
    }
}

/// The snap candidates of `space` for `query`, ranked: by kind (in [`SnapKind::ALL`] order),
/// then actual before extended intersections, then by distance to the cursor, then by the
/// identifiers of the entities, then by coordinates. Equal candidates are reported once.
///
/// A zero or negative aperture and an empty running set yield no candidates.
///
/// # Errors
///
/// `geom.input.not_finite` for a non-finite cursor, aperture, `from` or `ucs_x` component;
/// `geom.tolerance.invalid` for a tolerance that is not finite and positive (linear) or not
/// finite and non-negative (angular); `draft.geometry.invalid` for a zero `ucs_x`;
/// `draft.snap.limit_exceeded` when the space holds more top-level entities than
/// `limits.record_count`, or the curves near the cursor form more pairs than
/// [`MAX_INTERSECTION_PAIRS`]; `draft.block.nesting_too_deep` and the other display errors of a
/// block reference that cannot be drawn.
pub fn snap_candidates(
    snapshot: &Snapshot,
    space: ObjectId,
    query: &SnapQuery,
) -> Result<Vec<SnapCandidate>> {
    check_query(query)?;
    if query.aperture <= 0.0 || query.modes == SnapModes::NONE {
        return Ok(Vec::new());
    }
    let scene = Scene::collect(snapshot, space, &BuildParams::of(query))?;
    query_scene(&scene, query)
}

/// The geometry of one space at one document revision, kept by a host between snap queries.
///
/// The index holds what [`snap_candidates`] builds for every call: the entities of the space
/// through their display lists, the layer states applied (hidden entities and layers are left
/// out), and a spatial index over the bounded curves. It is valid for the revision it was built
/// from and for the space it names; [`snap_candidates_indexed`] refuses a query against another
/// revision with `draft.snap.index_stale`, and the host rebuilds the index. The revision number
/// identifies a state only within one document, so a host pairs an index with the document it
/// built it from.
///
/// The index is bounded by the record count of the query it was built with: it holds at most
/// `limits.record_count` curves, points, insertion points and closed curves together, and a
/// build that needs more fails with `draft.snap.limit_exceeded`.
pub struct SnapIndex {
    revision: u64,
    space: ObjectId,
    params: BuildParams,
    scene: Scene,
}

impl SnapIndex {
    /// Builds the index of `space` in `snapshot`. The tolerance, the record bound and the nesting
    /// bound of `query` are fixed for the index; the other fields of a query may differ on each
    /// call.
    ///
    /// # Errors
    ///
    /// The errors of [`snap_candidates`] for a space that cannot be read, and
    /// `draft.snap.limit_exceeded` when the index would hold more items than `limits.record_count`.
    pub fn build(snapshot: &Snapshot, space: ObjectId, query: &SnapQuery) -> Result<Self> {
        check_query(query)?;
        let params = BuildParams::of(query);
        let scene = Scene::collect(snapshot, space, &params)?;
        let items = scene.items();
        if u64::try_from(items).unwrap_or(u64::MAX) > query.limits.record_count {
            return Err(limit_error(query.limits.record_count));
        }
        Ok(Self {
            revision: snapshot.revision(),
            space,
            params,
            scene,
        })
    }

    /// The document revision the index was built from.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The space the index was built for.
    #[must_use]
    pub fn space(&self) -> ObjectId {
        self.space
    }

    /// The number of stored curves, points, insertion points and closed curves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.scene.items()
    }

    /// Whether the index stores nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for SnapIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapIndex")
            .field("revision", &self.revision)
            .field("space", &self.space)
            .field("items", &self.len())
            .finish_non_exhaustive()
    }
}

/// The snap candidates of the space of `index` for `query` at the revision of `snapshot`. The
/// result is identical to [`snap_candidates`] on the same snapshot and space.
///
/// # Errors
///
/// `draft.snap.index_stale` when `snapshot` is not the revision the index was built from;
/// `draft.snap.index_mismatch` when the tolerance, the record bound or the nesting bound of
/// `query` differ from those the index was built with; the errors of [`snap_candidates`] for
/// the query's values.
pub fn snap_candidates_indexed(
    snapshot: &Snapshot,
    index: &SnapIndex,
    query: &SnapQuery,
) -> Result<Vec<SnapCandidate>> {
    check_query(query)?;
    if snapshot.revision() != index.revision {
        return Err(Error::new(codes::SNAP_INDEX_STALE)
            .with_detail("built", index.revision.to_string())
            .with_detail("current", snapshot.revision().to_string()));
    }
    if BuildParams::of(query) != index.params {
        return Err(Error::new(codes::SNAP_INDEX_MISMATCH));
    }
    if query.aperture <= 0.0 || query.modes == SnapModes::NONE {
        return Ok(Vec::new());
    }
    query_scene(&index.scene, query)
}

/// The values of a query that the scene is built with.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildParams {
    tolerance: Tolerance,
    record_count: u64,
    max_depth: u32,
}

impl BuildParams {
    fn of(query: &SnapQuery) -> Self {
        Self {
            tolerance: query.tolerance,
            record_count: query.limits.record_count,
            max_depth: query.max_depth,
        }
    }
}

/// The ranked candidates of `query` over a scene.
fn query_scene(scene: &Scene, query: &SnapQuery) -> Result<Vec<SnapCandidate>> {
    let window = Aabb2::from_corners(
        query.cursor - Vector2::repeat(query.aperture),
        query.cursor + Vector2::repeat(query.aperture),
    );
    let mut sink = Sink {
        query,
        out: Vec::new(),
    };
    for (source, point) in &scene.nodes {
        sink.offer(SnapKind::Node, *point, false, std::slice::from_ref(source));
    }
    for (source, point) in &scene.insertions {
        sink.offer(
            SnapKind::Insertion,
            *point,
            false,
            std::slice::from_ref(source),
        );
    }
    let near = scene.near(&window)?;
    for &index in &near {
        scene.entries[index].offer_all(&mut sink);
    }
    if let Some(from) = query.from {
        geometry::parallels(&mut sink, &scene.entries, from);
    }
    for (source, curve) in &scene.closed {
        if sink.wants(SnapKind::GeometricCenter) && intersects(curve, &window) {
            geometry::geometric_center(&mut sink, curve, source);
        }
    }
    if sink.wants(SnapKind::Intersection) {
        let pieces: Vec<&Entry> = near.iter().map(|&i| &scene.entries[i]).collect();
        geometry::intersections(&mut sink, &pieces, query)?;
        geometry::extended_intersections(&mut sink, &scene.entries, query)?;
    }
    let mut out = sink.out;
    out.sort_by(compare);
    out.dedup_by(|later, earlier| {
        later.kind == earlier.kind
            && later.extended == earlier.extended
            && later.sources == earlier.sources
            && (later.point - earlier.point).norm() <= query.tolerance.linear
    });
    Ok(out)
}

/// The output of one query, with the offers of its geometry functions.
struct Sink<'a> {
    query: &'a SnapQuery,
    out: Vec<SnapCandidate>,
}

impl Sink<'_> {
    /// Whether the kind is enabled.
    fn wants(&self, kind: SnapKind) -> bool {
        self.query.modes.contains(kind)
    }

    /// Adds a candidate when the kind is enabled, the point is finite and it lies within the
    /// aperture of the cursor.
    fn offer(&mut self, kind: SnapKind, point: Point2, extended: bool, sources: &[SnapSource]) {
        if !self.wants(kind) || !is_finite(&point) {
            return;
        }
        let distance = (point - self.query.cursor).norm();
        if distance > self.query.aperture {
            return;
        }
        self.out.push(SnapCandidate {
            kind,
            point,
            distance,
            extended,
            sources: sources.to_vec(),
        });
    }
}

/// A curve that draws in the space, with the entity it comes from. Polyline segments are entries
/// of their own, sharing the source of their polyline.
struct Entry {
    source: SnapSource,
    curve: Curve2,
}

impl Entry {
    /// Offers every point of the entry's curve that the enabled kinds compute.
    fn offer_all(&self, sink: &mut Sink<'_>) {
        let (curve, source) = (&self.curve, &self.source);
        let tolerance = sink.query.tolerance;
        let cursor = sink.query.cursor;
        let from = sink.query.from;
        let ucs = sink.query.ucs_x;
        geometry::endpoints(sink, curve, source);
        geometry::midpoint(sink, curve, source);
        geometry::centers(sink, curve, source);
        geometry::quadrants(sink, curve, source, ucs, &tolerance);
        geometry::nearest(sink, curve, source, cursor, &tolerance);
        geometry::extension(sink, curve, source);
        if let Some(from) = from {
            geometry::perpendicular(sink, curve, source, from, &tolerance);
            geometry::tangent(sink, curve, source, from, &tolerance);
        }
    }
}

/// The geometry the query reads: the drawing entities of the space, their points and their
/// curves, with a spatial index over the bounded curves.
#[derive(Default)]
struct Scene {
    entries: Vec<Entry>,
    /// The entries whose boxes are unbounded: rays and lines, which are always near.
    unbounded: Vec<usize>,
    index: Option<SpatialIndex2<usize>>,
    nodes: Vec<(SnapSource, Point2)>,
    insertions: Vec<(SnapSource, Point2)>,
    /// Closed polylines and closed splines, whole, for the geometric center.
    closed: Vec<(SnapSource, Curve2)>,
}

impl Scene {
    fn items(&self) -> usize {
        self.entries.len() + self.nodes.len() + self.insertions.len() + self.closed.len()
    }

    fn collect(snapshot: &Snapshot, space: ObjectId, params: &BuildParams) -> Result<Self> {
        let members = display::members(snapshot, space);
        let count = u64::try_from(members.len()).unwrap_or(u64::MAX);
        if count > params.record_count {
            return Err(limit_error(params.record_count));
        }
        let options = DisplayOptions {
            tolerance: params.tolerance,
            max_nesting: params.max_depth,
            ..DisplayOptions::default()
        };
        let mut scene = Self::default();
        for member in members {
            let Some(key) = type_key(snapshot, member) else {
                continue;
            };
            if matches!(key.as_str(), BLOCK_REF | TEXT | RICH_TEXT) {
                scene.add_insertion(snapshot, member);
            }
            if !is_drawn(&key) {
                continue;
            }
            let list = display::entity_list(snapshot, member, &options)?;
            for primitive in list.primitives {
                scene.add_primitive(snapshot, primitive);
            }
        }
        scene.finish()?;
        Ok(scene)
    }

    /// The insertion point of a top-level block reference or text entity.
    fn add_insertion(&mut self, snapshot: &Snapshot, member: ObjectId) {
        if hidden(snapshot, member) {
            return;
        }
        let Some(Value::Point2(point)) = field(snapshot, member, "position") else {
            return;
        };
        let source = SnapSource {
            entity: member,
            block_path: Vec::new(),
        };
        self.insertions.push((source, point));
    }

    fn add_primitive(&mut self, snapshot: &Snapshot, primitive: Primitive) {
        if primitive
            .layers
            .iter()
            .any(|layer| layer_hidden(snapshot, *layer))
        {
            return;
        }
        let drawing = primitive.path.last().copied().unwrap_or(primitive.source);
        let Some(key) = type_key(snapshot, drawing) else {
            return;
        };
        let source = SnapSource {
            entity: drawing,
            block_path: block_path(&primitive),
        };
        match primitive.shape {
            Shape::Point(point) if key == POINT => self.nodes.push((source, point)),
            Shape::Stroke { curve, .. } if is_curve(&key) => self.push_curve(source, curve),
            _ => {}
        }
    }

    fn push_curve(&mut self, source: SnapSource, curve: Curve2) {
        match &curve {
            Curve2::Polyline(polyline) => {
                if polyline.is_closed() {
                    self.closed.push((source.clone(), curve.clone()));
                }
                for segment in polyline.segments() {
                    let curve = match segment {
                        PolylineSegment::Line(line) => Curve2::Segment(line),
                        PolylineSegment::Arc(arc) => Curve2::Arc(arc),
                    };
                    self.entries.push(Entry {
                        source: source.clone(),
                        curve,
                    });
                }
            }
            Curve2::Nurbs(_) => {
                if curve.is_closed() {
                    self.closed.push((source.clone(), curve.clone()));
                }
                self.entries.push(Entry { source, curve });
            }
            _ => self.entries.push(Entry { source, curve }),
        }
    }

    fn finish(&mut self) -> Result<()> {
        let mut boxed = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            match entry.curve.bounding_box() {
                Some(bounds) => boxed.push((index, bounds)),
                None => self.unbounded.push(index),
            }
        }
        let index = SpatialIndex2::from_entries(boxed)?;
        self.index = Some(index);
        Ok(())
    }

    /// The entries whose boxes meet `window`, and every unbounded entry, ascending.
    fn near(&self, window: &Aabb2) -> Result<Vec<usize>> {
        let mut hits = match &self.index {
            Some(index) => index.crossing(window)?,
            None => Vec::new(),
        };
        hits.extend(self.unbounded.iter().copied());
        hits.sort_unstable();
        hits.dedup();
        Ok(hits)
    }
}

fn block_path(primitive: &Primitive) -> Vec<ObjectId> {
    match primitive.path.split_last() {
        None => Vec::new(),
        Some((_, inserting)) => std::iter::once(primitive.source)
            .chain(inserting.iter().copied())
            .collect(),
    }
}

/// The curve entity types whose stroke primitives are snap curves.
fn is_curve(key: &str) -> bool {
    matches!(
        key,
        LINE | RAY
            | CONSTRUCTION_LINE
            | CIRCLE
            | ARC
            | ELLIPSE
            | ELLIPTICAL_ARC
            | POLYLINE
            | SPLINE
    )
}

/// The entity types whose display lists the scene reads.
fn is_drawn(key: &str) -> bool {
    is_curve(key) || key == POINT || key == BLOCK_REF || key == crate::array::ARRAY
}

fn intersects(curve: &Curve2, window: &Aabb2) -> bool {
    curve
        .bounding_box()
        .is_some_and(|bounds| bounds.intersects(window))
}

fn type_key(snapshot: &Snapshot, id: ObjectId) -> Option<String> {
    let handle = snapshot.resolve(id).ok()?;
    snapshot.type_key(handle).map(|key| key.as_str().to_owned())
}

/// The value of a field of an object, by its declared name.
fn field(snapshot: &Snapshot, id: ObjectId, name: &str) -> Option<Value> {
    let handle = snapshot.resolve(id).ok()?;
    let key = snapshot.type_key(handle)?.as_str().to_owned();
    let position = snapshot
        .registry()
        .get(&key)?
        .schema
        .fields
        .iter()
        .position(|f| f.name == name)?;
    let object = snapshot.get(handle).ok()?;
    object.values.get(position).cloned().flatten()
}

/// Whether the entity is turned off, or hidden by its own visibility flag.
fn hidden(snapshot: &Snapshot, id: ObjectId) -> bool {
    matches!(field(snapshot, id, "visible"), Some(Value::Bool(false)))
        || match field(snapshot, id, "layer") {
            Some(Value::Reference(layer)) => layer_hidden(snapshot, layer),
            _ => false,
        }
}

/// Whether a layer is off or frozen. A locked layer still snaps.
fn layer_hidden(snapshot: &Snapshot, layer_id: ObjectId) -> bool {
    let Ok(handle) = snapshot.resolve(layer_id) else {
        return false;
    };
    let Ok(object) = snapshot.get(handle) else {
        return false;
    };
    matches!(object.values.get(layer::ON), Some(Some(Value::Bool(false))))
        || matches!(
            object.values.get(layer::FROZEN),
            Some(Some(Value::Bool(true)))
        )
}

fn is_finite(point: &Point2) -> bool {
    point.coords.iter().all(|c| c.is_finite())
}

fn check_query(query: &SnapQuery) -> Result<()> {
    let finite = is_finite(&query.cursor)
        && query.aperture.is_finite()
        && query.from.as_ref().is_none_or(is_finite)
        && query.ucs_x.iter().all(|c| c.is_finite());
    if !finite {
        return Err(Error::new(arc_geom::error::codes::INPUT_NOT_FINITE));
    }
    if query.ucs_x.norm() == 0.0 {
        return Err(Error::new(codes::GEOMETRY_INVALID).with_detail("field", "ucs_x"));
    }
    let tolerance = &query.tolerance;
    let linear_ok = tolerance.linear.is_finite() && tolerance.linear > 0.0;
    let angular_ok = tolerance.angular.is_finite() && tolerance.angular >= 0.0;
    if !(linear_ok && angular_ok) {
        return Err(Error::new(arc_geom::error::codes::TOLERANCE_INVALID));
    }
    Ok(())
}

/// The error of a query that exceeds a bound.
fn limit_error(limit: u64) -> Error {
    Error::new(codes::SNAP_LIMIT_EXCEEDED).with_detail("limit", limit.to_string())
}

/// The ranking: kind, actual before extended, distance, entity identifiers, coordinates.
fn compare(a: &SnapCandidate, b: &SnapCandidate) -> Ordering {
    a.kind
        .cmp(&b.kind)
        .then(a.extended.cmp(&b.extended))
        .then(a.distance.total_cmp(&b.distance))
        .then_with(|| {
            a.sources
                .iter()
                .map(|s| s.entity)
                .cmp(b.sources.iter().map(|s| s.entity))
        })
        .then_with(|| a.point.x.total_cmp(&b.point.x))
        .then_with(|| a.point.y.total_cmp(&b.point.y))
}
