//! Dimensions (CORE-DRAFT-010), their associativity (CORE-DRAFT-011) and dimension chains
//! (CORE-DRAFT-012).
//!
//! A dimension stores its kind, its definition points, the position of its dimension line, an
//! optional text override and text position, a dimension style and its annotation scales. A
//! definition point is a stored position and, optionally, a reference to geometry: an entity,
//! a path of nested entities below it when it is a block reference, and a sub-object key. The
//! derived node [`RESOLVED`] of a dimension reads the referenced geometry and gives the points
//! and the measured value; the display list reads that node. The measured value is never
//! stored.
//!
//! [`measure`] and [`geometry`] are pure functions of a [`Definition`]. The rules are stated in
//! the crate specification, section "Dimensions, leaders and annotation scaling as
//! implemented".

use std::f64::consts::TAU;
use std::sync::Arc;

use arc_base::math::{Point2, Transform2, Vector2};
use arc_base::{Error, ObjectId, Result, Tolerance};
use arc_doc::graph::{
    Derived, DerivedKey, DerivedValue, EvalContext, Evaluators, NodeKey, content_hash,
};
use arc_doc::{
    Bound, Cardinality, FieldDecl, Finding, OnCopy, OnDelete, ReferenceSpec, RelationshipRule,
    Severity, Snapshot, Strength, TypeKey, TypeValidator, Value, ValueType,
};
use arc_geom::Curve2;

use crate::annotation::{
    self, Arrow, Drawn, Part, TextPlacement, angle_of, arrowhead, direction, normalized, perp,
    readable, unit,
};
use crate::display::{DisplayOptions, Reader};
use crate::error::codes;
use crate::style::{DIM_STYLE, style_reference};
use crate::text::{Run, Stack, content_ok};
use crate::types::{
    ARC, BLOCK_REF, CIRCLE, CONSTRUCTION_LINE, ELLIPSE, ELLIPTICAL_ARC, LINE, POINT, POLYLINE, RAY,
    TEXT_BYTES, common_findings, finding, real, text, variants, with_kind,
};

/// A dimension entity.
pub const DIMENSION: &str = "core.draft.dimension";
/// A baseline or continued chain of dimensions.
pub const CHAIN: &str = "core.draft.dim_chain";
/// The derived key of a dimension's resolved points and measurement.
pub const RESOLVED: &str = "core.draft.dimension.resolved";

/// The most entries of a definition point's path.
pub const MAX_PATH: u64 = 64;
/// The most members of a chain.
pub const MAX_MEMBERS: u64 = 1024;

/// Field indices of a dimension.
pub mod field {
    use crate::types::entity::KIND as K;
    /// `kind`.
    pub const KIND: usize = K;
    /// `points`: the definition point records.
    pub const POINTS: usize = K + 1;
    /// `line_position`: a point the dimension line or arc passes through.
    pub const LINE_POSITION: usize = K + 2;
    /// `rotation`: the direction of a linear dimension in radians.
    pub const ROTATION: usize = K + 3;
    /// `text_override`: a template; `{}` stands for the measured value.
    pub const TEXT_OVERRIDE: usize = K + 4;
    /// `text_position`: the moved text's center; absent places the text by rule.
    pub const TEXT_POSITION: usize = K + 5;
    /// `style`: the dimension style.
    pub const STYLE: usize = K + 6;
    /// `annotative`.
    pub const ANNOTATIVE: usize = K + 7;
    /// `scales`: the representations.
    pub const SCALES: usize = K + 8;
}

/// The kind of a dimension, with its enum code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The distance of two points along the direction `rotation` (0 horizontal, π/2
    /// vertical).
    Linear = 0,
    /// The distance of two points.
    Aligned = 1,
    /// The angle of two lines given by two points each.
    AngularLines = 2,
    /// The angle at a vertex between two points: vertex, first, second.
    AngularPoints = 3,
    /// The counterclockwise angle of an arc: center, start, end.
    AngularArc = 4,
    /// The radius: center and a point on the curve.
    Radial = 5,
    /// The diameter: center and a point on the curve.
    Diameter = 6,
    /// The length of the counterclockwise arc: center, start, end.
    ArcLength = 7,
    /// The x distance of a feature from a datum: datum, feature.
    OrdinateX = 8,
    /// The y distance of a feature from a datum: datum, feature.
    OrdinateY = 9,
}

/// The kind names in code order.
pub const KIND_NAMES: [&str; 10] = [
    "linear",
    "aligned",
    "angular_lines",
    "angular_points",
    "angular_arc",
    "radial",
    "diameter",
    "arc_length",
    "ordinate_x",
    "ordinate_y",
];

impl Kind {
    /// The kind of an enum code.
    #[must_use]
    pub fn of(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Linear,
            1 => Self::Aligned,
            2 => Self::AngularLines,
            3 => Self::AngularPoints,
            4 => Self::AngularArc,
            5 => Self::Radial,
            6 => Self::Diameter,
            7 => Self::ArcLength,
            8 => Self::OrdinateX,
            9 => Self::OrdinateY,
            _ => return None,
        })
    }

    /// The number of definition points.
    #[must_use]
    pub fn points(self) -> usize {
        match self {
            Self::AngularLines => 4,
            Self::AngularPoints | Self::AngularArc | Self::ArcLength => 3,
            _ => 2,
        }
    }

    /// Whether the measurement is an angle.
    #[must_use]
    pub fn is_angle(self) -> bool {
        matches!(
            self,
            Self::AngularLines | Self::AngularPoints | Self::AngularArc
        )
    }

    /// The `kind` code of the dimension style child that applies to this kind.
    #[must_use]
    pub fn style_kind(self) -> u32 {
        match self {
            Self::Linear | Self::Aligned => 1,
            Self::AngularLines | Self::AngularPoints | Self::AngularArc => 2,
            Self::Radial => 3,
            Self::Diameter => 4,
            Self::OrdinateX | Self::OrdinateY => 5,
            Self::ArcLength => 6,
        }
    }
}

