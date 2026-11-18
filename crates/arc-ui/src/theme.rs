//! Theme tokens, shipped themes, density and UI scale (CORE-UI-021, CORE-UI-022, CORE-UI-028).
//!
//! A [`Theme`] is a token set: [`Colors`] and [`Metrics`]. Widgets read tokens, never literal
//! colors. [`Appearance`] combines a theme with a [`Density`], a [`UiScale`] and the
//! reduced-motion flag; [`Appearance::style`] maps the transformed tokens onto an
//! [`egui::Style`] and [`Appearance::apply`] installs it in a context at run time.

use arc_base::{Error, Result};
use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};
use serde::{Deserialize, Serialize};

use crate::error::codes;

/// The color tokens of a theme, opaque sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    /// Panel background.
    pub surface: Color32,
    /// Background of windows, menus, popups and notifications.
    pub surface_raised: Color32,
    /// Background of text and number fields.
    pub field: Color32,
    /// Fill of buttons and other controls at rest.
    pub control: Color32,
    /// Fill of a hovered, pressed or open control.
    pub control_hover: Color32,
    /// Fill behind selected items and selected text.
    pub selection: Color32,
    /// Viewport background.
    pub viewport: Color32,
    /// Body text.
    pub text: Color32,
    /// Secondary text: hints, binding text, captions.
    pub text_muted: Color32,
    /// Text of disabled controls (exempt from contrast minimums, as inactive components are).
    pub text_disabled: Color32,
    /// Boundary of controls and fields.
    pub border: Color32,
    /// Accent: links, the active control's boundary, progress.
    pub accent: Color32,
    /// Text on an accent fill.
    pub on_accent: Color32,
    /// Keyboard focus indicator.
    pub focus: Color32,
    /// Information severity.
    pub info: Color32,
    /// Success severity.
    pub success: Color32,
    /// Warning severity.
    pub warning: Color32,
    /// Error severity.
    pub error: Color32,
    /// Selected objects in a viewport.
    pub viewport_selection: Color32,
    /// Pre-selected (hovered) objects in a viewport.
    pub viewport_preselection: Color32,
}

/// The size tokens of a theme, in logical points at 100 % scale and standard density.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    /// Gap between neighboring items.
    pub spacing: f32,
    /// Horizontal padding inside a control.
    pub padding_x: f32,
    /// Vertical padding inside a control.
    pub padding_y: f32,
    /// Minimum height of a row or control.
    pub row_height: f32,
    /// Indentation of nested items.
    pub indent: f32,
    /// Inner margin of windows and menus.
    pub margin: f32,
    /// Corner radius of controls.
    pub radius_control: f32,
    /// Corner radius of windows and menus.
    pub radius_window: f32,
    /// Width of control boundaries.
    pub stroke: f32,
    /// Width of the focus indicator.
    pub focus_stroke: f32,
    /// Small text size.
    pub text_small: f32,
    /// Body text size.
    pub text_body: f32,
    /// Heading text size.
    pub text_heading: f32,
    /// Monospace text size.
    pub text_mono: f32,
    /// Icon edge length.
    pub icon: f32,
}

/// The metrics every shipped theme uses.
pub const STANDARD_METRICS: Metrics = Metrics {
    spacing: 6.0,
    padding_x: 8.0,
    padding_y: 4.0,
    row_height: 24.0,
    indent: 16.0,
    margin: 8.0,
    radius_control: 3.0,
    radius_window: 6.0,
    stroke: 1.0,
    focus_stroke: 2.0,
    text_small: 11.0,
    text_body: 13.0,
    text_heading: 17.0,
    text_mono: 13.0,
    icon: 16.0,
};

/// A shipped theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    /// Light.
    #[default]
    Light,
    /// Dark.
    Dark,
    /// High-contrast light.
    HighContrastLight,
    /// High-contrast dark.
    HighContrastDark,
}

impl ThemeId {
    /// Every shipped theme.
    pub const ALL: [Self; 4] = [
        Self::Light,
        Self::Dark,
        Self::HighContrastLight,
        Self::HighContrastDark,
    ];

    /// The theme's tokens.
    #[must_use]
    pub fn theme(self) -> Theme {
        let colors = match self {
            Self::Light => LIGHT,
            Self::Dark => DARK,
            Self::HighContrastLight => HIGH_CONTRAST_LIGHT,
            Self::HighContrastDark => HIGH_CONTRAST_DARK,
        };
        Theme {
            id: self,
            colors,
            metrics: STANDARD_METRICS,
        }
    }

    /// True for the dark themes.
    #[must_use]
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Dark | Self::HighContrastDark)
    }
}

