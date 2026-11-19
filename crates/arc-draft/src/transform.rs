//! The entity transform command (`core.draft.entity.transform`): translation, rotation, uniform
//! scale and mirror of existing entities, applied through the transaction so validators and undo
//! apply as for every modify command.
//!
//! The transform is `T · R · S · M` applied to a point, where `M` mirrors across the line through
//! the origin with the given direction (first), `S` scales uniformly about the origin, `R` rotates
//! counterclockwise about the origin and `T` translates (last). The transform acts in the plane of
//! the drawing: `x` and `y` change, `elevation`, `thickness` and the `scale.z` of a block reference
//! do not. An entity whose extrusion is not the default `(0, 0, 1)` is refused, because a mirror
//! or rotation of its plane is not expressed by these fields.
//!
//! Per kind:
//!
//! - a point, a line, ray, construction line, circle, arc, ellipse, elliptical arc, polyline,
//!   wipeout and spline: the geometry (and the fit points of a spline) under the transform, in
//!   any form;
//! - a block reference: position, rotation and per-axis scale, recomputed from the transformed
//!   linear part (a rotation times a diagonal, which every transform of this command gives: a
//!   uniform scale and an orthogonal map keep orthogonal columns; the check is a guard);
//! - a single-line text: position, rotation and height, in any form except a mirror;
//! - a rich text: position and rotation, for translations and rotations only.
//!
//! Dimensions, leaders, multileaders, hatches, tables and arrays are derived from other objects
//! or their own parameters, and external references need their own placement rules; they are
//! refused with `draft.entity.transform_unsupported` naming the kind. The command refuses the
//! whole list when one entity is refused, so nothing changes.

use std::collections::BTreeSet;

use arc_base::math::{Matrix3, Point2, Transform2, Vector3};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{FieldPath, GeometryBytes, Snapshot, Value};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{expect, field_invalid, spec, values_of};
use crate::error::codes;
use crate::text::{RICH_TEXT, TEXT};
use crate::types::{self, ENTITIES};

/// The command identifier.
const ID: &str = "core.draft.entity.transform";

/// Parameters of `core.draft.entity.transform`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Params {
    /// The entities to transform, each of an entity type; a repeated identifier counts once. At
    /// most 10,000; the count is checked against the parameter schema before the list is read.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// The transform.
    pub transform: TransformParam,
}

/// The transform: mirror, then uniform scale and rotation about the origin, then translation.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformParam {
    /// The direction of the mirror line through the origin, applied first; absent for no mirror.
    /// Must be nonzero.
    #[serde(default)]
    pub mirror: Option<[f64; 2]>,
    /// The uniform scale factor about the origin, greater than zero. Default 1.
    #[serde(default = "one")]
    pub scale: f64,
    /// The rotation about the origin in radians, counterclockwise. Default 0.
    #[serde(default)]
    pub rotation: f64,
    /// The translation in drawing units, applied last. Default `[0, 0]`.
    #[serde(default)]
    pub translation: [f64; 2],
}

fn one() -> f64 {
    1.0
}

/// The registered command specifications of this module.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![spec(ID, transform)]
}

/// The affine map of a [`TransformParam`]: `x ↦ L x + t` with the linear part `L` given by its
/// entries, plus the scale and rotation angle the map was built from.
#[derive(Debug, Clone, Copy)]
struct Affine {
    /// The entries of `L` by row: `[[a, b], [c, d]]`.
    l: [[f64; 2]; 2],
    t: [f64; 2],
    scale: f64,
    angle: f64,
}

impl Affine {
    fn new(p: &TransformParam) -> Result<Self> {
        let invalid = |field: &str| field_invalid(field);
        let finite = |v: f64| v.is_finite();
        if !finite(p.scale) || p.scale <= 0.0 {
            return Err(invalid("scale"));
        }
        if !finite(p.rotation) || !p.translation.iter().all(|v| finite(*v)) {
            return Err(invalid("transform"));
        }
        // The mirror matrix 2 d dᵀ / |d|² − I for the line through the origin with direction d.
        let mirror = match p.mirror {
            None => [[1.0, 0.0], [0.0, 1.0]],
            Some([dx, dy]) => {
                let n2 = dx * dx + dy * dy;
                if !(finite(dx) && finite(dy) && n2.is_finite() && n2 > 0.0) {
                    return Err(invalid("mirror"));
                }
                [
                    [2.0 * dx * dx / n2 - 1.0, 2.0 * dx * dy / n2],
                    [2.0 * dx * dy / n2, 2.0 * dy * dy / n2 - 1.0],
                ]
            }
        };
        let (sin, cos) = p.rotation.sin_cos();
        // L = R · (s I) · M, with R the counterclockwise rotation.
        let s = p.scale;
        let r = [[cos * s, -sin * s], [sin * s, cos * s]];
        let l = [
            [
                r[0][0] * mirror[0][0] + r[0][1] * mirror[1][0],
                r[0][0] * mirror[0][1] + r[0][1] * mirror[1][1],
            ],
            [
                r[1][0] * mirror[0][0] + r[1][1] * mirror[1][0],
                r[1][0] * mirror[0][1] + r[1][1] * mirror[1][1],
            ],
        ];
        if !l.iter().flatten().all(|v| finite(*v)) {
            return Err(invalid("transform"));
        }
        Ok(Self {
            l,
            t: p.translation,
            scale: s,
            angle: p.rotation,
        })
    }

