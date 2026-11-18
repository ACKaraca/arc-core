//! The dock, command line, property grid, tree and diagnostics widgets driven through a headless
//! context: intents from simulated input and a keyboard focus walk over the AccessKit tree
//! (CORE-UI-001, 006, 010, 011, 012, 020, 026, 031, 032).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_base::error::Value as ArgValue;
use arc_base::id::Identifier;
use arc_base::{
    Dimension, DisplayFormat, DisplayUnits, Error, ErrorCode, ObjectId, Quantity, Unit,
};
use arc_command::{
    CommandLine, CommandSpec, Permission, Registry, Session, Submitted, Surface,
    Target as LineTarget,
};
use arc_doc::diagnostic::{Diagnostic, Origin};
use arc_doc::diagnostics::DiagnosticsSnapshot;
use arc_doc::graph::Evaluators;
use arc_doc::schema::{FieldDecl, Severity, Value, ValueType};
use arc_doc::{Document, TypeRegistry};
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    Axis, CommandLineView, DiagnosticsPanel, Dock, DockArea, DockLayout, GridContext, Intent,
    LineAction, LineCommands, LineContext, Localize, PropertyGrid, PropertyModel, PropertyObject,
    TreeCommands, TreeItem, TreeSource, TreeView,
};

struct English;
impl Localize for English {
    fn message(&self, id: &str, _: &[(&str, ArgValue)]) -> Option<String> {
        Some(
            match id {
                "ui-command-line" => "Command",
                "ui-dock-close" => "Close",
                "ui-dock-separator" => "Panel divider",
                "ui-tree-filter" => "Filter",
                "ui-tree-visible" => "Visible",
                "ui-tree-locked" => "Locked",
                "ui-tree-expand" => "Expand",
                "ui-diagnostics-filter" => "Filter diagnostics",
                "ui-severity-error" => "Error",
                "ui-severity-warning" => "Warning",
                "ui-quantity-scrub" => "Drag to change the value",
                "demo-radius" => "Radius",
                "demo-wall" => "Wall has a gap",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

struct Pass {
    nodes: Vec<(accesskit::NodeId, accesskit::Node)>,
    focus: Option<accesskit::NodeId>,
}

impl Pass {
    fn named(&self, role: accesskit::Role, name: &str) -> Option<&accesskit::Node> {
        self.nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role() == role && n.label().or(n.value()).is_some_and(|l| l.contains(name)))
    }

    fn focused(&self) -> Option<&accesskit::Node> {
        let focus = self.focus?;
        self.nodes
            .iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, n)| n)
            .filter(|n| n.bounds().is_some() && n.role() != accesskit::Role::Window)
    }

    /// The node's own name, or the name of the label it is labelled by.
    fn name_of(&self, node: &accesskit::Node) -> String {
        if let Some(label) = node.label().filter(|l| !l.is_empty()) {
            return label.to_owned();
        }
        node.labelled_by()
            .iter()
            .filter_map(|id| self.nodes.iter().find(|(n, _)| n == id))
            .filter_map(|(_, n)| n.label().or(n.value()))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn run<R>(
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    mut ui_fn: impl FnMut(&mut egui::Ui) -> R,
) -> (Pass, Option<R>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 1400.0),
        )),
        events,
        ..Default::default()
    };
    let mut result = None;
    let mut full = ctx.run_ui(input, |ui| result = Some(ui_fn(ui)));
    full.textures_delta.clear();
    let update = full.platform_output.accesskit_update;
    let focus = update.as_ref().map(|u| u.focus);
    let nodes = update.map(|u| u.nodes).unwrap_or_default();
    let focus = focus.filter(|f| nodes.first().is_none_or(|(root, _)| root != f));
    (Pass { nodes, focus }, result)
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
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

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Circle {
    /// The center x.
    x: f64,
    /// The radius.
    radius: f64,
}

fn line_commands() -> LineCommands {
    LineCommands {
        undo: "core.edit.undo".into(),
        redo: "core.edit.redo".into(),
        clear_selection: "core.select.clear".into(),
    }
}