/// The sub-object key of a definition point, with its enum code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Key {
    /// A free point: the stored position.
    #[default]
    None = 0,
    /// The start of a curve, a point entity's position or a block reference's insertion point.
    Start = 1,
    /// The end of a curve.
    End = 2,
    /// The point at the middle of the parameter range.
    Mid = 3,
    /// The center of a circle, arc, ellipse or elliptical arc.
    Center = 4,
    /// The point at `quadrant` quarter turns of a circle, arc or (in eccentric angle) ellipse.
    Quadrant = 5,
    /// The point at the curve parameter `parameter`.
    Parameter = 6,
}

/// The key names in code order.
pub const KEY_NAMES: [&str; 7] = [
    "none",
    "start",
    "end",
    "mid",
    "center",
    "quadrant",
    "parameter",
];

impl Key {
    /// The key of an enum code; unknown codes read as [`Key::None`].
    #[must_use]
    pub fn of(code: u32) -> Self {
        match code {
            1 => Self::Start,
            2 => Self::End,
            3 => Self::Mid,
            4 => Self::Center,
            5 => Self::Quadrant,
            6 => Self::Parameter,
            _ => Self::None,
        }
    }
}

/// A definition point as stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefinitionPoint {
    /// The stored position: the point of a free point, the last resolved point otherwise.
    pub position: Point2,
    /// The referenced entity in the dimension's container.
    pub object: Option<ObjectId>,
    /// Nested entities below `object`, outermost first; an absent entry was deleted.
    pub path: Vec<Option<ObjectId>>,
    /// The sub-object key.
    pub key: Key,
    /// The quarter turns of [`Key::Quadrant`], 0 to 3.
    pub quadrant: i64,
    /// The parameter of [`Key::Parameter`].
    pub parameter: f64,
}

impl DefinitionPoint {
    /// A free point.
    #[must_use]
    pub fn free(position: Point2) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }

    /// Whether the point references geometry that is gone: a key without its object or with
    /// a deleted path entry.
    #[must_use]
    pub fn is_disassociated(&self) -> bool {
        self.key != Key::None && (self.object.is_none() || self.path.iter().any(Option::is_none))
    }

    /// The point of a stored record.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let Value::Record(f) = value else {
            return None;
        };
        let reference = |v: Option<&Option<Value>>| match v {
            Some(Some(Value::Reference(id))) => Some(*id),
            _ => None,
        };
        let path = match f.get(2) {
            Some(Some(Value::List(items))) => items
                .iter()
                .map(|item| match item {
                    Value::Record(m) => reference(m.first()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(Self {
            position: match f.first() {
                Some(Some(Value::Point2(p))) => *p,
                _ => return None,
            },
            object: reference(f.get(1)),
            path,
            key: match f.get(3) {
                Some(Some(Value::Enum(c))) => Key::of(*c),
                _ => Key::None,
            },
            quadrant: match f.get(4) {
                Some(Some(Value::I64(q))) => *q,
                _ => 0,
            },
            parameter: match f.get(5) {
                Some(Some(Value::F64(t))) => *t,
                _ => 0.0,
            },
        })
    }

    /// The stored record.
    #[must_use]
    pub fn to_value(&self) -> Value {
        Value::Record(vec![
            Some(Value::Point2(self.position)),
            self.object.map(Value::Reference),
            Some(Value::List(
                self.path
                    .iter()
                    .map(|m| Value::Record(vec![m.map(Value::Reference)]))
                    .collect(),
            )),
            Some(Value::Enum(self.key as u32)),
            Some(Value::I64(self.quadrant)),
            Some(Value::F64(self.parameter)),
        ])
    }
}

/// A reference to any entity whose deletion clears it.
fn clearing_reference(targets: &[&str], cardinality: Cardinality) -> ValueType {
    ValueType::Reference(ReferenceSpec {
        targets: targets
            .iter()
            .filter_map(|t| TypeKey::new(*t).ok())
            .collect(),
        rule: RelationshipRule {
            strength: Strength::Associative,
            on_delete: OnDelete::Clear,
            on_copy: OnCopy::ShareTarget,
            cardinality,
        },
    })
}

fn point_type() -> ValueType {
    let member = ValueType::Record(vec![
        FieldDecl::new("member", clearing_reference(&[], Cardinality::Optional)).optional(),
    ]);
    ValueType::Record(vec![
        FieldDecl::new("position", ValueType::Point2),
        FieldDecl::new("object", clearing_reference(&[], Cardinality::Optional)).optional(),
        FieldDecl::new("path", ValueType::List(Box::new(member)))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length { max: MAX_PATH }),
        FieldDecl::new("key", variants(&KEY_NAMES)).with_default(Value::Enum(0)),
        FieldDecl::new("quadrant", ValueType::I64)
            .with_default(Value::I64(0))
            .with_bound(Bound::Int { min: 0, max: 3 }),
        real("parameter", 0.0, f64::MIN, f64::MAX),
    ])
}

fn dimension_fields() -> Vec<FieldDecl> {
    let mut fields = vec![
        FieldDecl::new("kind", variants(&KIND_NAMES)).with_default(Value::Enum(0)),
        FieldDecl::new("points", ValueType::List(Box::new(point_type())))
            .with_bound(Bound::Length { max: 4 }),
        FieldDecl::new("line_position", ValueType::Point2),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        text("text_override", TEXT_BYTES),
        FieldDecl::new("text_position", ValueType::Point2).optional(),
        FieldDecl::new("style", style_reference(DIM_STYLE, true)).optional(),
    ];
    fields.extend(annotation::fields());
    with_kind(fields)
}

/// The definition points of a dimension's values.
#[must_use]
pub fn points_of(values: &[Option<Value>]) -> Vec<DefinitionPoint> {
    match values.get(field::POINTS) {
        Some(Some(Value::List(items))) => items
            .iter()
            .filter_map(DefinitionPoint::from_value)
            .collect(),
        _ => Vec::new(),
    }
}

fn dimension_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = common_findings(values);
    let kind = match values.get(field::KIND) {
        Some(Some(Value::Enum(c))) => Kind::of(*c),
        _ => None,
    };
    let count = match values.get(field::POINTS) {
        Some(Some(Value::List(items))) => items.len(),
        _ => 0,
    };
    if kind.is_none_or(|k| k.points() != count) {
        findings.push(finding(Error::new(codes::DIMENSION_POINTS_INVALID)));
    }
    if let Some(Some(Value::String(s))) = values.get(field::TEXT_OVERRIDE)
        && !content_ok(s)
    {
        findings.push(finding(Error::new(codes::TEXT_CONTENT_INVALID)));
    }
    findings.extend(annotation::scale_findings(values, field::SCALES));
    findings
}

