//! Hatches and fills (CORE-DRAFT-003): the `core.draft.hatch` entity, its boundary resolution
//! and associativity, and the generation of pattern lines.
//!
//! A hatch stores its boundary as loops (closed circles, ellipses and closed bulge polylines
//! in the `arc.geom` encoding) and, optionally, references to the boundary objects it follows.
//! The derived node [`RESOLVED`] gives the [`Region`] to fill: recomputed from the referenced
//! objects while all of them exist, otherwise built from the stored loops. The display list
//! reads that node. [`generate`] is a pure function from a region and pattern lines to the
//! marks of a pattern. The rules are stated in the crate specification, section "Hatches as
//! implemented".

use std::sync::Arc;

use arc_base::math::{Point2, Vector2};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{
    Bound, Cardinality, ColorValue, FieldDecl, Finding, GeometryBytes, OnDelete, Severity,
    Snapshot, Strength, TypeKey, TypeValidator, Value, ValueType,
};
use arc_geom::edit::{Composite2, join};
use arc_geom::encoding::GeomCodec;
use arc_geom::intersect::{Intersection, intersect};
use arc_geom::region::{Containment, Loop2, RegionPart};
use arc_geom::trace::{IslandMode, trace_region};
use arc_geom::{Circle2, Curve2, Line2, Region, Segment2, build_region};

use crate::appearance::{Source, Sourced};
use crate::dimension::clearing_reference;
use crate::display::{DisplayOptions, Primitive, Reader, Shape, read_enum, read_number};
use crate::error::codes;
use crate::hatch_pattern::{HatchPattern, MAX_DASHES, MAX_LINES, PatternLine};
use crate::types::{
    ARC, CIRCLE, ELLIPSE, ELLIPTICAL_ARC, LINE, POLYLINE, common_findings, finding, real,
    reference, variants, with_kind,
};

/// A hatch entity.
pub const HATCH: &str = "core.draft.hatch";
/// A named hatch pattern in the document's style tables.
pub const HATCH_PATTERN: &str = "core.draft.hatch_pattern";
/// The derived key of a hatch's resolved boundary.
pub const RESOLVED: &str = "core.draft.hatch.resolved";

/// The most stored loops of a hatch.
pub const MAX_LOOPS: u64 = 4096;
/// The most boundary objects of a hatch.
pub const MAX_OBJECTS: u64 = 4096;

/// Field indices of a hatch.
pub mod field {
    use crate::types::entity::KIND as K;
    /// `kind`: `pattern`, `solid` or `gradient`.
    pub const KIND: usize = K;
    /// `loops`: the stored boundary.
    pub const LOOPS: usize = K + 1;
    /// `island`: `alternating`, `outermost` or `ignore`.
    pub const ISLAND: usize = K + 2;
    /// `boundary_objects`: records of one optional reference each.
    pub const OBJECTS: usize = K + 3;
    /// `pick`: the point a traced boundary was picked at.
    pub const PICK: usize = K + 4;
    /// `pattern`: the pattern object.
    pub const PATTERN: usize = K + 5;
    /// `inline`: the line families of a pattern defined in the hatch.
    pub const INLINE: usize = K + 6;
    /// `scale`.
    pub const SCALE: usize = K + 7;
    /// `angle` in radians.
    pub const ANGLE: usize = K + 8;
    /// `origin`: the fixed pattern origin.
    pub const ORIGIN: usize = K + 9;
    /// `gradient_kind`: `linear` or `radial`.
    pub const GRADIENT_KIND: usize = K + 10;
    /// `gradient_angle` in radians.
    pub const GRADIENT_ANGLE: usize = K + 11;
    /// `gradient_color1`.
    pub const GRADIENT_COLOR1: usize = K + 12;
    /// `gradient_color2`.
    pub const GRADIENT_COLOR2: usize = K + 13;
}

/// The kind of a hatch, with its enum code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Pattern lines.
    Pattern = 0,
    /// A solid fill.
    Solid = 1,
    /// A gradient of two colors.
    Gradient = 2,
}

