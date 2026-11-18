//! Layouts (CORE-DRAFT-027) and sheet viewports (CORE-DRAFT-028).
//!
//! A layout is a space of kind `layout`; its paper and plot settings are a `core.draft.layout`
//! object owned by that space, at most one per space. A sheet viewport is a `core.draft.viewport`
//! object owned by a layout space that shows another space at a scale. Both are deleted with
//! their layout space. The commit validator in [`crate::validate`] checks the owners.
//!
//! **Viewport mapping.** A model point `p` appears on the sheet at
//! `center + R(rotation) · k · (p − view_center)`, with
//! `k = scale · factor(model unit → layout unit)` ([`crate::types::SpaceUnit::factor_to`]):
//! `scale` is the ratio of a paper length to the model length it shows, both measured in one
//! unit, so a 1:100 viewport has the scale 0.01 whatever the units of the two spaces.

use std::collections::{BTreeMap, BTreeSet};

use arc_base::math::{Matrix3, Point2, Transform2};
use arc_base::{Error, Limits, ObjectId};
use arc_doc::ValueType::Record;
use arc_doc::{
    Bound, FieldDecl, Finding, OnDelete, Snapshot, Strength, TypeValidator, Value, ValueType,
};
use arc_geom::encoding::GeomCodec;
use arc_geom::{Polyline2, PolylineVertex};

use crate::appearance::LayerOverride;
use crate::error::codes;
use crate::types::{
    LAYER, LINETYPE, MAX_LINEWEIGHT_MM, NAME_BYTES, SPACE, SpaceUnit, f64_at, finding, flag,
    geometry, real, reference, reference_at, table, text, variants,
};

/// The paper and plot settings of a layout space.
pub const LAYOUT: &str = "core.draft.layout";
/// A sheet viewport owned by a layout space.
pub const VIEWPORT: &str = "core.draft.viewport";

/// The most layer override records of one viewport.
pub const MAX_OVERRIDES: u64 = 4096;

/// Field indices of a layout.
pub mod settings {
    /// `paper_name`: optional name of the paper size.
    pub const PAPER_NAME: usize = 0;
    /// `paper_width`: the width of the paper as named, in the layout's unit, positive.
    pub const PAPER_WIDTH: usize = 1;
    /// `paper_height`: the height of the paper as named, positive.
    pub const PAPER_HEIGHT: usize = 2;
    /// `orientation`: `portrait` or `landscape`.
    pub const ORIENTATION: usize = 3;
    /// `margin_left`: the unprintable margin at the left of the sheet as plotted.
    pub const MARGIN_LEFT: usize = 4;
    /// `margin_bottom`.
    pub const MARGIN_BOTTOM: usize = 5;
    /// `margin_right`.
    pub const MARGIN_RIGHT: usize = 6;
    /// `margin_top`.
    pub const MARGIN_TOP: usize = 7;
    /// `origin`: the plot origin relative to the lower left corner of the printable area.
    pub const ORIGIN: usize = 8;
    /// `plot_scale`: plotted length per layout length, positive.
    pub const PLOT_SCALE: usize = 9;
    /// `plot_style_table`: optional name of the plot style table.
    pub const PLOT_STYLE_TABLE: usize = 10;
    /// `plot_area`: `layout`, `extents` or `window`.
    pub const PLOT_AREA: usize = 11;
    /// `window_min`: the lower left corner of a plotted window.
    pub const WINDOW_MIN: usize = 12;
    /// `window_max`: the upper right corner of a plotted window.
    pub const WINDOW_MAX: usize = 13;
}

/// Field indices of a viewport.
pub mod viewport {
    /// `model`: the space shown.
    pub const MODEL: usize = 0;
    /// `center`: the center of the viewport on the sheet.
    pub const CENTER: usize = 1;
    /// `width`: the width of the viewport rectangle on the sheet, positive.
    pub const WIDTH: usize = 2;
    /// `height`: the height of the viewport rectangle, positive.
    pub const HEIGHT: usize = 3;
    /// `view_center`: the model point shown at `center`.
    pub const VIEW_CENTER: usize = 4;
    /// `scale`: paper length per model length in one unit, positive.
    pub const SCALE: usize = 5;
    /// `rotation`: the turn of the model on the sheet in radians, counterclockwise.
    pub const ROTATION: usize = 6;
    /// `clip`: optional closed polyline on the sheet; absent clips to the rectangle.
    pub const CLIP: usize = 7;
    /// `locked`: the view (`model`, `view_center`, `scale`, `rotation`) cannot change.
    pub const LOCKED: usize = 8;
    /// `annotation_scale`: the scale whose representations of annotative objects the viewport
    /// shows, positive.
    pub const ANNOTATION_SCALE: usize = 9;
    /// `layer_overrides`: records of `layer`, `frozen`, `color`, `linetype`, `lineweight`,
    /// `transparency`.
    pub const LAYER_OVERRIDES: usize = 10;
    /// The view fields a locked viewport keeps.
    pub const VIEW_FIELDS: [&str; 4] = ["model", "view_center", "scale", "rotation"];
}

