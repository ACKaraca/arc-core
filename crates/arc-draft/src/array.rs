//! Reference arrays (CORE-DRAFT-024): an entity that inserts a block definition many times.
//!
//! An array holds the reference fields of a block reference (`block`, `position`, `scale`,
//! `rotation`, which place the first item) and the parameters of one of three layouts
//! ([`Kind`]). [`items`] maps the parameters to one placement per item, and the display list
//! of the array is the list of a block reference for each of them, so the array is regenerated
//! as one unit when a parameter, the definition or an entity of the definition changes. The
//! rules are stated in the crate specification, section "Fields, tables and arrays as
//! implemented".

use std::f64::consts::TAU;

use arc_base::math::{Point2, Vector3};
use arc_base::{Error, ObjectId, Result, Tolerance};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{
    Bound, Cardinality, FieldDecl, Finding, GeometryBytes, OnDelete, Snapshot, Strength, Value,
    ValueType,
};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Curve2, CurveEval};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{Created, create_entity, expect, family, field_invalid, point2, spec};
use crate::dimension::clearing_reference;
use crate::display::{
    self, DisplayOptions, EntityList, Look, Primitive, Reader, decode_curve, read, reference_list,
};
use crate::error::codes;
use crate::types::{
    BLOCK, common_findings, entity, finding, flag, geometry, real, reference, variants, with_kind,
};

/// A reference array: common fields, then the fields of [`field`].
pub const ARRAY: &str = "core.draft.array";

/// The most items of one array.
pub const MAX_ITEMS: u64 = 10_000;

/// Field indices of an array.
pub mod field {
    use crate::types::entity::KIND as K;
    /// `block`: the inserted definition.
    pub const BLOCK: usize = K;
    /// `position`: the insertion point of the first item.
    pub const POSITION: usize = K + 1;
    /// `scale`: per-axis factors of every item.
    pub const SCALE: usize = K + 2;
    /// `rotation` of every item, in radians.
    pub const ROTATION: usize = K + 3;
    /// `kind`: `rectangular`, `polar` or `path`.
    pub const KIND: usize = K + 4;
    /// `rows` of a rectangular array.
    pub const ROWS: usize = K + 5;
    /// `columns` of a rectangular array.
    pub const COLUMNS: usize = K + 6;
    /// `row_spacing` of a rectangular array.
    pub const ROW_SPACING: usize = K + 7;
    /// `column_spacing` of a rectangular array.
    pub const COLUMN_SPACING: usize = K + 8;
    /// `angle` of the axes of a rectangular array.
    pub const ANGLE: usize = K + 9;
    /// `center` of a polar array.
    pub const CENTER: usize = K + 10;
    /// `count` of a polar array, or of a path array without spacing.
    pub const COUNT: usize = K + 11;
    /// `total_angle` of a polar array.
    pub const TOTAL_ANGLE: usize = K + 12;
    /// `rotate_items` of a polar array.
    pub const ROTATE_ITEMS: usize = K + 13;
    /// `path`: the curve entity of a path array.
    pub const PATH: usize = K + 14;
    /// `curve`: the stored curve of a path array without a path entity.
    pub const CURVE: usize = K + 15;
    /// `spacing` along a path array; 0 divides the path by `count`.
    pub const SPACING: usize = K + 16;
    /// `align` of the items of a path array to the path.
    pub const ALIGN: usize = K + 17;
}

const NAMES: [&str; 18] = [
    "block",
    "position",
    "scale",
    "rotation",
    "kind",
    "rows",
    "columns",
    "row_spacing",
    "column_spacing",
    "angle",
    "center",
    "count",
    "total_angle",
    "rotate_items",
    "path",
    "curve",
    "spacing",
    "align",
];

/// The layout of an array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Rows and columns on axes at an angle.
    Rectangular = 0,
    /// Items on a circle about a center.
    Polar = 1,
    /// Items along a curve.
    Path = 2,
}