/// The kind names in code order.
pub const KIND_NAMES: [&str; 3] = ["pattern", "solid", "gradient"];
/// The island mode names in code order.
pub const ISLAND_NAMES: [&str; 3] = ["alternating", "outermost", "ignore"];
/// The gradient kind names in code order.
pub const GRADIENT_NAMES: [&str; 2] = ["linear", "radial"];

/// The island mode of an enum code; unknown codes read as alternating.
#[must_use]
pub fn island_of(code: u32) -> IslandMode {
    match code {
        1 => IslandMode::Outermost,
        2 => IslandMode::Ignore,
        _ => IslandMode::Alternating,
    }
}

/// The enum code of an island mode.
#[must_use]
pub fn island_code(mode: IslandMode) -> u32 {
    match mode {
        IslandMode::Alternating => 0,
        IslandMode::Outermost => 1,
        IslandMode::Ignore => 2,
    }
}

fn geometry_type() -> ValueType {
    let encoding = arc_geom::GeomEncoding::CURRENT;
    ValueType::Geometry(arc_doc::GeometryEncoding {
        name: encoding.name.to_owned(),
        version: u32::from(encoding.version),
    })
}

fn line_type() -> ValueType {
    ValueType::Record(vec![
        real("angle", 0.0, -360.0, 360.0),
        FieldDecl::new("origin", ValueType::Point2)
            .with_default(Value::Point2(Point2::new(0.0, 0.0))),
        real("shift", 0.0, -1e6, 1e6),
        real("spacing", 1.0, -1e6, 1e6),
        FieldDecl::new("dashes", ValueType::List(Box::new(ValueType::F64)))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length {
                max: MAX_DASHES as u64,
            }),
    ])
}

fn color(name: &str, rgba: arc_base::Color) -> FieldDecl {
    FieldDecl::new(name, ValueType::Color)
        .with_default(Value::Color(Box::new(ColorValue { rgba, source: None })))
}

fn hatch_fields() -> Vec<FieldDecl> {
    let object = ValueType::Record(vec![
        FieldDecl::new("object", clearing_reference(&[], Cardinality::Optional)).optional(),
    ]);
    with_kind(vec![
        FieldDecl::new("kind", variants(&KIND_NAMES)).with_default(Value::Enum(0)),
        FieldDecl::new("loops", ValueType::List(Box::new(geometry_type())))
            .with_bound(Bound::Length { max: MAX_LOOPS }),
        FieldDecl::new("island", variants(&ISLAND_NAMES)).with_default(Value::Enum(0)),
        FieldDecl::new("boundary_objects", ValueType::List(Box::new(object)))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_OBJECTS }),
        FieldDecl::new("pick", ValueType::Point2).optional(),
        FieldDecl::new(
            "pattern",
            reference(HATCH_PATTERN, Strength::Associative, OnDelete::Block, true),
        )
        .optional(),
        FieldDecl::new("inline", ValueType::List(Box::new(line_type())))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length {
                max: MAX_LINES as u64,
            }),
        real("scale", 1.0, f64::MIN_POSITIVE, f64::MAX),
        real("angle", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("origin", ValueType::Point2)
            .with_default(Value::Point2(Point2::new(0.0, 0.0))),
        FieldDecl::new("gradient_kind", variants(&GRADIENT_NAMES)).with_default(Value::Enum(0)),
        real("gradient_angle", 0.0, f64::MIN, f64::MAX),
        color("gradient_color1", arc_base::Color::new(0.0, 0.0, 0.0, 1.0)),
        color("gradient_color2", arc_base::Color::WHITE),
    ])
}

fn pattern_fields() -> Vec<FieldDecl> {
    vec![
        crate::types::name_field(),
        crate::types::text("description", crate::hatch_pattern::MAX_TEXT_BYTES as u64),
        FieldDecl::new("lines", ValueType::List(Box::new(line_type()))).with_bound(Bound::Length {
            max: MAX_LINES as u64,
        }),
    ]
}

/// The inline lines or the lines of a pattern object, as stored.
fn lines_of(value: Option<&Value>) -> Vec<PatternLine> {
    match value {
        Some(Value::List(items)) => items.iter().filter_map(PatternLine::from_value).collect(),
        _ => Vec::new(),
    }
}

