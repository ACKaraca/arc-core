//! Appearance resolution (CORE-DRAFT-017).
//!
//! [`resolve`] is a pure function. Its input is the chain of levels from the entity outward:
//! the entity itself, then the block reference that inserts the block containing it, then the
//! reference inserting that one, and so on; each level has its own [`Style`] and layer. Each of
//! color, linetype, lineweight and transparency resolves on its own, in this order:
//!
//! 1. A value set to `object` is the result at that level: its own value, or the default when
//!    it has none (white, continuous, the default lineweight, opaque).
//! 2. A value set to `layer` is the value of that level's layer, unless the viewport overrides
//!    passed in name that layer and override the property; a level without a layer gives the
//!    default.
//! 3. A value set to `block` moves one level outward and applies these rules to the same
//!    property of the inserting block reference; at the outermost level it gives the default.
//!
//! The result records the source of every value ([`Source`]).

use std::collections::BTreeMap;

use arc_base::{Color, ObjectId, Result};
use arc_doc::{ColorValue, Snapshot, Value};

use crate::types::{By, entity, enum_at, f64_at, layer, reference_at};

/// One appearance property of an entity: from its layer, from its block, or its own value
/// (`None` for the default).
#[derive(Debug, Clone, PartialEq)]
pub enum Setting<T> {
    /// From the level's layer.
    Layer,
    /// From the inserting block reference.
    Block,
    /// The level's own value; `None` is the default.
    Object(Option<T>),
}

/// The appearance settings of an entity or block reference.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// The color.
    pub color: Setting<ColorValue>,
    /// The linetype; `Object(None)` is continuous.
    pub linetype: Setting<ObjectId>,
    /// The lineweight in millimeters; `Object(None)` is the default lineweight.
    pub lineweight: Setting<f64>,
    /// The transparency from 0 to 1.
    pub transparency: Setting<f64>,
}

/// The appearance values of a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerStyle {
    /// The layer.
    pub id: ObjectId,
    /// The color.
    pub color: ColorValue,
    /// The linetype; `None` is continuous.
    pub linetype: Option<ObjectId>,
    /// The lineweight in millimeters; `None` is the default lineweight.
    pub lineweight: Option<f64>,
    /// The transparency.
    pub transparency: f64,
}

/// The per-viewport overrides of one layer; `None` keeps the layer's value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerOverride {
    /// The color.
    pub color: Option<ColorValue>,
    /// The linetype.
    pub linetype: Option<ObjectId>,
    /// The lineweight in millimeters.
    pub lineweight: Option<f64>,
    /// The transparency.
    pub transparency: Option<f64>,
}

/// One level of the chain: the style and the layer of an entity or block reference.
#[derive(Debug, Clone, Copy)]
pub struct Level<'a> {
    /// The style.
    pub style: &'a Style,
    /// The layer, if the object has one.
    pub layer: Option<&'a LayerStyle>,
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The entity's own value.
    Object,
    /// The own value of the block reference `level` steps outward (1 is the innermost).
    Block {
        /// The level of the reference.
        level: usize,
    },
    /// The layer's value.
    Layer(ObjectId),
    /// The viewport override of the layer.
    ViewportOverride(ObjectId),
    /// The default.
    Default,
}

/// A resolved value and its source.
#[derive(Debug, Clone, PartialEq)]
pub struct Sourced<T> {
    /// The value.
    pub value: T,
    /// Where it came from.
    pub source: Source,
}

/// The resolved appearance of an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The color.
    pub color: Sourced<ColorValue>,
    /// The linetype; `None` is continuous.
    pub linetype: Sourced<Option<ObjectId>>,
    /// The lineweight in millimeters; `None` is the default lineweight.
    pub lineweight: Sourced<Option<f64>>,
    /// The transparency.
    pub transparency: Sourced<f64>,
}

/// The default color: opaque white without a source.
#[must_use]
pub fn default_color() -> ColorValue {
    ColorValue {
        rgba: Color::WHITE,
        source: None,
    }
}