/// The parameters of an array.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// The layout.
    pub kind: Kind,
    /// The insertion point of the first item.
    pub position: Point2,
    /// The rotation of every item.
    pub rotation: f64,
    /// Rectangular: the number of rows (items along the y axis of the array).
    pub rows: i64,
    /// Rectangular: the number of columns (items along its x axis).
    pub columns: i64,
    /// Rectangular: the distance between rows.
    pub row_spacing: f64,
    /// Rectangular: the distance between columns.
    pub column_spacing: f64,
    /// Rectangular: the angle of the x axis of the array.
    pub angle: f64,
    /// Polar: the center.
    pub center: Point2,
    /// Polar, and path without spacing: the number of items.
    pub count: i64,
    /// Polar: the angle from the first to the last item, or the whole circle for a full turn.
    pub total_angle: f64,
    /// Polar: whether the items turn with their position.
    pub rotate_items: bool,
    /// Path: the distance between items along the path; 0 uses `count`.
    pub spacing: f64,
    /// Path: whether the items turn with the tangent of the path.
    pub align: bool,
}

/// One item: the insertion point and rotation of a block reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    /// The insertion point.
    pub position: Point2,
    /// The rotation in radians.
    pub rotation: f64,
}

fn invalid(reason: &str) -> Error {
    Error::new(codes::ARRAY_INVALID).with_arg("reason", reason)
}

fn too_many() -> Error {
    Error::new(codes::ARRAY_TOO_MANY_ITEMS)
        .with_arg("limit", i64::try_from(MAX_ITEMS).unwrap_or(i64::MAX))
}

impl Params {
    /// The parameters of the stored values of an array (all fields, common ones first).
    #[must_use]
    pub fn from_values(values: &[Option<Value>]) -> Self {
        let real = |i: usize, default: f64| match values.get(i) {
            Some(Some(Value::F64(x))) => *x,
            _ => default,
        };
        let int = |i: usize| match values.get(i) {
            Some(Some(Value::I64(n))) => *n,
            _ => 1,
        };
        let flag = |i: usize| !matches!(values.get(i), Some(Some(Value::Bool(false))));
        let point = |i: usize| match values.get(i) {
            Some(Some(Value::Point2(p))) => *p,
            _ => Point2::origin(),
        };
        Self {
            kind: match values.get(field::KIND) {
                Some(Some(Value::Enum(1))) => Kind::Polar,
                Some(Some(Value::Enum(2))) => Kind::Path,
                _ => Kind::Rectangular,
            },
            position: point(field::POSITION),
            rotation: real(field::ROTATION, 0.0),
            rows: int(field::ROWS),
            columns: int(field::COLUMNS),
            row_spacing: real(field::ROW_SPACING, 0.0),
            column_spacing: real(field::COLUMN_SPACING, 0.0),
            angle: real(field::ANGLE, 0.0),
            center: point(field::CENTER),
            count: int(field::COUNT),
            total_angle: real(field::TOTAL_ANGLE, TAU),
            rotate_items: flag(field::ROTATE_ITEMS),
            spacing: real(field::SPACING, 0.0),
            align: flag(field::ALIGN),
        }
    }

    /// Checks the parameters of the layout in use.
    ///
    /// # Errors
    ///
    /// `draft.array.invalid` (argument `reason`) for a value that is not finite, a count below
    /// 1, a polar total angle of 0 or a negative spacing; `draft.array.too_many_items` for more
    /// than [`MAX_ITEMS`] items.
    pub fn validate(&self) -> Result<()> {
        let finite = [
            self.position.x,
            self.position.y,
            self.rotation,
            self.row_spacing,
            self.column_spacing,
            self.angle,
            self.center.x,
            self.center.y,
            self.total_angle,
            self.spacing,
        ];
        if finite.iter().any(|x| !x.is_finite()) {
            return Err(invalid("not_finite"));
        }
        let count = match self.kind {
            Kind::Rectangular => {
                if self.rows < 1 || self.columns < 1 {
                    return Err(invalid("count"));
                }
                self.rows.checked_mul(self.columns).ok_or_else(too_many)?
            }
            Kind::Polar => {
                if self.total_angle == 0.0 {
                    return Err(invalid("total_angle"));
                }
                self.count
            }
            Kind::Path => {
                if self.spacing < 0.0 {
                    return Err(invalid("spacing"));
                }
                if self.spacing > 0.0 { 1 } else { self.count }
            }
        };
        if count < 1 {
            return Err(invalid("count"));
        }
        if u64::try_from(count).is_ok_and(|n| n > MAX_ITEMS) {
            return Err(too_many());
        }
        Ok(())
    }
}