fn stored_len(value: Option<&Option<Value>>) -> usize {
    match value {
        Some(Some(Value::List(items))) => items.len(),
        _ => 0,
    }
}

/// Whether `curve` is a closed shape a loop may store.
pub(crate) fn closed_shape(curve: &Curve2) -> bool {
    match curve {
        Curve2::Circle(_) | Curve2::Ellipse(_) => true,
        Curve2::Polyline(p) => p.is_closed(),
        _ => false,
    }
}

fn hatch_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    let invalid = || finding(Error::new(codes::HATCH_BOUNDARY_INVALID));
    match values.get(field::LOOPS) {
        Some(Some(Value::List(items))) if !items.is_empty() => {
            for item in items {
                let ok = matches!(item, Value::Geometry(b)
                    if Curve2::decode(&b.0, &Limits::DEFAULT).is_ok_and(|c| closed_shape(&c)));
                if !ok {
                    findings.push(invalid());
                }
            }
        }
        _ => findings.push(invalid()),
    }
    let inline = match values.get(field::INLINE) {
        Some(Some(Value::List(items))) => items.as_slice(),
        _ => &[],
    };
    for item in inline {
        match PatternLine::from_value(item) {
            Some(line) => {
                if let Err(e) = line.validate() {
                    findings.push(finding(e));
                }
            }
            None => findings.push(finding(Error::new(codes::PATTERN_LINE_INVALID))),
        }
    }
    let pattern = matches!(values.get(field::KIND), Some(Some(Value::Enum(0))));
    let has_ref = matches!(values.get(field::PATTERN), Some(Some(Value::Reference(_))));
    if pattern && !has_ref && inline.is_empty() {
        findings.push(finding(Error::new(codes::PATTERN_EMPTY)));
    }
    findings
}

fn pattern_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let lines = match values.get(2) {
        Some(Some(Value::List(items))) => items.as_slice(),
        _ => &[],
    };
    if lines.is_empty() {
        return vec![finding(Error::new(codes::PATTERN_EMPTY))];
    }
    lines
        .iter()
        .filter_map(|v| match PatternLine::from_value(v) {
            Some(line) => line.validate().err().map(finding),
            None => Some(finding(Error::new(codes::PATTERN_LINE_INVALID))),
        })
        .collect()
}

/// The hatch and hatch pattern declarations.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (HATCH, hatch_fields(), vec![hatch_findings]),
        (HATCH_PATTERN, pattern_fields(), vec![pattern_findings]),
    ]
}

// --- Boundary resolution ---

/// The boundary of a hatch: the result of its [`RESOLVED`] node.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The region to fill.
    pub region: Region,
    /// Whether it was recomputed from the boundary objects, not read from the stored loops.
    pub associated: bool,
}

impl Derived for Resolution {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text, as the display lists do.
        content_hash(&format!("{self:?}"))
    }
}

fn boundary_invalid(id: Option<ObjectId>) -> Error {
    let e = Error::new(codes::HATCH_BOUNDARY_INVALID);
    match id {
        Some(id) => e.with_detail("id", id.to_string()),
        None => e,
    }
}

/// The point of the middle of the first curve of `lp`.
fn probe(lp: &Loop2) -> Option<Point2> {
    let curve = lp.curves().first()?;
    let (a, b) = curve.parameter_range();
    Some(curve.point_at(0.5 * (a + b)))
}

/// The region bounded by closed composites, with the islands handled as `island` says:
/// `Alternating` fills by nesting parity; `Outermost` keeps the outermost loops with their direct
/// holes; `Ignore` keeps the outermost loops whole.
fn region_of(composites: &[Composite2], island: IslandMode, tol: &Tolerance) -> Result<Region> {
    let region = build_region(composites, tol, &Limits::DEFAULT)?;
    if island == IslandMode::Alternating {
        return Ok(region);
    }
    let parts = region.parts();
    let mut kept = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let point = probe(part.outer()).ok_or_else(|| boundary_invalid(None))?;
        let mut nested = false;
        for (j, other) in parts.iter().enumerate() {
            if i != j && other.outer().classify(point, tol)? == Containment::Inside {
                nested = true;
                break;
            }
        }
        if nested {
            continue;
        }
        kept.push(if island == IslandMode::Ignore {
            RegionPart::new(part.outer().clone(), Vec::new())
        } else {
            part.clone()
        });
    }
    Region::new(kept)
}

