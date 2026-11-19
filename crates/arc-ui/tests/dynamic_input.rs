//! Dynamic input near the cursor (CORE-UI-009): the panel stays inside the viewport at every
//! edge, Tab moves between the distance and angle fields, typing locks a field, Enter emits the
//! locked typed values and the unlocked pointer-derived values, and unlocked fields follow the
//! values the caller supplies each frame.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use arc_ui::dynamic_input::{Relative, place};
use arc_ui::egui::{self, accesskit};
use arc_ui::{DynamicInput, Localize};

struct Names;

impl Localize for Names {
    fn message(&self, id: &str, _args: &[(&str, arc_base::error::Value)]) -> Option<String> {
        Some(
            match id {
                "cmd-point" => "Specify next point",
                "ui-dynamic-distance" => "Distance",
                "ui-dynamic-angle" => "Angle",
                "ui-dynamic-input" => "Dynamic input",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

fn viewport() -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))
}

struct Pass {
    nodes: Vec<accesskit::Node>,
}

fn run(
    ctx: &egui::Context,
    input: &mut DynamicInput,
    pointer: egui::Pos2,
    derived: Option<Relative>,
    events: Vec<egui::Event>,
) -> (Pass, Option<Relative>) {
    let raw = egui::RawInput {
        screen_rect: Some(viewport()),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(raw, |ui| {
        result = input.show(ui.ctx(), &Names, "cmd-point", viewport(), pointer, derived);
    });
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    (Pass { nodes }, result)
}

fn new_context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    ctx
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

fn select_all() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::A,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    }
}

fn inside(rect: accesskit::Rect, outer: egui::Rect) -> bool {
    rect.x0 >= f64::from(outer.left())
        && rect.y0 >= f64::from(outer.top())
        && rect.x1 <= f64::from(outer.right())
        && rect.y1 <= f64::from(outer.bottom())
}

#[test]
fn the_panel_stays_inside_the_viewport_at_every_edge() {
    let outer = viewport();
    let size = egui::vec2(220.0, 96.0);
    let corners = [
        egui::pos2(790.0, 590.0),
        egui::pos2(790.0, 300.0),
        egui::pos2(5.0, 5.0),
        egui::pos2(5.0, 590.0),
        egui::pos2(400.0, 5.0),
        egui::pos2(400.0, 590.0),
    ];
    for pointer in corners {
        let placed = place(pointer, size, outer);
        assert!(
            placed.x >= outer.left() && placed.x + size.x <= outer.right(),
            "{pointer:?}"
        );
        assert!(
            placed.y >= outer.top() && placed.y + size.y <= outer.bottom(),
            "{pointer:?}"
        );
    }
    // Near the right edge the panel flips to the left of the pointer, and near the bottom
    // above it.
    let right = place(egui::pos2(790.0, 300.0), size, outer);
    assert!(right.x + size.x <= 790.0);
    let bottom = place(egui::pos2(400.0, 590.0), size, outer);
    assert!(bottom.y + size.y <= 590.0);
}

#[test]
fn the_drawn_panel_stays_inside_the_viewport_near_each_edge() {
    let ctx = new_context();
    let outer = viewport();
    for pointer in [
        egui::pos2(790.0, 590.0),
        egui::pos2(5.0, 5.0),
        egui::pos2(790.0, 300.0),
    ] {
        let mut input = DynamicInput::default();
        // The first frame learns the panel's size; the second places it with that size.
        let _ = run(&ctx, &mut input, pointer, None, vec![]);
        let (pass, _) = run(&ctx, &mut input, pointer, None, vec![]);
        let group = pass
            .nodes
            .iter()
            .find(|n| n.label() == Some("Dynamic input"))
            .expect("the panel is drawn");
        assert!(inside(group.bounds().unwrap(), outer), "{pointer:?}");
    }
}

#[test]
fn tab_moves_between_the_fields_and_typing_locks_only_the_focused_one() {
    let ctx = new_context();
    let mut input = DynamicInput::default();
    let derived = Relative {
        distance: 2.5,
        angle: 0.25,
    };
    let pointer = egui::pos2(100.0, 100.0);
    let _ = run(&ctx, &mut input, pointer, Some(derived), vec![]);
    assert_eq!(
        input.focused_field(),
        0,
        "the distance field starts focused"
    );
    let _ = run(
        &ctx,
        &mut input,
        pointer,
        Some(derived),
        vec![key(egui::Key::Tab)],
    );
    assert_eq!(input.focused_field(), 1, "Tab moves to the angle field");
    let _ = run(&ctx, &mut input, pointer, Some(derived), vec![select_all()]);
    let _ = run(
        &ctx,
        &mut input,
        pointer,
        Some(derived),
        vec![egui::Event::Text("45".into())],
    );
    assert!(input.is_locked(1), "typing locks the angle field");
    assert!(!input.is_locked(0), "the distance field stays unlocked");
}

#[test]
fn enter_emits_the_locked_typed_values_and_the_unlocked_derived_values() {
    let ctx = new_context();
    let mut input = DynamicInput::default();
    let derived = Relative {
        distance: 2.5,
        angle: 0.25,
    };
    let pointer = egui::pos2(100.0, 100.0);
    let _ = run(&ctx, &mut input, pointer, Some(derived), vec![]);
    let _ = run(&ctx, &mut input, pointer, Some(derived), vec![select_all()]);
    let _ = run(
        &ctx,
        &mut input,
        pointer,
        Some(derived),
        vec![egui::Event::Text("5".into())],
    );
    let (_, result) = run(
        &ctx,
        &mut input,
        pointer,
        Some(derived),
        vec![key(egui::Key::Enter)],
    );
    let result = result.expect("Enter emits the point input");
    assert_eq!(result.distance, 5.0, "the typed distance is locked");
    assert_eq!(
        result.angle, derived.angle,
        "the unlocked angle is the derived one, exactly"
    );
}

#[test]
fn unlocked_fields_follow_the_values_the_caller_supplies() {
    let ctx = new_context();
    let mut input = DynamicInput::default();
    let pointer = egui::pos2(100.0, 100.0);
    let first = Relative {
        distance: 2.5,
        angle: 0.25,
    };
    let second = Relative {
        distance: 3.0,
        angle: 1.0,
    };
    let _ = run(&ctx, &mut input, pointer, Some(first), vec![]);
    let _ = run(&ctx, &mut input, pointer, Some(second), vec![]);
    let (_, result) = run(
        &ctx,
        &mut input,
        pointer,
        Some(second),
        vec![key(egui::Key::Enter)],
    );
    assert_eq!(
        result,
        Some(second),
        "nothing is locked, so the latest values are emitted"
    );
    let (_, result) = run(&ctx, &mut input, pointer, None, vec![key(egui::Key::Enter)]);
    assert!(
        result.is_none(),
        "without a pointer value an unlocked field gives nothing"
    );
}