/// The kind of a chain, with its enum code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainKind {
    /// Members measure from one baseline; their dimension lines step outward by the spacing.
    Baseline = 0,
    /// Members continue each other; their dimension lines are collinear.
    Continued = 1,
}

/// Field indices of a chain.
pub mod chain {
    /// `kind`: `baseline` or `continued`.
    pub const KIND: usize = 0;
    /// `members`: the dimensions in order.
    pub const MEMBERS: usize = 1;
    /// `spacing`: the step between baseline dimension lines.
    pub const SPACING: usize = 2;
}

fn chain_fields() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("kind", variants(&["baseline", "continued"])).with_default(Value::Enum(0)),
        FieldDecl::new(
            "members",
            ValueType::List(Box::new(clearing_reference(
                &[DIMENSION],
                Cardinality::Many { max: 1024 },
            ))),
        )
        .with_default(Value::List(Vec::new()))
        .with_bound(Bound::Length { max: MAX_MEMBERS }),
        real("spacing", 10.0, 0.0, f64::MAX),
    ]
}

/// The dimension and chain declarations.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (DIMENSION, dimension_fields(), vec![dimension_findings]),
        (CHAIN, chain_fields(), vec![]),
    ]
}

/// What [`measure`] and [`geometry`] read: the kind, the resolved points and the positions.
#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    /// The kind.
    pub kind: Kind,
    /// The definition points, as many as the kind needs.
    pub points: Vec<Point2>,
    /// A point of the dimension line or arc (the leader end of an ordinate dimension).
    pub line_position: Point2,
    /// The moved text's center.
    pub text_position: Option<Point2>,
    /// The direction of a linear dimension.
    pub rotation: f64,
}

fn degenerate() -> Error {
    Error::new(codes::DIMENSION_DEGENERATE)
}

fn length(v: Vector2) -> f64 {
    v.x.hypot(v.y)
}

impl Definition {
    fn point(&self, i: usize) -> Result<Point2> {
        self.points
            .get(i)
            .copied()
            .ok_or_else(|| Error::new(codes::DIMENSION_POINTS_INVALID))
    }

    /// The direction of a linear or aligned dimension.
    fn along(&self) -> Result<Vector2> {
        match self.kind {
            Kind::Linear => Ok(direction(self.rotation)),
            _ => unit(self.point(1)? - self.point(0)?).ok_or_else(degenerate),
        }
    }
}

/// One side of an angle: a definition point, or a line given by two points.
#[derive(Debug, Clone, Copy)]
enum Side {
    Point(Point2),
    Line(Point2, Point2),
}

/// The angle of an angular or arc-length dimension: vertex, start, counterclockwise sweep and
/// the sides at start and end.
struct Sector {
    vertex: Point2,
    start: f64,
    sweep: f64,
    sides: [Side; 2],
}