fn decode_stored(value: &Value) -> Result<Curve2> {
    match value {
        Value::Geometry(b) => Curve2::decode(&b.0, &Limits::DEFAULT)
            .ok()
            .filter(closed_shape)
            .ok_or_else(|| boundary_invalid(None)),
        _ => Err(boundary_invalid(None)),
    }
}

pub(crate) const BOUNDARY_KINDS: [&str; 6] = [LINE, ARC, CIRCLE, ELLIPSE, ELLIPTICAL_ARC, POLYLINE];

/// The curves of the boundary objects `objects`.
pub(crate) fn object_curves(reader: &dyn Reader, objects: &[ObjectId]) -> Result<Vec<Curve2>> {
    objects
        .iter()
        .map(|id| {
            let key = reader
                .type_key(*id)
                .ok_or_else(|| boundary_invalid(Some(*id)))?;
            if !BOUNDARY_KINDS.contains(&key.as_str()) {
                return Err(boundary_invalid(Some(*id)));
            }
            crate::display::decode_curve(Some(&reader.field(*id, "geometry")?))
        })
        .collect()
}

/// The region the curves of `curves` bound: traced around `pick`, or, without one, the closed
/// chains they join into.
pub(crate) fn region_from_curves(
    curves: &[Curve2],
    pick: Option<Point2>,
    island: IslandMode,
    tol: &Tolerance,
) -> Result<Region> {
    if let Some(pick) = pick {
        return trace_region(curves, pick, island, tol, &Limits::DEFAULT);
    }
    let joined = join(curves, tol)?;
    let mut composites = Vec::new();
    for (chain, _) in joined.chains {
        if !chain.is_closed() {
            return Err(boundary_invalid(None));
        }
        composites.push(chain);
    }
    region_of(&composites, island, tol)
}

/// The boundary objects of a hatch's records, `None` for a deleted one.
pub(crate) fn objects_of(value: Option<&Value>) -> Vec<Option<ObjectId>> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .map(|item| match item {
                Value::Record(f) => match f.first() {
                    Some(Some(Value::Reference(id))) => Some(*id),
                    _ => None,
                },
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The stored record of a boundary object.
#[must_use]
pub fn object_value(id: ObjectId) -> Value {
    Value::Record(vec![Some(Value::Reference(id))])
}

/// Resolves the hatch `id` through `reader`.
pub(crate) fn resolve_with(
    reader: &dyn Reader,
    id: ObjectId,
    tol: &Tolerance,
) -> Result<Resolution> {
    let island = island_of(read_enum(reader, id, "island")?);
    let records = objects_of(reader.field(id, "boundary_objects")?.as_ref());
    let present: Vec<ObjectId> = records.iter().flatten().copied().collect();
    if !records.is_empty() && present.len() == records.len() {
        let curves = object_curves(reader, &present)?;
        let pick = match reader.field(id, "pick")? {
            Some(Value::Point2(p)) => Some(p),
            _ => None,
        };
        let region = region_from_curves(&curves, pick, island, tol)?;
        return Ok(Resolution {
            region,
            associated: true,
        });
    }
    let loops = match reader.field(id, "loops")? {
        Some(Value::List(items)) => items,
        _ => Vec::new(),
    };
    let composites = loops
        .iter()
        .map(|v| Composite2::new(vec![decode_stored(v)?], tol))
        .collect::<Result<Vec<_>>>()?;
    if composites.is_empty() {
        return Err(boundary_invalid(None));
    }
    Ok(Resolution {
        region: region_of(&composites, island, tol)?,
        associated: false,
    })
}

/// The node of the resolution of the hatch `id`.
///
/// # Errors
///
/// None for the constant key; the key check of `arc-doc` is passed on.
pub fn resolved_node(id: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(id, DerivedKey::new(RESOLVED)?))
}

/// Registers the [`RESOLVED`] evaluator of hatches with the tolerance of `options`.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when it is registered already.
pub fn register_evaluators(evaluators: &mut Evaluators, options: &DisplayOptions) -> Result<()> {
    let tol = options.tolerance;
    evaluators.register_object(
        TypeKey::new(HATCH)?,
        DerivedKey::new(RESOLVED)?,
        move |ctx: &EvalContext<'_>| {
            let id = ctx
                .object_id()
                .ok_or_else(|| Error::new(codes::OBJECT_WRONG_TYPE))?;
            let resolution = resolve_with(&crate::display::GraphReader { ctx }, id, &tol)?;
            Ok(Arc::new(resolution) as DerivedValue)
        },
    )
}