/// The orientation of a layout's paper, with its enum code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// The paper as named: `paper_width` across.
    Portrait = 0,
    /// The paper turned a quarter: `paper_height` across.
    Landscape = 1,
}

fn positive(name: &str) -> FieldDecl {
    FieldDecl::new(name, ValueType::F64).with_bound(Bound::Real {
        min: f64::MIN_POSITIVE,
        max: f64::MAX,
    })
}

fn named(name: &str, decl: FieldDecl) -> FieldDecl {
    FieldDecl {
        name: name.into(),
        ..decl
    }
}

fn layout_fields() -> Vec<FieldDecl> {
    let margin = |name: &str| real(name, 0.0, 0.0, f64::MAX);
    vec![
        text("paper_name", NAME_BYTES),
        positive("paper_width"),
        positive("paper_height"),
        FieldDecl::new("orientation", variants(&["portrait", "landscape"]))
            .with_default(Value::Enum(0)),
        margin("margin_left"),
        margin("margin_bottom"),
        margin("margin_right"),
        margin("margin_top"),
        FieldDecl::new("origin", ValueType::Point2).with_default(Value::Point2(Point2::origin())),
        positive("plot_scale").with_default(Value::F64(1.0)),
        text("plot_style_table", NAME_BYTES),
        FieldDecl::new("plot_area", variants(&["layout", "extents", "window"]))
            .with_default(Value::Enum(0)),
        FieldDecl::new("window_min", ValueType::Point2).optional(),
        FieldDecl::new("window_max", ValueType::Point2).optional(),
    ]
}

fn override_record() -> ValueType {
    Record(vec![
        FieldDecl::new(
            "layer",
            reference(LAYER, Strength::Associative, OnDelete::Block, false),
        ),
        flag("frozen", false),
        FieldDecl::new("color", ValueType::Color).optional(),
        FieldDecl::new(
            "linetype",
            reference(LINETYPE, Strength::Associative, OnDelete::Block, true),
        )
        .optional(),
        FieldDecl::new("lineweight", ValueType::F64)
            .with_bound(Bound::Real {
                min: 0.0,
                max: MAX_LINEWEIGHT_MM,
            })
            .optional(),
        FieldDecl::new("transparency", ValueType::F64)
            .with_bound(Bound::Real { min: 0.0, max: 1.0 })
            .optional(),
    ])
}

fn viewport_fields() -> Vec<FieldDecl> {
    let view = |f: FieldDecl| f.in_aspect("view");
    vec![
        view(FieldDecl::new(
            "model",
            reference(SPACE, Strength::Associative, OnDelete::Block, false),
        )),
        FieldDecl::new("center", ValueType::Point2),
        positive("width"),
        positive("height"),
        view(
            FieldDecl::new("view_center", ValueType::Point2)
                .with_default(Value::Point2(Point2::origin())),
        ),
        view(positive("scale").with_default(Value::F64(1.0))),
        view(real("rotation", 0.0, f64::MIN, f64::MAX)),
        named("clip", geometry()).optional(),
        flag("locked", false),
        positive("annotation_scale").with_default(Value::F64(1.0)),
        FieldDecl::new(
            "layer_overrides",
            ValueType::List(Box::new(override_record())),
        )
        .with_default(Value::List(Vec::new()))
        .with_bound(Bound::Length { max: MAX_OVERRIDES })
        .in_aspect("appearance"),
    ]
}

/// The width and height of the sheet of the layout values: the paper as named, turned for
/// `landscape`.
#[must_use]
pub fn sheet_size(values: &[Option<Value>]) -> (f64, f64) {
    let w = f64_at(values, settings::PAPER_WIDTH).unwrap_or(0.0);
    let h = f64_at(values, settings::PAPER_HEIGHT).unwrap_or(0.0);
    match values.get(settings::ORIENTATION) {
        Some(Some(Value::Enum(1))) => (h, w),
        _ => (w, h),
    }
}