fn sector(def: &Definition, tol: &Tolerance) -> Result<Sector> {
    let (p0, p1, p2) = (def.point(0)?, def.point(1)?, def.point(2)?);
    let ray = |p: Point2| unit(p - p0).map(angle_of).ok_or_else(degenerate);
    match def.kind {
        Kind::AngularPoints => {
            let (a1, a2) = (ray(p1)?, ray(p2)?);
            let ccw = normalized(a2 - a1);
            if ccw == 0.0 {
                return Err(degenerate());
            }
            let w = normalized(ray(def.line_position)? - a1);
            Ok(if w <= ccw {
                Sector {
                    vertex: p0,
                    start: a1,
                    sweep: ccw,
                    sides: [Side::Point(p1), Side::Point(p2)],
                }
            } else {
                Sector {
                    vertex: p0,
                    start: a2,
                    sweep: TAU - ccw,
                    sides: [Side::Point(p2), Side::Point(p1)],
                }
            })
        }
        Kind::AngularArc | Kind::ArcLength => {
            let (a1, a2) = (ray(p1)?, ray(p2)?);
            let sweep = normalized(a2 - a1);
            Ok(Sector {
                vertex: p0,
                start: a1,
                sweep: if sweep == 0.0 { TAU } else { sweep },
                sides: [Side::Point(p1), Side::Point(p2)],
            })
        }
        _ => {
            let p3 = def.point(3)?;
            let d1 = unit(p1 - p0).ok_or_else(degenerate)?;
            let d2 = unit(p3 - p2).ok_or_else(degenerate)?;
            let cross = d1.x * d2.y - d1.y * d2.x;
            if cross.abs() <= tol.angular {
                return Err(degenerate());
            }
            let w = p2 - p0;
            let t = (w.x * d2.y - w.y * d2.x) / cross;
            let vertex = p0 + d1 * t;
            let aw = angle_of(unit(def.line_position - vertex).ok_or_else(degenerate)?);
            let lines = [Side::Line(p0, p1), Side::Line(p2, p3)];
            let bounds = [
                (angle_of(d1), 0),
                (angle_of(-d1), 0),
                (angle_of(d2), 1),
                (angle_of(-d2), 1),
            ];
            let pick = |f: &dyn Fn(f64) -> f64| {
                bounds
                    .iter()
                    .copied()
                    .min_by(|a, b| f(a.0).total_cmp(&f(b.0)))
                    .unwrap_or((0.0, 0))
            };
            let (start, s) = pick(&|b| normalized(aw - b));
            let (end, e) = pick(&|b| {
                let d = normalized(b - aw);
                // The start boundary itself is not an end.
                if d == 0.0 { TAU } else { d }
            });
            let sweep = normalized(end - start);
            if sweep == 0.0 || s == e {
                return Err(degenerate());
            }
            let side = |i: usize| lines.get(i).copied().unwrap_or(Side::Point(vertex));
            Ok(Sector {
                vertex,
                start,
                sweep,
                sides: [side(s), side(e)],
            })
        }
    }
}

/// The measured value of a dimension in drawing units, or radians for an angular kind: before
/// the style's measurement scale.
///
/// # Errors
///
/// `draft.dimension.points_invalid` when points are missing; `draft.dimension.degenerate` when
/// the points define no measurement.
pub fn measure(def: &Definition, tol: &Tolerance) -> Result<f64> {
    let (p0, p1) = (def.point(0)?, def.point(1)?);
    Ok(match def.kind {
        Kind::Linear | Kind::Aligned => (p1 - p0).dot(&def.along()?).abs(),
        Kind::AngularLines | Kind::AngularPoints | Kind::AngularArc => sector(def, tol)?.sweep,
        Kind::Radial | Kind::Diameter => {
            let r = length(p1 - p0);
            if r <= tol.linear {
                return Err(degenerate());
            }
            if def.kind == Kind::Diameter {
                2.0 * r
            } else {
                r
            }
        }
        Kind::ArcLength => length(p1 - p0) * sector(def, tol)?.sweep,
        Kind::OrdinateX => p1.x - p0.x,
        Kind::OrdinateY => p1.y - p0.y,
    })
}

/// The sizes of a dimension in drawing units and its style choices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sizes {
    /// The text height.
    pub text_height: f64,
    /// The gap around the text.
    pub gap: f64,
    /// The arrowhead.
    pub arrow: Arrow,
    /// The arrowhead size.
    pub arrow_size: f64,
    /// The gap between a definition point and its extension line.
    pub extension_offset: f64,
    /// How far an extension line passes the dimension line.
    pub extension_beyond: f64,
    /// Text above the line instead of centered on it.
    pub above: bool,
}

/// The extension line from `p` toward `q`: from `offset` past `p` to `beyond` past `q`.
fn extension(d: &mut Drawn, p: Point2, q: Point2, s: &Sizes) -> Result<()> {
    let v = q - p;
    let l = length(v);
    if let Some(e) = unit(v)
        && s.extension_offset < l + s.extension_beyond
    {
        d.line(
            Part::Extension,
            p + e * s.extension_offset,
            q + e * s.extension_beyond,
        )?;
    }
    Ok(())
}

/// The space the arrowheads take on the line.
fn arrow_room(s: &Sizes) -> f64 {
    if s.arrow == Arrow::None {
        0.0
    } else {
        2.0 * s.arrow_size
    }
}

/// The leader from `from` to a moved text at `to`, stopping `half` short of it.
fn text_leader(d: &mut Drawn, from: Point2, to: Point2, half: f64) -> Result<()> {
    if let Some(e) = unit(to - from)
        && length(to - from) > half
    {
        d.line(Part::Line, from, to - e * half)?;
    }
    Ok(())
}

/// A straight dimension line between the tips `a` and `b` with arrows, and its text.
fn straight(
    d: &mut Drawn,
    a: Point2,
    b: Point2,
    fallback: Vector2,
    s: &Sizes,
    width: f64,
    moved: Option<Point2>,
) -> Result<()> {
    let span = length(b - a);
    let e = unit(b - a).unwrap_or(fallback);
    let rotation = readable(angle_of(e));
    let lift = if s.above {
        perp(direction(rotation)) * (s.gap + s.text_height / 2.0)
    } else {
        Vector2::zeros()
    };
    let half = width / 2.0 + s.gap;
    let mid = a + (b - a) / 2.0;
    let inside = span >= width + 2.0 * s.gap + arrow_room(s);
    let center = if inside {
        arrowhead(d, s.arrow, a, -e, s.arrow_size)?;
        arrowhead(d, s.arrow, b, e, s.arrow_size)?;
        if !s.above && moved.is_none() {
            d.line(Part::Line, a, mid - e * half)?;
            d.line(Part::Line, mid + e * half, b)?;
        } else {
            d.line(Part::Line, a, b)?;
        }
        mid + lift
    } else {
        arrowhead(d, s.arrow, a, e, s.arrow_size)?;
        arrowhead(d, s.arrow, b, -e, s.arrow_size)?;
        let stub = 2.0 * s.arrow_size;
        d.line(Part::Line, a - e * stub, b + e * stub)?;
        b + e * (stub + half) + lift
    };
    let center = match moved {
        Some(t) => {
            text_leader(d, mid, t, half)?;
            t
        }
        None => center,
    };
    d.text = Some(TextPlacement { center, rotation });
    Ok(())
}

