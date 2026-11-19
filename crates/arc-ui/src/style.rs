//! The style pickers (CORE-UI-015): color (indexed and true color), layer, linetype, lineweight
//! and material.
//!
//! A [`StylePicker`] is a drop-down fed by the caller: a list of [`StyleEntry`] values, each with
//! an identifier, a display name and a [`Preview`]. The caller maps its own styles to entries, so
//! this module knows nothing of document types. The drop-down shows the current entry, opens with
//! a click, Enter or Space, filters the list as the user types (matching the name or the
//! identifier, as [`crate::text::matches_filter`] does) and moves through it with the arrow keys.
//! Enter returns the chosen identifier; Escape closes without a choice.
//!
//! The color picker also offers a true color: when the filter text is a six-digit hexadecimal
//! color (with or without `#`), an extra last row offers it, and choosing it returns the identifier
//! `#rrggbb` in lower case. Previews are painted with egui: a swatch, a dash sample or a line of
//! the given weight.

use egui::{
    Button, Color32, Key, Modifiers, Role, ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2,
    WidgetInfo, pos2, vec2,
};

use arc_base::error::Value;
use arc_base::{Error, Result};

use crate::error::codes;
use crate::text::{Direction, Localize, Message, matches_filter, messages, resolve};

/// The most entries a picker offers; a longer list is checked with [`StylePicker::check`].
pub const MAX_ENTRIES: usize = 4_096;
/// The most items in one dash pattern; longer patterns are drawn solid.
pub const MAX_DASH_ITEMS: usize = 16;
/// The most dashes drawn in a preview.
pub const MAX_DASH_SEGMENTS: usize = 256;

/// Which style a picker chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleKind {
    /// A color; the picker also accepts a true color.
    Color,
    /// A layer.
    Layer,
    /// A linetype.
    Linetype,
    /// A lineweight.
    Lineweight,
    /// A material.
    Material,
}

impl StyleKind {
    fn message(self) -> Message {
        match self {
            Self::Color => messages::STYLE_COLOR,
            Self::Layer => messages::STYLE_LAYER,
            Self::Linetype => messages::STYLE_LINETYPE,
            Self::Lineweight => messages::STYLE_LINEWEIGHT,
            Self::Material => messages::STYLE_MATERIAL,
        }
    }
}

/// What an entry shows beside its name.
#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// Nothing; the name only.
    None,
    /// A color swatch: an indexed color's RGB value or a layer's or material's color.
    Color(Color32),
    /// A dash pattern: alternating dash and gap lengths in logical points. A pattern with a
    /// negative or non-finite item, more than [`MAX_DASH_ITEMS`] items or no length is drawn solid.
    Dash(Vec<f32>),
    /// A lineweight in millimeters.
    Weight(f32),
}

/// One choice of a picker, supplied by the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleEntry {
    /// The identifier returned when the entry is chosen.
    pub id: String,
    /// The display name (user data, not a message).
    pub name: String,
    /// The preview.
    pub preview: Preview,
}

/// What the drop-down lists: an entry by index, or the true color offered by the filter text.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Entry(usize),
    TrueColor([u8; 3]),
}

/// The state of one style picker. Keep one per picker in the owning panel.
#[derive(Debug, Clone)]
pub struct StylePicker {
    /// The style the picker chooses.
    pub kind: StyleKind,
    /// The active language tag, which selects the case-folding rule of the filter; empty for none.
    pub language: String,
    open: bool,
    focus_filter: bool,
    filter: String,
    highlight: usize,
}

impl StylePicker {
    /// A closed picker for `kind`.
    #[must_use]
    pub fn new(kind: StyleKind) -> Self {
        Self {
            kind,
            language: String::new(),
            open: false,
            focus_filter: false,
            filter: String::new(),
            highlight: 0,
        }
    }

    /// Whether the list is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The text the filter currently holds.
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Checks that a list can be offered.
    ///
    /// # Errors
    ///
    /// `ui.style.too_many` when `entries` holds more than [`MAX_ENTRIES`] entries. [`Self::show`]
    /// offers only the first [`MAX_ENTRIES`] entries, so a caller should check first.
    pub fn check(entries: &[StyleEntry]) -> Result<()> {
        if entries.len() > MAX_ENTRIES {
            return Err(Error::new(codes::STYLE_TOO_MANY));
        }
        Ok(())
    }