#[test]
fn command_line_escape_enter_and_undo_drive_the_driver() {
    let mut registry = Registry::new();
    registry
        .register(
            CommandSpec::new(
                "demo.draw.circle",
                "demo-circle",
                "demo-circle",
                |_, p: Circle| Ok(p.x + p.radius),
            )
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
    let commands = line_commands();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut line = CommandLine::new();
    let mut view = CommandLineView::default();
    let step = |line: &mut CommandLine,
                view: &mut CommandLineView,
                events: Vec<egui::Event>,
                selection: bool| {
        let completion = line.complete(&target, &view.input, view.input.len());
        let context = LineContext {
            localize: &English,
            has_selection: selection,
            commands: &commands,
        };
        run(&ctx, events, |ui| {
            view.show(ui, line, &completion, &context)
        })
        .1
        .flatten()
    };
    let apply = |line: &mut CommandLine, action: LineAction| match action {
        LineAction::Submit(text) => line.submit(&target, &text).unwrap(),
        LineAction::Repeat => line.repeat_last(&target).unwrap(),
        LineAction::Back => line.back(&target).unwrap(),
        LineAction::Cancel => line.cancel(),
        other => panic!("unexpected {other:?}"),
    };
    let none = egui::Modifiers::NONE;
    let ctrl = egui::Modifiers::COMMAND;

    // Idle: Escape with a selection clears it; undo goes to the document.
    assert_eq!(
        step(
            &mut line,
            &mut view,
            vec![key(egui::Key::Escape, none)],
            true
        ),
        Some(LineAction::Invoke(Intent::new("core.select.clear")))
    );
    assert_eq!(
        step(&mut line, &mut view, vec![key(egui::Key::Z, ctrl)], false),
        Some(LineAction::Invoke(Intent::new("core.edit.undo")))
    );
    // Focus the field and type a command with one inline parameter.
    let (pass, _) = run(&ctx, vec![], |ui| {
        let completion = arc_command::Completion::default();
        let context = LineContext {
            localize: &English,
            has_selection: false,
            commands: &commands,
        };
        view.show(ui, &line, &completion, &context)
    });
    let field = pass
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == accesskit::Role::TextInput)
        .unwrap();
    assert_eq!(pass.name_of(field), "Command");
    step(&mut line, &mut view, click(center(field)), false);
    step(
        &mut line,
        &mut view,
        vec![egui::Event::Text("demo.draw.circle 1".into())],
        false,
    );
    let submit = step(
        &mut line,
        &mut view,
        vec![key(egui::Key::Enter, none)],
        false,
    )
    .unwrap();
    assert_eq!(submit, LineAction::Submit("demo.draw.circle 1".into()));
    assert!(matches!(apply(&mut line, submit), Submitted::Prompt(_)));
    // Prompting for the radius: Escape at the first prompt cancels the command.
    let cancel = step(
        &mut line,
        &mut view,
        vec![key(egui::Key::Escape, none)],
        true,
    );
    assert_eq!(cancel, Some(LineAction::Cancel));
    apply(&mut line, LineAction::Cancel);
    // Start without parameters: two prompts; answer one, then Ctrl+Z steps back inside.
    assert!(matches!(
        line.submit(&target, "demo.draw.circle").unwrap(),
        Submitted::Prompt(_)
    ));
    assert!(matches!(
        line.submit(&target, "2").unwrap(),
        Submitted::Prompt(_)
    ));
    let back = step(&mut line, &mut view, vec![key(egui::Key::Z, ctrl)], false).unwrap();
    assert_eq!(back, LineAction::Back);
    apply(&mut line, back);
    assert_eq!(line.active().unwrap().answers().len(), 0);
    // Redo during a command cancels it first.
    assert_eq!(
        step(&mut line, &mut view, vec![key(egui::Key::Y, ctrl)], false),
        Some(LineAction::CancelThen(Intent::new("core.edit.redo")))
    );
    // Answer both prompts by typing, then Enter on an empty idle line repeats the command.
    for text in ["3", "4"] {
        step(
            &mut line,
            &mut view,
            vec![egui::Event::Text(text.into())],
            false,
        );
        let action = step(
            &mut line,
            &mut view,
            vec![key(egui::Key::Enter, none)],
            false,
        )
        .unwrap();
        apply(&mut line, action);
    }
    assert_eq!(
        line.last().unwrap().params,
        serde_json::json!({"x": 3.0, "radius": 4.0})
    );
    assert_eq!(
        step(
            &mut line,
            &mut view,
            vec![key(egui::Key::Enter, none)],
            false
        ),
        Some(LineAction::Repeat)
    );
    assert!(matches!(
        apply(&mut line, LineAction::Repeat),
        Submitted::Ran(_)
    ));
    // History holds the submitted lines and Up recalls them.
    assert_eq!(
        view.history.search("circle").next(),
        Some("demo.draw.circle 1")
    );
    step(
        &mut line,
        &mut view,
        vec![key(egui::Key::ArrowUp, none)],
        false,
    );
    assert_eq!(view.input, "4");
}