fn counted(n: i64) -> impl Iterator<Item = u32> {
    0..u32::try_from(n).unwrap_or(0)
}

/// The items of an array, in order: row by row for a rectangular array, around the circle for
/// a polar one, along the path for a path array. The first item is at the array's position and
/// rotation.
///
/// - Rectangular: item (row, column) is at `position` plus the vector
///   (column · `column_spacing`, row · `row_spacing`) turned by `angle`; the items keep the
///   rotation of the array.
/// - Polar: item *i* is at the position turned about `center` by *i* · *step*, where *step* is
///   `total_angle` / `count` for a full turn (`total_angle` at least 2π in magnitude, within
///   the angular tolerance) and `total_angle` / (`count` − 1) otherwise (0 for one item). With
///   `rotate_items` it is also rotated by the same angle.
/// - Path: item *i* is at `position` plus the vector from the start of the curve to its point at
///   the arc length *sᵢ*. With a positive `spacing`, *sᵢ* = *i* · `spacing` for every *i* up to
///   the length of the curve; otherwise `count` items are spread over the curve (the first at
///   the start, the last at the end, or for a closed curve evenly around it). With `align` an
///   item is rotated by the angle of the tangent there minus that at the start.
///
/// # Errors
///
/// The errors of [`Params::validate`]; `draft.array.invalid` for a path array without a curve
/// or with a curve of no finite length; the errors of the curve evaluation.
pub fn items(params: &Params, curve: Option<&Curve2>, tol: &Tolerance) -> Result<Vec<Item>> {
    params.validate()?;
    let p = params;
    match p.kind {
        Kind::Rectangular => {
            let (sin, cos) = p.angle.sin_cos();
            let mut out = Vec::new();
            for r in counted(p.rows) {
                for c in counted(p.columns) {
                    let (x, y) = (
                        f64::from(c) * p.column_spacing,
                        f64::from(r) * p.row_spacing,
                    );
                    out.push(Item {
                        position: Point2::new(
                            p.position.x + cos * x - sin * y,
                            p.position.y + sin * x + cos * y,
                        ),
                        rotation: p.rotation,
                    });
                }
            }
            Ok(out)
        }
        Kind::Polar => {
            let full = p.total_angle.abs() >= TAU - tol.angular;
            let step = if full {
                p.total_angle / f64::from(u32::try_from(p.count).unwrap_or(1))
            } else if p.count > 1 {
                p.total_angle / f64::from(u32::try_from(p.count - 1).unwrap_or(1))
            } else {
                0.0
            };
            let (dx, dy) = (p.position.x - p.center.x, p.position.y - p.center.y);
            Ok(counted(p.count)
                .map(|i| {
                    let angle = f64::from(i) * step;
                    let (sin, cos) = angle.sin_cos();
                    Item {
                        position: Point2::new(
                            p.center.x + cos * dx - sin * dy,
                            p.center.y + sin * dx + cos * dy,
                        ),
                        rotation: p.rotation + if p.rotate_items { angle } else { 0.0 },
                    }
                })
                .collect())
        }
        Kind::Path => path_items(p, curve.ok_or_else(|| invalid("path"))?, tol),
    }
}