    /// The indices of the entries whose name or identifier matches `filter`, in list order. An
    /// empty filter matches every entry.
    #[must_use]
    pub fn matching(entries: &[StyleEntry], filter: &str, language: &str) -> Vec<usize> {
        entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                matches_filter(&e.name, filter, language) || matches_filter(&e.id, filter, language)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn rows(&self, entries: &[StyleEntry]) -> Vec<Row> {
        let mut rows: Vec<Row> = Self::matching(entries, &self.filter, &self.language)
            .into_iter()
            .map(Row::Entry)
            .collect();
        if self.kind == StyleKind::Color
            && let Some(rgb) = true_color(&self.filter)
        {
            rows.push(Row::TrueColor(rgb));
        }
        rows
    }

    /// Draws the picker for `entries` with `current` (an identifier) shown, and returns the
    /// identifier chosen this frame.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        entries: &[StyleEntry],
        current: Option<&str>,
        localize: &dyn Localize,
    ) -> Option<String> {
        let entries = entries
            .get(..entries.len().min(MAX_ENTRIES))
            .unwrap_or(entries);
        let (preview, name) = match current {
            Some(id) => match entries.iter().find(|e| e.id == id) {
                Some(entry) => (entry.preview.clone(), entry.name.clone()),
                None => (
                    true_color(id).map_or(Preview::None, |rgb| Preview::Color(color_of(rgb))),
                    id.to_owned(),
                ),
            },
            None => (Preview::None, String::new()),
        };
        let kind = self.kind.message().text(localize);
        let direction = Direction::of_language(&self.language);
        let mut chosen = None;
        direction.row(ui, false, |ui| {
            draw_preview(ui, &preview);
            let response = ui.add(Button::new(name.as_str()));
            let label = format!("{kind}: {name}");
            response.widget_info(|| WidgetInfo::labeled(Role::ComboBox, true, label.as_str()));
            if response.clicked() {
                self.toggle(entries, current);
            }
        });
        if self.open {
            chosen = self.dropdown(ui, entries, localize);
        }
        if chosen.is_some() {
            self.open = false;
        }
        chosen
    }

    fn toggle(&mut self, entries: &[StyleEntry], current: Option<&str>) {
        self.open = !self.open;
        self.filter.clear();
        self.focus_filter = self.open;
        self.highlight = current
            .and_then(|id| {
                self.rows(entries)
                    .iter()
                    .position(|row| matches!(row, Row::Entry(i) if entries.get(*i).is_some_and(|e| e.id == id)))
            })
            .unwrap_or(0);
    }