fn fields() -> Vec<FieldDecl> {
    vec![
        FieldDecl::new("width", ValueType::Quantity(Dimension::LENGTH)).in_aspect("geometry"),
        FieldDecl::new("visible", ValueType::Bool),
    ]
}

#[test]
fn one_property_edit_emits_one_intent_and_invalid_input_none() {
    let decls = fields();
    let values = [
        Some(Value::Quantity(Quantity::new(1.0, Dimension::LENGTH))),
        Some(Value::Bool(true)),
    ];
    let objects = [
        PropertyObject {
            id: ObjectId::from_u128(1),
            fields: &decls,
            values: &values,
            read_only: &[],
            overridden: &[],
        },
        PropertyObject {
            id: ObjectId::from_u128(2),
            fields: &decls,
            values: &values,
            read_only: &[],
            overridden: &[],
        },
    ];
    let model = PropertyModel::build(&objects).unwrap();
    let mut units = DisplayUnits::new();
    units.set(DisplayFormat::new(*Unit::by_symbol("mm").unwrap()));
    let label = |name: &str| name.to_owned();
    let context = GridContext {
        localize: &English,
        label: &label,
        units: &units,
        decimal_separator: '.',
        edit_command: "core.object.set_field",
    };
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut grid = PropertyGrid::default();
    let intents = std::cell::RefCell::new(Vec::<Intent>::new());
    let mut step = |events: Vec<egui::Event>| {
        let (pass, intent) = run(&ctx, events, |ui| grid.show(ui, &model, &context));
        intents.borrow_mut().extend(intent.flatten());
        pass
    };
    let pass = step(vec![]);
    let field = pass
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == accesskit::Role::TextInput && n.value() == Some("1000 mm"))
        .unwrap();
    assert_eq!(pass.name_of(field), "width");
    let at = center(field);
    // Focus and leave without a change: nothing.
    step(click(at));
    step(vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
    step(vec![]);
    assert!(intents.borrow().is_empty());
    // Invalid input: an inline error and no intent, also when focus leaves.
    step(click(at));
    step(vec![
        key(egui::Key::A, egui::Modifiers::COMMAND),
        egui::Event::Text("2 kg".into()),
    ]);
    step(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    let pass = step(vec![]);
    assert!(pass.nodes.iter().any(|(_, n)| {
        n.value()
            .is_some_and(|l| l.contains("solve-expr-dimension-mismatch"))
    }));
    assert!(intents.borrow().is_empty());
    // A valid expression with units: exactly one intent with the field path and SI value.
    step(click(at));
    step(vec![
        key(egui::Key::A, egui::Modifiers::COMMAND),
        egui::Event::Text("1 m + 500 mm".into()),
    ]);
    step(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    for _ in 0..3 {
        step(vec![]);
    }
    let all = intents.borrow().clone();
    assert_eq!(all.len(), 1, "{all:?}");
    let intent = &all[0];
    assert_eq!(intent.command, "core.object.set_field");
    assert_eq!(intent.params["field"], "width");
    assert_eq!(intent.params["value"], 1.5);
    assert_eq!(intent.params["objects"].as_array().unwrap().len(), 2);
    // The check box commits at once, once.
    let pass = step(vec![]);
    let check = pass
        .nodes
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.role() == accesskit::Role::CheckBox)
        .unwrap();
    step(click(center(check)));
    step(vec![]);
    let all = intents.borrow().clone();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].params["value"], false);
}

struct Two;
impl TreeSource for Two {
    fn child_count(&self, parent: Option<ObjectId>, _: &str) -> usize {
        if parent.is_none() { 2 } else { 0 }
    }
    fn child(&self, parent: Option<ObjectId>, index: usize, _: &str) -> Option<TreeItem> {
        (parent.is_none() && index < 2).then(|| TreeItem {
            id: ObjectId::from_u128(index as u128 + 1),
            label: format!("Wall {}", index + 1),
            has_children: false,
            selected: index == 0,
            visible: Some(true),
            locked: Some(false),
        })
    }
    fn index_of(&self, _: Option<ObjectId>, id: ObjectId, _: &str) -> Option<usize> {
        usize::try_from(id.as_u128()).ok()?.checked_sub(1)
    }
}

fn tree_commands() -> TreeCommands {
    TreeCommands {
        select: "core.select.set".into(),
        rename: "core.object.rename".into(),
        set_visible: "core.object.set_visible".into(),
        set_locked: "core.object.set_locked".into(),
        move_to: "core.object.move_to".into(),
    }
}

