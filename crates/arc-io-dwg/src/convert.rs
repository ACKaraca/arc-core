//! Conversions of library values into the neutral model: handles, points, colors, the limits every
//! copied value is checked against, and the properties every entity shares.

use acadrust::entities::EntityCommon;
use acadrust::types::{Color, Handle as DwgHandle, LineWeight, Transparency, Vector2, Vector3};
use arc_base::{Limits, Result};
use arc_io_dxf::Handle;
use arc_io_dxf::model::{EntityProps, Head, P2, P3};

use crate::error::limit;

/// A library handle as a model handle; the null handle is absent.
pub(crate) fn handle(h: DwgHandle) -> Option<Handle> {
    (!h.is_null()).then(|| Handle(h.value()))
}

/// The head of a record: its handle and owner.
pub(crate) fn head(h: DwgHandle, owner: DwgHandle) -> Head {
    Head {
        handle: handle(h),
        owner: handle(owner),
        app_groups: Vec::new(),
    }
}

pub(crate) fn p3(v: Vector3) -> P3 {
    [v.x, v.y, v.z]
}

pub(crate) fn p2(v: Vector2) -> P2 {
    [v.x, v.y]
}

/// The horizontal part of a point, for models that store two-dimensional points.
pub(crate) fn xy(v: Vector3) -> P2 {
    [v.x, v.y]
}