/// A token set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// The shipped theme it is.
    pub id: ThemeId,
    /// Color tokens.
    pub colors: Colors,
    /// Size tokens.
    pub metrics: Metrics,
}

const fn rgb(hex: u32) -> Color32 {
    let [_, r, g, b] = hex.to_be_bytes();
    Color32::from_rgb(r, g, b)
}

const LIGHT: Colors = Colors {
    surface: rgb(0xF4F5F7),
    surface_raised: rgb(0xFFFFFF),
    field: rgb(0xFFFFFF),
    control: rgb(0xE3E6EA),
    control_hover: rgb(0xD5D9DF),
    selection: rgb(0xCCDDF7),
    viewport: rgb(0xE8EAED),
    text: rgb(0x1B1D21),
    text_muted: rgb(0x4D535C),
    text_disabled: rgb(0x8A9099),
    border: rgb(0x767C86),
    accent: rgb(0x1D5BB8),
    on_accent: rgb(0xFFFFFF),
    focus: rgb(0x1D5BB8),
    info: rgb(0x1D5BB8),
    success: rgb(0x1B6E34),
    warning: rgb(0x835500),
    error: rgb(0xB0241C),
    viewport_selection: rgb(0x1D5BB8),
    viewport_preselection: rgb(0xC25E00),
};

const DARK: Colors = Colors {
    surface: rgb(0x1E2024),
    surface_raised: rgb(0x272A2F),
    field: rgb(0x15171A),
    control: rgb(0x33373D),
    control_hover: rgb(0x3E434A),
    selection: rgb(0x24446F),
    viewport: rgb(0x2A2D31),
    text: rgb(0xE8EAED),
    text_muted: rgb(0xB2B7BF),
    text_disabled: rgb(0x6D737C),
    border: rgb(0x878D96),
    accent: rgb(0x5C9BEB),
    on_accent: rgb(0x0B1220),
    focus: rgb(0x7FB2F2),
    info: rgb(0x7FB2F2),
    success: rgb(0x5CC27A),
    warning: rgb(0xE3B03F),
    error: rgb(0xF2847A),
    viewport_selection: rgb(0x7FB2F2),
    viewport_preselection: rgb(0xF0A35A),
};

const HIGH_CONTRAST_LIGHT: Colors = Colors {
    surface: rgb(0xFFFFFF),
    surface_raised: rgb(0xFFFFFF),
    field: rgb(0xFFFFFF),
    control: rgb(0xFFFFFF),
    control_hover: rgb(0xE6E6E6),
    selection: rgb(0xFFE680),
    viewport: rgb(0xFFFFFF),
    text: rgb(0x000000),
    text_muted: rgb(0x1F1F1F),
    text_disabled: rgb(0x6B6B6B),
    border: rgb(0x000000),
    accent: rgb(0x0037A6),
    on_accent: rgb(0xFFFFFF),
    focus: rgb(0xC2185B),
    info: rgb(0x0037A6),
    success: rgb(0x00591F),
    warning: rgb(0x5C3A00),
    error: rgb(0xA30000),
    viewport_selection: rgb(0x0037A6),
    viewport_preselection: rgb(0x9A3D00),
};

const HIGH_CONTRAST_DARK: Colors = Colors {
    surface: rgb(0x000000),
    surface_raised: rgb(0x000000),
    field: rgb(0x000000),
    control: rgb(0x000000),
    control_hover: rgb(0x1F1F1F),
    selection: rgb(0x3D2E00),
    viewport: rgb(0x000000),
    text: rgb(0xFFFFFF),
    text_muted: rgb(0xE6E6E6),
    text_disabled: rgb(0x9E9E9E),
    border: rgb(0xFFFFFF),
    accent: rgb(0xFFD60A),
    on_accent: rgb(0x000000),
    focus: rgb(0x3FF0FF),
    info: rgb(0x8CC8FF),
    success: rgb(0x6CF08A),
    warning: rgb(0xFFD60A),
    error: rgb(0xFF8A80),
    viewport_selection: rgb(0x3FF0FF),
    viewport_preselection: rgb(0xFFB060),
};

