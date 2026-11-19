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
//! - a block reference and an external reference: position, rotation and per-axis scale,
//!   recomputed from the transformed linear part (a rotation times a diagonal, which every
//!   transform of this command gives: a uniform scale and an orthogonal map keep orthogonal
//!   columns; the check is a guard). The linked drawing of an external reference is in its own
//!   coordinates, so its clip boundary does not change;
//! - a single-line text: position, rotation and height, in any form except a mirror;
//! - a rich text: position and rotation, for translations and rotations only;
//! - a leader: its points; a multileader: its leader points, position and block scale, refused
//!   for a mirror unless it has no content (text reads in one direction);
//! - a hatch: its loops, origin and pick point; a pattern hatch also turns its pattern angle and
//!   scales its pattern, a gradient turns its gradient angle, and both refuse a mirror, which
//!   would reverse the pattern or gradient direction. A solid fill takes a mirror;
//! - a dimension: its definition points, dimension line, text position and rotation, and the
//!   annotative positions; refused for a mirror, as text is. An ordinate dimension measures along
//!   the drawing axes, so it takes translation and scale only and refuses any rotation. A quadrant
//!   key of a circle or arc advances its quarter turn when the rotation is a whole number of quarter
//!   turns (a mirror included), else the dimension is refused; a parameter key of a circle or arc
//!   takes the image angle, and an arc refuses a result outside its new range. Keys of other curves
//!   keep their stored values. The top-level referenced object must be selected; the entries of a
//!   nested path are never required, and selecting one is refused because the shared block
//!   definition would move the point twice;
//! - a table: position, rotation, column widths, row heights, cell block scales and the break
//!   limits; refused for a mirror, as text is;
//! - an array: its frame as a block reference's (position, per-axis scale and rotation), the
//!   grid axis and row spacing of a rectangular array, the center and total angle of a polar
//!   array (negated under a mirror, because the items turn the other way), and the spacing of a
//!   path array; a path array refuses a path that is not selected, and one without a path entity
//!   moves its stored curve with the same map.
//!
//! Associative kinds follow one rule: every object they reference must be in the selection,
//! otherwise the entity is refused with reason `associated`. A hatch is associated only when all of
//! its boundary records still name an object (the rule resolution uses); a hatch with a missing
//! boundary object keeps its stored loops, which move as free data. Their stored geometry is then transformed with the same map as the objects it
//! was derived from, so regeneration gives the transformed result. Text size and arrow size come
//! from the style and are not scaled: only the stored geometry is.
//!
//! The command refuses the whole list when one entity is refused, so nothing changes. An entity
//! on a locked layer is refused with `draft.layer.locked`. Locks apply per entity by its own layer:
//! an entity inside a block definition is judged by its own layer when it is edited, and editing a
//! block reference is allowed even when its definition holds entities on locked layers.

use std::collections::BTreeSet;

use arc_base::math::{Matrix3, Point2, Transform2, Vector3};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{FieldPath, GeometryBytes, Snapshot, Value};
use arc_geom::Curve2;
use arc_geom::encoding::GeomCodec;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::annotation::{Representation, representations};
use crate::commands::{expect, field_invalid, spec, values_of};
use crate::dimension::DefinitionPoint;
use crate::error::codes;
use crate::text::{RICH_TEXT, TEXT};
use crate::types::{self, ENTITIES};

/// A quarter turn in radians.
const QUARTER: f64 = std::f64::consts::FRAC_PI_2;
/// A full turn in radians.
const TAU: f64 = std::f64::consts::TAU;

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
    selected: &BTreeSet<ObjectId>,
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
        crate::leader::LEADER => leader_changes(snapshot, key, &values, map)?,
        crate::leader::MULTILEADER => multileader_changes(snapshot, key, &values, map)?,
        crate::hatch::HATCH => hatch_changes(snapshot, key, &values, map, limits, selected)?,
        crate::dimension::DIMENSION => {
            dimension_changes(snapshot, key, &values, map, limits, selected)?
        }
        crate::table::TABLE => table_changes(key, &values, map)?,
        crate::array::ARRAY => array_changes(snapshot, key, &values, map, limits, selected)?,
        types::EXTERNAL_REF => block_reference(snapshot, key, &values, map)?,
        _ => return Err(refuse(key, "unsupported")),
    })
}