pub(crate) fn finite3(v: Vector3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

pub(crate) fn finite2(v: Vector2) -> bool {
    v.x.is_finite() && v.y.is_finite()
}

/// Whether every value of a slice is finite.
pub(crate) fn all_finite(values: &[f64]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Checks the number of elements of a collection against `array_length`.
pub(crate) fn array(len: usize, limits: &Limits) -> Result<()> {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if len > limits.array_length {
        return Err(limit("array_length", len));
    }
    Ok(())
}

/// Checks the size of a text value against `string_bytes` and copies it.
pub(crate) fn text(value: &str, limits: &Limits) -> Result<String> {
    let len = u64::try_from(value.len()).unwrap_or(u64::MAX);
    if len > limits.string_bytes {
        return Err(limit("string_bytes", len));
    }
    Ok(value.to_owned())
}

/// The records of one mapping, counted against `record_count`. Vertices and attributes count as
/// records, as the records of the DXF reader do.
pub(crate) struct RecordBudget<'a> {
    used: &'a mut u64,
    limit: u64,
}

impl<'a> RecordBudget<'a> {
    pub(crate) fn new(used: &'a mut u64, limit: u64) -> Self {
        Self { used, limit }
    }

    /// Adds `count` records; `io.limit_exceeded` naming `record_count` once the limit is passed.
    pub(crate) fn spend(&mut self, count: usize) -> Result<()> {
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        *self.used = self.used.saturating_add(count);
        if *self.used > self.limit {
            return Err(limit("record_count", *self.used));
        }
        Ok(())
    }
}

/// The color properties of a color value, and whether part of it was not carried.
pub(crate) struct ColorValue {
    pub(crate) color: Option<i16>,
    pub(crate) true_color: Option<i32>,
    pub(crate) partial: bool,
}

/// Maps a library color. By-layer colors are absent; by-block is color 0, as the model stores it;
/// a color without a value has no place in the model and is reported as partial.
pub(crate) fn color(value: &Color) -> ColorValue {
    match value {
        Color::ByLayer => ColorValue {
            color: None,
            true_color: None,
            partial: false,
        },
        Color::ByBlock => ColorValue {
            color: Some(0),
            true_color: None,
            partial: false,
        },
        Color::Index(index) => ColorValue {
            color: Some(i16::from(*index)),
            true_color: None,
            partial: false,
        },
        Color::Rgb { r, g, b } => ColorValue {
            color: None,
            true_color: Some((i32::from(*r) << 16) | (i32::from(*g) << 8) | i32::from(*b)),
            partial: false,
        },
        Color::None => ColorValue {
            color: None,
            true_color: None,
            partial: true,
        },
    }
}

/// The lineweight of an entity. An entity that takes the lineweight of its layer or the default
/// has none, as an entity without the lineweight group has none in the DXF reader; by-block keeps
/// its code.
pub(crate) fn lineweight(value: LineWeight) -> Option<i16> {
    match value {
        LineWeight::ByLayer | LineWeight::Default => None,
        LineWeight::ByBlock => Some(value.value()),
        LineWeight::Value(v) => Some(v),
    }
}

/// Whether a linetype name means the entity takes the linetype of its layer.
pub(crate) fn is_by_layer(name: &str) -> bool {
    name.is_empty() || name.eq_ignore_ascii_case("ByLayer")
}

/// The shared properties of an entity, and the names of the data the model does not carry.
///
/// The thickness, extrusion and elevation of each kind are set by its own mapping; the rest is
/// here. A number that is not finite makes the entity invalid; the caller checks it. Every name
/// copied from the library is checked against the string limit.
pub(crate) fn props(
    common: &EntityCommon,
    paper_space: bool,
    limits: &Limits,
) -> Result<(EntityProps, Vec<&'static str>)> {
    let mut partial = Vec::new();
    let color = color(&common.color);
    if color.partial {
        partial.push("color");
    }
    match common.transparency {
        Transparency::ByLayer => {}
        _ => partial.push("transparency"),
    }
    if common.graphic_data.is_some() {
        partial.push("graphic_data");
    }
    if !common.extended_data.is_empty() {
        partial.push("extended_data");
    }
    if !common.reactors.is_empty() || common.xdictionary_handle.is_some() {
        partial.push("references");
    }
    let linetype = (!is_by_layer(&common.linetype))
        .then(|| text(&common.linetype, limits))
        .transpose()?;
    let color_name = common
        .color_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .map(|name| text(name, limits))
        .transpose()?;
    let props = EntityProps {
        paper_space,
        layout: None,
        layer: text(&common.layer, limits)?,
        linetype,
        color: color.color,
        true_color: color.true_color,
        color_name,
        lineweight: lineweight(common.line_weight),
        transparency: None,
        linetype_scale: (common.linetype_scale != 1.0).then_some(common.linetype_scale),
        invisible: common.invisible,
        elevation: None,
        thickness: None,
        extrusion: None,
    };
    Ok((props, partial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_kinds_map_to_model_forms() {
        let rgb = color(&Color::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(rgb.true_color, Some(0x0001_0203));
        assert_eq!(rgb.color, None);
        assert!(!rgb.partial);
        assert_eq!(color(&Color::ByBlock).color, Some(0));
        assert_eq!(color(&Color::ByLayer).color, None);
        assert_eq!(color(&Color::Index(7)).color, Some(7));
        assert!(color(&Color::None).partial);
    }

    #[test]
    fn null_handles_are_absent() {
        assert_eq!(handle(DwgHandle::new(0)), None);
        assert_eq!(handle(DwgHandle::new(0x2A)), Some(Handle(0x2A)));
    }

    #[test]
    fn by_layer_linetype_names() {
        assert!(is_by_layer("ByLayer"));
        assert!(is_by_layer("BYLAYER"));
        assert!(!is_by_layer("DASHED"));
    }

    #[test]
    fn lineweight_is_absent_for_by_layer_and_default() {
        assert_eq!(lineweight(LineWeight::ByLayer), None);
        assert_eq!(lineweight(LineWeight::Default), None);
        assert_eq!(lineweight(LineWeight::ByBlock), Some(-2));
        assert_eq!(lineweight(LineWeight::Value(25)), Some(25));
    }

    #[test]
    fn record_budget_names_the_limit_it_passes() {
        let mut used = 0;
        let mut budget = RecordBudget::new(&mut used, 3);
        assert!(budget.spend(2).is_ok());
        let err = budget.spend(2).err();
        assert_eq!(err.map(|e| e.code()), Some("io.limit_exceeded"));
        assert_eq!(used, 4);
    }

    #[test]
    fn long_names_are_refused_by_the_string_limit() {
        let limits = Limits {
            string_bytes: 4,
            ..Limits::CONSTRAINED
        };
        let common = EntityCommon {
            layer: "LONGER_THAN_FOUR".to_owned(),
            ..EntityCommon::default()
        };
        let err = props(&common, false, &limits).err();
        assert_eq!(err.map(|e| e.code()), Some("io.limit_exceeded"));
    }
}
