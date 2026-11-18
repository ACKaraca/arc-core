//! Spline entities (the spline part of CORE-DRAFT-002): an `arc-geom` NURBS curve stored in the
//! geometry encoding, with the fit points it was interpolated from kept as data.
//!
//! The `geometry` field of a spline names the encoding version 2 (`arc.geom` v2), which is the
//! version `GeomCodec::encoding` reports for a NURBS curve (ADR-0013, amendment). The fit points
//! are not read back to rebuild the curve: they are what the author gave, and the commands
//! replace both fields together.

use arc_base::math::Point2;
use arc_base::{Error, Limits, Result, Tolerance};
use arc_command::{CommandSpec, Context, IdParam};
use arc_doc::{Bound, FieldDecl, FieldPath, GeometryBytes, Value, ValueType};
use arc_geom::Nurbs2;
use arc_geom::encoding::GeomCodec;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::commands::{Created, create_entity, expect, field_invalid, spec};
use crate::types::{self, SPLINE, entity};

/// The degree of a spline created without one.
pub const DEFAULT_DEGREE: usize = 3;

/// The field declaration of the `geometry` field of a spline: encoding version 2.
pub(crate) fn geometry_field() -> FieldDecl {
    let encoding = arc_geom::GeomEncoding::VERSION_2;
    FieldDecl::new(
        "geometry",
        ValueType::Geometry(arc_doc::GeometryEncoding {
            name: encoding.name.to_owned(),
            version: u32::from(encoding.version),
        }),
    )
    .in_aspect("geometry")
}

/// The field declarations after the common fields: `geometry` and `fit_points`.
pub(crate) fn fields() -> Vec<FieldDecl> {
    vec![
        geometry_field(),
        FieldDecl::new("fit_points", ValueType::List(Box::new(ValueType::Point2)))
            .with_default(Value::List(Vec::new()))
            .with_bound(Bound::Length {
                max: Limits::DEFAULT.array_length,
            })
            .in_aspect("geometry"),
    ]
}

/// The type validator of a spline: the common findings and `draft.geometry.invalid` unless the
/// geometry decodes as a NURBS curve.
pub(crate) fn findings(values: &[Option<Value>]) -> Vec<arc_doc::Finding> {
    let mut findings = types::common_findings(values);
    if let Some(Some(Value::Geometry(bytes))) = values.get(entity::KIND)
        && let Err(error) = Nurbs2::decode(&bytes.0, &Limits::DEFAULT)
    {
        findings.push(types::finding(
            Error::new(crate::error::codes::GEOMETRY_INVALID).with_source(error),
        ));
    }
    findings
}

/// The geometry and fit-point values of a spline.
pub(crate) fn values(curve: &Nurbs2, fit: &[Point2]) -> Vec<(usize, Value)> {
    vec![
        (entity::KIND, Value::Geometry(GeometryBytes(curve.encode()))),
        (
            entity::SPLINE_FIT_POINTS,
            Value::List(fit.iter().map(|p| Value::Point2(*p)).collect()),
        ),
    ]
}

/// How a spline is described: from control points, or from fit points.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSpline {
    /// The space or block definition the spline belongs to.
    pub owner: IdParam,
    /// The layer, if any.
    #[serde(default)]
    pub layer: Option<IdParam>,
    /// The degree; default 3.
    #[serde(default)]
    pub degree: Option<usize>,
    /// The control points; the knots are the clamped uniform ones. Give these or `fit_points`.
    #[serde(default)]
    pub control_points: Option<Vec<[f64; 2]>>,
    /// The weights of the control points (positive); default all 1.
    #[serde(default)]
    pub weights: Option<Vec<f64>>,
    /// The points the curve passes through, in order. Give these or `control_points`.
    #[serde(default)]
    pub fit_points: Option<Vec<[f64; 2]>>,
}

/// Parameters of `core.draft.spline.set`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSpline {
    /// The splines to change.
    #[schemars(length(min = 1, max = 10000))]
    pub objects: Vec<IdParam>,
    /// The degree; default 3.
    #[serde(default)]
    pub degree: Option<usize>,
    /// The control points. Give these or `fit_points`.
    #[serde(default)]
    pub control_points: Option<Vec<[f64; 2]>>,
    /// The weights of the control points (positive); default all 1.
    #[serde(default)]
    pub weights: Option<Vec<f64>>,
    /// The points the curve passes through. Give these or `control_points`.
    #[serde(default)]
    pub fit_points: Option<Vec<[f64; 2]>>,
}

type Source = (
    Option<usize>,
    Option<Vec<[f64; 2]>>,
    Option<Vec<f64>>,
    Option<Vec<[f64; 2]>>,
);

/// The curve and the fit points a description gives.
///
/// # Errors
///
/// `draft.field.invalid` for both or neither of `control_points` and `fit_points`, `weights` with
/// fit points, or more points than `limits` allows; the errors of `Nurbs2::from_control_points`
/// and `Nurbs2::interpolate`.
fn build(
    (degree, control, weights, fit): Source,
    limits: &Limits,
) -> Result<(Nurbs2, Vec<Point2>)> {
    let degree = degree.unwrap_or(DEFAULT_DEGREE);
    let bound = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
    let points = |list: &[[f64; 2]], field: &str| -> Result<Vec<Point2>> {
        if list.len() > bound {
            return Err(field_invalid(field));
        }
        Ok(list.iter().map(|[x, y]| Point2::new(*x, *y)).collect())
    };
    match (control, fit) {
        (Some(control), None) => {
            let control = points(&control, "control_points")?;
            let weights = match weights {
                Some(w) => w,
                None => vec![1.0; control.len()],
            };
            let curve = Nurbs2::from_control_points(degree, control, weights)?;
            Ok((curve, Vec::new()))
        }
        (None, Some(fit)) if weights.is_none() => {
            let fit = points(&fit, "fit_points")?;
            let curve = Nurbs2::interpolate(&fit, degree, &Tolerance::DEFAULT)?;
            Ok((curve, fit))
        }
        (None, Some(_)) => Err(field_invalid("weights")),
        _ => Err(field_invalid("control_points")),
    }
}

fn create(ctx: &mut Context<'_>, p: CreateSpline) -> Result<Created> {
    let limits = *ctx.limits();
    let (curve, fit) = build(
        (p.degree, p.control_points, p.weights, p.fit_points),
        &limits,
    )?;
    create_entity(ctx, SPLINE, p.owner.0, p.layer, values(&curve, &fit))
}

fn set(ctx: &mut Context<'_>, p: SetSpline) -> Result<()> {
    let limits = *ctx.limits();
    let (curve, fit) = build(
        (p.degree, p.control_points, p.weights, p.fit_points),
        &limits,
    )?;
    let txn = ctx.txn()?;
    for IdParam(id) in p.objects {
        expect(txn.snapshot(), id, &[SPLINE])?;
        for (index, value) in values(&curve, &fit) {
            let name = if index == entity::KIND {
                "geometry"
            } else {
                "fit_points"
            };
            txn.modify(id, FieldPath::new(name), Some(value))?;
        }
    }
    Ok(())
}

/// The commands of splines.
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        spec("core.draft.spline.create", create),
        spec("core.draft.spline.set", set),
    ]
}

/// The fit points of a spline as points, for display and tests.
#[must_use]
pub fn fit_points(values: &[Option<Value>]) -> Vec<Point2> {
    match values.get(entity::SPLINE_FIT_POINTS) {
        Some(Some(Value::List(items))) => items
            .iter()
            .filter_map(|v| match v {
                Value::Point2(p) => Some(*p),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
