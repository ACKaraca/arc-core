//! The commands of hatches and hatch patterns.

use arc_base::math::Point2;
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{Snapshot, Value};
use arc_geom::Curve2;
use arc_geom::trace::{IslandMode, trace_region};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::commands::{
    Created, color, create, create_entity, expect, family, field_invalid, point2, spec,
};
use crate::error::codes;
use crate::hatch::{
    BOUNDARY_KINDS, HATCH, HATCH_PATTERN, field, island_code, loops_of_region, object_curves,
    object_value, region_from_curves,
};
use crate::hatch_pattern::{PatternFile, PatternLine};
use crate::types::CONTAINERS;

/// The kind of a new hatch.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KindParam {
    /// Pattern lines.
    #[default]
    Pattern,
    /// A solid fill.
    Solid,
    /// A gradient of two colors.
    Gradient,
}

/// What islands inside the boundary become.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IslandParam {
    /// Filled again at every second nesting level.
    #[default]
    Alternating,
    /// Holes, with nothing inside them filled.
    Outermost,
    /// Ignored: the whole outer area is filled.
    Ignore,
}

impl IslandParam {
    fn mode(self) -> IslandMode {
        match self {
            Self::Alternating => IslandMode::Alternating,
            Self::Outermost => IslandMode::Outermost,
            Self::Ignore => IslandMode::Ignore,
        }
    }
}

/// A line family of an inline pattern.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineParam {
    /// The direction in degrees.
    pub angle: f64,
    /// A point on line 0.
    #[serde(default)]
    pub origin: [f64; 2],
    /// The shift of each next line along the direction.
    #[serde(default)]
    pub shift: f64,
    /// The perpendicular distance to the next line; not zero.
    pub spacing: f64,
    /// The dash list: positive drawn, negative gap, zero a dot.
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub dashes: Vec<f64>,
}

/// A gradient of two colors.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GradientParam {
    /// `linear` (default) or `radial`.
    #[serde(default)]
    pub kind: Option<String>,
    /// The angle in radians.
    #[serde(default)]
    pub angle: f64,
    /// The first color, linear RGBA.
    pub color1: [f32; 4],
    /// The second color, linear RGBA.
    pub color2: [f32; 4],
}

/// Parameters of `core.draft.hatch.create` and `core.draft.hatch.pick`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateHatch {
    /// The space or block definition.
    pub owner: IdParam,
    /// The kind; default `pattern`.
    #[serde(default)]
    pub kind: KindParam,
    /// `create`: closed boundary curves (`arc-geom` curves in JSON form), or open curves that
    /// join into closed ones. Not associative.
    #[serde(default)]
    pub boundary: Vec<serde_json::Value>,
    /// `create`: the boundary objects the hatch follows. `pick`: the entities traced, default
    /// every curve entity of the owner.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub objects: Vec<IdParam>,
    /// `pick`: the point to hatch around.
    #[serde(default)]
    pub pick: Option<[f64; 2]>,
    /// `pick`: whether the hatch follows the traced objects; default true.
    #[serde(default)]
    pub associative: Option<bool>,
    /// The island mode.
    #[serde(default)]
    pub island: IslandParam,
    /// The pattern object of a pattern hatch.
    #[serde(default)]
    pub pattern: Option<IdParam>,
    /// The line families of a pattern defined in the hatch.
    #[serde(default)]
    #[schemars(length(max = 64))]
    pub inline: Vec<LineParam>,
    /// The pattern scale, positive; default 1.
    #[serde(default)]
    pub scale: Option<f64>,
    /// The pattern angle in radians.
    #[serde(default)]
    pub angle: f64,
    /// The pattern origin; default (0, 0).
    #[serde(default)]
    pub origin: Option<[f64; 2]>,
    /// The gradient of a gradient hatch.
    #[serde(default)]
    pub gradient: Option<GradientParam>,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