    /// Whether the map reverses orientation.
    fn mirrors(&self) -> bool {
        self.l[0][0] * self.l[1][1] - self.l[0][1] * self.l[1][0] < 0.0
    }

    fn point(&self, p: Point2) -> Point2 {
        Point2::new(
            self.l[0][0] * p.x + self.l[0][1] * p.y + self.t[0],
            self.l[1][0] * p.x + self.l[1][1] * p.y + self.t[1],
        )
    }

    fn transform2(&self) -> Transform2 {
        Transform2::from_matrix_unchecked(Matrix3::new(
            self.l[0][0],
            self.l[0][1],
            self.t[0],
            self.l[1][0],
            self.l[1][1],
            self.t[1],
            0.0,
            0.0,
            1.0,
        ))
    }
}

fn refuse(kind: &str, reason: &'static str) -> Error {
    Error::new(codes::ENTITY_TRANSFORM_UNSUPPORTED)
        .with_arg("kind", kind)
        .with_arg("reason", reason)
}

/// The stored value of `name` in an entity of type `key`, `None` when absent or unset.
fn stored<'a>(
    snapshot: &Snapshot,
    key: &str,
    values: &'a [Option<Value>],
    name: &str,
) -> Option<&'a Value> {
    let index = snapshot
        .registry()
        .get(key)?
        .schema
        .fields
        .iter()
        .position(|f| f.name == name)?;
    values.get(index)?.as_ref()
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::F64(v) => Some(*v),
        _ => None,
    }
}

fn point(value: Option<&Value>) -> Point2 {
    match value {
        Some(Value::Point2(p)) => *p,
        _ => Point2::origin(),
    }
}

/// The field changes that move the entity `id` of type `key` under `map`.
fn changes(
    snapshot: &Snapshot,
    id: ObjectId,
    key: &str,
    map: &Affine,
    limits: &Limits,
) -> Result<Vec<(&'static str, Value)>> {
    let values = values_of(snapshot, id)?;
    let field = |name: &str| stored(snapshot, key, &values, name);
    if let Some(Value::Vector3(v)) = field("extrusion")
        && *v != Vector3::new(0.0, 0.0, 1.0)
    {
        return Err(refuse(key, "extrusion"));
    }
    let geometry = || match field("geometry") {
        Some(Value::Geometry(bytes)) => Ok(bytes),
        _ => Err(Error::new(codes::GEOMETRY_INVALID)),
    };
    let curve_kinds = [
        types::LINE,
        types::RAY,
        types::CONSTRUCTION_LINE,
        types::CIRCLE,
        types::ARC,
        types::ELLIPSE,
        types::ELLIPTICAL_ARC,
        types::POLYLINE,
        types::WIPEOUT,
    ];
    Ok(match key {
        types::POINT => vec![(
            "position",
            Value::Point2(map.point(point(field("position")))),
        )],
        k if curve_kinds.contains(&k) => {
            let curve = Curve2::decode(&geometry()?.0, limits)?;
            let image = curve.transformed(&map.transform2(), &Tolerance::DEFAULT)?;
            vec![("geometry", Value::Geometry(GeometryBytes(image.encode())))]
        }
        types::SPLINE => {
            let bytes = geometry()?;
            let curve = arc_geom::Nurbs2::decode(&bytes.0, limits)?;
            let image = Curve2::Nurbs(curve).transformed(&map.transform2(), &Tolerance::DEFAULT)?;
            let Curve2::Nurbs(image) = image else {
                return Err(Error::new(codes::GEOMETRY_INVALID));
            };
            let mut out = vec![("geometry", Value::Geometry(GeometryBytes(image.encode())))];
            // Only stored fit points move: a spline without them gets none.
            if let Some(Value::List(items)) = field("fit_points") {
                let fit = items
                    .iter()
                    .map(|v| match v {
                        Value::Point2(p) => Ok(Value::Point2(map.point(*p))),
                        _ => Err(Error::new(codes::GEOMETRY_INVALID)),
                    })
                    .collect::<Result<Vec<_>>>()?;
                out.push(("fit_points", Value::List(fit)));
            }
            out
        }
        types::BLOCK_REF => block_reference(snapshot, key, &values, map)?,
        TEXT => {
            if map.mirrors() {
                return Err(refuse(key, "mirror"));
            }
            let rotation = number(field("rotation")).unwrap_or(0.0);
            let height = number(field("height")).unwrap_or(0.0);
            vec![
                (
                    "position",
                    Value::Point2(map.point(point(field("position")))),
                ),
                ("rotation", Value::F64(rotation + map.angle)),
                ("height", Value::F64(height * map.scale)),
            ]
        }
        RICH_TEXT => {
            if map.mirrors() {
                return Err(refuse(key, "mirror"));
            }
            if map.scale != 1.0 {
                return Err(refuse(key, "scale"));
            }
            let rotation = number(field("rotation")).unwrap_or(0.0);
            vec![
                (
                    "position",
                    Value::Point2(map.point(point(field("position")))),
                ),
                ("rotation", Value::F64(rotation + map.angle)),
            ]
        }
        _ => {
            let reason = match key {
                crate::array::ARRAY | types::EXTERNAL_REF => "external",
                _ => "associative",
            };
            return Err(refuse(key, reason));
        }
    })
}

