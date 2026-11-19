//! The style pickers through a headless egui context (CORE-UI-015): a click opens the drop-down,
//! typing filters the entries, the arrow keys move the highlight, Enter returns the chosen
//! identifier, Escape closes without a choice, and the color picker offers a true color.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use arc_base::error::Value;
use arc_ui::egui::{self, accesskit};
use arc_ui::{Localize, Preview, StyleEntry, StyleKind, StylePicker};

struct English;
impl Localize for English {
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        let hex = args
            .iter()
            .find_map(|(k, v)| match (k, v) {
                (&"hex", Value::Text(t)) => Some(t.clone()),
                _ => None,
            })
            .unwrap_or_default();
        Some(match id {
            "ui-style-color" => "Color".to_owned(),
            "ui-style-layer" => "Layer".to_owned(),
            "ui-style-linetype" => "Linetype".to_owned(),
            "ui-style-lineweight" => "Lineweight".to_owned(),
            "ui-style-material" => "Material".to_owned(),
            "ui-style-filter" => "Filter".to_owned(),
            "ui-style-true-color" => format!("True color {hex}"),
            _ => return None,
        })
    }
}

fn entries() -> Vec<StyleEntry> {
    [("1", "Red"), ("2", "Green"), ("3", "Blue")]
        .into_iter()
        .map(|(id, name)| StyleEntry {
            id: id.into(),
            name: name.into(),
            preview: Preview::Color(egui::Color32::GRAY),
        })
        .collect()
}

fn pass(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    picker: &mut StylePicker,
    entries: &[StyleEntry],
) -> (Vec<accesskit::Node>, Option<String>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| {
        result = picker.show(ui, entries, Some("1"), &English);
    });
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    (nodes, result)
}

fn has_label(nodes: &[accesskit::Node], text: &str) -> bool {
    nodes
        .iter()
        .any(|n| n.label().is_some_and(|l| l.contains(text)))
}

fn center_of(nodes: &[accesskit::Node], text: &str) -> egui::Pos2 {
    let node = nodes
        .iter()
        .find(|n| n.label().is_some_and(|l| l.contains(text)))
        .expect("node");
    let b = node.bounds().unwrap();
    egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

fn click(at: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx
}

/// Opens the picker and types `text` into its filter, one frame each.
fn open_and_type(
    ctx: &egui::Context,
    picker: &mut StylePicker,
    list: &[StyleEntry],
    button: &str,
    text: &str,
) -> Vec<accesskit::Node> {
    let (nodes, _) = pass(ctx, vec![], picker, list);
    let _ = pass(ctx, click(center_of(&nodes, button)), picker, list);
    assert!(picker.is_open());
    let (nodes, _) = pass(ctx, vec![egui::Event::Text(text.into())], picker, list);
    nodes
}

#[test]
fn typing_filters_and_enter_returns_the_chosen_identifier() {
    let ctx = context();
    let list = entries();
    let mut picker = StylePicker::new(StyleKind::Color);
    let nodes = open_and_type(&ctx, &mut picker, &list, "Color: Red", "gre");
    assert!(has_label(&nodes, "Green"));
    assert!(!has_label(&nodes, "Blue"));
    let (_, chosen) = pass(&ctx, vec![press(egui::Key::Enter)], &mut picker, &list);
    assert_eq!(chosen.as_deref(), Some("2"));
    assert!(!picker.is_open());
}

#[test]
fn arrow_keys_move_the_highlight_before_enter() {
    let ctx = context();
    let list = entries();
    let mut picker = StylePicker::new(StyleKind::Layer);
    let (nodes, _) = pass(&ctx, vec![], &mut picker, &list);
    let _ = pass(
        &ctx,
        click(center_of(&nodes, "Layer: Red")),
        &mut picker,
        &list,
    );
    let (_, chosen) = pass(
        &ctx,
        vec![
            press(egui::Key::ArrowDown),
            press(egui::Key::ArrowDown),
            press(egui::Key::Enter),
        ],
        &mut picker,
        &list,
    );
    assert_eq!(chosen.as_deref(), Some("3"));
}

#[test]
fn escape_closes_without_a_choice() {
    let ctx = context();
    let list = entries();
    let mut picker = StylePicker::new(StyleKind::Linetype);
    let (nodes, _) = pass(&ctx, vec![], &mut picker, &list);
    let _ = pass(
        &ctx,
        click(center_of(&nodes, "Linetype: Red")),
        &mut picker,
        &list,
    );
    assert!(picker.is_open());
    let (_, chosen) = pass(&ctx, vec![press(egui::Key::Escape)], &mut picker, &list);
    assert_eq!(chosen, None);
    assert!(!picker.is_open());
}

#[test]
fn color_picker_offers_a_true_color_from_typed_hex_and_other_kinds_do_not() {
    let ctx = context();
    let list = entries();
    let mut color = StylePicker::new(StyleKind::Color);
    let nodes = open_and_type(&ctx, &mut color, &list, "Color: Red", "#FF8000");
    assert!(has_label(&nodes, "True color #ff8000"));
    let (_, chosen) = pass(&ctx, vec![press(egui::Key::Enter)], &mut color, &list);
    assert_eq!(chosen.as_deref(), Some("#ff8000"));

    let mut layer = StylePicker::new(StyleKind::Layer);
    let nodes = open_and_type(&ctx, &mut layer, &list, "Layer: Red", "#FF8000");
    assert!(!has_label(&nodes, "True color"));
    let (_, chosen) = pass(&ctx, vec![press(egui::Key::Enter)], &mut layer, &list);
    assert_eq!(chosen, None);
}