    fn dropdown(
        &mut self,
        ui: &mut Ui,
        entries: &[StyleEntry],
        localize: &dyn Localize,
    ) -> Option<String> {
        let (up, down, enter, escape) = ui.input_mut(|i| {
            (
                i.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
                i.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.open = false;
            return None;
        }
        let rows = self.rows(entries);
        let last = rows.len().saturating_sub(1);
        self.highlight = self.highlight.saturating_add(down).min(last);
        self.highlight = self.highlight.saturating_sub(up);
        self.highlight = self.highlight.min(last);

        let filter_label = messages::STYLE_FILTER.text(localize);
        let direction = Direction::of_language(&self.language);
        let mut changed = false;
        direction.row(ui, false, |ui| {
            let label = ui.label(filter_label);
            let edit = ui
                .add(TextEdit::singleline(&mut self.filter))
                .labelled_by(label.id);
            if self.focus_filter {
                edit.request_focus();
                self.focus_filter = false;
            }
            changed = edit.changed();
        });
        if changed {
            self.highlight = 0;
        }
        let rows = self.rows(entries);
        let mut chosen = None;
        if enter && let Some(row) = rows.get(self.highlight) {
            chosen = Some(row_id(row, entries));
        }
        let height = ui.spacing().interact_size.y;
        ScrollArea::vertical()
            .max_height(240.0)
            .auto_shrink([false, true])
            .show_rows(ui, height, rows.len(), |ui, range| {
                for index in range {
                    let Some(row) = rows.get(index) else { continue };
                    let (name, preview) = row_text(row, entries, localize);
                    direction.row(ui, false, |ui| {
                        draw_preview(ui, &preview);
                        let response = ui.selectable_label(index == self.highlight, name);
                        if response.clicked() {
                            chosen = Some(row_id(row, entries));
                        }
                    });
                }
            });
        if chosen.is_some() {
            self.filter.clear();
        }
        chosen
    }
}

fn row_id(row: &Row, entries: &[StyleEntry]) -> String {
    match row {
        Row::Entry(index) => entries
            .get(*index)
            .map(|e| e.id.clone())
            .unwrap_or_default(),
        Row::TrueColor(rgb) => true_color_id(*rgb),
    }
}

fn row_text(row: &Row, entries: &[StyleEntry], localize: &dyn Localize) -> (String, Preview) {
    match row {
        Row::Entry(index) => entries
            .get(*index)
            .map_or((String::new(), Preview::None), |e| {
                (e.name.clone(), e.preview.clone())
            }),
        Row::TrueColor(rgb) => {
            let id = true_color_id(*rgb);
            let name = resolve(
                localize,
                messages::STYLE_TRUE_COLOR.message_id,
                &[("hex", Value::Text(id.clone()))],
            );
            (name, Preview::Color(color_of(*rgb)))
        }
    }
}

/// Parses a six-digit hexadecimal color, with or without a leading `#`.
#[must_use]
pub fn true_color(text: &str) -> Option<[u8; 3]> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if hex.len() != 6 {
        return None;
    }
    let channel = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
    };
    Some([channel(0)?, channel(2)?, channel(4)?])
}

fn color_of(rgb: [u8; 3]) -> Color32 {
    let [r, g, b] = rgb;
    Color32::from_rgb(r, g, b)
}

