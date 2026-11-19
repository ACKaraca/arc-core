//! The tool option bar (CORE-UI-008) through a headless egui context: a keyword button returns
//! its keyword, an edited parameter returns its typed value on Enter, both are reachable by the
//! keyboard, the bar is ordered from the logical start, and an empty prompt draws nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use arc_ui::egui::{self, accesskit};
use arc_ui::text::Direction;
use arc_ui::{Localize, OptionBar, OptionControl, ParameterControl, PromptControls};

struct Names;

impl Localize for Names {
    fn message(&self, id: &str, _args: &[(&str, arc_base::error::Value)]) -> Option<String> {
        Some(
            match id {
                "cmd-line" => "Line",
                "cmd-arc" => "Arc",
                "cmd-radius" => "Radius",
                "ui-option-bar" => "Tool options",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

fn controls() -> PromptControls {
    PromptControls {
        options: vec![
            OptionControl {
                id: "line".into(),
                label: "cmd-line".into(),
                shortcut: Some('l'),
            },
            OptionControl {
                id: "arc".into(),
                label: "cmd-arc".into(),
                shortcut: None,
            },
        ],
        parameters: vec![ParameterControl {
            name: "radius".into(),
            label: "cmd-radius".into(),
            value: "10".into(),
            editable: true,
        }],
    }
}

struct Pass {
    nodes: Vec<accesskit::Node>,
}

fn run(
    ctx: &egui::Context,
    bar: &mut OptionBar,
    controls: &PromptControls,
    events: Vec<egui::Event>,
) -> (Pass, Option<String>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| result = bar.show(ui, controls, &Names));
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    (Pass { nodes }, result)
}

fn node(pass: &Pass, role: accesskit::Role, label: &str) -> Option<accesskit::Node> {
    pass.nodes
        .iter()
        .find(|n| n.role() == role && n.label() == Some(label))
        .cloned()
}

fn center(node: &accesskit::Node) -> egui::Pos2 {
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

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx
}

#[test]
fn a_keyword_button_returns_its_keyword() {
    let ctx = new_context();
    let controls = controls();
    let mut bar = OptionBar::default();
    let (first, _) = run(&ctx, &mut bar, &controls, vec![]);
    let button = node(&first, accesskit::Role::Button, "Line (l)").expect("keyword button");
    let (_, result) = run(&ctx, &mut bar, &controls, click(center(&button)));
    assert_eq!(result.as_deref(), Some("line"));
}

#[test]
fn an_edited_parameter_returns_its_value_on_enter() {
    let ctx = new_context();
    let controls = controls();
    let mut bar = OptionBar::default();
    let (first, _) = run(&ctx, &mut bar, &controls, vec![]);
    let field = first
        .nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::TextInput)
        .cloned()
        .expect("the parameter is an editable field");
    let b = field.bounds().unwrap();
    let at = egui::pos2((b.x1 - 3.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    let _ = run(&ctx, &mut bar, &controls, click(at));
    let backspaces: Vec<egui::Event> = (0..5).map(|_| key(egui::Key::Backspace)).collect();
    let _ = run(&ctx, &mut bar, &controls, backspaces);
    let _ = run(
        &ctx,
        &mut bar,
        &controls,
        vec![egui::Event::Text("25".into())],
    );
    let (_, result) = run(&ctx, &mut bar, &controls, vec![key(egui::Key::Enter)]);
    assert_eq!(result.as_deref(), Some("25"));
}

#[test]
fn the_keyboard_reaches_each_control_and_operates_it() {
    let ctx = new_context();
    let controls = controls();
    let mut bar = OptionBar::default();
    let _ = run(&ctx, &mut bar, &controls, vec![]);
    let _ = run(&ctx, &mut bar, &controls, vec![key_tab()]);
    let (_, result) = run(&ctx, &mut bar, &controls, vec![key(egui::Key::Enter)]);
    assert_eq!(
        result.as_deref(),
        Some("line"),
        "Tab then Enter activates the first button"
    );

    // Two more Tabs reach the parameter field; typing and Enter return its value.
    let _ = run(&ctx, &mut bar, &controls, vec![key_tab()]);
    let _ = run(&ctx, &mut bar, &controls, vec![key_tab()]);
    let backspaces: Vec<egui::Event> = (0..5).map(|_| key(egui::Key::Backspace)).collect();
    let _ = run(&ctx, &mut bar, &controls, backspaces);
    let _ = run(
        &ctx,
        &mut bar,
        &controls,
        vec![egui::Event::Text("7".into())],
    );
    let (_, result) = run(&ctx, &mut bar, &controls, vec![key(egui::Key::Enter)]);
    assert_eq!(result.as_deref(), Some("7"));
}

fn key_tab() -> egui::Event {
    key(egui::Key::Tab)
}

#[test]
fn right_to_left_puts_the_first_control_at_the_right_edge() {
    let ctx = new_context();
    let controls = controls();
    let mut bar = OptionBar::default();
    bar.direction = Some(Direction::RightToLeft);
    let (pass, _) = run(&ctx, &mut bar, &controls, vec![]);
    let line = node(&pass, accesskit::Role::Button, "Line (l)")
        .unwrap()
        .bounds()
        .unwrap();
    let arc = node(&pass, accesskit::Role::Button, "Arc")
        .unwrap()
        .bounds()
        .unwrap();
    let bar_node = node(&pass, accesskit::Role::Toolbar, "Tool options")
        .expect("the bar is a named toolbar")
        .bounds()
        .unwrap();
    assert!(line.x0 > arc.x0, "the first control is to the right");
    assert!((line.x1 - bar_node.x1).abs() < 1e-3, "at the right edge");

    let mut bar = OptionBar::default();
    let (pass, _) = run(&ctx, &mut bar, &controls, vec![]);
    let line = node(&pass, accesskit::Role::Button, "Line (l)")
        .unwrap()
        .bounds()
        .unwrap();
    let arc = node(&pass, accesskit::Role::Button, "Arc")
        .unwrap()
        .bounds()
        .unwrap();
    let bar_node = node(&pass, accesskit::Role::Toolbar, "Tool options")
        .unwrap()
        .bounds()
        .unwrap();
    assert!(
        line.x0 < arc.x0,
        "left to right keeps the first control left"
    );
    assert!((line.x0 - bar_node.x0).abs() < 1e-3);
}

#[test]
fn an_empty_prompt_draws_nothing() {
    let ctx = new_context();
    let mut bar = OptionBar::default();
    let (pass, result) = run(&ctx, &mut bar, &PromptControls::default(), vec![]);
    assert!(result.is_none());
    assert!(node(&pass, accesskit::Role::Toolbar, "Tool options").is_none());
    assert!(node(&pass, accesskit::Role::Button, "Line (l)").is_none());
}