/// The relative luminance of an sRGB color (WCAG 2.x).
#[must_use]
pub fn relative_luminance(color: Color32) -> f64 {
    fn channel(c: u8) -> f64 {
        let c = f64::from(c) / 255.0;
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// The WCAG contrast ratio of two opaque colors, from 1 to 21.
#[must_use]
pub fn contrast_ratio(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// What a token pair must meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairKind {
    /// Text on a background: 4.5 : 1.
    Text,
    /// A control boundary, indicator or graphic on a background: 3 : 1.
    Control,
}

impl PairKind {
    /// The minimum contrast ratio.
    #[must_use]
    pub fn minimum(self) -> f64 {
        match self {
            Self::Text => 4.5,
            Self::Control => 3.0,
        }
    }
}

/// A foreground and background token used together, with the names for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenPair {
    /// What the pair must meet.
    pub kind: PairKind,
    /// Foreground token name.
    pub foreground: &'static str,
    /// Background token name.
    pub background: &'static str,
    /// Foreground color.
    pub fg: Color32,
    /// Background color.
    pub bg: Color32,
}

impl Colors {
    /// Every foreground and background pair the style mapping and the widgets of this crate
    /// draw together. Disabled text is not listed.
    #[must_use]
    pub fn pairs(&self) -> Vec<TokenPair> {
        let c = self;
        let backgrounds = [
            ("surface", c.surface),
            ("surface_raised", c.surface_raised),
            ("field", c.field),
            ("control", c.control),
            ("control_hover", c.control_hover),
            ("selection", c.selection),
        ];
        let plain = [("surface", c.surface), ("surface_raised", c.surface_raised)];
        let mut out = Vec::new();
        let mut add = |kind, foreground, fg, list: &[(&'static str, Color32)]| {
            for &(background, bg) in list {
                out.push(TokenPair {
                    kind,
                    foreground,
                    background,
                    fg,
                    bg,
                });
            }
        };
        add(PairKind::Text, "text", c.text, &backgrounds);
        add(
            PairKind::Text,
            "text_muted",
            c.text_muted,
            &backgrounds[..3],
        );
        for (name, color) in [
            ("info", c.info),
            ("success", c.success),
            ("warning", c.warning),
            ("error", c.error),
            ("accent", c.accent),
        ] {
            add(PairKind::Text, name, color, &plain);
        }
        add(
            PairKind::Text,
            "on_accent",
            c.on_accent,
            &[("accent", c.accent)],
        );
        add(PairKind::Control, "border", c.border, &backgrounds[..3]);
        add(PairKind::Control, "accent", c.accent, &backgrounds[..3]);
        add(PairKind::Control, "focus", c.focus, &backgrounds[..4]);
        let viewport = [("viewport", c.viewport)];
        add(
            PairKind::Control,
            "viewport_selection",
            c.viewport_selection,
            &viewport,
        );
        add(
            PairKind::Control,
            "viewport_preselection",
            c.viewport_preselection,
            &viewport,
        );
        out
    }
}

/// Spacing density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Spacing, padding, margins and row heights at 80 %.
    Compact,
    /// The token values.
    #[default]
    Standard,
    /// Spacing, padding, margins and row heights at 125 %.
    Comfortable,
}

impl Density {
    /// The factor on spacing-like tokens.
    #[must_use]
    pub fn factor(self) -> f32 {
        match self {
            Self::Compact => 0.8,
            Self::Standard => 1.0,
            Self::Comfortable => 1.25,
        }
    }
}

/// The UI scale in percent, 75 to 300.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct UiScale(u16);

impl UiScale {
    /// The smallest scale, in percent.
    pub const MIN_PERCENT: u16 = 75;
    /// The largest scale, in percent.
    pub const MAX_PERCENT: u16 = 300;
    /// 100 %.
    pub const NORMAL: Self = Self(100);

    /// A scale of `percent`; fails with `ui.scale.out_of_range` outside 75 to 300.
    pub fn new(percent: u16) -> Result<Self> {
        if (Self::MIN_PERCENT..=Self::MAX_PERCENT).contains(&percent) {
            Ok(Self(percent))
        } else {
            Err(Error::new(codes::SCALE_OUT_OF_RANGE).with_arg("percent", i64::from(percent)))
        }
    }

    /// The scale in percent.
    #[must_use]
    pub fn percent(self) -> u16 {
        self.0
    }

    /// The scale as a factor.
    #[must_use]
    pub fn factor(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for UiScale {
    fn default() -> Self {
        Self::NORMAL
    }
}

impl TryFrom<u16> for UiScale {
    type Error = Error;
    fn try_from(percent: u16) -> Result<Self> {
        Self::new(percent)
    }
}

impl From<UiScale> for u16 {
    fn from(scale: UiScale) -> Self {
        scale.0
    }
}

impl Metrics {
    /// The tokens after density and scale: density multiplies spacing, padding, indentation,
    /// margins and row height; scale then multiplies every token.
    #[must_use]
    pub fn transformed(&self, density: Density, scale: UiScale) -> Self {
        let d = density.factor();
        let s = scale.factor();
        Self {
            spacing: self.spacing * d * s,
            padding_x: self.padding_x * d * s,
            padding_y: self.padding_y * d * s,
            row_height: self.row_height * d * s,
            indent: self.indent * d * s,
            margin: self.margin * d * s,
            radius_control: self.radius_control * s,
            radius_window: self.radius_window * s,
            stroke: self.stroke * s,
            focus_stroke: self.focus_stroke * s,
            text_small: self.text_small * s,
            text_body: self.text_body * s,
            text_heading: self.text_heading * s,
            text_mono: self.text_mono * s,
            icon: self.icon * s,
        }
    }
}

/// The caller's appearance choice: theme, density, scale and reduced motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Appearance {
    /// The theme.
    pub theme: ThemeId,
    /// The density.
    pub density: Density,
    /// The UI scale; it applies on top of the platform's scale factor.
    pub scale: UiScale,
    /// Turns animations off. The caller passes the operating system's setting.
    pub reduced_motion: bool,
}

fn widget(
    fill: Color32,
    border: Stroke,
    text: Color32,
    radius: CornerRadius,
) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: border,
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

impl Appearance {
    /// The theme's tokens after density and scale.
    #[must_use]
    pub fn tokens(&self) -> Theme {
        let theme = self.theme.theme();
        Theme {
            metrics: theme.metrics.transformed(self.density, self.scale),
            ..theme
        }
    }

    /// The egui style that draws with the transformed tokens.
    #[must_use]
    pub fn style(&self) -> egui::Style {
        let Theme {
            colors: c,
            metrics: m,
            ..
        } = self.tokens();
        let mut style = egui::Style::default();
        let control_radius = CornerRadius::from(m.radius_control);
        let window_radius = CornerRadius::from(m.radius_window);
        let border = Stroke::new(m.stroke, c.border);

        let v = &mut style.visuals;
        v.dark_mode = self.theme.is_dark();
        v.override_text_color = None;
        v.weak_text_color = Some(c.text_muted);
        v.panel_fill = c.surface;
        v.window_fill = c.surface_raised;
        v.window_stroke = border;
        v.window_corner_radius = window_radius;
        v.menu_corner_radius = window_radius;
        v.faint_bg_color = c.control;
        v.extreme_bg_color = c.field;
        v.text_edit_bg_color = Some(c.field);
        v.code_bg_color = c.control;
        v.hyperlink_color = c.accent;
        v.warn_fg_color = c.warning;
        v.error_fg_color = c.error;
        v.selection.bg_fill = c.selection;
        v.selection.stroke = Stroke::new(m.focus_stroke, c.text);
        v.text_cursor.stroke = Stroke::new(m.focus_stroke, c.text);
        v.widgets.noninteractive = widget(c.surface, border, c.text, control_radius);
        v.widgets.inactive = widget(c.control, border, c.text, control_radius);
        v.widgets.hovered = widget(c.control_hover, border, c.text, control_radius);
        v.widgets.active = widget(
            c.control_hover,
            Stroke::new(m.focus_stroke, c.accent),
            c.text,
            control_radius,
        );
        v.widgets.open = widget(c.control_hover, border, c.text, control_radius);

        let sp = &mut style.spacing;
        sp.item_spacing = Vec2::splat(m.spacing);
        sp.button_padding = Vec2::new(m.padding_x, m.padding_y);
        sp.interact_size = Vec2::new(m.row_height * 2.0, m.row_height);
        sp.indent = m.indent;
        sp.window_margin = Margin::from(m.margin);
        sp.menu_margin = Margin::from(m.margin);
        sp.icon_width = m.icon;
        sp.icon_width_inner = m.icon * 0.5;
        sp.icon_spacing = m.spacing;

        style.text_styles = [
            (
                TextStyle::Small,
                FontId::new(m.text_small, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(m.text_body, FontFamily::Proportional),
            ),
            (
                TextStyle::Button,
                FontId::new(m.text_body, FontFamily::Proportional),
            ),
            (
                TextStyle::Heading,
                FontId::new(m.text_heading, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(m.text_mono, FontFamily::Monospace),
            ),
        ]
        .into();

        if self.reduced_motion {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
            style.visuals.text_cursor.blink = false;
        }
        style
    }

    /// Installs [`Appearance::style`] in `ctx` for both the light and the dark slot, so the
    /// platform's light or dark preference does not override the chosen theme. Takes effect on
    /// the next pass; nothing restarts.
    pub fn apply(&self, ctx: &egui::Context) {
        let style = self.style();
        ctx.all_styles_mut(|s| *s = style.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn contrast_ratio_matches_known_values() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(Color32::WHITE, Color32::WHITE) - 1.0).abs() < 1e-12);
        // #767676 on white is the classic 4.54 : 1.
        let grey = Color32::from_rgb(0x76, 0x76, 0x76);
        assert!((contrast_ratio(grey, Color32::WHITE) - 4.54).abs() < 0.01);
    }

    #[test]
    fn every_theme_meets_text_and_control_contrast() {
        let mut failures = Vec::new();
        for id in ThemeId::ALL {
            let pairs = id.theme().colors.pairs();
            assert!(pairs.len() > 20);
            for p in pairs {
                let ratio = contrast_ratio(p.fg, p.bg);
                if ratio < p.kind.minimum() {
                    failures.push(format!(
                        "{id:?}: {} on {} is {ratio:.2}",
                        p.foreground, p.background
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn style_maps_text_and_background_tokens_that_are_checked() {
        for id in ThemeId::ALL {
            let c = id.theme().colors;
            let style = Appearance {
                theme: id,
                ..Appearance::default()
            }
            .style();
            let v = &style.visuals;
            for w in [&v.widgets.inactive, &v.widgets.hovered, &v.widgets.active] {
                assert!(contrast_ratio(w.fg_stroke.color, w.bg_fill) >= 4.5);
            }
            assert!(contrast_ratio(v.text_color(), v.panel_fill) >= 4.5);
            assert!(contrast_ratio(v.selection.stroke.color, v.selection.bg_fill) >= 4.5);
            assert_eq!(v.window_fill, c.surface_raised);
            assert_eq!(v.dark_mode, id.is_dark());
        }
    }

    #[test]
    fn switching_theme_takes_effect_on_the_next_pass_without_restart() {
        let ctx = egui::Context::default();
        let fill = |ctx: &egui::Context| {
            let mut out = Color32::TRANSPARENT;
            let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
                out = ui.visuals().panel_fill;
            });
            full.textures_delta.clear();
            out
        };
        for id in ThemeId::ALL {
            Appearance {
                theme: id,
                ..Appearance::default()
            }
            .apply(&ctx);
            assert_eq!(fill(&ctx), id.theme().colors.surface);
        }
    }

    #[test]
    fn reduced_motion_turns_animations_off() {
        let on = Appearance::default().style();
        assert!(on.animation_time > 0.0);
        let off = Appearance {
            reduced_motion: true,
            ..Appearance::default()
        }
        .style();
        assert_eq!(off.animation_time, 0.0);
        assert!(!off.visuals.text_cursor.blink);
    }

    #[test]
    fn scale_outside_the_range_is_rejected() {
        for bad in [0, 74, 301, u16::MAX] {
            assert_eq!(
                UiScale::new(bad).map_err(|e| e.code()),
                Err("ui.scale.out_of_range")
            );
        }
        let parsed: std::result::Result<UiScale, _> = serde_json::from_str("50");
        assert!(parsed.is_err());
        assert_eq!(
            serde_json::from_str::<UiScale>("150").ok(),
            UiScale::new(150).ok()
        );
    }

    #[test]
    fn density_changes_spacing_but_not_type() {
        let base = STANDARD_METRICS;
        let compact = base.transformed(Density::Compact, UiScale::NORMAL);
        let comfortable = base.transformed(Density::Comfortable, UiScale::NORMAL);
        assert!(compact.row_height < base.row_height);
        assert!(comfortable.row_height > base.row_height);
        assert_eq!(compact.text_body, base.text_body);
        assert_eq!(comfortable.icon, base.icon);
    }

    proptest! {
        #[test]
        fn scale_multiplies_every_token(percent in 75u16..=300, density in prop_oneof![
            Just(Density::Compact), Just(Density::Standard), Just(Density::Comfortable)]) {
            let scale = UiScale::new(percent).unwrap();
            let unscaled = STANDARD_METRICS.transformed(density, UiScale::NORMAL);
            let scaled = STANDARD_METRICS.transformed(density, scale);
            let f = f32::from(percent) / 100.0;
            for (a, b) in [
                (unscaled.row_height, scaled.row_height),
                (unscaled.spacing, scaled.spacing),
                (unscaled.text_body, scaled.text_body),
                (unscaled.stroke, scaled.stroke),
                (unscaled.icon, scaled.icon),
            ] {
                prop_assert!((a * f - b).abs() <= 1e-4 * b.max(1.0));
            }
            let style = Appearance { theme: ThemeId::Dark, density, scale, reduced_motion: false }.style();
            prop_assert!((style.spacing.interact_size.y - scaled.row_height).abs() < 1e-4);
            prop_assert!(style.spacing.item_spacing.x.is_finite());
        }
    }
}