fn angular(d: &mut Drawn, def: &Definition, s: &Sizes, width: f64, tol: &Tolerance) -> Result<()> {
    let sec = sector(def, tol)?;
    let v = sec.vertex;
    let r = length(def.line_position - v);
    if r <= tol.linear {
        return Err(degenerate());
    }
    let end = sec.start + sec.sweep;
    let (q0, q1) = (v + direction(sec.start) * r, v + direction(end) * r);
    for (side, q) in sec.sides.iter().zip([q0, q1]) {
        match *side {
            Side::Point(p) => extension(d, p, q, s)?,
            Side::Line(a, b) => {
                let e = unit(q - v).unwrap_or_else(Vector2::zeros);
                let (ta, tb) = ((a - v).dot(&e), (b - v).dot(&e));
                let (lo, hi) = (ta.min(tb), ta.max(tb));
                if r > hi {
                    extension(d, v + e * hi, q, s)?;
                } else if r < lo {
                    extension(d, v + e * lo, q, s)?;
                }
            }
        }
    }
    let (t0, t1) = (perp(direction(sec.start)), perp(direction(end)));
    let half = width / 2.0 + s.gap;
    let mid = sec.start + sec.sweep / 2.0;
    let on_arc = v + direction(mid) * r;
    let inside = r * sec.sweep >= width + 2.0 * s.gap + arrow_room(s);
    let lift = |rotation: f64| {
        if s.above {
            perp(direction(rotation)) * (s.gap + s.text_height / 2.0)
        } else {
            Vector2::zeros()
        }
    };
    let (center, rotation) = if inside {
        arrowhead(d, s.arrow, q0, -t0, s.arrow_size)?;
        arrowhead(d, s.arrow, q1, t1, s.arrow_size)?;
        let delta = half / r;
        if !s.above && def.text_position.is_none() && 2.0 * delta < sec.sweep {
            d.arc(v, r, sec.start, sec.sweep / 2.0 - delta)?;
            d.arc(v, r, mid + delta, sec.sweep / 2.0 - delta)?;
        } else {
            d.arc(v, r, sec.start, sec.sweep)?;
        }
        let rotation = readable(mid + std::f64::consts::FRAC_PI_2);
        (on_arc + lift(rotation), rotation)
    } else {
        arrowhead(d, s.arrow, q0, t0, s.arrow_size)?;
        arrowhead(d, s.arrow, q1, -t1, s.arrow_size)?;
        d.arc(v, r, sec.start, sec.sweep)?;
        let stub = 2.0 * s.arrow_size;
        d.line(Part::Line, q0, q0 - t0 * stub)?;
        d.line(Part::Line, q1, q1 + t1 * stub)?;
        let rotation = readable(angle_of(t1));
        (q1 + t1 * (stub + half) + lift(rotation), rotation)
    };
    let center = match def.text_position {
        Some(t) => {
            text_leader(d, on_arc, t, half)?;
            t
        }
        None => center,
    };
    d.text = Some(TextPlacement { center, rotation });
    Ok(())
}

fn radial(d: &mut Drawn, def: &Definition, s: &Sizes, width: f64, tol: &Tolerance) -> Result<()> {
    let (c, p) = (def.point(0)?, def.point(1)?);
    let r = length(p - c);
    let e = unit(p - c).ok_or_else(degenerate)?;
    if r <= tol.linear {
        return Err(degenerate());
    }
    let rotation = readable(angle_of(e));
    let lift = if s.above {
        perp(direction(rotation)) * (s.gap + s.text_height / 2.0)
    } else {
        Vector2::zeros()
    };
    let half = width / 2.0 + s.gap;
    let k = (def.line_position - c).dot(&e);
    let at = c + e * k;
    let center = if let Some(t) = def.text_position {
        arrowhead(d, s.arrow, p, e, s.arrow_size)?;
        d.line(Part::Line, c, p)?;
        text_leader(d, p, t, half)?;
        t
    } else if k <= r {
        arrowhead(d, s.arrow, p, e, s.arrow_size)?;
        if !s.above && k - half > 0.0 && k + half < r {
            d.line(Part::Line, c, at - e * half)?;
            d.line(Part::Line, at + e * half, p)?;
        } else {
            d.line(Part::Line, c, p)?;
        }
        at + lift
    } else {
        arrowhead(d, s.arrow, p, -e, s.arrow_size)?;
        if k - half > r {
            d.line(Part::Line, p, at - e * half)?;
        }
        at + lift
    };
    d.text = Some(TextPlacement { center, rotation });
    Ok(())
}