fn layout_findings(values: &[Option<Value>]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let (w, h) = sheet_size(values);
    let at = |i| f64_at(values, i).unwrap_or(0.0);
    let across = at(settings::MARGIN_LEFT) + at(settings::MARGIN_RIGHT);
    let up = at(settings::MARGIN_BOTTOM) + at(settings::MARGIN_TOP);
    // Written so that a NaN fails.
    if !(across < w && up < h) {
        findings.push(finding(Error::new(codes::LAYOUT_MARGINS_INVALID)));
    }
    if matches!(values.get(settings::PLOT_AREA), Some(Some(Value::Enum(2)))) {
        let valid = match (
            values.get(settings::WINDOW_MIN),
            values.get(settings::WINDOW_MAX),
        ) {
            (Some(Some(Value::Point2(a))), Some(Some(Value::Point2(b)))) => a.x < b.x && a.y < b.y,
            _ => false,
        };
        if !valid {
            findings.push(finding(Error::new(codes::LAYOUT_WINDOW_INVALID)));
        }
    }
    findings
}

fn viewport_findings(values: &[Option<Value>]) -> Vec<Finding> {
    match values.get(viewport::CLIP) {
        Some(Some(Value::Geometry(bytes))) => match Polyline2::decode(&bytes.0, &Limits::DEFAULT) {
            Ok(p) if p.is_closed() => Vec::new(),
            _ => vec![finding(Error::new(codes::VIEWPORT_CLIP_INVALID))],
        },
        _ => Vec::new(),
    }
}

/// The layout and viewport declarations.
pub(crate) fn declarations() -> Vec<(&'static str, Vec<FieldDecl>, Vec<TypeValidator>)> {
    vec![
        (LAYOUT, layout_fields(), vec![layout_findings]),
        (VIEWPORT, viewport_fields(), vec![viewport_findings]),
    ]
}

/// The per-viewport overrides of layers: the frozen layers and the appearance overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overrides {
    /// Layers frozen in the viewport.
    pub frozen: BTreeSet<ObjectId>,
    /// Appearance overrides by layer, the input of [`crate::appearance::resolve`].
    pub layers: BTreeMap<ObjectId, LayerOverride>,
}

/// The overrides of the `layer_overrides` value of a viewport; for a layer listed twice the
/// last record wins.
#[must_use]
pub fn overrides_of(values: &[Option<Value>]) -> Overrides {
    let mut out = Overrides::default();
    let Some(Some(Value::List(records))) = values.get(viewport::LAYER_OVERRIDES) else {
        return out;
    };
    for record in records {
        let Value::Record(fields) = record else {
            continue;
        };
        let Some(layer) = reference_at(fields, 0) else {
            continue;
        };
        if matches!(fields.get(1), Some(Some(Value::Bool(true)))) {
            out.frozen.insert(layer);
        } else {
            out.frozen.remove(&layer);
        }
        let color = match fields.get(2) {
            Some(Some(Value::Color(c))) => Some((**c).clone()),
            _ => None,
        };
        out.layers.insert(
            layer,
            LayerOverride {
                color,
                linetype: reference_at(fields, 3),
                lineweight: f64_at(fields, 4),
                transparency: f64_at(fields, 5),
            },
        );
    }
    out
}

/// A viewport read from a snapshot: the space it shows, its mapping to the sheet, its clip
/// boundary on the sheet and its overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// The space shown.
    pub model: ObjectId,
    /// The mapping from model to sheet coordinates.
    pub transform: Transform2,
    /// The clip boundary on the sheet: `clip`, or the rectangle.
    pub clip: Polyline2,
    /// The layer overrides.
    pub overrides: Overrides,
    /// The annotation scale: annotative objects show their representation for it.
    pub annotation_scale: f64,
}

fn unit_of(snapshot: &Snapshot, space: Option<ObjectId>) -> SpaceUnit {
    space
        .and_then(|id| snapshot.get(snapshot.resolve(id).ok()?).ok())
        .and_then(|o| match o.values.get(table::SPACE_UNIT) {
            Some(Some(Value::Enum(code))) => SpaceUnit::of(*code),
            _ => None,
        })
        .unwrap_or(SpaceUnit::Unitless)
}

