//! The settings the windowed shell applies without a restart (CORE-APP-010, CORE-APP-014):
//! `ui.language` (built in) and the appearance settings `ui.theme`, `ui.density` and `ui.scale`,
//! which [`register_settings`] adds to a product.

use arc_base::Result;
use arc_ui::{Appearance, Density, ThemeId, UiScale};
use serde_json::Value as Json;

use crate::application::Application;
use crate::locale::{self, Localizer, Selection};
use crate::product::ProductBuilder;
use crate::settings::{Scope, Setting, SettingType, SettingValue, SettingsStore};

/// The language of the user interface (a built-in setting).
pub const LANGUAGE: &str = "ui.language";
/// The theme: `light`, `dark`, `high_contrast_light` or `high_contrast_dark`.
pub const THEME: &str = "ui.theme";
/// The spacing density: `compact`, `standard` or `comfortable`.
pub const DENSITY: &str = "ui.density";
/// The UI scale in percent, 75 to 300.
pub const SCALE: &str = "ui.scale";

/// Registers [`THEME`], [`DENSITY`] and [`SCALE`], each settable up to the user scope.
///
/// # Errors
/// `app.settings.duplicate_key` when the product registered one of them already.
pub fn register_settings(builder: &mut ProductBuilder) -> Result<()> {
    builder.setting(Setting {
        key: THEME,
        ty: SettingType::Choice(&["light", "dark", "high_contrast_light", "high_contrast_dark"]),
        default: SettingValue::Text("light".into()),
        scope: Scope::User,
        message_id: "app-setting-ui-theme",
    })?;
    builder.setting(Setting {
        key: DENSITY,
        ty: SettingType::Choice(&["compact", "standard", "comfortable"]),
        default: SettingValue::Text("standard".into()),
        scope: Scope::User,
        message_id: "app-setting-ui-density",
    })?;
    builder.setting(Setting {
        key: SCALE,
        ty: SettingType::Int {
            min: i64::from(UiScale::MIN_PERCENT),
            max: i64::from(UiScale::MAX_PERCENT),
        },
        default: SettingValue::Int(100),
        scope: Scope::User,
        message_id: "app-setting-ui-scale",
    })
}

/// A value set in some scope, so the product's own appearance is overridden only by a choice.
fn chosen(store: &SettingsStore, key: &str) -> Option<SettingValue> {
    store
        .get(key)
        .ok()
        .filter(|e| e.source != Scope::Default)
        .map(|e| e.value)
}

fn text_as<T: serde::de::DeserializeOwned>(value: Option<SettingValue>) -> Option<T> {
    match value? {
        SettingValue::Text(t) => serde_json::from_value(Json::String(t)).ok(),
        _ => None,
    }
}

/// `base` with the theme, density and scale that a settings scope chose; unregistered or unset
/// settings keep `base`'s values.
#[must_use]
pub fn appearance(store: &SettingsStore, base: Appearance) -> Appearance {
    let scale = match chosen(store, SCALE) {
        Some(SettingValue::Int(p)) => u16::try_from(p).ok().and_then(|p| UiScale::new(p).ok()),
        _ => None,
    };
    Appearance {
        theme: text_as::<ThemeId>(chosen(store, THEME)).unwrap_or(base.theme),
        density: text_as::<Density>(chosen(store, DENSITY)).unwrap_or(base.density),
        scale: scale.unwrap_or(base.scale),
        reduced_motion: base.reduced_motion,
    }
}

/// Switches `localizer` to the language of [`LANGUAGE`] (`system` follows `os_locales`).
pub fn apply_language(
    app: &Application,
    localizer: &mut Localizer,
    os_locales: &[String],
) -> Selection {
    let setting = match app.settings().get(LANGUAGE).map(|e| e.value) {
        Ok(SettingValue::Text(t)) => t,
        _ => String::new(),
    };
    localizer.set_requested(&locale::requested_from_setting(&setting, os_locales))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsRegistry;

    #[test]
    fn appearance_follows_chosen_settings_only() {
        let mut b = ProductBuilder::new("t", "0");
        register_settings(&mut b).unwrap();
        assert!(register_settings(&mut b).is_err());
        // The builder's registry is not public; a store over the same kinds of settings.
        let mut registry = SettingsRegistry::builtin();
        registry
            .register(Setting {
                key: THEME,
                ty: SettingType::Choice(&["light", "dark"]),
                default: SettingValue::Text("light".into()),
                scope: Scope::User,
                message_id: "x",
            })
            .unwrap();
        registry
            .register(Setting {
                key: SCALE,
                ty: SettingType::Int { min: 75, max: 300 },
                default: SettingValue::Int(100),
                scope: Scope::User,
                message_id: "x",
            })
            .unwrap();
        let mut store = SettingsStore::new(registry);
        let base = Appearance {
            density: Density::Compact,
            ..Appearance::default()
        };
        assert_eq!(appearance(&store, base), base);
        store
            .set(Scope::User, THEME, SettingValue::Text("dark".into()))
            .unwrap();
        store
            .set(Scope::User, SCALE, SettingValue::Int(150))
            .unwrap();
        let a = appearance(&store, base);
        assert_eq!(
            (a.theme, a.density, a.scale.percent()),
            (ThemeId::Dark, Density::Compact, 150)
        );
    }
}