/// The resolution of the hatch `id` evaluated from `snapshot`, without the graph.
///
/// # Errors
///
/// `doc.object.not_found`; `draft.hatch.boundary_invalid`; the errors of region construction
/// and tracing of `arc-geom`.
pub fn resolve(snapshot: &Snapshot, id: ObjectId, options: &DisplayOptions) -> Result<Resolution> {
    crate::display::with_snapshot(snapshot, options, |reader| {
        resolve_with(reader, id, &options.tolerance)
    })
}

/// The stored loops of `region`: each loop of each part as a geometry value.
///
/// # Errors
///
/// `draft.hatch.boundary_invalid` for a loop that is not a circle, an ellipse, a closed polyline
/// or a chain of lines and arcs.
pub fn loops_of_region(region: &Region, tol: &Tolerance) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for part in region.parts() {
        for lp in std::iter::once(part.outer()).chain(part.holes()) {
            let curve = match lp.curves() {
                [single] if closed_shape(single) => single.clone(),
                // A full circle or ellipse is traced as an arc of a whole turn.
                [Curve2::Arc(a)] if a.sweep().abs() >= std::f64::consts::TAU => {
                    Curve2::Circle(Circle2::new(a.center(), a.radius())?)
                }
                [Curve2::EllipticalArc(a)] if a.sweep().abs() >= std::f64::consts::TAU => {
                    Curve2::Ellipse(*a.ellipse())
                }
                curves => Curve2::Polyline(
                    Composite2::new(curves.to_vec(), tol)?
                        .to_polyline()
                        .ok_or_else(|| boundary_invalid(None))?,
                ),
            };
            out.push(Value::Geometry(GeometryBytes(curve.encode())));
        }
    }
    Ok(out)
}

/// The `loops` value of the hatch `id` with its current resolved region, when it is associated:
/// what a deletion stores before the references are cleared.
pub(crate) fn frozen_loops(
    snapshot: &Snapshot,
    id: ObjectId,
    options: &DisplayOptions,
) -> Option<Value> {
    let resolution = resolve(snapshot, id, options)
        .ok()
        .filter(|r| r.associated)?;
    loops_of_region(&resolution.region, &options.tolerance)
        .ok()
        .map(Value::List)
}

/// A warning `draft.hatch.disassociated` (detail `id`) for every hatch of `snapshot` that lost
/// a boundary object.
pub(crate) fn disassociated(snapshot: &Snapshot) -> Vec<Finding> {
    let Ok(all) = snapshot.iter_type(HATCH) else {
        return Vec::new();
    };
    all.filter(|(_, o)| {
        stored_len(o.values.get(field::OBJECTS)) > 0
            && objects_of(o.values.get(field::OBJECTS).and_then(Option::as_ref))
                .iter()
                .any(Option::is_none)
    })
    .map(|(_, o)| Finding {
        severity: Severity::Warning,
        error: Error::new(codes::HATCH_DISASSOCIATED).with_detail("id", o.id.to_string()),
    })
    .collect()
}

// --- Pattern line generation ---

/// Where a pattern sits in the drawing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The factor from pattern units to drawing units; finite and positive.
    pub scale: f64,
    /// The rotation of the whole pattern in radians.
    pub angle: f64,
    /// The pattern origin in the drawing; fixed, so moving a boundary does not shift the phase.
    pub origin: Point2,
}