#[test]
fn tree_and_diagnostics_emit_intents() {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut tree = TreeView::default();
    let commands = tree_commands();
    let (pass, _) = run(&ctx, vec![], |ui| tree.show(ui, &Two, &commands, &English));
    let visible = pass
        .named(accesskit::Role::CheckBox, "Visible Wall 2")
        .unwrap();
    let (_, intent) = run(&ctx, click(center(visible)), |ui| {
        tree.show(ui, &Two, &commands, &English)
    });
    let intent = intent.flatten().unwrap();
    assert_eq!(intent.command, "core.object.set_visible");
    assert_eq!(intent.params["visible"], false);
    let row = pass.named(accesskit::Role::Button, "Wall 2").unwrap();
    let (_, intent) = run(&ctx, click(center(row)), |ui| {
        tree.show(ui, &Two, &commands, &English)
    });
    let intent = intent.flatten().unwrap();
    assert_eq!(intent.params["mode"], "replace");
    assert_eq!(
        intent.params["objects"],
        serde_json::json!([ObjectId::from_u128(2).to_string()])
    );
    // F2 renames the focused row (focused as assistive technology does); Enter commits one
    // rename intent.
    let row_id = pass
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::Button && n.label() == Some("Wall 2"))
        .unwrap()
        .0;
    let focus = egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
        action: accesskit::Action::Focus,
        target_tree: accesskit::TreeId::ROOT,
        target_node: row_id,
        data: None,
    });
    run(&ctx, vec![focus], |ui| {
        tree.show(ui, &Two, &commands, &English)
    });
    let (_, _) = run(
        &ctx,
        vec![key(egui::Key::F2, egui::Modifiers::NONE)],
        |ui| tree.show(ui, &Two, &commands, &English),
    );
    run(&ctx, vec![], |ui| tree.show(ui, &Two, &commands, &English));
    run(
        &ctx,
        vec![
            key(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Text("Wall B".into()),
        ],
        |ui| tree.show(ui, &Two, &commands, &English),
    );
    let (_, intent) = run(
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        |ui| tree.show(ui, &Two, &commands, &English),
    );
    let intent = intent.flatten().unwrap();
    assert_eq!(intent.command, "core.object.rename");
    assert_eq!(intent.params["name"], "Wall B");

    let code = ErrorCode {
        code: "demo.wall.gap",
        category: arc_base::Category::InvalidInput,
        message_id: "demo-wall",
    };
    let snapshot = DiagnosticsSnapshot {
        version: 1,
        diagnostics: vec![Diagnostic::new(
            Severity::Warning,
            Error::new(code),
            vec![ObjectId::from_u128(9)],
            Origin::Validator,
        )]
        .into(),
        dropped: 0,
    };
    let mut panel = DiagnosticsPanel::default();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let (pass, _) = run(&ctx, vec![], |ui| {
        panel.show(ui, &snapshot, &English, "core.view.show_objects")
    });
    let row = pass
        .named(accesskit::Role::Button, "Warning: Wall has a gap")
        .expect("severity is named");
    let (_, intent) = run(&ctx, click(center(row)), |ui| {
        panel.show(ui, &snapshot, &English, "core.view.show_objects")
    });
    assert_eq!(
        intent.flatten().unwrap().params["objects"],
        serde_json::json!([ObjectId::from_u128(9).to_string()])
    );
}

