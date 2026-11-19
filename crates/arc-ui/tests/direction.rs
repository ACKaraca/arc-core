//! Right-to-left layout (CORE-UI-025), without a window: the dock's first part of a horizontal
//! split is at the right, the tabs of a group run from the right, the command line's label and
//! keyword buttons start at the right, and a left-to-right layout is unchanged. Positions come
//! from the widget bounds of a headless context.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::error::Value;
use arc_command::{
    CommandLine, CommandSpec, Completion, Permission, Registry, Session, Surface,
    Target as LineTarget,
};
use arc_doc::graph::Evaluators;
use arc_doc::{Document, TypeRegistry};
use arc_ui::dock::{Axis, Dock};
use arc_ui::egui::{self, accesskit};
use arc_ui::text::Direction;
use arc_ui::{CommandLineView, DockArea, DockLayout, LineCommands, LineContext, Localize};

struct Silent;

impl Localize for Silent {
    fn message(&self, _id: &str, _args: &[(&str, Value)]) -> Option<String> {
        None
    }
}

fn tabs(name: &str) -> Dock {
    Dock::Tabs {
        tabs: vec![name.to_owned()],
        active: 0,
    }
}

/// The left edge of the content of `first` and `second` in a horizontal split, drawn in `ui`.
fn panel_lefts(direction: Direction) -> (f32, f32) {
    let root = Dock::Split {
        axis: Axis::Horizontal,
        fraction: 0.3,
        first: Box::new(tabs("first")),
        second: Box::new(tabs("second")),
    };
    let mut layout = DockLayout::new(root, vec![]).unwrap();
    let ctx = egui::Context::default();
    let mut lefts = Vec::new();
    let area = DockArea {
        direction,
        ..Default::default()
    };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 400.0),
        )),
        ..Default::default()
    };
    let mut full = ctx.run_ui(input, |ui| {
        let mut content = |ui: &mut egui::Ui, name: &str| {
            lefts.push((name.to_owned(), ui.max_rect().left()));
        };
        area.show(
            ui,
            &mut layout,
            &Silent,
            &|name: &str| name.to_owned(),
            &mut content,
        );
    });
    full.textures_delta.clear();
    let find = |name: &str| {
        lefts
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, x)| *x)
            .expect("the panel is drawn")
    };
    (find("first"), find("second"))
}

#[test]
fn right_to_left_puts_the_first_part_of_a_horizontal_split_at_the_right() {
    let (first, second) = panel_lefts(Direction::RightToLeft);
    assert!(
        first > second,
        "the first part is at the right: {first} vs {second}"
    );
}

#[test]
fn left_to_right_keeps_the_first_part_at_the_left() {
    let (first, second) = panel_lefts(Direction::LeftToRight);
    assert!(
        first < second,
        "the first part is at the left: {first} vs {second}"
    );
}

/// The name (label, or text value) and bounds of each node with `role`.
fn labelled_bounds(
    full: &mut egui::FullOutput,
    role: accesskit::Role,
) -> Vec<(String, accesskit::Rect)> {
    full.textures_delta.clear();
    let Some(update) = full.platform_output.accesskit_update.as_ref() else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter(|(_, n)| n.role() == role)
        .filter_map(|(_, n)| Some((n.label().or(n.value())?.to_owned(), n.bounds()?)))
        .collect()
}

#[test]
fn right_to_left_orders_the_tabs_of_a_group_from_the_right() {
    let names = ["alpha", "beta", "gamma"];
    let x_of = |direction: Direction| {
        let mut layout = DockLayout::new(
            Dock::Tabs {
                tabs: names.iter().map(|n| (*n).to_owned()).collect(),
                active: 0,
            },
            vec![],
        )
        .unwrap();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let area = DockArea {
            direction,
            ..Default::default()
        };
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 400.0),
            )),
            ..Default::default()
        };
        let mut content = |_: &mut egui::Ui, _: &str| {};
        let mut full = ctx.run_ui(input, |ui| {
            area.show(
                ui,
                &mut layout,
                &Silent,
                &|name: &str| name.to_owned(),
                &mut content,
            );
        });
        let tabs = labelled_bounds(&mut full, accesskit::Role::Button);
        names.map(|name| {
            let (_, bounds) = tabs
                .iter()
                .find(|(label, _)| label == name)
                .expect("the tab is drawn");
            (bounds.x0 + bounds.x1) / 2.0
        })
    };

    let rtl = x_of(Direction::RightToLeft);
    assert!(
        rtl[0] > rtl[1] && rtl[1] > rtl[2],
        "the first tab is the rightmost: {rtl:?}"
    );
    let ltr = x_of(Direction::LeftToRight);
    assert!(
        ltr[0] < ltr[1] && ltr[1] < ltr[2],
        "the first tab is the leftmost: {ltr:?}"
    );
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Fast,
    Slow,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Pick {
    mode: Mode,
}

