//! The AccessKit tree egui produces for the menu, toolbar and palette widgets, driven through a
//! headless context (CORE-UI-026, CORE-UI-027 in part), and intents from simulated input.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use arc_base::error::Value;
use arc_command::Registry;
use arc_command::spec::{CommandSpec, Disabled, Permission};
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    CommandState, IconAtlas, Intent, Keymap, Localize, Lookup, Menu, Palette, Recent, Toolbar,
    ToolbarItem,
};

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

struct English;
impl Localize for English {
    fn message(&self, id: &str, _: &[(&str, Value)]) -> Option<String> {
        Some(
            match id {
                "demo-save" => "Save",
                "demo-undo" => "Undo",
                "demo-file" => "File",
                "demo-nothing" => "Nothing to undo",
                "ui-palette-search" => "Search commands",
                "ui-palette-no-results" => "No matching command",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

struct UndoDisabled;
impl CommandState for UndoDisabled {
    fn enabled(&self, spec: &CommandSpec) -> Result<(), Disabled> {
        if spec.id() == "demo.edit.undo" {
            Err(Disabled::new("demo-nothing"))
        } else {
            Ok(())
        }
    }
}

fn registry() -> Registry {
    let mut r = Registry::new();
    for (id, label) in [
        ("demo.file.save", "demo-save"),
        ("demo.edit.undo", "demo-undo"),
    ] {
        r.register(
            CommandSpec::new(id, label, "demo-save", |_, _: NoParams| Ok(()))
                .with_permissions([Permission::Read]),
        )
        .unwrap();
    }
    r
}

/// Runs one pass and returns the AccessKit nodes and the closure's result.
fn pass<R>(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut ui_fn: impl FnMut(&mut egui::Ui) -> R,
) -> (Vec<accesskit::Node>, Option<R>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| result = Some(ui_fn(ui)));
    full.textures_delta.clear();
    let nodes = full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    (nodes, result)
}

fn named<'a>(
    nodes: &'a [accesskit::Node],
    role: accesskit::Role,
    name: &str,
) -> Option<&'a accesskit::Node> {
    nodes.iter().find(|n| {
        n.role() == role
            && [n.label(), n.value()]
                .into_iter()
                .flatten()
                .any(|l| l.contains(name))
    })
}

fn click_events(at: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn center(node: &accesskit::Node) -> egui::Pos2 {
    let b = node.bounds().unwrap();
    egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
}

#[test]
fn menu_bar_exposes_names_roles_and_disabled_state_and_emits_intents() {
    let registry = registry();
    let (keymap, _) = Keymap::build(
        &[arc_ui::Binding {
            keys: "Ctrl+S".into(),
            command: "demo.file.save".into(),
        }],
        &registry,
    );
    let lookup = Lookup {
        registry: &registry,
        localize: &English,
        state: &UndoDisabled,
        keymap: Some(&keymap),
    };
    let menu: Menu = serde_json::from_str(
        r#"{"items":[{"kind":"submenu","label":"demo-file","items":[
            {"kind":"command","command":"demo.file.save"},
            {"kind":"command","command":"demo.edit.undo"}]}]}"#,
    )
    .unwrap();
    let (entries, problems) = lookup.menu(&menu);
    assert!(problems.is_empty());

    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let draw = |ui: &mut egui::Ui| arc_ui::menu::show_menu_bar(ui, &entries);
    let (nodes, _) = pass(&ctx, vec![], draw);
    let file = named(&nodes, accesskit::Role::Button, "File").expect("menu button");
    // Open the menu, then inspect its items.
    let (_, _) = pass(&ctx, click_events(center(file)), draw);
    let (nodes, _) = pass(&ctx, vec![], draw);
    let save = named(&nodes, accesskit::Role::Button, "Save").expect("save item");
    assert!(
        save.label().unwrap().contains("Ctrl+S"),
        "binding text is part of the name"
    );
    assert!(!save.is_disabled());
    let undo = named(&nodes, accesskit::Role::Button, "Undo").expect("undo item");
    assert!(undo.is_disabled());
    for node in nodes.iter().filter(|n| n.role() == accesskit::Role::Button) {
        assert!(
            node.label().is_some_and(|l| !l.is_empty()),
            "every button is named"
        );
    }
    let (_, intent) = pass(&ctx, click_events(center(save)), draw);
    assert_eq!(
        intent.flatten(),
        Some(Intent {
            command: "demo.file.save".into()
        })
    );
}

#[test]
fn icon_toolbar_buttons_are_named_by_their_label() {
    let registry = registry();
    let lookup = Lookup {
        registry: &registry,
        localize: &English,
        state: &UndoDisabled,
        keymap: None,
    };
    let toolbar = Toolbar::new(vec![
        ToolbarItem::Command {
            command: "demo.file.save".into(),
            icon: Some("check".into()),
        },
        ToolbarItem::Separator,
        ToolbarItem::Command {
            command: "demo.edit.undo".into(),
            icon: None,
        },
    ])
    .unwrap();
    let (entries, _) = lookup.toolbar(&toolbar);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let icons = IconAtlas::build(arc_ui::icons::STARTER, 16)
        .unwrap()
        .upload(&ctx);
    let draw = |ui: &mut egui::Ui| arc_ui::menu::show_toolbar(ui, &entries, Some(&icons));
    let (nodes, _) = pass(&ctx, vec![], draw);
    let save = named(&nodes, accesskit::Role::Button, "Save").expect("icon button is named");
    assert!(
        named(&nodes, accesskit::Role::Button, "Undo")
            .unwrap()
            .is_disabled()
    );
    let (_, intent) = pass(&ctx, click_events(center(save)), draw);
    assert_eq!(
        intent.flatten(),
        Some(Intent {
            command: "demo.file.save".into()
        })
    );
}

#[test]
fn palette_exposes_a_labelled_search_field_and_named_results() {
    let registry = registry();
    let lookup = Lookup {
        registry: &registry,
        localize: &English,
        state: &UndoDisabled,
        keymap: None,
    };
    let views = lookup.all();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut palette = Palette::default();
    let recent = Recent::default();
    let (nodes, _) = pass(&ctx, vec![], |ui| {
        palette.show(ui, &views, &recent, &English)
    });
    let field = nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::TextInput)
        .expect("search field");
    let label_ids = field.labelled_by();
    assert_eq!(label_ids.len(), 1);
    assert!(named(&nodes, accesskit::Role::Label, "Search commands").is_some());
    // Only the enabled command is listed.
    assert!(named(&nodes, accesskit::Role::Button, "Save").is_some());
    assert!(named(&nodes, accesskit::Role::Button, "Undo").is_none());
    palette.query = "sav".into();
    let enter = egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (_, intent) = pass(&ctx, vec![enter], |ui| {
        palette.show(ui, &views, &recent, &English)
    });
    assert_eq!(
        intent.flatten(),
        Some(Intent {
            command: "demo.file.save".into()
        })
    );
    palette.query = "zzz".into();
    let (nodes, _) = pass(&ctx, vec![], |ui| {
        palette.show(ui, &views, &recent, &English)
    });
    assert!(named(&nodes, accesskit::Role::Label, "No matching command").is_some());
}