/// The identifier of a true color: `#` and six lower-case hexadecimal digits.
#[must_use]
pub fn true_color_id(rgb: [u8; 3]) -> String {
    {
        let [r, g, b] = rgb;
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// The on and off segments of a dash pattern along `[0, length]`, as `(start, end)` pairs.
/// Items alternate dash and gap, starting with a dash, and the pattern repeats. An invalid
/// pattern, or one with no length, gives one solid segment.
#[must_use]
pub fn dash_segments(pattern: &[f32], length: f32) -> Vec<(f32, f32)> {
    if !(length.is_finite() && length > 0.0) {
        return Vec::new();
    }
    let valid =
        pattern.len() <= MAX_DASH_ITEMS && pattern.iter().all(|v| v.is_finite() && *v >= 0.0);
    let total: f32 = if valid { pattern.iter().sum() } else { 0.0 };
    if !valid || !total.is_finite() || total <= 0.0 {
        return vec![(0.0, length)];
    }
    let mut out = Vec::new();
    let mut at = 0.0_f32;
    let mut index = 0usize;
    let budget = MAX_DASH_SEGMENTS.saturating_mul(MAX_DASH_ITEMS + 1);
    while at < length && index < budget && out.len() < MAX_DASH_SEGMENTS {
        let step = pattern.get(index % pattern.len()).copied().unwrap_or(0.0);
        let end = (at + step).min(length);
        if index.is_multiple_of(2) && end > at {
            out.push((at, end));
        }
        at += step;
        index += 1;
    }
    out
}

/// The drawn width of a lineweight in millimeters, in logical points: 1 mm is about 3.8 points at
/// 96 dpi, clamped to 1 to 6 points so that a preview stays legible.
#[must_use]
pub fn stroke_width(millimeters: f32) -> f32 {
    if millimeters.is_finite() {
        (millimeters * 3.78).clamp(1.0, 6.0)
    } else {
        1.0
    }
}

fn draw_preview(ui: &mut Ui, preview: &Preview) {
    let size: Vec2 = vec2(48.0, 14.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let ink = ui.visuals().text_color();
    let painter = ui.painter();
    match preview {
        Preview::None => {}
        Preview::Color(color) => {
            painter.rect_filled(rect, 2.0, *color);
        }
        Preview::Dash(pattern) => {
            let y = rect.center().y;
            for (start, end) in dash_segments(pattern, rect.width()) {
                painter.line_segment(
                    [pos2(rect.left() + start, y), pos2(rect.left() + end, y)],
                    Stroke::new(2.0, ink),
                );
            }
        }
        Preview::Weight(millimeters) => {
            let y = rect.center().y;
            let width = stroke_width(*millimeters);
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(width, ink),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn entry(id: &str, name: &str) -> StyleEntry {
        StyleEntry {
            id: id.into(),
            name: name.into(),
            preview: Preview::None,
        }
    }

    #[test]
    fn filter_matches_name_or_identifier_and_keeps_list_order() {
        let entries = vec![
            entry("l-1", "Walls"),
            entry("l-2", "Dimensions"),
            entry("wall-x", "Other"),
        ];
        assert_eq!(StylePicker::matching(&entries, "", ""), vec![0, 1, 2]);
        assert_eq!(StylePicker::matching(&entries, "WALL", "en-US"), vec![0, 2]);
        assert_eq!(StylePicker::matching(&entries, "l-2", ""), vec![1]);
        assert!(StylePicker::matching(&entries, "zzz", "").is_empty());
    }

    #[test]
    fn true_color_accepts_six_hex_digits_with_or_without_hash() {
        assert_eq!(true_color("#FF8000"), Some([255, 128, 0]));
        assert_eq!(true_color(" 0a0b0c "), Some([10, 11, 12]));
        assert_eq!(true_color("#12345"), None);
        assert_eq!(true_color("#GG0000"), None);
        assert_eq!(true_color("é12345"), None);
        assert_eq!(true_color_id([255, 128, 0]), "#ff8000");
    }

    #[test]
    fn dash_patterns_alternate_and_invalid_patterns_are_solid() {
        assert_eq!(
            dash_segments(&[4.0, 2.0], 10.0),
            vec![(0.0, 4.0), (6.0, 10.0)]
        );
        assert_eq!(dash_segments(&[], 10.0), vec![(0.0, 10.0)]);
        assert_eq!(dash_segments(&[0.0, 0.0], 10.0), vec![(0.0, 10.0)]);
        assert_eq!(dash_segments(&[-1.0, 2.0], 10.0), vec![(0.0, 10.0)]);
        assert_eq!(dash_segments(&[f32::NAN], 10.0), vec![(0.0, 10.0)]);
        assert!(dash_segments(&[1.0], 0.0).is_empty());
        assert!(dash_segments(&[1e-6], 1e9).len() <= MAX_DASH_SEGMENTS);
    }

    #[test]
    fn lineweight_preview_width_is_bounded() {
        assert_eq!(stroke_width(0.0), 1.0);
        assert_eq!(stroke_width(100.0), 6.0);
        assert_eq!(stroke_width(f32::NAN), 1.0);
    }

    #[test]
    fn too_many_entries_are_refused() {
        let many: Vec<StyleEntry> = (0..=MAX_ENTRIES)
            .map(|i| entry(&i.to_string(), "n"))
            .collect();
        assert!(StylePicker::check(&many).is_err());
        assert!(StylePicker::check(&many[..MAX_ENTRIES]).is_ok());
    }

    proptest! {
        #[test]
        fn true_color_round_trips(rgb in any::<[u8; 3]>()) {
            prop_assert_eq!(true_color(&true_color_id(rgb)), Some(rgb));
        }

        #[test]
        fn dash_segments_are_ordered_and_inside_the_length(
            pattern in proptest::collection::vec(0.0f32..20.0, 1..8),
            length in 0.0f32..500.0,
        ) {
            let segments = dash_segments(&pattern, length);
            prop_assert!(segments.len() <= MAX_DASH_SEGMENTS);
            let mut last = 0.0f32;
            for (start, end) in segments {
                prop_assert!(start >= last && end >= start && end <= length + 1e-3);
                last = end;
            }
        }
    }
}