/// The fields of a block reference under `map`. Its map `p ↦ position + A (p − base)` becomes
/// `p ↦ L position + t + (L A)(p − base)`, so the new linear part `B = L R(θ) diag(sx, sy)` is
/// decomposed as `R(θ') diag(p, q)`: it is possible when the columns of `B` are orthogonal.
fn block_reference(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
) -> Result<Vec<(&'static str, Value)>> {
    let field = |name: &str| stored(snapshot, key, values, name);
    let position = point(field("position"));
    let (sx, sy, sz) = match field("scale") {
        Some(Value::Vector3(v)) => (v.x, v.y, v.z),
        _ => (1.0, 1.0, 1.0),
    };
    let rotation = number(field("rotation")).unwrap_or(0.0);
    let (sin, cos) = rotation.sin_cos();
    // B = L · R(θ) · diag(sx, sy), with R(θ) · diag(sx, sy) = [[c sx, −s sy], [s sx, c sy]].
    let a = [[cos * sx, -sin * sy], [sin * sx, cos * sy]];
    let b: [[f64; 2]; 2] = [
        [
            map.l[0][0] * a[0][0] + map.l[0][1] * a[1][0],
            map.l[0][0] * a[0][1] + map.l[0][1] * a[1][1],
        ],
        [
            map.l[1][0] * a[0][0] + map.l[1][1] * a[1][0],
            map.l[1][0] * a[0][1] + map.l[1][1] * a[1][1],
        ],
    ];
    // Columns c1 = (b00, b10) and c2 = (b01, b11): orthogonal columns give R(θ') diag(p, q) with
    // p = |c1|, θ' = angle of c1 and q = det / p.
    let (c1, c2) = ([b[0][0], b[1][0]], [b[0][1], b[1][1]]);
    let norm1 = c1[0].hypot(c1[1]);
    let norm2 = c2[0].hypot(c2[1]);
    let dot = c1[0] * c2[0] + c1[1] * c2[1];
    let det = b[0][0] * b[1][1] - b[0][1] * b[1][0];
    let orthogonal = dot.abs() <= 1e-9 * norm1 * norm2;
    if norm1 <= 0.0 || !orthogonal || !det.is_finite() {
        return Err(refuse(key, "non_uniform_scale"));
    }
    let p = norm1;
    let q = det / norm1;
    let angle = c1[1].atan2(c1[0]);
    let scale = Vector3::new(p, q, sz);
    Ok(vec![
        ("position", Value::Point2(map.point(position))),
        ("scale", Value::Vector3(scale)),
        ("rotation", Value::F64(angle)),
    ])
}

/// Whether every number of a value is finite: a result that overflowed is refused, not stored.
fn finite(value: &Value) -> bool {
    match value {
        Value::F64(v) => v.is_finite(),
        Value::Point2(p) => p.x.is_finite() && p.y.is_finite(),
        Value::Vector3(v) => v.x.is_finite() && v.y.is_finite() && v.z.is_finite(),
        Value::List(items) => items.iter().all(finite),
        _ => true,
    }
}

/// Transforms the listed entities. Each one is checked before any change, and every new value
/// must be finite; a refused entity or value fails the whole command.
fn transform(ctx: &mut Context<'_>, p: Params) -> Result<()> {
    let map = Affine::new(&p.transform)?;
    let limits = *ctx.limits();
    let mut seen = BTreeSet::new();
    let ids: Vec<ObjectId> = p
        .objects
        .into_iter()
        .map(|IdParam(id)| id)
        .filter(|id| seen.insert(*id))
        .collect();
    let txn = ctx.txn()?;
    let mut planned = Vec::with_capacity(ids.len());
    for id in ids {
        let snapshot = txn.snapshot();
        let key = expect(snapshot, id, ENTITIES)?;
        let changes = changes(snapshot, id, &key, &map, &limits)?;
        if !changes.iter().all(|(_, value)| finite(value)) {
            return Err(field_invalid("transform"));
        }
        planned.push((id, changes));
    }
    for (id, changes) in planned {
        for (name, value) in changes {
            txn.modify(id, FieldPath::new(name), Some(value))?;
        }
    }
    Ok(())
}
