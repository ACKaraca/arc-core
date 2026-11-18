//! The commands of dimensions, chains, leaders, multi-leaders and annotation scales.

use arc_base::math::Point2;
use arc_base::{Error, ObjectId, Result};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{FieldPath, Snapshot, Value};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::annotation::{Representation, direction, perp, representations, unit};
use crate::commands::{Created, create_entity, expect, field_invalid, point2, spec, values_of};
use crate::dimension::{
    self, CHAIN, ChainKind, DIMENSION, DefinitionPoint, Key, Kind, chain, field,
    resolve_definition_point, respace,
};
use crate::error::codes;
use crate::leader::{LEADER, MULTILEADER, line, multi};

/// The kind of a new dimension.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindParam {
    /// Along `rotation` (0 horizontal, π/2 vertical).
    Linear,
    /// Along the two points.
    Aligned,
    /// The angle of two lines: four points.
    AngularLines,
    /// The angle at a vertex: vertex and two points.
    AngularPoints,
    /// The angle of an arc: center, start, end.
    AngularArc,
    /// A radius: center and a point on the curve.
    Radial,
    /// A diameter: center and a point on the curve.
    Diameter,
    /// An arc length: center, start, end.
    ArcLength,
    /// An x ordinate: datum and feature.
    OrdinateX,
    /// A y ordinate: datum and feature.
    OrdinateY,
}

/// The sub-object key of a definition point.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyParam {
    /// A free point.
    #[default]
    None,
    /// The start.
    Start,
    /// The end.
    End,
    /// The middle of the parameter range.
    Mid,
    /// The center.
    Center,
    /// A quadrant point.
    Quadrant,
    /// A curve parameter.
    Parameter,
}

/// A definition point: a free `position`, or `object`, `path`, `key` and the key's values.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointParam {
    /// The position of a free point.
    #[serde(default)]
    pub position: Option<[f64; 2]>,
    /// The referenced entity, in the dimension's container.
    #[serde(default)]
    pub object: Option<IdParam>,
    /// Nested entities below `object`, outermost first.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub path: Vec<IdParam>,
    /// The sub-object key.
    #[serde(default)]
    pub key: KeyParam,
    /// The quarter turns of a quadrant, 0 to 3.
    #[serde(default)]
    pub quadrant: i64,
    /// The curve parameter.
    #[serde(default)]
    pub parameter: f64,
}

/// Parameters of `core.draft.dimension.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateDimension {
    /// The space or block definition.
    pub owner: IdParam,
    /// The kind.
    pub kind: KindParam,
    /// The definition points, as many as the kind needs.
    #[schemars(length(max = 4))]
    pub points: Vec<PointParam>,
    /// A point of the dimension line or arc.
    pub line_position: [f64; 2],
    /// The direction of a linear dimension in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The text template; `{}` stands for the measured value.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub text_override: Option<String>,
    /// The moved text's center.
    #[serde(default)]
    pub text_position: Option<[f64; 2]>,
    /// The dimension style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
    /// Whether the dimension is annotative.
    #[serde(default)]
    pub annotative: bool,
    /// The annotation scales it supports.
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub scales: Vec<f64>,
}

/// An arrowhead.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrowParam {
    /// A filled triangle.
    ClosedFilled,
    /// A triangle outline.
    Closed,
    /// Two sides of a triangle.
    Open,
    /// A filled disc.
    Dot,
    /// An oblique stroke.
    Tick,
    /// Nothing.
    None,
}

/// Parameters of `core.draft.leader.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateLeader {
    /// The space or block definition.
    pub owner: IdParam,
    /// The points, the arrow tip first; at least two.
    #[schemars(length(min = 2, max = 4096))]
    pub points: Vec<[f64; 2]>,
    /// The spline flag, stored.
    #[serde(default)]
    pub spline: bool,
    /// The arrowhead; absent takes the style's.
    #[serde(default)]
    pub arrow: Option<ArrowParam>,
    /// The leader style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
    /// Whether the leader is annotative.
    #[serde(default)]
    pub annotative: bool,
    /// The annotation scales it supports.
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub scales: Vec<f64>,
}