fn property<T: Clone>(
    levels: &[Level<'_>],
    overrides: &BTreeMap<ObjectId, LayerOverride>,
    setting: impl Fn(&Style) -> &Setting<T>,
    of_layer: impl Fn(&LayerStyle) -> Option<T>,
    of_override: impl Fn(&LayerOverride) -> Option<T>,
) -> Sourced<Option<T>> {
    let default = Sourced {
        value: None,
        source: Source::Default,
    };
    for (index, level) in levels.iter().enumerate() {
        match setting(level.style) {
            Setting::Block => {}
            Setting::Object(None) => return default,
            Setting::Object(Some(value)) => {
                let source = if index == 0 {
                    Source::Object
                } else {
                    Source::Block { level: index }
                };
                return Sourced {
                    value: Some(value.clone()),
                    source,
                };
            }
            Setting::Layer => {
                let Some(layer) = level.layer else {
                    return default;
                };
                if let Some(value) = overrides.get(&layer.id).and_then(&of_override) {
                    return Sourced {
                        value: Some(value),
                        source: Source::ViewportOverride(layer.id),
                    };
                }
                return Sourced {
                    value: of_layer(layer),
                    source: Source::Layer(layer.id),
                };
            }
        }
    }
    default
}

/// Resolves the appearance of `levels[0]` as described in the [module documentation](self).
/// An empty chain resolves to the defaults.
#[must_use]
pub fn resolve(levels: &[Level<'_>], overrides: &BTreeMap<ObjectId, LayerOverride>) -> Resolved {
    let color = property(
        levels,
        overrides,
        |s| &s.color,
        |l| Some(l.color.clone()),
        |o| o.color.clone(),
    );
    let transparency = property(
        levels,
        overrides,
        |s| &s.transparency,
        |l| Some(l.transparency),
        |o| o.transparency,
    );
    Resolved {
        color: Sourced {
            value: color.value.unwrap_or_else(default_color),
            source: color.source,
        },
        linetype: property(
            levels,
            overrides,
            |s| &s.linetype,
            |l| l.linetype,
            |o| o.linetype,
        ),
        lineweight: property(
            levels,
            overrides,
            |s| &s.lineweight,
            |l| l.lineweight,
            |o| o.lineweight,
        ),
        transparency: Sourced {
            value: transparency.value.unwrap_or(0.0),
            source: transparency.source,
        },
    }
}

fn setting<T>(values: &[Option<Value>], by: usize, value: Option<T>) -> Setting<T> {
    match By::of(enum_at(values, by)) {
        By::Layer => Setting::Layer,
        By::Block => Setting::Block,
        By::Object => Setting::Object(value),
    }
}

/// The style of the field values of an entity.
#[must_use]
pub fn style_of(values: &[Option<Value>]) -> Style {
    let color = match values.get(entity::COLOR) {
        Some(Some(Value::Color(c))) => Some((**c).clone()),
        _ => None,
    };
    Style {
        color: setting(values, entity::COLOR_BY, color),
        linetype: setting(
            values,
            entity::LINETYPE_BY,
            reference_at(values, entity::LINETYPE),
        ),
        lineweight: setting(
            values,
            entity::LINEWEIGHT_BY,
            f64_at(values, entity::LINEWEIGHT),
        ),
        transparency: setting(
            values,
            entity::TRANSPARENCY_BY,
            f64_at(values, entity::TRANSPARENCY),
        ),
    }
}

/// The layer style of the field values of the layer `id`.
#[must_use]
pub fn layer_style_of(id: ObjectId, values: &[Option<Value>]) -> LayerStyle {
    LayerStyle {
        id,
        color: match values.get(layer::COLOR) {
            Some(Some(Value::Color(c))) => (**c).clone(),
            _ => default_color(),
        },
        linetype: reference_at(values, layer::LINETYPE),
        lineweight: f64_at(values, layer::LINEWEIGHT),
        transparency: f64_at(values, layer::TRANSPARENCY).unwrap_or(0.0),
    }
}

/// Resolves the appearance of the entity `chain[0]` in `snapshot`, inserted through the block
/// references `chain[1..]` (innermost first), with the viewport `overrides`.
///
/// # Errors
///
/// `doc.object.not_found` for an identifier of the chain, or a layer, that is not live.
pub fn resolve_in(
    snapshot: &Snapshot,
    chain: &[ObjectId],
    overrides: &BTreeMap<ObjectId, LayerOverride>,
) -> Result<Resolved> {
    let values = |id: ObjectId| -> Result<Vec<Option<Value>>> {
        Ok(snapshot.get(snapshot.resolve(id)?)?.values.clone())
    };
    let mut styles = Vec::with_capacity(chain.len());
    for id in chain {
        let entity = values(*id)?;
        let layer = match reference_at(&entity, entity::LAYER) {
            Some(layer) => Some(layer_style_of(layer, &values(layer)?)),
            None => None,
        };
        styles.push((style_of(&entity), layer));
    }
    let levels: Vec<Level<'_>> = styles
        .iter()
        .map(|(style, layer)| Level {
            style,
            layer: layer.as_ref(),
        })
        .collect();
    Ok(resolve(&levels, overrides))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn id(n: u128) -> ObjectId {
        use arc_base::id::Identifier;
        ObjectId::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n)
    }

    fn gray(level: f32) -> ColorValue {
        ColorValue {
            rgba: Color::new(level, level, level, 1.0),
            source: None,
        }
    }

    fn setting_strategy<T: Clone + std::fmt::Debug + 'static>(
        value: impl Strategy<Value = T> + 'static,
    ) -> impl Strategy<Value = Setting<T>> {
        prop_oneof![
            Just(Setting::Layer),
            Just(Setting::Block),
            prop::option::of(value).prop_map(Setting::Object),
        ]
    }

    fn style() -> impl Strategy<Value = Style> {
        (
            setting_strategy((0_u8..=255).prop_map(|l| gray(f32::from(l) / 255.0))),
            setting_strategy((0_u128..4).prop_map(|n| id(100 + n))),
            setting_strategy(0.0_f64..2.0),
            setting_strategy(0.0_f64..1.0),
        )
            .prop_map(|(color, linetype, lineweight, transparency)| Style {
                color,
                linetype,
                lineweight,
                transparency,
            })
    }

    fn layer(n: u128) -> impl Strategy<Value = LayerStyle> {
        (
            0_u8..=255,
            prop::option::of((0_u128..4).prop_map(|n| id(100 + n))),
            prop::option::of(0.0_f64..2.0),
            0.0_f64..1.0,
        )
            .prop_map(move |(c, linetype, lineweight, transparency)| LayerStyle {
                id: id(n),
                color: gray(f32::from(c) / 255.0),
                linetype,
                lineweight,
                transparency,
            })
    }

    /// The reference model: the documented order written as a recursion over the chain.
    fn model_color(
        chain: &[(Style, Option<LayerStyle>)],
        at: usize,
        overrides: &BTreeMap<ObjectId, LayerOverride>,
    ) -> (ColorValue, Source) {
        let Some((style, layer)) = chain.get(at) else {
            return (default_color(), Source::Default);
        };
        match &style.color {
            Setting::Object(Some(c)) if at == 0 => (c.clone(), Source::Object),
            Setting::Object(Some(c)) => (c.clone(), Source::Block { level: at }),
            Setting::Object(None) => (default_color(), Source::Default),
            Setting::Block => model_color(chain, at + 1, overrides),
            Setting::Layer => match layer {
                None => (default_color(), Source::Default),
                Some(l) => match overrides.get(&l.id).and_then(|o| o.color.clone()) {
                    Some(c) => (c, Source::ViewportOverride(l.id)),
                    None => (l.color.clone(), Source::Layer(l.id)),
                },
            },
        }
    }

    fn model_lineweight(chain: &[(Style, Option<LayerStyle>)], at: usize) -> Option<f64> {
        let (style, layer) = chain.get(at)?;
        match &style.lineweight {
            Setting::Object(v) => *v,
            Setting::Block => model_lineweight(chain, at + 1),
            Setting::Layer => layer.as_ref().and_then(|l| l.lineweight),
        }
    }

    proptest! {
        #[test]
        fn resolution_matches_the_reference_model_through_two_nesting_levels(
            chain in prop::collection::vec((style(), prop::option::of(layer(0))), 1..=3),
            override_color in prop::option::of(0_u8..=255),
        ) {
            // Give each level its own layer identifier.
            let chain: Vec<_> = chain
                .into_iter()
                .zip(1_u128..)
                .map(|((s, l), n)| (s, l.map(|l| LayerStyle { id: id(n), ..l })))
                .collect();
            let mut overrides = BTreeMap::new();
            if let Some(c) = override_color {
                overrides.insert(id(1), LayerOverride { color: Some(gray(f32::from(c) / 255.0)), ..LayerOverride::default() });
            }
            let levels: Vec<_> = chain.iter().map(|(s, l)| Level { style: s, layer: l.as_ref() }).collect();
            let resolved = resolve(&levels, &overrides);
            let (color, source) = model_color(&chain, 0, &overrides);
            prop_assert_eq!(&resolved.color.value, &color);
            prop_assert_eq!(resolved.color.source, source);
            prop_assert_eq!(resolved.lineweight.value, model_lineweight(&chain, 0));
        }
    }

    #[test]
    fn by_block_through_two_levels_reaches_the_outer_layer() {
        let by_block = Style {
            color: Setting::Block,
            linetype: Setting::Block,
            lineweight: Setting::Block,
            transparency: Setting::Block,
        };
        let by_layer = Style {
            color: Setting::Layer,
            linetype: Setting::Layer,
            lineweight: Setting::Object(Some(0.5)),
            transparency: Setting::Object(None),
        };
        let outer = LayerStyle {
            id: id(9),
            color: gray(0.25),
            linetype: Some(id(100)),
            lineweight: None,
            transparency: 0.5,
        };
        let levels = [
            Level {
                style: &by_block,
                layer: None,
            },
            Level {
                style: &by_block,
                layer: None,
            },
            Level {
                style: &by_layer,
                layer: Some(&outer),
            },
        ];
        let r = resolve(&levels, &BTreeMap::new());
        assert_eq!(
            r.color,
            Sourced {
                value: gray(0.25),
                source: Source::Layer(id(9))
            }
        );
        assert_eq!(r.linetype.value, Some(id(100)));
        assert_eq!(
            r.lineweight,
            Sourced {
                value: Some(0.5),
                source: Source::Block { level: 2 }
            }
        );
        assert_eq!(
            r.transparency,
            Sourced {
                value: 0.0,
                source: Source::Default
            }
        );
        assert_eq!(
            resolve(&levels[..2], &BTreeMap::new()).color.source,
            Source::Default
        );
        assert_eq!(resolve(&[], &BTreeMap::new()).color.value, default_color());
    }
}