/// One generated mark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    /// A drawn dash or line stretch.
    Segment(Point2, Point2),
    /// A dot.
    Dot(Point2),
}

/// The result of [`generate`].
#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    /// The marks: by family in order, by region part, by line index, along the line.
    pub marks: Vec<Mark>,
    /// Whether the bound was exceeded: `marks` is then empty and the consumer fills the region
    /// solid.
    pub fallback: bool,
}

const FALLBACK: Generated = Generated {
    marks: Vec::new(),
    fallback: true,
};

fn parameter_invalid(name: &'static str) -> Error {
    Error::new(codes::HATCH_PARAMETER_INVALID).with_arg("parameter", name)
}

fn dot(a: Point2, b: Point2, n: Vector2) -> f64 {
    (a.x - b.x) * n.x + (a.y - b.y) * n.y
}

/// The stretches of the infinite line through `foot` with unit direction `u` that lie inside
/// `part`, as parameter intervals from `foot`.
fn inside_stretches(
    part: &RegionPart,
    region: &Region,
    foot: Point2,
    u: Vector2,
    n: Vector2,
    tol: &Tolerance,
) -> Result<Vec<(f64, f64)>> {
    let line = Curve2::Line(Line2::new(foot, u)?);
    let mut cuts: Vec<f64> = Vec::new();
    for lp in std::iter::once(part.outer()).chain(part.holes()) {
        for edge in lp.curves() {
            if let Some(b) = edge.bounding_box() {
                let (lo, hi) = (b.min(), b.max());
                let d = [
                    dot(Point2::new(lo.x, lo.y), foot, n),
                    dot(Point2::new(hi.x, lo.y), foot, n),
                    dot(Point2::new(lo.x, hi.y), foot, n),
                    dot(Point2::new(hi.x, hi.y), foot, n),
                ];
                if d.iter().all(|x| *x > tol.linear) || d.iter().all(|x| *x < -tol.linear) {
                    continue;
                }
            }
            for hit in intersect(&line, edge, tol)? {
                match hit {
                    Intersection::Point { a, .. } => cuts.push(a),
                    Intersection::Overlap { a, .. } => cuts.extend(a),
                }
            }
        }
    }
    cuts.retain(|t| t.is_finite());
    cuts.sort_by(f64::total_cmp);
    let mut stretches: Vec<(f64, f64)> = Vec::new();
    for pair in cuts.windows(2) {
        let &[a, b] = pair else { continue };
        if b - a <= tol.linear {
            continue;
        }
        let mid = 0.5 * (a + b);
        let at = Point2::new(foot.x + mid * u.x, foot.y + mid * u.y);
        if region.classify(at, tol)? != Containment::Inside {
            continue;
        }
        match stretches.last_mut() {
            Some(last) if a - last.1 <= tol.linear => last.1 = b,
            _ => stretches.push((a, b)),
        }
    }
    Ok(stretches)
}