/// The changes of a leader: its points and its annotative positions.
fn leader_changes(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
) -> Result<Vec<(&'static str, Value)>> {
    let field = |name: &str| stored(snapshot, key, values, name);
    let mut out = vec![("points", mapped_points(field("points"), map)?)];
    out.extend(mapped_representations(field("scales"), map));
    Ok(out)
}

/// The changes of a multileader. Text and tolerance content keep their reading direction, so a
/// mirror is refused unless the leader has no content.
fn multileader_changes(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
) -> Result<Vec<(&'static str, Value)>> {
    let field = |name: &str| stored(snapshot, key, values, name);
    if map.mirrors() && !matches!(field("content"), Some(Value::Enum(3))) {
        return Err(refuse(key, "mirror"));
    }
    let leaders = match field("leaders") {
        Some(Value::List(lists)) => Value::List(
            lists
                .iter()
                .map(|list| mapped_points(Some(list), map))
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => Value::List(Vec::new()),
    };
    let mut out = vec![
        ("leaders", leaders),
        (
            "position",
            Value::Point2(map.point(point(field("position")))),
        ),
        (
            "block_scale",
            Value::F64(number(field("block_scale")).unwrap_or(1.0) * map.scale),
        ),
    ];
    out.extend(mapped_representations(field("scales"), map));
    Ok(out)
}

/// The changes of a hatch. Every boundary object must move with the hatch; otherwise its stored
/// loops and pick point would disagree with the boundary that regeneration reads. Pattern lines
/// and gradients have a direction that a mirror would reverse; only a solid fill takes one.
fn hatch_changes(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
    limits: &Limits,
    selected: &BTreeSet<ObjectId>,
) -> Result<Vec<(&'static str, Value)>> {
    let field = |name: &str| stored(snapshot, key, values, name);
    // A hatch is associated only when every boundary record still names an object, which is the
    // rule resolution uses; otherwise its stored loops are the boundary and move as free data.
    let records = crate::hatch::objects_of(field("boundary_objects"));
    let associated = !records.is_empty() && records.iter().all(Option::is_some);
    if associated && records.iter().flatten().any(|b| !selected.contains(b)) {
        return Err(refuse(key, "associated"));
    }
    let kind = match field("kind") {
        Some(Value::Enum(k)) => *k,
        _ => 0,
    };
    if kind != 1 && map.mirrors() {
        return Err(refuse(key, "mirror"));
    }
    let mut out = vec![
        ("loops", mapped_curves(field("loops"), map, limits)?),
        ("origin", Value::Point2(map.point(point(field("origin"))))),
    ];
    if let Some(Value::Point2(pick)) = field("pick") {
        out.push(("pick", Value::Point2(map.point(*pick))));
    }
    match kind {
        0 => {
            let angle = number(field("angle")).unwrap_or(0.0);
            let scale = number(field("scale")).unwrap_or(1.0);
            out.push(("angle", Value::F64(angle + map.angle)));
            out.push(("scale", Value::F64(scale * map.scale)));
        }
        2 => {
            let angle = number(field("gradient_angle")).unwrap_or(0.0);
            out.push(("gradient_angle", Value::F64(angle + map.angle)));
        }
        _ => {}
    }
    Ok(out)
}

/// The angle of the image of the unit direction at `angle` under the linear part of `map`.
fn image_angle(map: &Affine, angle: f64) -> f64 {
    let (sin, cos) = angle.sin_cos();
    (map.l[1][0] * cos + map.l[1][1] * sin).atan2(map.l[0][0] * cos + map.l[0][1] * sin)
}

/// The angle of quarter turn `q`, clamped to 0 to 3.
fn quarter_angle(q: i64) -> f64 {
    f64::from(i32::try_from(q.clamp(0, 3)).unwrap_or(0)) * QUARTER
}

/// The quarter turn (0 to 3) an angle lands on, or `None` when it is not a whole number of quarter
/// turns.
fn quarter_of(angle: f64) -> Option<i64> {
    let turn = angle.rem_euclid(TAU) / QUARTER;
    (0..4_i64).find(|q| {
        let d = (turn - f64::from(i32::try_from(*q).unwrap_or(0))).rem_euclid(4.0);
        d < 1e-9 || 4.0 - d < 1e-9
    })
}

/// Whether `angle` is a whole number of turns.
fn is_whole_turn(angle: f64) -> bool {
    let r = angle.rem_euclid(TAU);
    r < 1e-12 || TAU - r < 1e-12
}

/// The circle or arc `id`, decoded, when `id` is one; `None` for any other entity.
fn circle_or_arc(snapshot: &Snapshot, id: ObjectId, limits: &Limits) -> Result<Option<Curve2>> {
    let handle = snapshot.resolve(id)?;
    let Some(key) = snapshot.type_key(handle).map(|k| k.as_str().to_owned()) else {
        return Ok(None);
    };
    if key != types::CIRCLE && key != types::ARC {
        return Ok(None);
    }
    let values = values_of(snapshot, id)?;
    match stored(snapshot, &key, &values, "geometry") {
        Some(Value::Geometry(bytes)) => Ok(Some(Curve2::decode(&bytes.0, limits)?)),
        _ => Ok(None),
    }
}

/// The changes of a dimension. Its text reads in one direction, so a mirror is refused as for
/// single-line text. Every referenced object, and every nested path entry, must be selected.
fn dimension_changes(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
    limits: &Limits,
    selected: &BTreeSet<ObjectId>,
) -> Result<Vec<(&'static str, Value)>> {
    let field = |name: &str| stored(snapshot, key, values, name);
    if map.mirrors() {
        return Err(refuse(key, "mirror"));
    }
    // An ordinate measures along the drawing axes, so only translation and scale keep its value.
    let ordinate = matches!(
        field("kind"),
        Some(Value::Enum(code))
            if matches!(
                crate::dimension::Kind::of(*code),
                Some(crate::dimension::Kind::OrdinateX | crate::dimension::Kind::OrdinateY)
            )
    );
    if ordinate && !is_whole_turn(map.angle) {
        return Err(refuse(key, "rotation"));
    }
    let points = crate::dimension::points_of(values);
    // The top-level referenced objects must move with the dimension. The entries of a nested path
    // are inside a shared block definition that the top-level block reference moves: they are never
    // required, and selecting one would move the point twice, so it is refused.
    let referenced: Vec<ObjectId> = points.iter().filter_map(|p| p.object).collect();
    if referenced.iter().any(|r| !selected.contains(r)) {
        return Err(refuse(key, "associated"));
    }
    let nested = points.iter().flat_map(|p| p.path.iter().flatten().copied());
    if nested.into_iter().any(|r| selected.contains(&r)) {
        return Err(refuse(key, "associated"));
    }
    let mut moved: Vec<Value> = Vec::with_capacity(points.len());
    for p in &points {
        let mut image = DefinitionPoint {
            position: map.point(p.position),
            ..p.clone()
        };
        // A quadrant or parameter of a circle or arc is an angle about the center; the image of
        // the point gives the new one. A nested key is in the block's frame and moves with it.
        if p.path.is_empty()
            && let Some(object) = p.object
            && matches!(
                p.key,
                crate::dimension::Key::Quadrant | crate::dimension::Key::Parameter
            )
            && let Some(curve) = circle_or_arc(snapshot, object, limits)?
        {
            let refused = || refuse(key, "associated");
            match (p.key, &curve) {
                (crate::dimension::Key::Quadrant, _) => {
                    let angle = image_angle(map, quarter_angle(p.quadrant));
                    image.quadrant = quarter_of(angle).ok_or_else(refused)?;
                }
                (_, Curve2::Circle(_)) => {
                    image.parameter = image_angle(map, p.parameter).rem_euclid(TAU);
                }
                (_, Curve2::Arc(arc)) => {
                    let angle = image_angle(map, arc.angle_at(p.parameter));
                    let Curve2::Arc(new) =
                        Curve2::Arc(*arc).transformed(&map.transform2(), &Tolerance::DEFAULT)?
                    else {
                        return Err(refused());
                    };
                    let t = ((angle - new.start_angle()) * new.sweep().signum()).rem_euclid(TAU);
                    if t > new.sweep().abs() + 1e-9 {
                        return Err(refused());
                    }
                    image.parameter = t;
                }
                _ => {}
            }
        }
        moved.push(image.to_value());
    }
    let rotation = number(field("rotation")).unwrap_or(0.0);
    let mut out = vec![
        ("points", Value::List(moved)),
        (
            "line_position",
            Value::Point2(map.point(point(field("line_position")))),
        ),
        ("rotation", Value::F64(rotation + map.angle)),
    ];
    if let Some(Value::Point2(text)) = field("text_position") {
        out.push(("text_position", Value::Point2(map.point(*text))));
    }
    out.extend(mapped_representations(field("scales"), map));
    Ok(out)
}

/// The changes of a table. Cell text reads in one direction, so a mirror is refused as for
/// dimensions. Sizes scale with the map; the text heights come from the table style.
fn table_changes(
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
) -> Result<Vec<(&'static str, Value)>> {
    if map.mirrors() {
        return Err(refuse(key, "mirror"));
    }
    let mut table = crate::table::Table::from_values(values);
    table.position = map.point(table.position);
    table.rotation += map.angle;
    for width in &mut table.widths {
        *width *= map.scale;
    }
    for row in &mut table.rows {
        row.height *= map.scale;
    }
    for cell in &mut table.cells {
        if let crate::table::Content::Block { scale, .. } = &mut cell.content {
            *scale *= map.scale;
        }
    }
    table.max_height *= map.scale;
    table.break_gap *= map.scale;
    let mut out = table.stored();
    out.push(("position", Value::Point2(table.position)));
    out.push(("rotation", Value::F64(table.rotation)));
    out.push(("max_height", Value::F64(table.max_height)));
    out.push(("break_gap", Value::F64(table.break_gap)));
    Ok(out)
}

/// The changes of an array: its frame moves like a block reference's (position, per-axis scale
/// and rotation), and its layout follows the map. A rectangular grid keeps its shape: the image
/// of its column axis gives the new axis angle, and a mirror negates the row spacing. A polar
/// array turns the other way under a mirror, so its total angle is negated. The spacing of a
/// path array is a length; its path must be selected, because the items follow it.
fn array_changes(
    snapshot: &Snapshot,
    key: &str,
    values: &[Option<Value>],
    map: &Affine,
    limits: &Limits,
    selected: &BTreeSet<ObjectId>,
) -> Result<Vec<(&'static str, Value)>> {
    use crate::array::{Kind, Params};
    let field_of = |name: &str| stored(snapshot, key, values, name);
    let params = Params::from_values(values);
    let mut out = block_reference(snapshot, key, values, map)?;
    match params.kind {
        Kind::Rectangular => {
            let (sin, cos) = params.angle.sin_cos();
            let axis = [
                map.l[0][0] * cos + map.l[0][1] * sin,
                map.l[1][0] * cos + map.l[1][1] * sin,
            ];
            let mirror = if map.mirrors() { -1.0 } else { 1.0 };
            out.push(("angle", Value::F64(axis[1].atan2(axis[0]))));
            out.push((
                "column_spacing",
                Value::F64(params.column_spacing * map.scale),
            ));
            out.push((
                "row_spacing",
                Value::F64(params.row_spacing * mirror * map.scale),
            ));
        }
        Kind::Polar => {
            let mirror = if map.mirrors() { -1.0 } else { 1.0 };
            out.push(("center", Value::Point2(map.point(params.center))));
            out.push(("total_angle", Value::F64(params.total_angle * mirror)));
        }
        Kind::Path => {
            match (field_of("path"), field_of("curve")) {
                // A referenced path must be selected: the items follow its transformed geometry.
                (Some(Value::Reference(path)), _) if !selected.contains(path) => {
                    return Err(refuse(key, "associated"));
                }
                (Some(Value::Reference(_)), _) => {}
                // Without a path entity the array stores its own curve, which moves with the map.
                (_, Some(Value::Geometry(bytes))) => {
                    let curve = Curve2::decode(&bytes.0, limits)?;
                    let image = curve.transformed(&map.transform2(), &Tolerance::DEFAULT)?;
                    out.push(("curve", Value::Geometry(GeometryBytes(image.encode()))));
                }
                _ => {}
            }
            out.push(("spacing", Value::F64(params.spacing * map.scale)));
        }
    }
    Ok(out)
}

/// The stored points of a list value, under `map`.
fn mapped_points(value: Option<&Value>, map: &Affine) -> Result<Value> {
    match value {
        Some(Value::List(items)) => Ok(Value::List(
            items
                .iter()
                .map(|item| match item {
                    Value::Point2(p) => Ok(Value::Point2(map.point(*p))),
                    _ => Err(Error::new(codes::GEOMETRY_INVALID)),
                })
                .collect::<Result<Vec<_>>>()?,
        )),
        _ => Err(Error::new(codes::GEOMETRY_INVALID)),
    }
}

/// The stored curves of a list of geometry values, each under `map`.
fn mapped_curves(value: Option<&Value>, map: &Affine, limits: &Limits) -> Result<Value> {
    let Some(Value::List(items)) = value else {
        return Err(Error::new(codes::GEOMETRY_INVALID));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Value::Geometry(bytes) = item else {
            return Err(Error::new(codes::GEOMETRY_INVALID));
        };
        let curve = Curve2::decode(&bytes.0, limits)?;
        let image = curve.transformed(&map.transform2(), &Tolerance::DEFAULT)?;
        out.push(Value::Geometry(GeometryBytes(image.encode())));
    }
    Ok(Value::List(out))
}

/// The `scales` field of an annotative object with its positions under `map`; absent when it
/// has no representations.
fn mapped_representations(value: Option<&Value>, map: &Affine) -> Option<(&'static str, Value)> {
    let reps = representations(value);
    if reps.is_empty() {
        return None;
    }
    let moved = reps
        .iter()
        .map(|r| {
            Representation {
                scale: r.scale,
                position: r.position.map(|p| map.point(p)),
                text_position: r.text_position.map(|p| map.point(p)),
            }
            .to_value()
        })
        .collect();
    Some(("scales", Value::List(moved)))
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
fn finite(value: &Value, limits: &Limits) -> bool {
    match value {
        Value::F64(v) => v.is_finite(),
        Value::Point2(p) => p.x.is_finite() && p.y.is_finite(),
        Value::Vector3(v) => v.x.is_finite() && v.y.is_finite() && v.z.is_finite(),
        Value::List(items) => items.iter().all(|v| finite(v, limits)),
        Value::Record(fields) => fields.iter().flatten().all(|v| finite(v, limits)),
        // A geometry value is finite when it decodes: the encoding carries only finite numbers.
        Value::Geometry(bytes) => Curve2::decode(&bytes.0, limits).is_ok(),
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
    let selected: BTreeSet<ObjectId> = ids.iter().copied().collect();
    let txn = ctx.txn()?;
    let mut planned = Vec::with_capacity(ids.len());
    for id in ids {
        let snapshot = txn.snapshot();
        let key = expect(snapshot, id, ENTITIES)?;
        crate::lock::check_entity(snapshot, id)?;
        let changes = changes(snapshot, id, &key, &map, &limits, &selected)?;
        if !changes.iter().all(|(_, value)| finite(value, &limits)) {
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