/// The result of `core.draft.hatch_pattern.load`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Loaded {
    /// The new pattern objects, in name order.
    pub ids: Vec<IdParam>,
}

/// Parameters of `core.draft.hatch_pattern.load`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadPatterns {
    /// The text of a pattern file; absent loads from the shipped library.
    #[serde(default)]
    pub text: Option<String>,
    /// The patterns to load; empty loads all of the file.
    #[serde(default)]
    #[schemars(length(max = 4096))]
    pub names: Vec<String>,
}

fn boundary_invalid(id: Option<ObjectId>) -> Error {
    let e = Error::new(codes::HATCH_BOUNDARY_INVALID);
    match id {
        Some(id) => e.with_detail("id", id.to_string()),
        None => e,
    }
}

/// The curve entities of `owner`, in ascending identifier order, at most `limit`.
fn candidates(snapshot: &Snapshot, owner: ObjectId, limit: usize) -> Result<Vec<ObjectId>> {
    let mut out: Vec<ObjectId> = snapshot
        .referrers(owner)
        .filter(|r| r.field == arc_doc::relationship::ReferenceField::Owner)
        .map(|r| r.source)
        .filter(|id| {
            snapshot
                .resolve(*id)
                .ok()
                .and_then(|h| snapshot.type_key(h))
                .is_some_and(|k| BOUNDARY_KINDS.contains(&k.as_str()))
        })
        .collect();
    out.sort();
    if out.len() > limit {
        return Err(Error::new(codes::DISPLAY_TOO_LARGE)
            .with_arg("limit", i64::try_from(limit).unwrap_or(i64::MAX)));
    }
    Ok(out)
}

