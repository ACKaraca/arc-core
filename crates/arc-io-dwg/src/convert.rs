//! Conversions of library values into the neutral model: handles, points, colors and the
//! properties every entity shares.

use acadrust::entities::EntityCommon;
use acadrust::types::{Color, Handle as DwgHandle, Transparency, Vector2, Vector3};
use arc_io_dxf::Handle;
use arc_io_dxf::model::{EntityProps, Head, P2, P3};

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

/// Whether a linetype name means the entity takes the linetype of its layer.
pub(crate) fn is_by_layer(name: &str) -> bool {
    name.is_empty() || name.eq_ignore_ascii_case("ByLayer")
}

/// The shared properties of an entity, and the names of the data the model does not carry.
///
/// The thickness, extrusion and elevation of each kind are set by its own mapping; the rest is
/// here. A number that is not finite makes the entity invalid; the caller checks it.
pub(crate) fn props(common: &EntityCommon, paper_space: bool) -> (EntityProps, Vec<&'static str>) {
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
    let props = EntityProps {
        paper_space,
        layout: None,
        layer: common.layer.clone(),
        linetype: (!is_by_layer(&common.linetype)).then(|| common.linetype.clone()),
        color: color.color,
        true_color: color.true_color,
        color_name: common.color_name.clone().filter(|name| !name.is_empty()),
        lineweight: Some(common.line_weight.value()),
        transparency: None,
        linetype_scale: (common.linetype_scale != 1.0).then_some(common.linetype_scale),
        invisible: common.invisible,
        elevation: None,
        thickness: None,
        extrusion: None,
    };
    (props, partial)
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
}
