//! A scripted session over the shell composition on the software adapter (CORE-APP-025):
//! create objects through the command line, select one by clicking in the viewport, edit a
//! property in the grid with a unit expression, undo, save and reopen, with the document state
//! asserted after each step and every edit going through `Application::invoke`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests"
)]

mod common;

use arc_app::application::TabId;
use arc_app::commands;
use arc_app::shell::WAIT;
use arc_base::math::Point3;
use arc_doc::Value;
use arc_gpu::{Gpu, GpuSettings};
use arc_ui::egui::{self, accesskit};
use common::{Harness, TempDir, center, click, key};

/// The objects of the active document as sorted text, and the active tab.
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

#[test]
fn create_select_edit_undo_save_and_reopen() {
    let dir = TempDir::new("session");
    let gpu = Gpu::new(&GpuSettings::default()).unwrap();
    let mut h = Harness::new(&dir.join("data"), Some(gpu));

    // A new document through the menu.
    h.menu(commands::FILE_NEW);
    assert_eq!(h.app.tabs().len(), 1);
    h.frame(Vec::new());

    // Create a layer, a point at the origin and a line away from it on the command line.
    h.command("shell.layer.create name=walls");
    h.command("shell.point.create position=[0, 0, 0]");
    h.command("shell.line.create start=[5, 5, 0] end=[8, 5, 0]");
    let (tab, created) = objects(&h);
    assert_eq!(created.len(), 3, "{:?} {:?}", created, h.shell.errors());
    assert_eq!(point_position(&h), Some(Point3::origin()));

    // Click the point in the middle of the viewport: it becomes the selection.
    h.shell.settle(&h.app, WAIT);
    let pass = h.frame(Vec::new());
    let viewport = pass
        .find(accesskit::Role::Canvas, "")
        .expect("viewport canvas");
    let at = center(viewport);
    h.shell.settle(&h.app, WAIT);
    h.frame(click(at));
    let view = h.shell.view(tab).unwrap();
    assert_eq!(view.selection().len(), 1, "{:?}", h.shell.errors());
    let snapshot = h.app.tab(tab).unwrap().document().snapshot();
    let handle = snapshot.resolve(view.selection()[0]).unwrap();
    assert_eq!(
        snapshot.type_key(handle).unwrap().as_str(),
        arc_shell_sample::POINT
    );

    // Edit the point's x coordinate in the property grid with a unit expression.
    let pass = h.frame(Vec::new());
    // The axes are stacked: x is the topmost field.
    let mut fields = pass.all(accesskit::Role::TextInput, "position");
    assert_eq!(fields.len(), 3);
    fields.sort_by(|a, b| a.bounds().unwrap().y0.total_cmp(&b.bounds().unwrap().y0));
    let x = center(fields[0]);
    h.frame(click(x));
    h.frame(vec![key(egui::Key::A, egui::Modifiers::COMMAND)]);
    h.frame(vec![egui::Event::Text("1200 mm + 3 in".into())]);
    h.frame(vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
    let moved = point_position(&h).unwrap();
    assert!(
        (moved.x - 1.2762).abs() < 1e-9,
        "{moved:?} {:?}",
        h.shell.errors()
    );
    assert!(h.app.tab(tab).unwrap().is_dirty());

    // Undo restores the origin.
    h.menu(commands::EDIT_UNDO);
    assert_eq!(
        point_position(&h),
        Some(Point3::origin()),
        "{:?} {:?}",
        h.shell.errors(),
        h.shell.invocations()
    );
    let (_, after_undo) = objects(&h);
    assert_eq!(after_undo, created);

    // Save: the document was never saved, so the save dialog asks for a file.
    let file = dir.join("session.arc");
    h.dialogs.save = Some(file.clone());
    h.menu(commands::FILE_SAVE);
    assert_eq!(h.dialogs.asked, ["save"]);
    assert!(file.exists());
    assert!(!h.app.tab(tab).unwrap().is_dirty());

    // Close and reopen through the open dialog: the same objects.
    h.menu(commands::FILE_CLOSE);
    assert!(h.app.tabs().is_empty());
    h.dialogs.open = Some(file.clone());
    h.menu(commands::FILE_OPEN);
    assert_eq!(h.dialogs.asked, ["save", "open"]);
    let (reopened_tab, reopened) = objects(&h);
    assert_ne!(reopened_tab, tab);
    assert_eq!(reopened, created);
    assert_eq!(
        h.app.tab(reopened_tab).unwrap().path(),
        Some(file.as_path())
    );

    // Every edit went through Application::invoke, in order; undo, save and the file
    // commands are shell commands and do not appear.
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