/// Which side of the landing multi-leader content lies on.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DirectionParam {
    /// To the right.
    #[default]
    Right,
    /// To the left.
    Left,
}

/// The content of a multi-leader.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentParam {
    /// `text`.
    #[default]
    Text,
    /// `block` at `block_scale`.
    Block,
    /// The tolerance frame `tolerance`.
    Tolerance,
    /// Nothing.
    None,
}

/// The vertical alignment of multi-leader content to the landing.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentParam {
    /// The top.
    Top,
    /// The middle.
    #[default]
    Middle,
    /// The bottom.
    Bottom,
}

/// Parameters of `core.draft.multileader.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateMultileader {
    /// The space or block definition.
    pub owner: IdParam,
    /// The leader lines, each from its arrow tip; at least one point each.
    #[schemars(length(max = 4096))]
    pub leaders: Vec<Vec<[f64; 2]>>,
    /// The end of the landing.
    pub position: [f64; 2],
    /// The side of the content.
    #[serde(default)]
    pub direction: DirectionParam,
    /// The content kind.
    #[serde(default)]
    pub content: ContentParam,
    /// The text, one paragraph per line.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub text: Option<String>,
    /// The block of block content.
    #[serde(default)]
    pub block: Option<IdParam>,
    /// The scale of block content.
    #[serde(default)]
    pub block_scale: Option<f64>,
    /// The rows of cell texts of a tolerance frame.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub tolerance: Vec<Vec<String>>,
    /// The alignment of the content to the landing.
    #[serde(default)]
    pub attachment: AttachmentParam,
    /// The leader style.
    #[serde(default)]
    pub style: Option<IdParam>,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
    /// Whether the multi-leader is annotative.
    #[serde(default)]
    pub annotative: bool,
    /// The annotation scales it supports.
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub scales: Vec<f64>,
}

/// The kind of a new chain.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChainKindParam {
    /// Dimension lines step outward by the spacing.
    Baseline,
    /// Dimension lines are collinear.
    Continued,
}

/// Parameters of `core.draft.chain.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateChain {
    /// The kind.
    pub kind: ChainKindParam,
    /// The members in order: linear or aligned dimensions.
    #[schemars(length(min = 1, max = 1024))]
    pub members: Vec<IdParam>,
    /// The step of a baseline chain; default 10.
    #[serde(default)]
    pub spacing: Option<f64>,
}

/// Parameters of `core.draft.chain.add` and `core.draft.chain.remove`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChainMember {
    /// The chain.
    pub chain: IdParam,
    /// The dimension to add at the end or to remove.
    pub dimension: IdParam,
}

/// Parameters of `core.draft.chain.respace`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Respace {
    /// The chain.
    pub chain: IdParam,
    /// A new spacing; absent keeps the chain's.
    #[serde(default)]
    pub spacing: Option<f64>,
}

/// Parameters of `core.draft.annotation.scales`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scales {
    /// Dimensions, leaders and multi-leaders.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// A new value of `annotative`.
    #[serde(default)]
    pub annotative: Option<bool>,
    /// Scales to add a representation for; an existing one is kept as it is.
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub add: Vec<f64>,
    /// Scales whose representation is removed.
    #[serde(default)]
    #[schemars(length(max = 256))]
    pub remove: Vec<f64>,
}

fn scale_invalid() -> Error {
    Error::new(codes::ANNOTATION_SCALE_INVALID)
}

fn scales_value(scales: &[f64]) -> Result<Value> {
    let mut reps: Vec<Representation> = Vec::new();
    for s in scales {
        if !(s.is_finite() && *s > 0.0) || reps.iter().any(|r| r.scale == *s) {
            return Err(scale_invalid());
        }
        reps.push(Representation {
            scale: *s,
            position: None,
            text_position: None,
        });
    }
    Ok(Value::List(
        reps.iter().map(Representation::to_value).collect(),
    ))
}