fn path_items(p: &Params, curve: &Curve2, tol: &Tolerance) -> Result<Vec<Item>> {
    let length = curve.length(tol)?;
    if !(length.is_finite() && length > 0.0) {
        return Err(invalid("path"));
    }
    let distances: Vec<f64> = if p.spacing > 0.0 {
        let n = ((length + tol.linear) / p.spacing).floor() + 1.0;
        if !n.is_finite() || n > MAX_ITEMS as f64 {
            return Err(too_many());
        }
        (0..u32::try_from(MAX_ITEMS).unwrap_or(u32::MAX))
            .take_while(|i| f64::from(*i) < n)
            .map(|i| f64::from(i) * p.spacing)
            .collect()
    } else {
        let divisions = if curve.is_closed() {
            p.count
        } else {
            p.count - 1
        };
        let divisions = f64::from(u32::try_from(divisions.max(1)).unwrap_or(1));
        counted(p.count)
            .map(|i| f64::from(i) * length / divisions)
            .collect()
    };
    let at = |s: f64| -> Result<(Point2, f64)> {
        let t = curve.parameter_at_length(s.clamp(0.0, length), tol)?;
        let tangent = curve.tangent(t);
        Ok((curve.point_at(t), tangent.y.atan2(tangent.x)))
    };
    let (start, start_angle) = at(0.0)?;
    distances
        .into_iter()
        .map(|s| {
            let (point, angle) = at(s)?;
            Ok(Item {
                position: Point2::new(
                    p.position.x + point.x - start.x,
                    p.position.y + point.y - start.y,
                ),
                rotation: p.rotation + if p.align { angle - start_angle } else { 0.0 },
            })
        })
        .collect()
}

pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<arc_doc::TypeValidator>)> {
    let items = |name: &str, default: i64| {
        FieldDecl::new(name, ValueType::I64)
            .with_default(Value::I64(default))
            .with_bound(Bound::Int {
                min: 1,
                max: i64::try_from(MAX_ITEMS).unwrap_or(i64::MAX),
            })
    };
    let mut curve = geometry().optional();
    curve.name = "curve".into();
    let fields = with_kind(vec![
        FieldDecl::new(
            "block",
            reference(BLOCK, Strength::Associative, OnDelete::Block, false),
        ),
        FieldDecl::new("position", ValueType::Point2).with_default(Value::Point2(Point2::origin())),
        FieldDecl::new("scale", ValueType::Vector3)
            .with_default(Value::Vector3(Vector3::new(1.0, 1.0, 1.0))),
        real("rotation", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("kind", variants(&["rectangular", "polar", "path"]))
            .with_default(Value::Enum(0)),
        items("rows", 1),
        items("columns", 1),
        real("row_spacing", 0.0, f64::MIN, f64::MAX),
        real("column_spacing", 0.0, f64::MIN, f64::MAX),
        real("angle", 0.0, f64::MIN, f64::MAX),
        FieldDecl::new("center", ValueType::Point2).with_default(Value::Point2(Point2::origin())),
        items("count", 1),
        real("total_angle", TAU, f64::MIN, f64::MAX),
        flag("rotate_items", true),
        FieldDecl::new("path", clearing_reference(&[], Cardinality::Optional)).optional(),
        curve,
        real("spacing", 0.0, 0.0, f64::MAX),
        flag("align", true),
    ]);
    vec![(ARRAY, fields, vec![findings])]
}

fn findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut out = common_findings(values);
    if let Some(Some(Value::Vector3(s))) = values.get(field::SCALE)
        && (s.x == 0.0 || s.y == 0.0 || s.z == 0.0)
    {
        out.push(finding(Error::new(codes::BLOCK_SCALE_ZERO)));
    }
    if let Err(error) = Params::from_values(values).validate() {
        out.push(finding(error));
    }
    out
}

/// The curve of a path array: the geometry of its `path` entity, or its stored `curve`.
fn path_curve(reader: &dyn Reader, values: &[Option<Value>]) -> Result<Option<Curve2>> {
    if let Some(Some(Value::Reference(path))) = values.get(field::PATH) {
        let geometry = reader.field(*path, "geometry")?;
        return decode_curve(Some(&geometry))
            .map(Some)
            .map_err(|_| invalid("path"));
    }
    match values.get(field::CURVE) {
        Some(Some(Value::Geometry(bytes))) => Curve2::decode(&bytes.0, &arc_base::Limits::DEFAULT)
            .map(Some)
            .map_err(|e| invalid("curve").with_source(e)),
        _ => Ok(None),
    }
}