/// The command line with an open prompt that offers the two keyword options of `Mode`.
fn line_with_keywords() -> CommandLine {
    let mut registry = Registry::new();
    registry
        .register(
            CommandSpec::new("demo.draw.pick", "demo-pick", "demo-pick", |_, p: Pick| {
                Ok(if matches!(p.mode, Mode::Fast) {
                    1.0_f64
                } else {
                    0.0
                })
            })
            .with_permissions([Permission::Read])
            .with_undo(arc_command::UndoBehavior::NotUndoable),
        )
        .unwrap();
    let document =
        Document::new(Arc::new(TypeRegistry::new()), Arc::new(Evaluators::new())).unwrap();
    let session = Session::new(Surface::CommandLine, [Permission::Read]);
    let localized = BTreeMap::new();
    let target = LineTarget {
        registry: &registry,
        document: &document,
        session: &session,
        selection: &[],
        localized: &localized,
        seed: 1,
    };
    let mut line = CommandLine::new();
    line.submit(&target, "demo.draw.pick").unwrap();
    assert!(
        line.active().and_then(|s| s.prompt()).is_some(),
        "a prompt is open"
    );
    line
}

/// The bounds of the command line's label and of its keyword buttons, drawn in `direction`.
fn command_line_bounds(direction: Direction) -> (accesskit::Rect, Vec<accesskit::Rect>) {
    let line = line_with_keywords();
    let commands = LineCommands {
        undo: "core.edit.undo".into(),
        redo: "core.edit.redo".into(),
        clear_selection: "core.select.clear".into(),
    };
    let mut view = CommandLineView::default();
    view.direction = Some(direction);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let completion = Completion::default();
    let context = LineContext {
        localize: &Silent,
        has_selection: false,
        commands: &commands,
    };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 400.0),
        )),
        ..Default::default()
    };
    let mut full = ctx.run_ui(input, |ui| {
        view.show(ui, &line, &completion, &context);
    });
    let buttons: Vec<accesskit::Rect> = labelled_bounds(&mut full, accesskit::Role::Button)
        .into_iter()
        .map(|(_, b)| b)
        .collect();
    let label = labelled_bounds(&mut full, accesskit::Role::Label)
        .into_iter()
        .map(|(_, b)| b)
        .next()
        .expect("the label is drawn");
    (label, buttons)
}

#[test]
fn right_to_left_starts_the_command_line_label_and_keyword_buttons_at_the_right() {
    let (label, buttons) = command_line_bounds(Direction::RightToLeft);
    assert_eq!(buttons.len(), 2, "both keyword options are drawn");
    let right_most = buttons.iter().map(|b| b.x1).fold(f64::MIN, f64::max);
    let left_most = buttons.iter().map(|b| b.x0).fold(f64::MAX, f64::min);
    assert!(
        right_most > 700.0,
        "the first keyword is at the right edge: {right_most}"
    );
    assert!(
        left_most > 0.0 && left_most < right_most,
        "the second keyword is to its left"
    );
    assert!(
        label.x1 > 700.0,
        "the label starts at the right: {:?}",
        label
    );
}

#[test]
fn left_to_right_starts_the_command_line_label_and_keyword_buttons_at_the_left() {
    let (label, buttons) = command_line_bounds(Direction::LeftToRight);
    assert_eq!(buttons.len(), 2);
    let left_most = buttons.iter().map(|b| b.x0).fold(f64::MAX, f64::min);
    let right_most = buttons.iter().map(|b| b.x1).fold(f64::MIN, f64::max);
    assert!(
        left_most < 100.0,
        "the first keyword is at the left edge: {left_most}"
    );
    assert!(right_most < 800.0 && right_most > left_most);
    assert!(
        label.x0 < 100.0,
        "the label starts at the left: {:?}",
        label
    );
}