fn definition_point(
    snapshot: &Snapshot,
    owner: ObjectId,
    p: PointParam,
) -> Result<DefinitionPoint> {
    let key = match p.key {
        KeyParam::None => Key::None,
        KeyParam::Start => Key::Start,
        KeyParam::End => Key::End,
        KeyParam::Mid => Key::Mid,
        KeyParam::Center => Key::Center,
        KeyParam::Quadrant => Key::Quadrant,
        KeyParam::Parameter => Key::Parameter,
    };
    let mut dp = DefinitionPoint {
        position: p
            .position
            .map_or_else(Point2::origin, |[x, y]| Point2::new(x, y)),
        object: p.object.map(|IdParam(id)| id),
        path: p.path.into_iter().map(|IdParam(id)| Some(id)).collect(),
        key,
        quadrant: p.quadrant,
        parameter: p.parameter,
    };
    match (dp.object, key) {
        (None, Key::None) if p.position.is_some() && dp.path.is_empty() => Ok(dp),
        (Some(object), k) if k != Key::None => {
            if !(0..=3).contains(&dp.quadrant) {
                return Err(field_invalid("quadrant"));
            }
            let container = snapshot.get(snapshot.resolve(object)?)?.header.owner;
            if container != Some(owner) {
                return Err(Error::new(codes::DIMENSION_REFERENCE_INVALID)
                    .with_detail("id", object.to_string()));
            }
            if let Some(resolved) = resolve_definition_point(snapshot, &dp)? {
                dp.position = resolved;
            }
            Ok(dp)
        }
        _ => Err(field_invalid("points")),
    }
}

fn create_dimension(ctx: &mut Context<'_>, p: CreateDimension) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    let owner = p.owner.0;
    let points = p
        .points
        .into_iter()
        .map(|point| definition_point(&snapshot, owner, point).map(|dp| dp.to_value()))
        .collect::<Result<Vec<_>>>()?;
    let mut kind = vec![
        (field::KIND, Value::Enum(p.kind as u32)),
        (field::POINTS, Value::List(points)),
        (field::LINE_POSITION, point2(p.line_position)),
        (field::ROTATION, Value::F64(p.rotation)),
        (field::ANNOTATIVE, Value::Bool(p.annotative)),
        (field::SCALES, scales_value(&p.scales)?),
    ];
    kind.extend(
        p.text_override
            .map(|t| (field::TEXT_OVERRIDE, Value::String(t))),
    );
    kind.extend(p.text_position.map(|t| (field::TEXT_POSITION, point2(t))));
    kind.extend(
        p.style
            .map(|IdParam(s)| (field::STYLE, Value::Reference(s))),
    );
    create_entity(ctx, DIMENSION, owner, p.layer, kind)
}

fn arrow_code(arrow: ArrowParam) -> u32 {
    arrow as u32
}

fn points_value(points: &[[f64; 2]]) -> Value {
    Value::List(points.iter().map(|p| point2(*p)).collect())
}

fn create_leader(ctx: &mut Context<'_>, p: CreateLeader) -> Result<Created> {
    let mut kind = vec![
        (line::POINTS, points_value(&p.points)),
        (line::SPLINE, Value::Bool(p.spline)),
        (line::ANNOTATIVE, Value::Bool(p.annotative)),
        (line::SCALES, scales_value(&p.scales)?),
    ];
    kind.extend(p.arrow.map(|a| (line::ARROW, Value::Enum(arrow_code(a)))));
    kind.extend(p.style.map(|IdParam(s)| (line::STYLE, Value::Reference(s))));
    create_entity(ctx, LEADER, p.owner.0, p.layer, kind)
}