#[expect(
    clippy::too_many_lines,
    reason = "one pass over every parameter of the command"
)]
fn create_hatch(ctx: &mut Context<'_>, p: CreateHatch, picking: bool) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    let owner = p.owner.0;
    expect(&snapshot, owner, CONTAINERS)?;
    let tol = Tolerance::DEFAULT;
    let options = crate::dimension::resolution_options();
    let island = p.island.mode();
    let mut objects: Vec<ObjectId> = p.objects.iter().map(|IdParam(id)| *id).collect();
    for id in &objects {
        expect(&snapshot, *id, BOUNDARY_KINDS.as_slice())
            .map_err(|_| boundary_invalid(Some(*id)))?;
        if snapshot.get(snapshot.resolve(*id)?)?.header.owner != Some(owner) {
            return Err(boundary_invalid(Some(*id)));
        }
    }
    let read = |ids: &[ObjectId]| {
        crate::display::with_snapshot(&snapshot, &options, |reader| object_curves(reader, ids))
    };
    let (loops, associated, pick) = if picking {
        let point = p.pick.ok_or_else(|| field_invalid("pick"))?;
        if !p.boundary.is_empty() {
            return Err(field_invalid("boundary"));
        }
        if objects.is_empty() {
            let limit = usize::try_from(Limits::DEFAULT.array_length).unwrap_or(usize::MAX);
            objects = candidates(&snapshot, owner, limit)?;
        }
        let pick = Point2::new(point[0], point[1]);
        let region = trace_region(&read(&objects)?, pick, island, &tol, &Limits::DEFAULT)?;
        let associated = p.associative.unwrap_or(true);
        (loops_of_region(&region, &tol)?, associated, Some(pick))
    } else {
        if p.pick.is_some() {
            return Err(field_invalid("pick"));
        }
        let curves: Vec<Curve2> = if p.boundary.is_empty() {
            read(&objects)?
        } else if objects.is_empty() {
            p.boundary
                .iter()
                .map(|v| serde_json::from_value(v.clone()).map_err(|_| field_invalid("boundary")))
                .collect::<Result<_>>()?
        } else {
            return Err(field_invalid("boundary"));
        };
        // The stored loops are all loops; the island mode applies when the hatch resolves.
        let region = region_from_curves(&curves, None, IslandMode::Alternating, &tol)?;
        // A region that ignores nothing must still be valid under the chosen mode.
        region_from_curves(&curves, None, island, &tol)?;
        (loops_of_region(&region, &tol)?, !objects.is_empty(), None)
    };
    let mut kind: Vec<(usize, Value)> = vec![
        (field::KIND, Value::Enum(p.kind as u32)),
        (field::LOOPS, Value::List(loops)),
        (field::ISLAND, Value::Enum(island_code(island))),
        (field::ANGLE, Value::F64(p.angle)),
    ];
    if associated {
        kind.push((
            field::OBJECTS,
            Value::List(objects.iter().copied().map(object_value).collect()),
        ));
        kind.extend(pick.map(|q| (field::PICK, Value::Point2(q))));
    }
    if let Some(scale) = p.scale {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Error::new(codes::HATCH_PARAMETER_INVALID).with_arg("parameter", "scale"));
        }
        kind.push((field::SCALE, Value::F64(scale)));
    }
    kind.extend(p.origin.map(|o| (field::ORIGIN, point2(o))));
    if let Some(IdParam(pattern)) = p.pattern {
        expect(&snapshot, pattern, &[HATCH_PATTERN])?;
        kind.push((field::PATTERN, Value::Reference(pattern)));
    }
    let mut inline = Vec::new();
    for l in p.inline {
        let line = PatternLine {
            angle: l.angle,
            origin: Point2::new(l.origin[0], l.origin[1]),
            shift: l.shift,
            spacing: l.spacing,
            dashes: l.dashes,
        };
        line.validate()?;
        inline.push(line.to_value());
    }
    if matches!(p.kind, KindParam::Pattern)
        && inline.is_empty()
        && !kind.iter().any(|(i, _)| *i == field::PATTERN)
    {
        return Err(Error::new(codes::PATTERN_EMPTY));
    }
    kind.push((field::INLINE, Value::List(inline)));
    if let Some(g) = p.gradient {
        let radial = match g.kind.as_deref() {
            None | Some("linear") => 0,
            Some("radial") => 1,
            Some(_) => return Err(field_invalid("gradient")),
        };
        kind.push((field::GRADIENT_KIND, Value::Enum(radial)));
        kind.push((field::GRADIENT_ANGLE, Value::F64(g.angle)));
        kind.push((field::GRADIENT_COLOR1, color(g.color1)));
        kind.push((field::GRADIENT_COLOR2, color(g.color2)));
    }
    create_entity(ctx, HATCH, owner, p.layer, kind)
}

fn load_patterns(ctx: &mut Context<'_>, p: LoadPatterns) -> Result<Loaded> {
    let limits = *ctx.limits();
    let file = match &p.text {
        Some(text) => std::sync::Arc::new(PatternFile::parse(text.as_bytes(), &limits)?),
        None => crate::hatch_pattern::library()?,
    };
    let mut chosen = Vec::new();
    if p.names.is_empty() {
        chosen.extend(file.patterns.iter());
    }
    for name in &p.names {
        chosen.push(
            file.get(name).ok_or_else(|| {
                Error::new(codes::PATTERN_UNKNOWN).with_arg("name", name.as_str())
            })?,
        );
    }
    let mut ids = Vec::new();
    for pattern in chosen {
        let values = vec![
            Some(Value::String(pattern.name.clone())),
            pattern.description.clone().map(Value::String),
            Some(Value::List(Vec::<Value>::from(pattern))),
        ];
        ids.push(IdParam(create(ctx, HATCH_PATTERN, None, values)?));
    }
    Ok(Loaded { ids })
}

/// The commands of this module.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.hatch.create", |ctx, p: CreateHatch| {
            create_hatch(ctx, p, false)
        }),
        spec("core.draft.hatch.pick", |ctx, p: CreateHatch| {
            create_hatch(ctx, p, true)
        }),
        family("core.draft.hatch.modify", &[HATCH]),
        spec("core.draft.hatch_pattern.load", load_patterns),
    ]
}