fn ordinate(d: &mut Drawn, def: &Definition, s: &Sizes, width: f64) -> Result<()> {
    let f = def.point(1)?;
    let l = def.line_position;
    let knee = if def.kind == Kind::OrdinateX {
        Point2::new(f.x, l.y)
    } else {
        Point2::new(l.x, f.y)
    };
    if let Some(e) = unit(knee - f)
        && length(knee - f) > s.extension_offset
    {
        d.line(Part::Line, f + e * s.extension_offset, knee)?;
    }
    d.line(Part::Line, knee, l)?;
    let g = unit(l - knee)
        .or_else(|| unit(knee - f))
        .unwrap_or_else(|| {
            direction(if def.kind == Kind::OrdinateX {
                std::f64::consts::FRAC_PI_2
            } else {
                0.0
            })
        });
    let center = def.text_position.unwrap_or(l + g * (s.gap + width / 2.0));
    d.text = Some(TextPlacement {
        center,
        rotation: readable(angle_of(g)),
    });
    Ok(())
}

/// The drawn geometry of a dimension whose text is `text_width` wide at the sizes `sizes`.
///
/// # Errors
///
/// `draft.dimension.points_invalid`, `draft.dimension.degenerate`, and the `arc-geom` errors
/// of shapes that cannot be represented.
pub fn geometry(
    def: &Definition,
    sizes: &Sizes,
    text_width: f64,
    tol: &Tolerance,
) -> Result<Drawn> {
    let mut d = Drawn::default();
    let (p0, p1) = (def.point(0)?, def.point(1)?);
    match def.kind {
        Kind::Linear | Kind::Aligned => {
            let u = def.along()?;
            let n = perp(u);
            let foot = |p: Point2| p + n * (def.line_position - p).dot(&n);
            let (a, b) = (foot(p0), foot(p1));
            extension(&mut d, p0, a, sizes)?;
            extension(&mut d, p1, b, sizes)?;
            straight(&mut d, a, b, u, sizes, text_width, def.text_position)?;
        }
        Kind::Diameter => {
            let e = unit(p1 - p0).ok_or_else(degenerate)?;
            let far = p0 - (p1 - p0);
            straight(&mut d, far, p1, e, sizes, text_width, def.text_position)?;
        }
        Kind::Radial => radial(&mut d, def, sizes, text_width, tol)?,
        Kind::AngularLines | Kind::AngularPoints | Kind::AngularArc | Kind::ArcLength => {
            angular(&mut d, def, sizes, text_width, tol)?;
        }
        Kind::OrdinateX | Kind::OrdinateY => ordinate(&mut d, def, sizes, text_width)?,
    }
    Ok(d)
}

/// The tolerance written after a measured value, already formatted.
#[derive(Debug, Clone, PartialEq)]
pub enum ToleranceText {
    /// None.
    None,
    /// `±value`.
    Symmetric(String),
    /// Upper and lower deviations, stacked: `+upper` over `-lower`.
    Deviation(String, String),
    /// The upper and lower limits, stacked in place of the value.
    Limits(String, String),
}

/// The runs of a dimension text: the template `override_text` (or `{}`) with its first `{}`
/// replaced by the measured value `measured` and its tolerance. A template without `{}` is
/// written as is, without value or tolerance.
#[must_use]
pub fn text_runs(
    measured: &str,
    override_text: Option<&str>,
    tolerance: &ToleranceText,
) -> Vec<Run> {
    let template = override_text.unwrap_or("{}");
    let plain = |text: String| Run {
        text,
        ..Run::default()
    };
    let stacked = |upper: &str, lower: &str| Run {
        text: upper.to_owned(),
        stack: Stack::Horizontal,
        denominator: Some(lower.to_owned()),
        ..Run::default()
    };
    let Some((pre, post)) = template.split_once("{}") else {
        return vec![plain(template.to_owned())];
    };
    let mut runs = vec![plain(pre.to_owned())];
    match tolerance {
        ToleranceText::None => runs.push(plain(measured.to_owned())),
        ToleranceText::Symmetric(t) => runs.push(plain(format!("{measured} ±{t}"))),
        ToleranceText::Deviation(u, l) => {
            runs.push(plain(format!("{measured} ")));
            runs.push(stacked(&format!("+{u}"), &format!("-{l}")));
        }
        ToleranceText::Limits(u, l) => runs.push(stacked(u, l)),
    }
    runs.push(plain(post.to_owned()));
    runs.retain(|r| !r.text.is_empty() || r.denominator.is_some());
    runs
}

// --- Associativity (CORE-DRAFT-011) ---

/// The resolved points of a dimension and its measured value: the result of its [`RESOLVED`]
/// node.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// The definition points: resolved from geometry, or the stored ones.
    pub points: Vec<Point2>,
    /// Whether each point was resolved from geometry.
    pub associated: Vec<bool>,
    /// The measured value ([`measure`]).
    pub measurement: f64,
}

impl Derived for Resolution {
    fn content_hash(&self) -> u128 {
        // shortcut: hashes the exact `Debug` text, as the display lists do.
        content_hash(&format!("{self:?}"))
    }
}

fn reference_invalid(id: ObjectId) -> Error {
    Error::new(codes::DIMENSION_REFERENCE_INVALID).with_detail("id", id.to_string())
}

const CURVES: [&str; 8] = [
    LINE,
    RAY,
    CONSTRUCTION_LINE,
    CIRCLE,
    ARC,
    ELLIPSE,
    ELLIPTICAL_ARC,
    POLYLINE,
];