/// The stored fields of the array `id` (common ones first, as [`Params::from_values`] reads them)
/// and the placement of each item. The display list and [`expansion`] both place items here.
fn placed(
    reader: &dyn Reader,
    id: ObjectId,
    tolerance: &Tolerance,
) -> Result<(Vec<Option<Value>>, Vec<Item>)> {
    let mut values = vec![None; entity::KIND];
    values.extend(read(reader, id, &NAMES)?);
    let params = Params::from_values(&values);
    let curve = if params.kind == Kind::Path {
        path_curve(reader, &values)?
    } else {
        None
    };
    let all = items(&params, curve.as_ref(), tolerance)?;
    Ok((values, all))
}

/// The placements of an array as its display list draws them: the inserted definition, the
/// scale of every item and the placement of each item, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Expansion {
    /// The inserted block definition, when the array refers to one.
    pub block: Option<ObjectId>,
    /// The per-axis scale of every item.
    pub scale: Vector3,
    /// The placement of every item, in order.
    pub items: Vec<Item>,
}

/// The expansion of the array `id` of `snapshot`, computed with the placements of the display
/// list ([`DisplayOptions`] with `tolerance`), for consumers that write the items out.
///
/// # Errors
///
/// The errors of the display list for the array: `draft.array.invalid`,
/// `draft.array.too_many_items`, and the errors of reading its path curve.
pub fn expansion(snapshot: &Snapshot, id: ObjectId, tolerance: &Tolerance) -> Result<Expansion> {
    let options = DisplayOptions {
        tolerance: *tolerance,
        ..DisplayOptions::default()
    };
    display::with_snapshot(snapshot, &options, |reader| {
        let (values, items) = placed(reader, id, tolerance)?;
        let block = match values.get(field::BLOCK) {
            Some(Some(Value::Reference(block))) => Some(*block),
            _ => None,
        };
        let scale = match values.get(field::SCALE) {
            Some(Some(Value::Vector3(scale))) => *scale,
            _ => Vector3::new(1.0, 1.0, 1.0),
        };
        Ok(Expansion {
            block,
            scale,
            items,
        })
    })
}

/// The display list of the array `base.source`: the list of a block reference for every item.
pub(crate) fn list(
    reader: &dyn Reader,
    base: Primitive,
    look: &Look,
    top: bool,
    options: &DisplayOptions,
) -> Result<EntityList> {
    let id: ObjectId = base.source;
    let (values, all) = placed(reader, id, &options.tolerance)?;
    let block = values.get(field::BLOCK).cloned().flatten();
    let scale = values.get(field::SCALE).cloned().flatten();
    let (mut depth, mut primitives) = (0, Vec::new());
    for item in all {
        let mut one = vec![None; entity::KIND];
        one.extend([
            block.clone(),
            Some(Value::Point2(item.position)),
            scale.clone(),
            Some(Value::F64(item.rotation)),
        ]);
        let list = reference_list(reader, &one, base.clone(), look, top, options)?;
        depth = depth.max(list.depth);
        primitives.extend(list.primitives);
        if primitives.len() > options.max_primitives {
            return Err(Error::new(codes::DISPLAY_TOO_LARGE).with_arg(
                "limit",
                i64::try_from(options.max_primitives).unwrap_or(i64::MAX),
            ));
        }
    }
    Ok(EntityList {
        order: base.draw_order,
        depth,
        primitives,
        problems: Vec::new(),
    })
}

/// The layout of a new array.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArrayParam {
    /// Rows and columns.
    Rectangular {
        /// The number of rows.
        rows: i64,
        /// The number of columns.
        columns: i64,
        /// The distance between rows.
        #[serde(default)]
        row_spacing: f64,
        /// The distance between columns.
        #[serde(default)]
        column_spacing: f64,
        /// The angle of the x axis of the array in radians.
        #[serde(default)]
        angle: f64,
    },
    /// Items on a circle.
    Polar {
        /// The center.
        center: [f64; 2],
        /// The number of items.
        count: i64,
        /// The angle from the first to the last item (a full turn by default).
        #[serde(default)]
        total_angle: Option<f64>,
        /// Whether the items turn with their position; default true.
        #[serde(default)]
        rotate_items: Option<bool>,
    },
    /// Items along a curve: the `path` entity, or a stored `curve`.
    Path {
        /// The curve entity.
        #[serde(default)]
        path: Option<IdParam>,
        /// A curve in its `arc-geom` JSON form, when there is no entity.
        #[serde(default)]
        curve: Option<serde_json::Value>,
        /// The number of items when there is no spacing.
        #[serde(default)]
        count: Option<i64>,
        /// The distance between items along the path; 0 or absent uses `count`.
        #[serde(default)]
        spacing: f64,
        /// Whether the items turn with the path; default true.
        #[serde(default)]
        align: Option<bool>,
    },
}