fn create_multileader(ctx: &mut Context<'_>, p: CreateMultileader) -> Result<Created> {
    let tolerance = Value::List(
        p.tolerance
            .into_iter()
            .map(|row| Value::List(row.into_iter().map(Value::String).collect()))
            .collect(),
    );
    let mut kind = vec![
        (
            multi::LEADERS,
            Value::List(p.leaders.iter().map(|l| points_value(l)).collect()),
        ),
        (multi::POSITION, point2(p.position)),
        (multi::DIRECTION, Value::Enum(p.direction as u32)),
        (multi::CONTENT, Value::Enum(p.content as u32)),
        (multi::BLOCK_SCALE, Value::F64(p.block_scale.unwrap_or(1.0))),
        (multi::TOLERANCE, tolerance),
        (multi::ATTACHMENT, Value::Enum(p.attachment as u32)),
        (multi::ANNOTATIVE, Value::Bool(p.annotative)),
        (multi::SCALES, scales_value(&p.scales)?),
    ];
    kind.extend(p.text.map(|t| (multi::TEXT, Value::String(t))));
    kind.extend(
        p.block
            .map(|IdParam(b)| (multi::BLOCK, Value::Reference(b))),
    );
    kind.extend(
        p.style
            .map(|IdParam(s)| (multi::STYLE, Value::Reference(s))),
    );
    create_entity(ctx, MULTILEADER, p.owner.0, p.layer, kind)
}

fn member_invalid(id: ObjectId) -> Error {
    Error::new(codes::CHAIN_MEMBER_INVALID).with_detail("id", id.to_string())
}

/// Fails unless every member is a linear or aligned dimension listed once.
fn check_members(snapshot: &Snapshot, members: &[ObjectId]) -> Result<()> {
    for (i, id) in members.iter().enumerate() {
        expect(snapshot, *id, &[DIMENSION]).map_err(|_| member_invalid(*id))?;
        let kind = match values_of(snapshot, *id)?.get(field::KIND) {
            Some(Some(Value::Enum(c))) => Kind::of(*c),
            _ => None,
        };
        if !matches!(kind, Some(Kind::Linear | Kind::Aligned))
            || members.iter().take(i).any(|m| m == id)
        {
            return Err(member_invalid(*id));
        }
    }
    Ok(())
}