/// The point the key of `dp` names on the entity `id`, in the entity's container.
fn sub_point(reader: &dyn Reader, id: ObjectId, dp: &DefinitionPoint) -> Result<Point2> {
    let invalid = || reference_invalid(id);
    let key = reader.type_key(id).ok_or_else(invalid)?;
    if key == POINT || key == BLOCK_REF {
        let position = reader.field(id, "position")?;
        return match (position, dp.key) {
            (Some(Value::Point2(p)), Key::Start) => Ok(p),
            (Some(Value::Point2(p)), Key::End | Key::Mid | Key::Center) if key == POINT => Ok(p),
            _ => Err(invalid()),
        };
    }
    if !CURVES.contains(&key.as_str()) {
        return Err(invalid());
    }
    let curve = crate::display::decode_curve(Some(&reader.field(id, "geometry")?))?;
    let (a, b) = curve.parameter_range();
    let quarter = |q: i64| {
        #[expect(clippy::cast_precision_loss, reason = "the quadrant is 0 to 3")]
        let t = q.clamp(0, 3) as f64 * std::f64::consts::FRAC_PI_2;
        t
    };
    match (dp.key, &curve) {
        (Key::Start, _) => curve.start_point(),
        (Key::End, _) => curve.end_point(),
        (Key::Mid, _) if a.is_finite() && b.is_finite() => Some(curve.point_at((a + b) / 2.0)),
        (Key::Center, Curve2::Circle(c)) => Some(c.center()),
        (Key::Center, Curve2::Arc(c)) => Some(c.center()),
        (Key::Center, Curve2::Ellipse(c)) => Some(c.center()),
        (Key::Center, Curve2::EllipticalArc(c)) => Some(c.ellipse().center()),
        (Key::Quadrant, Curve2::Circle(c)) => {
            Some(c.center() + direction(quarter(dp.quadrant)) * c.radius())
        }
        (Key::Quadrant, Curve2::Arc(c)) => {
            Some(c.center() + direction(quarter(dp.quadrant)) * c.radius())
        }
        (Key::Quadrant, Curve2::Ellipse(c)) => Some(curve_point(*c, quarter(dp.quadrant))),
        (Key::Quadrant, Curve2::EllipticalArc(c)) => {
            Some(curve_point(*c.ellipse(), quarter(dp.quadrant)))
        }
        (Key::Parameter, _)
            if dp.parameter.is_finite() && dp.parameter >= a && dp.parameter <= b =>
        {
            Some(curve.point_at(dp.parameter))
        }
        _ => None,
    }
    .ok_or_else(invalid)
}

fn curve_point(ellipse: arc_geom::Ellipse2, t: f64) -> Point2 {
    Curve2::Ellipse(ellipse).point_at(t)
}

/// The point `dp` references, in the dimension's container; `None` for a free or
/// disassociated point.
fn resolve_point(reader: &dyn Reader, dp: &DefinitionPoint) -> Result<Option<Point2>> {
    if dp.key == Key::None || dp.is_disassociated() {
        return Ok(None);
    }
    let Some(object) = dp.object else {
        return Ok(None);
    };
    if dp.path.len() > usize::try_from(MAX_PATH).unwrap_or(usize::MAX) {
        return Err(reference_invalid(object));
    }
    let mut chain = vec![object];
    chain.extend(dp.path.iter().flatten());
    let mut map = Transform2::identity();
    for pair in chain.windows(2) {
        let [outer, inner] = pair else {
            continue;
        };
        let (block, t) = crate::display::block_ref_transform(reader, *outer)
            .map_err(|_| reference_invalid(*outer))?;
        if reader.owner(*inner)? != Some(block) {
            return Err(reference_invalid(*inner));
        }
        map *= t;
    }
    let last = chain.last().copied().unwrap_or(object);
    Ok(Some(map * sub_point(reader, last, dp)?))
}

fn point_value(reader: &dyn Reader, id: ObjectId, name: &str) -> Result<Point2> {
    match reader.field(id, name)? {
        Some(Value::Point2(p)) => Ok(p),
        _ => Ok(Point2::origin()),
    }
}

/// The kind and stored definition points of the dimension `id`.
fn stored(reader: &dyn Reader, id: ObjectId) -> Result<(Kind, Vec<DefinitionPoint>)> {
    let kind = match reader.field(id, "kind")? {
        Some(Value::Enum(c)) => Kind::of(c),
        _ => None,
    }
    .ok_or_else(|| Error::new(codes::DIMENSION_POINTS_INVALID))?;
    let points = match reader.field(id, "points")? {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(DefinitionPoint::from_value)
            .collect(),
        _ => Vec::new(),
    };
    Ok((kind, points))
}

/// Resolves the dimension `id` through `reader`.
pub(crate) fn resolve_with(
    reader: &dyn Reader,
    id: ObjectId,
    tol: &Tolerance,
) -> Result<Resolution> {
    let (kind, stored) = stored(reader, id)?;
    let mut points = Vec::with_capacity(stored.len());
    let mut associated = Vec::with_capacity(stored.len());
    for dp in &stored {
        let resolved = resolve_point(reader, dp)?;
        associated.push(resolved.is_some());
        points.push(resolved.unwrap_or(dp.position));
    }
    let def = Definition {
        kind,
        points,
        line_position: point_value(reader, id, "line_position")?,
        text_position: None,
        rotation: match reader.field(id, "rotation")? {
            Some(Value::F64(r)) => r,
            _ => 0.0,
        },
    };
    let measurement = measure(&def, tol)?;
    Ok(Resolution {
        points: def.points,
        associated,
        measurement,
    })
}

/// The node of the resolution of the dimension `id`.
///
/// # Errors
///
/// None for the constant key; the key check of `arc-doc` is passed on.
pub fn resolved_node(id: ObjectId) -> Result<NodeKey> {
    Ok(NodeKey::object(id, DerivedKey::new(RESOLVED)?))
}