/// Parameters of `core.draft.array.create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArray {
    /// The space or block definition.
    pub owner: IdParam,
    /// The block definition to insert.
    pub block: IdParam,
    /// The insertion point of the first item.
    #[serde(default)]
    pub position: [f64; 2],
    /// The uniform scale of every item; default 1.
    #[serde(default)]
    pub scale: Option<f64>,
    /// The rotation of every item in radians.
    #[serde(default)]
    pub rotation: f64,
    /// The layout and its parameters.
    pub array: ArrayParam,
    /// The layer.
    #[serde(default)]
    pub layer: Option<IdParam>,
}

fn create(ctx: &mut Context<'_>, p: CreateArray) -> Result<Created> {
    let snapshot = ctx.txn()?.snapshot().clone();
    expect(&snapshot, p.block.0, &[BLOCK])?;
    let scale = p.scale.unwrap_or(1.0);
    let mut kind = vec![
        (field::BLOCK, Value::Reference(p.block.0)),
        (field::POSITION, point2(p.position)),
        (
            field::SCALE,
            Value::Vector3(Vector3::new(scale, scale, 1.0)),
        ),
        (field::ROTATION, Value::F64(p.rotation)),
    ];
    match p.array {
        ArrayParam::Rectangular {
            rows,
            columns,
            row_spacing,
            column_spacing,
            angle,
        } => kind.extend([
            (field::KIND, Value::Enum(Kind::Rectangular as u32)),
            (field::ROWS, Value::I64(rows)),
            (field::COLUMNS, Value::I64(columns)),
            (field::ROW_SPACING, Value::F64(row_spacing)),
            (field::COLUMN_SPACING, Value::F64(column_spacing)),
            (field::ANGLE, Value::F64(angle)),
        ]),
        ArrayParam::Polar {
            center,
            count,
            total_angle,
            rotate_items,
        } => kind.extend([
            (field::KIND, Value::Enum(Kind::Polar as u32)),
            (field::CENTER, point2(center)),
            (field::COUNT, Value::I64(count)),
            (field::TOTAL_ANGLE, Value::F64(total_angle.unwrap_or(TAU))),
            (
                field::ROTATE_ITEMS,
                Value::Bool(rotate_items.unwrap_or(true)),
            ),
        ]),
        ArrayParam::Path {
            path,
            curve,
            count,
            spacing,
            align,
        } => {
            kind.extend([
                (field::KIND, Value::Enum(Kind::Path as u32)),
                (field::COUNT, Value::I64(count.unwrap_or(1))),
                (field::SPACING, Value::F64(spacing)),
                (field::ALIGN, Value::Bool(align.unwrap_or(true))),
            ]);
            match (path, curve) {
                (Some(IdParam(path)), None) => {
                    kind.push((field::PATH, Value::Reference(path)));
                }
                (None, Some(json)) => {
                    let curve: Curve2 =
                        serde_json::from_value(json).map_err(|_| field_invalid("curve"))?;
                    kind.push((field::CURVE, Value::Geometry(GeometryBytes(curve.encode()))));
                }
                _ => return Err(field_invalid("path")),
            }
        }
    }
    let mut check = vec![None; entity::KIND + NAMES.len()];
    for (i, v) in &kind {
        if let Some(slot) = check.get_mut(*i) {
            *slot = Some(v.clone());
        }
    }
    Params::from_values(&check).validate()?;
    create_entity(ctx, ARRAY, p.owner.0, p.layer, kind)
}

/// The commands of this module.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.array.create", create),
        family("core.draft.array.modify", &[ARRAY]),
    ]
}
