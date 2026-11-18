//! A scripted session over the shell composition driven by keyboard events only (CORE-APP-025,
//! CORE-UI-026): a new document with Ctrl+N, objects created on the command line, the point
//! selected by focusing its outliner row and pressing Enter, its coordinate edited in the
//! property grid, undo with Ctrl+Z in the command line and save with Ctrl+S. No pointer event
//! is sent: focus moves with Tab and Shift+Tab. The document state is asserted after each step
//! as in `session.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests"
)]

mod common;

use arc_app::application::TabId;
use arc_base::math::Point3;
use arc_doc::Value;
use arc_ui::egui::{self, accesskit};
use common::{Harness, Pass, TempDir, key};

const NONE: egui::Modifiers = egui::Modifiers::NONE;
/// What a platform reports for the Ctrl key on Linux and Windows.
const CTRL: egui::Modifiers = egui::Modifiers {
    ctrl: true,
    command: true,
    ..egui::Modifiers::NONE
};

fn objects(h: &Harness) -> (TabId, Vec<String>) {
    let tab = h.app.active().unwrap();
    let snapshot = h.app.tab(tab).unwrap().document().snapshot();
    let mut out: Vec<String> = snapshot
        .iter()
        .map(|(_, o)| format!("{} {:?}", o.id, o.values))
        .collect();
    out.sort();
    (tab, out)
}

fn point_position(h: &Harness) -> Option<Point3> {
    let tab = h.app.active()?;
    let snapshot = h.app.tab(tab).ok()?.document().snapshot();
    let points = snapshot.iter_type(arc_shell_sample::POINT).ok()?;
    let (_, o) = points.into_iter().next()?;
    match o.values.first()? {
        Some(Value::Point3(p)) => Some(*p),
        _ => None,
    }
}

/// The name of the node that has keyboard focus: its label, or the label it is labelled by.
fn focused(h: &Harness, pass: &Pass) -> Option<(accesskit::Role, String)> {
    let id = h.ctx.memory(|m| m.focused())?;
    let (_, node) = pass.nodes.iter().find(|(n, _)| n.0 == id.value())?;
    let name = node
        .label()
        .map(str::to_owned)
        .or_else(|| {
            node.labelled_by().iter().find_map(|l| {
                let (_, label) = pass.nodes.iter().find(|(n, _)| n == l)?;
                label.label().or(label.value()).map(str::to_owned)
            })
        })
        .unwrap_or_default();
    Some((node.role(), name))
}

/// Presses Tab until the focused node has `role` and a name containing `name`; panics after a
/// full walk so a missing keyboard route fails loudly instead of looping.
fn tab_to(h: &mut Harness, role: accesskit::Role, name: &str) -> Pass {
    let mut seen = Vec::new();
    for _ in 0..80 {
        let pass = h.frame(vec![key(egui::Key::Tab, NONE)]);
        let here = focused(h, &pass);
        if let Some((r, n)) = &here
            && *r == role
            && n.contains(name)
        {
            return pass;
        }
        seen.push(here);
    }
    panic!("no {role:?} named {name} reachable by Tab; focus visited {seen:?}");
}

/// Types `line` into the command line and presses Enter, focusing the field with Tab first.
fn command(h: &mut Harness, line: &str) {
    let label = h.text("ui-command-line");
    tab_to(h, accesskit::Role::TextInput, &label);
    h.frame(vec![egui::Event::Text(line.into())]);
    h.frame(vec![key(egui::Key::Enter, NONE)]);
}

#[test]
fn keyboard_only_create_select_edit_undo_and_save() {
    let dir = TempDir::new("keyboard");
    let mut h = Harness::new(&dir.join("data"), None);

    // Ctrl+N is bound to the new-document command.
    h.frame(vec![key(egui::Key::N, CTRL)]);
    assert_eq!(h.app.tabs().len(), 1, "{:?}", h.shell.errors());
    h.frame(Vec::new());

    command(&mut h, "shell.layer.create name=walls");
    command(&mut h, "shell.point.create position=[0, 0, 0]");
    command(&mut h, "shell.line.create start=[5, 5, 0] end=[8, 5, 0]");
    let (tab, created) = objects(&h);
    assert_eq!(created.len(), 3, "{:?} {:?}", created, h.shell.errors());
    assert_eq!(point_position(&h), Some(Point3::origin()));
    h.shell.settle(&h.app, arc_app::shell::WAIT);
    h.frame(Vec::new());

    // Select the point: Tab to its outliner row and press Enter.
    tab_to(&mut h, accesskit::Role::Button, "shell.point");
    h.frame(vec![key(egui::Key::Enter, NONE)]);
    h.frame(Vec::new());
    let view = h.shell.view(tab).unwrap();
    assert_eq!(view.selection().len(), 1, "{:?}", h.shell.errors());
    let snapshot = h.app.tab(tab).unwrap().document().snapshot();
    let handle = snapshot.resolve(view.selection()[0]).unwrap();
    assert_eq!(
        snapshot.type_key(handle).unwrap().as_str(),
        arc_shell_sample::POINT
    );

    // Edit the point's x coordinate: Tab to the first position field of the property grid.
    h.frame(Vec::new());
    let pass = tab_to(&mut h, accesskit::Role::TextInput, "position");
    let focus = h.ctx.memory(|m| m.focused()).unwrap();
    let field = pass
        .nodes
        .iter()
        .find(|(n, _)| n.0 == focus.value())
        .unwrap();
    let top = pass
        .all(accesskit::Role::TextInput, "position")
        .iter()
        .map(|n| n.bounds().unwrap().y0)
        .fold(f64::INFINITY, f64::min);
    assert_eq!(field.1.bounds().unwrap().y0, top, "the first field is x");
    h.frame(vec![key(egui::Key::A, CTRL)]);
    h.frame(vec![egui::Event::Text("1200 mm + 3 in".into())]);
    h.frame(vec![key(egui::Key::Enter, NONE)]);
    let moved = point_position(&h).unwrap();
    assert!(
        (moved.x - 1.2762).abs() < 1e-9,
        "{moved:?} {:?}",
        h.shell.errors()
    );
    assert!(h.app.tab(tab).unwrap().is_dirty());

    // Undo: Ctrl+Z in the idle command line.
    let label = h.text("ui-command-line");
    tab_to(&mut h, accesskit::Role::TextInput, &label);
    h.frame(vec![key(egui::Key::Z, CTRL)]);
    assert_eq!(
        point_position(&h),
        Some(Point3::origin()),
        "{:?} {:?}",
        h.shell.errors(),
        h.shell.invocations()
    );
    let (_, after_undo) = objects(&h);
    assert_eq!(after_undo, created);

    // Save: Ctrl+S; the document was never saved, so the dialog asks for a file.
    let file = dir.join("keyboard.arc");
    h.dialogs.save = Some(file.clone());
    h.frame(vec![key(egui::Key::S, CTRL)]);
    assert_eq!(h.dialogs.asked, ["save"]);
    assert!(file.exists());
    assert!(!h.app.tab(tab).unwrap().is_dirty());

    assert_eq!(
        h.shell.invocations(),
        [
            "shell.layer.create",
            "shell.point.create",
            "shell.line.create",
            "shell.object.set_field",
        ]
    );
    assert!(h.shell.errors().is_empty(), "{:?}", h.shell.errors());
}