/// Registers the [`RESOLVED`] evaluator of dimensions with the tolerance of `options`.
///
/// # Errors
///
/// `doc.graph.duplicate_key` when it is registered already.
pub fn register_evaluators(evaluators: &mut Evaluators, options: &DisplayOptions) -> Result<()> {
    let tol = options.tolerance;
    evaluators.register_object(
        TypeKey::new(DIMENSION)?,
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

/// The resolution of the dimension `id` evaluated from `snapshot`, without the graph.
///
/// # Errors
///
/// `doc.object.not_found`; `draft.dimension.points_invalid`, `draft.dimension.degenerate` and
/// `draft.dimension.reference_invalid`.
pub fn resolve(snapshot: &Snapshot, id: ObjectId, options: &DisplayOptions) -> Result<Resolution> {
    crate::display::with_snapshot(snapshot, options, |reader| {
        resolve_with(reader, id, &options.tolerance)
    })
}

/// The `points` value of the dimension `id` with every associated point's position replaced by
/// its resolved one: what a deletion stores before the references are cleared. `None` when no
/// point resolves.
pub(crate) fn frozen_points(
    snapshot: &Snapshot,
    id: ObjectId,
    options: &DisplayOptions,
) -> Option<Value> {
    crate::display::with_snapshot(snapshot, options, |reader| {
        let (_, stored) = stored(reader, id).ok()?;
        let mut changed = false;
        let records = stored
            .into_iter()
            .map(|mut dp| {
                if let Ok(Some(p)) = resolve_point(reader, &dp) {
                    changed |= p != dp.position;
                    dp.position = p;
                }
                dp.to_value()
            })
            .collect();
        changed.then_some(Value::List(records))
    })
}

/// Display options for resolution only: the default tolerance and bounds, and no fonts (they
/// are not read).
pub(crate) fn resolution_options() -> DisplayOptions {
    DisplayOptions {
        tolerance: Tolerance::DEFAULT,
        max_nesting: crate::validate::DEFAULT_MAX_NESTING,
        max_primitives: usize::MAX,
        max_dashes: 0,
        fonts: crate::fonts::Fonts {
            provider: Arc::new(crate::fonts::FontRegistry::default()),
            user: crate::fonts::Substitutions::default(),
            default_stroke: String::new(),
        },
        annotation_scale: 1.0,
    }
}

/// The point `dp` references in `snapshot`; `None` for a free point.
///
/// # Errors
///
/// `draft.dimension.reference_invalid` when the reference gives no point.
pub fn resolve_definition_point(
    snapshot: &Snapshot,
    dp: &DefinitionPoint,
) -> Result<Option<Point2>> {
    crate::display::with_snapshot(snapshot, &resolution_options(), |reader| {
        resolve_point(reader, dp)
    })
}

/// A warning `draft.dimension.disassociated` (detail `id`) for every dimension of `snapshot`
/// with a disassociated point.
pub(crate) fn disassociated(snapshot: &Snapshot) -> Vec<Finding> {
    let Ok(all) = snapshot.iter_type(DIMENSION) else {
        return Vec::new();
    };
    all.filter(|(_, o)| {
        points_of(&o.values)
            .iter()
            .any(DefinitionPoint::is_disassociated)
    })
    .map(|(_, o)| Finding {
        severity: Severity::Warning,
        error: Error::new(codes::DIMENSION_DISASSOCIATED).with_detail("id", o.id.to_string()),
    })
    .collect()
}

/// The new line positions of a chain's members after the first, from the first member's first
/// point `origin`, the unit normal `normal` of its dimension line and its line position
/// `first`: a baseline chain steps each member `spacing` farther from the definition points
/// than the one before; a continued chain puts every dimension line on the first one's.
#[must_use]
pub fn respace(
    kind: ChainKind,
    spacing: f64,
    origin: Point2,
    normal: Vector2,
    first: Point2,
    count: usize,
) -> Vec<Point2> {
    let side = if (first - origin).dot(&normal) < 0.0 {
        -1.0
    } else {
        1.0
    };
    (1..count)
        .map(|i| match kind {
            ChainKind::Continued => first,
            ChainKind::Baseline => {
                #[expect(clippy::cast_precision_loss, reason = "members are at most 1024")]
                let step = i as f64 * spacing * side;
                first + normal * step
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use arc_base::id::Identifier;

    use super::*;

    #[test]
    fn templates_replace_their_first_placeholder() {
        let none = ToleranceText::None;
        let texts = |runs: Vec<Run>| runs.into_iter().map(|r| r.text).collect::<Vec<_>>();
        assert_eq!(texts(text_runs("12", None, &none)), ["12"]);
        assert_eq!(
            texts(text_runs("12", Some("R{} typ"), &none)),
            ["R", "12", " typ"]
        );
        assert_eq!(texts(text_runs("12", Some("FIXED"), &none)), ["FIXED"]);
        let dev = text_runs(
            "12",
            None,
            &ToleranceText::Deviation("0.1".into(), "0.2".into()),
        );
        assert_eq!(dev[1].denominator.as_deref(), Some("-0.2"));
        assert_eq!(
            texts(text_runs(
                "12",
                None,
                &ToleranceText::Symmetric("0.1".into())
            )),
            ["12 ±0.1"]
        );
    }

    #[test]
    fn definition_points_round_trip_as_values() {
        let dp = DefinitionPoint {
            position: Point2::new(1.0, 2.0),
            object: Some(ObjectId::from_u128(7)),
            path: vec![Some(ObjectId::from_u128(8)), None],
            key: Key::Quadrant,
            quadrant: 2,
            parameter: 0.5,
        };
        assert_eq!(
            DefinitionPoint::from_value(&dp.to_value()),
            Some(dp.clone())
        );
        assert!(dp.is_disassociated());
    }
}