/// Tabs through a dock of a command line, a property grid, a tree and a diagnostics panel.
#[test]
fn keyboard_focus_walk_follows_visual_order_with_names_and_roles() {
    let root: Dock = serde_json::from_value::<DockLayout>(serde_json::json!({
        "root": 6,
        "nodes": [
            {"kind": "tabs", "tabs": ["line"]},
            {"kind": "tabs", "tabs": ["props"]},
            {"kind": "tabs", "tabs": ["tree"]},
            {"kind": "tabs", "tabs": ["diag"]},
            {"kind": "split", "axis": "vertical", "fraction": 0.5, "first": 2, "second": 3},
            {"kind": "split", "axis": "vertical", "fraction": 0.3, "first": 1, "second": 4},
            {"kind": "split", "axis": "vertical", "fraction": 0.15, "first": 0, "second": 5}
        ]
    }))
    .unwrap()
    .root()
    .clone();
    let layout = std::cell::RefCell::new(DockLayout::new(root, vec![]).unwrap());
    let decls = fields();
    let values = [
        Some(Value::Quantity(Quantity::new(1.0, Dimension::LENGTH))),
        Some(Value::Bool(true)),
    ];
    let objects = [PropertyObject {
        id: ObjectId::from_u128(1),
        fields: &decls,
        values: &values,
        read_only: &[],
        overridden: &[],
    }];
    let model = PropertyModel::build(&objects).unwrap();
    let units = DisplayUnits::new();
    let label = |name: &str| name.to_owned();
    let grid_context = GridContext {
        localize: &English,
        label: &label,
        units: &units,
        decimal_separator: '.',
        edit_command: "core.object.set_field",
    };
    let commands = line_commands();
    let line = CommandLine::new();
    let mut view = CommandLineView::default();
    let mut grid = PropertyGrid::default();
    let mut tree = TreeView::default();
    let mut diagnostics = DiagnosticsPanel::default();
    let snapshot = DiagnosticsSnapshot::default();
    let tree_commands = tree_commands();
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut draw = |ui: &mut egui::Ui| {
        let title = |panel: &str| panel.to_owned();
        let mut content = |ui: &mut egui::Ui, panel: &str| match panel {
            "line" => {
                let context = LineContext {
                    localize: &English,
                    has_selection: false,
                    commands: &commands,
                };
                let _ = view.show(ui, &line, &arc_command::Completion::default(), &context);
            }
            "props" => {
                let _ = grid.show(ui, &model, &grid_context);
            }
            "tree" => {
                let _ = tree.show(ui, &Two, &tree_commands, &English);
            }
            _ => {
                let _ = diagnostics.show(ui, &snapshot, &English, "core.view.show_objects");
            }
        };
        DockArea::default().show(ui, &mut layout.borrow_mut(), &English, &title, &mut content);
    };
    run(&ctx, vec![], &mut draw);
    let mut walk: Vec<(accesskit::Role, String, egui::Pos2)> = Vec::new();
    for _ in 0..60 {
        let (pass, _) = run(
            &ctx,
            vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
            &mut draw,
        );
        let Some(node) = pass.focused() else { continue };
        let entry = (node.role(), pass.name_of(node), center(node));
        if walk
            .first()
            .is_some_and(|first| first.1 == entry.1 && first.0 == entry.0)
        {
            break;
        }
        walk.push(entry);
    }
    let names: Vec<&str> = walk.iter().map(|(_, n, _)| n.as_str()).collect();
    for (role, name, _) in &walk {
        assert!(!name.is_empty(), "unnamed {role:?} in {names:?}");
    }
    for expected in [
        "line",
        "Close line",
        "Command",
        "Panel divider",
        "width",
        "Drag to change the value",
        "visible",
        "Filter",
        "Visible Wall 1",
        "Locked Wall 2",
        "Wall 2",
        "Error",
        "Filter diagnostics",
    ] {
        assert!(
            names.iter().any(|n| n.contains(expected)),
            "{expected} not reached in {names:?}"
        );
    }
    let roles: Vec<accesskit::Role> = walk.iter().map(|(r, ..)| *r).collect();
    for role in [
        accesskit::Role::Button,
        accesskit::Role::TextInput,
        accesskit::Role::CheckBox,
        accesskit::Role::Splitter,
        accesskit::Role::SpinButton,
    ] {
        assert!(roles.contains(&role), "{role:?} not reached");
    }
    // Focus order matches visual order: down the page, and left to right within a row.
    for pair in walk.windows(2) {
        let (a, b) = (pair[0].2, pair[1].2);
        let same_row = (a.y - b.y).abs() < 10.0;
        assert!(
            if same_row { b.x > a.x } else { b.y > a.y },
            "{:?} before {:?}",
            pair[0],
            pair[1]
        );
    }
    // A focused separator moves with the arrow keys.
    let before = layout.borrow().clone();
    while run(
        &ctx,
        vec![key(egui::Key::Tab, egui::Modifiers::NONE)],
        &mut draw,
    )
    .0
    .focused()
    .is_none_or(|n| n.role() != accesskit::Role::Splitter)
    {}
    run(
        &ctx,
        vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        &mut draw,
    );
    run(&ctx, vec![], &mut draw);
    let after = layout.borrow().clone();
    let Dock::Split {
        axis: Axis::Vertical,
        fraction,
        ..
    } = after.root()
    else {
        panic!("split")
    };
    assert!((fraction - 0.2).abs() < 1e-6, "{fraction}");
    assert_ne!(after, before);
}