/// Generates the marks of `lines` placed by `placement` inside `region`.
///
/// For each family the lines are indexed by `k`; line `k` is at the perpendicular offset
/// `k · scale · spacing` from the family's origin, and the range of `k` is computed from the
/// bounding box of each part of the region, so the work does not depend on the distance from
/// the pattern origin. A line is cut where it meets the boundary and the stretches inside the
/// region (outer loops minus holes) are kept; a dash list is laid along each stretch from the
/// line's own start point, so the pattern phase depends only on the placement. Dashes and dots
/// are scaled by `placement.scale`; a dash shorter than the linear tolerance is dropped.
///
/// At most `max_marks` lines and marks are visited in all; beyond that the result is the
/// fallback: no marks and `fallback` set, which display lists draw as a solid fill. The same
/// holds when positions cannot be told apart in `f64`.
///
/// # Errors
///
/// `draft.hatch.parameter_invalid` (argument `parameter`) for a scale that is not finite and
/// positive or a non-finite angle or origin; `draft.pattern.line_invalid` for an invalid family;
/// the errors of [`intersect`] and [`Region::classify`].
#[expect(
    clippy::too_many_lines,
    reason = "one loop nest over families, parts, lines and dashes"
)]
pub fn generate(
    region: &Region,
    lines: &[PatternLine],
    placement: &Placement,
    max_marks: usize,
    tol: &Tolerance,
) -> Result<Generated> {
    let Placement {
        scale,
        angle,
        origin,
    } = *placement;
    if !(scale.is_finite() && scale > 0.0) {
        return Err(parameter_invalid("scale"));
    }
    if !(angle.is_finite() && origin.x.is_finite() && origin.y.is_finite()) {
        return Err(parameter_invalid("angle"));
    }
    #[expect(clippy::cast_precision_loss, reason = "a bound far below 2^53")]
    let budget = max_marks as f64;
    let mut work = 0.0_f64;
    let mut marks: Vec<Mark> = Vec::new();
    let (sin, cos) = angle.sin_cos();
    for family in lines {
        family.validate()?;
        let direction = (angle + family.angle.to_radians()).sin_cos();
        let u = Vector2::new(direction.1, direction.0);
        let n = Vector2::new(-direction.0, direction.1);
        let o = Point2::new(
            origin.x + scale * (cos * family.origin.x - sin * family.origin.y),
            origin.y + scale * (sin * family.origin.x + cos * family.origin.y),
        );
        let (step_u, step_n) = (scale * family.shift, scale * family.spacing);
        let period = scale * family.period();
        if ![o.x, o.y, step_u, step_n, period]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(parameter_invalid("scale"));
        }
        for part in region.parts() {
            let single = Region::new(vec![part.clone()])?;
            let bounds = part.outer().bounding_box();
            let (lo, hi) = (bounds.min(), bounds.max());
            let center = Point2::new(0.5 * (lo.x + hi.x), 0.5 * (lo.y + hi.y));
            let corners = [
                Point2::new(lo.x, lo.y),
                Point2::new(hi.x, lo.y),
                Point2::new(lo.x, hi.y),
                Point2::new(hi.x, hi.y),
            ];
            let offsets = corners.map(|c| dot(c, o, n));
            let low = offsets.iter().copied().fold(f64::INFINITY, f64::min) - tol.linear;
            let high = offsets.iter().copied().fold(f64::NEG_INFINITY, f64::max) + tol.linear;
            let (a, b) = (low / step_n, high / step_n);
            let (first, last) = ((a.min(b)).ceil(), (a.max(b)).floor());
            if !(first.is_finite() && last.is_finite()) || last < first {
                continue;
            }
            work += last - first + 1.0;
            if work > budget {
                return Ok(FALLBACK);
            }
            #[expect(clippy::cast_possible_truncation, reason = "bounded by the budget")]
            let (first, last) = (first as i64, last as i64);
            let along_center = dot(center, o, u);
            let across_center = dot(center, o, n);
            for k in first..=last {
                #[expect(clippy::cast_precision_loss, reason = "a bound far below 2^53")]
                let kf = k as f64;
                // The foot of the center on line k, and the pattern coordinate of the foot.
                let h = across_center - kf * step_n;
                let foot = Point2::new(center.x - h * n.x, center.y - h * n.y);
                let x_foot = along_center - kf * step_u;
                for (s0, s1) in inside_stretches(part, &single, foot, u, n, tol)? {
                    let at = |x: f64| {
                        Point2::new(foot.x + (x - x_foot) * u.x, foot.y + (x - x_foot) * u.y)
                    };
                    let (xa, xb) = (x_foot + s0, x_foot + s1);
                    if family.dashes.is_empty() {
                        work += 1.0;
                        marks.push(Mark::Segment(at(xa), at(xb)));
                    } else {
                        #[expect(clippy::cast_precision_loss, reason = "at most 32 elements")]
                        let elements = family.dashes.len() as f64;
                        let repeats = (xb - xa) / period + 2.0;
                        work += repeats * elements;
                        if !(repeats.is_finite()) || work > budget || (xa / period).abs() > 1e15 {
                            return Ok(FALLBACK);
                        }
                        let mut m = (xa / period).floor();
                        while m * period < xb {
                            let mut c = m * period;
                            for d in &family.dashes {
                                let length = scale * d.abs();
                                if *d == 0.0 {
                                    if c >= xa && c <= xb {
                                        marks.push(Mark::Dot(at(c)));
                                    }
                                } else if *d > 0.0 {
                                    let (from, to) = (c.max(xa), (c + length).min(xb));
                                    if to - from > tol.linear {
                                        marks.push(Mark::Segment(at(from), at(to)));
                                    }
                                }
                                c += length;
                            }
                            m += 1.0;
                        }
                    }
                    if marks.len() > max_marks {
                        return Ok(FALLBACK);
                    }
                }
            }
        }
    }
    Ok(Generated {
        marks,
        fallback: false,
    })
}