fn chain_members(values: &[Option<Value>]) -> Vec<ObjectId> {
    match values.get(chain::MEMBERS) {
        Some(Some(Value::List(items))) => items
            .iter()
            .filter_map(|v| match v {
                Value::Reference(id) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Writes the members of `chain` and moves their dimension lines by the chain's rule.
fn write_chain(ctx: &mut Context<'_>, id: ObjectId, members: &[ObjectId]) -> Result<()> {
    let txn = ctx.txn()?;
    let snapshot = txn.snapshot().clone();
    check_members(&snapshot, members)?;
    txn.modify(
        id,
        FieldPath::new("members"),
        Some(Value::List(
            members.iter().map(|m| Value::Reference(*m)).collect(),
        )),
    )?;
    let values = values_of(&snapshot, id)?;
    let kind = match values.get(chain::KIND) {
        Some(Some(Value::Enum(1))) => ChainKind::Continued,
        _ => ChainKind::Baseline,
    };
    let spacing = crate::types::f64_at(&values, chain::SPACING).unwrap_or(10.0);
    let Some(first) = members.first() else {
        return Ok(());
    };
    let resolution = dimension::resolve(&snapshot, *first, &dimension::resolution_options())?;
    let first_values = values_of(&snapshot, *first)?;
    let (p0, p1) = match resolution.points.as_slice() {
        [a, b, ..] => (*a, *b),
        _ => return Err(member_invalid(*first)),
    };
    let normal = match first_values.get(field::KIND) {
        Some(Some(Value::Enum(0))) => perp(direction(
            crate::types::f64_at(&first_values, field::ROTATION).unwrap_or(0.0),
        )),
        _ => perp(unit(p1 - p0).ok_or_else(|| member_invalid(*first))?),
    };
    let line = match first_values.get(field::LINE_POSITION) {
        Some(Some(Value::Point2(p))) => *p,
        _ => p0,
    };
    let positions = respace(kind, spacing, p0, normal, line, members.len());
    for (member, position) in members.iter().skip(1).zip(positions) {
        txn.modify(
            *member,
            FieldPath::new("line_position"),
            Some(Value::Point2(position)),
        )?;
    }
    Ok(())
}

fn create_chain(ctx: &mut Context<'_>, p: CreateChain) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    let members: Vec<ObjectId> = p.members.iter().map(|IdParam(id)| *id).collect();
    check_members(&snapshot, &members)?;
    let spacing = p.spacing.unwrap_or(10.0);
    if !(spacing.is_finite() && spacing >= 0.0) {
        return Err(field_invalid("spacing"));
    }
    let owner = match members.first() {
        Some(m) => snapshot.get(snapshot.resolve(*m)?)?.header.owner,
        None => None,
    };
    let id: ObjectId = ctx.new_id();
    let header = arc_doc::Header {
        owner,
        ..arc_doc::Header::default()
    };
    ctx.txn()?.create(
        CHAIN,
        id,
        header,
        vec![
            Some(Value::Enum(p.kind as u32)),
            Some(Value::List(Vec::new())),
            Some(Value::F64(spacing)),
        ],
    )?;
    write_chain(ctx, id, &members)?;
    ctx.set_selection_after(vec![id]);
    Ok(Created { id: IdParam(id) })
}

fn change_chain(ctx: &mut Context<'_>, p: ChainMember, add: bool) -> Result<()> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.chain.0, &[CHAIN])?;
    let mut members = chain_members(&values_of(&snapshot, p.chain.0)?);
    let target = p.dimension.0;
    if add {
        members.push(target);
    } else if members.contains(&target) {
        members.retain(|m| *m != target);
    } else {
        return Err(member_invalid(target));
    }
    write_chain(ctx, p.chain.0, &members)
}

fn respace_chain(ctx: &mut Context<'_>, p: Respace) -> Result<()> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.chain.0, &[CHAIN])?;
    if let Some(spacing) = p.spacing {
        if !(spacing.is_finite() && spacing >= 0.0) {
            return Err(field_invalid("spacing"));
        }
        ctx.txn()?.modify(
            p.chain.0,
            FieldPath::new("spacing"),
            Some(Value::F64(spacing)),
        )?;
    }
    let members = chain_members(&values_of(&snapshot, p.chain.0)?);
    write_chain(ctx, p.chain.0, &members)
}

fn set_scales(ctx: &mut Context<'_>, p: Scales) -> Result<()> {
    if p.add
        .iter()
        .chain(&p.remove)
        .any(|s| !(s.is_finite() && *s > 0.0))
    {
        return Err(scale_invalid());
    }
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        let snapshot = txn.snapshot().clone();
        expect(&snapshot, id, &[DIMENSION, LEADER, MULTILEADER])?;
        let values = values_of(&snapshot, id)?;
        // The annotative fields are the last two of every annotative type.
        let at = values.len().saturating_sub(1);
        let mut reps = representations(values.get(at).and_then(Option::as_ref));
        reps.retain(|r| !p.remove.contains(&r.scale));
        for s in &p.add {
            if !reps.iter().any(|r| r.scale == *s) {
                reps.push(Representation {
                    scale: *s,
                    position: None,
                    text_position: None,
                });
            }
        }
        txn.modify(
            id,
            FieldPath::new("scales"),
            Some(Value::List(
                reps.iter().map(Representation::to_value).collect(),
            )),
        )?;
        if let Some(flag) = p.annotative {
            txn.modify(id, FieldPath::new("annotative"), Some(Value::Bool(flag)))?;
        }
    }
    Ok(())
}

/// The commands of this module.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.dimension.create", create_dimension),
        spec("core.draft.chain.create", create_chain),
        spec("core.draft.chain.add", |ctx, p: ChainMember| {
            change_chain(ctx, p, true)
        }),
        spec("core.draft.chain.remove", |ctx, p: ChainMember| {
            change_chain(ctx, p, false)
        }),
        spec("core.draft.chain.respace", respace_chain),
        spec("core.draft.leader.create", create_leader),
        spec("core.draft.multileader.create", create_multileader),
        spec("core.draft.annotation.scales", set_scales),
    ]
}