fn point_at(values: &[Option<Value>], index: usize) -> Point2 {
    match values.get(index) {
        Some(Some(Value::Point2(p))) => *p,
        _ => Point2::origin(),
    }
}

/// The view of the viewport `id` in `snapshot`, as described in the
/// [module documentation](self).
///
/// # Errors
///
/// `doc.object.not_found` when it is not live; `draft.object.wrong_type` when it is not a
/// viewport; `draft.viewport.clip_invalid` when the clip or the rectangle is not a valid
/// closed polyline.
pub fn view(snapshot: &Snapshot, id: ObjectId) -> arc_base::Result<View> {
    let handle = snapshot.resolve(id)?;
    if snapshot.type_key(handle).map(arc_doc::TypeKey::as_str) != Some(VIEWPORT) {
        return Err(Error::new(codes::OBJECT_WRONG_TYPE).with_detail("id", id.to_string()));
    }
    let object = snapshot.get(handle)?;
    let values = &object.values;
    let model = reference_at(values, viewport::MODEL)
        .ok_or_else(|| Error::new(codes::FIELD_INVALID).with_arg("field", "model"))?;
    let factor = unit_of(snapshot, Some(model)).factor_to(unit_of(snapshot, object.header.owner));
    let k = f64_at(values, viewport::SCALE).unwrap_or(1.0) * factor;
    let rotation = f64_at(values, viewport::ROTATION).unwrap_or(0.0);
    let (sin, cos) = rotation.sin_cos();
    let center = point_at(values, viewport::CENTER);
    let eye = point_at(values, viewport::VIEW_CENTER);
    let (a, b, c, d) = (k * cos, -k * sin, k * sin, k * cos);
    let tx = center.x - (a * eye.x + b * eye.y);
    let ty = center.y - (c * eye.x + d * eye.y);
    let transform =
        Transform2::from_matrix_unchecked(Matrix3::new(a, b, tx, c, d, ty, 0.0, 0.0, 1.0));
    let invalid = |e: Error| Error::new(codes::VIEWPORT_CLIP_INVALID).with_source(e);
    let clip = match values.get(viewport::CLIP) {
        Some(Some(Value::Geometry(bytes))) => {
            Polyline2::decode(&bytes.0, &Limits::DEFAULT).map_err(invalid)?
        }
        _ => {
            let w = f64_at(values, viewport::WIDTH).unwrap_or(0.0) / 2.0;
            let h = f64_at(values, viewport::HEIGHT).unwrap_or(0.0) / 2.0;
            let corner = |x: f64, y: f64| PolylineVertex::new(Point2::new(x, y), 0.0);
            Polyline2::new(
                vec![
                    corner(center.x - w, center.y - h),
                    corner(center.x + w, center.y - h),
                    corner(center.x + w, center.y + h),
                    corner(center.x - w, center.y + h),
                ],
                true,
            )
            .map_err(invalid)?
        }
    };
    if !clip.is_closed() {
        return Err(Error::new(codes::VIEWPORT_CLIP_INVALID));
    }
    Ok(View {
        model,
        transform,
        clip,
        overrides: overrides_of(values),
        annotation_scale: f64_at(values, viewport::ANNOTATION_SCALE).unwrap_or(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_must_leave_a_printable_area_on_the_turned_sheet() {
        let mut v: Vec<Option<Value>> = vec![None; 14];
        v[settings::PAPER_WIDTH] = Some(Value::F64(210.0));
        v[settings::PAPER_HEIGHT] = Some(Value::F64(297.0));
        v[settings::MARGIN_LEFT] = Some(Value::F64(100.0));
        v[settings::MARGIN_RIGHT] = Some(Value::F64(150.0));
        assert_eq!(layout_findings(&v).len(), 1);
        v[settings::ORIENTATION] = Some(Value::Enum(1));
        assert_eq!(sheet_size(&v), (297.0, 210.0));
        assert!(layout_findings(&v).is_empty());
        v[settings::PLOT_AREA] = Some(Value::Enum(2));
        v[settings::WINDOW_MIN] = Some(Value::Point2(Point2::new(1.0, 1.0)));
        v[settings::WINDOW_MAX] = Some(Value::Point2(Point2::new(1.0, 2.0)));
        assert_eq!(
            layout_findings(&v)[0].error.code(),
            "draft.layout.window_invalid"
        );
        v[settings::MARGIN_TOP] = Some(Value::F64(f64::NAN));
        assert_eq!(layout_findings(&v).len(), 2);
    }
}