// --- Display ---

fn continuous() -> Option<Sourced<Option<ObjectId>>> {
    Some(Sourced {
        value: None,
        source: Source::Object,
    })
}

fn stored_color(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<Option<ColorValue>> {
    Ok(match reader.field(id, name)? {
        Some(Value::Color(c)) => Some(*c),
        _ => None,
    })
}

/// The line families of the pattern of the hatch `id`: the referenced pattern object's, or the
/// hatch's own.
fn pattern_lines(reader: &dyn Reader, id: ObjectId) -> Result<Vec<PatternLine>> {
    let lines = match reader.field(id, "pattern")? {
        Some(Value::Reference(pattern)) => lines_of(reader.field(pattern, "lines")?.as_ref()),
        _ => lines_of(reader.field(id, "inline")?.as_ref()),
    };
    if lines.is_empty() {
        return Err(Error::new(codes::PATTERN_EMPTY));
    }
    Ok(lines)
}

/// The primitives of the hatch `id`: a region fill for solid and gradient hatches (a gradient
/// is drawn in its first color), strokes and points for a pattern, and a region fill with
/// part 1 when the pattern exceeds the bound.
pub(crate) fn primitives(
    reader: &dyn Reader,
    id: ObjectId,
    base: &Primitive,
    options: &DisplayOptions,
) -> Result<Vec<Primitive>> {
    let resolution = reader.hatch(id)?;
    let fill = |part: u32, color: Option<ColorValue>| Primitive {
        shape: Shape::Region(resolution.region.clone()),
        linetype: continuous(),
        color: color
            .map(|value| Sourced {
                value,
                source: Source::Object,
            })
            .or_else(|| base.color.clone()),
        part,
        ..base.clone()
    };
    match read_enum(reader, id, "kind")? {
        1 => Ok(vec![fill(0, None)]),
        2 => Ok(vec![fill(0, stored_color(reader, id, "gradient_color1")?)]),
        _ => {
            let lines = pattern_lines(reader, id)?;
            let origin = match reader.field(id, "origin")? {
                Some(Value::Point2(p)) => p,
                _ => Point2::origin(),
            };
            let placement = Placement {
                scale: read_number(reader, id, "scale")?.unwrap_or(1.0),
                angle: read_number(reader, id, "angle")?.unwrap_or(0.0),
                origin,
            };
            let bound = options.max_dashes.min(options.max_primitives);
            let generated = generate(
                &resolution.region,
                &lines,
                &placement,
                bound,
                &options.tolerance,
            )?;
            if generated.fallback {
                return Ok(vec![fill(1, None)]);
            }
            let template = Primitive {
                linetype: continuous(),
                ..base.clone()
            };
            let mut out = Vec::with_capacity(generated.marks.len());
            for mark in generated.marks {
                let shape = match mark {
                    Mark::Dot(p) => Shape::Point(p),
                    Mark::Segment(a, b) => match Segment2::new(a, b) {
                        Ok(s) => Shape::Stroke {
                            curve: Curve2::Segment(s),
                            drawn: None,
                        },
                        Err(_) => continue,
                    },
                };
                out.push(Primitive {
                    shape,
                    ..template.clone()
                });
            }
            Ok(out)
        }
    }
}

impl From<&HatchPattern> for Vec<Value> {
    fn from(pattern: &HatchPattern) -> Self {
        pattern.lines.iter().map(PatternLine::to_value).collect()
    }
}
