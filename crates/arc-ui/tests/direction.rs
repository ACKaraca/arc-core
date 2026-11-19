//! Right-to-left layout (CORE-UI-025): the dock's first part of a horizontal split is at the
//! right, the tabs of a group run from the right, and a left-to-right layout is unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use arc_base::error::Value;
use arc_ui::dock::{Axis, Dock};
use arc_ui::egui;
use arc_ui::text::Direction;
use arc_ui::{DockArea, DockLayout, Localize};

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
