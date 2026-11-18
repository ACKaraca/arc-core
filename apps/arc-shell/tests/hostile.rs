//! Panic hunt over the shell composition: hostile egui event streams, command lines and
//! intents never make a frame panic, and the document stays readable (CORE-APP-025).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests"
)]

mod common;

use std::sync::OnceLock;

use arc_app::commands;
use arc_command::LineStep;
use arc_gpu::{Gpu, GpuSettings};
use arc_ui::Intent;
use arc_ui::egui::{self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Vec2};
use common::{Harness, TempDir};
use proptest::prelude::*;

fn gpu() -> Gpu {
    static GPU: OnceLock<Gpu> = OnceLock::new();
    GPU.get_or_init(|| Gpu::new(&GpuSettings::default()).unwrap())
        .clone()
}

fn coordinate() -> impl Strategy<Value = f32> {
    prop_oneof![
        4 => 0.0f32..1200.0,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(-1e30f32),
        1 => any::<f32>(),
    ]
}

fn modifiers() -> impl Strategy<Value = Modifiers> {
    (any::<bool>(), any::<bool>(), any::<bool>()).prop_map(|(shift, ctrl, alt)| Modifiers {
        shift,
        ctrl,
        command: ctrl,
        alt,
        mac_cmd: false,
    })
}

fn key() -> impl Strategy<Value = Key> {
    prop::sample::select(vec![
        Key::Enter,
        Key::Escape,
        Key::Tab,
        Key::Backspace,
        Key::Delete,
        Key::ArrowUp,
        Key::ArrowDown,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::Home,
        Key::Plus,
        Key::Minus,
        Key::A,
        Key::Z,
        Key::Y,
        Key::S,
        Key::N,
        Key::W,
        Key::F2,
        Key::Space,
    ])
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec![
            "shell.point.create position=[0, 0, 0]",
            "shell.point.create position=[1e308, -1e308, 0]",
            "shell.line.create start=[0, 0, 0] end=[1e-300, 0, 0]",
            "shell.layer.create name=",
            "shell.object.set_field",
            "shell.object.delete",
            "app.select.set objects=[]",
            "app.edit.undo",
            "app.file.save_as",
            "1200 mm + 3 in",
            "[",
            "\"",
            "=",
        ])
        .prop_map(str::to_owned),
        ".{0,40}",
    ]
}

fn event() -> impl Strategy<Value = Event> {
    let pos = || (coordinate(), coordinate()).prop_map(|(x, y)| Pos2::new(x, y));
    let button = prop::sample::select(vec![
        PointerButton::Primary,
        PointerButton::Secondary,
        PointerButton::Middle,
    ]);
    prop_oneof![
        3 => pos().prop_map(Event::PointerMoved),
        3 => (pos(), button, any::<bool>(), modifiers()).prop_map(
            |(pos, button, pressed, modifiers)| Event::PointerButton {
                pos,
                button,
                pressed,
                modifiers,
            }
        ),
        2 => (key(), any::<bool>(), modifiers()).prop_map(|(key, pressed, modifiers)| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }),
        2 => text().prop_map(Event::Text),
        1 => (coordinate(), coordinate(), modifiers()).prop_map(|(x, y, modifiers)| {
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(x, y),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers,
            }
        }),
        1 => coordinate().prop_map(Event::Zoom),
        1 => coordinate().prop_map(Event::Rotate),
        1 => Just(Event::PointerGone),
    ]
}

fn intent() -> impl Strategy<Value = Intent> {
    prop::sample::select(vec![
        commands::FILE_NEW,
        commands::FILE_SAVE,
        commands::FILE_CLOSE,
        commands::EDIT_UNDO,
        commands::EDIT_REDO,
        commands::SELECT_CLEAR,
        commands::SELECT_SET,
        "shell.object.delete",
        "shell.unknown.command",
    ])
    .prop_map(Intent::new)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn hostile_frames_and_command_lines_never_panic(
        frames in prop::collection::vec(prop::collection::vec(event(), 0..6), 1..12),
        lines in prop::collection::vec(text(), 0..6),
        intents in prop::collection::vec(intent(), 0..4),
    ) {
        let dir = TempDir::new("hostile");
        let mut h = Harness::new(&dir.join("data"), Some(gpu()));
        h.menu(commands::FILE_NEW);
        h.command("shell.point.create position=[0, 0, 0]");
        for line in &lines {
            let mut requests = Vec::new();
            // Errors are expected; panics are not.
            let _ = h.shell.line_step(&mut h.app, LineStep::Submit(line), &mut requests);
        }
        for (n, events) in frames.into_iter().enumerate() {
            let menu = intents.get(n).cloned().into_iter().collect();
            h.frame_with(events, menu);
        }
        // Every open document still has a readable snapshot.
        for tab in h.app.tabs() {
            let _ = tab.document().snapshot().len();
        }
    }
}
