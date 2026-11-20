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

/// A package with no objects at `path`.
fn new_package(path: &std::path::Path) {
    let owner = arc_doc::package::LockOwner {
        user: "creator".into(),
        host: "elsewhere".into(),
        application: "test".into(),
        time_unix_ms: 0,
    };
    drop(
        arc_doc::package::Package::create(path, &arc_doc::package::Metadata::default(), &owner)
            .unwrap(),
    );
}

fn point_count(h: &Harness, tab: TabId) -> usize {
    let snapshot = h.app.tab(tab).unwrap().document().snapshot();
    snapshot.iter_type(arc_shell_sample::POINT).unwrap().count()
}

#[test]
fn recovery_prompt_lists_a_dropped_session_and_acts_only_on_choice() {
    let dir = TempDir::new("recovery");
    let data = dir.join("data");
    let file = dir.join("doc.arc");
    new_package(&file);

    // An application with unsaved changes in a file and in a new document is dropped.
    let mut crashed = Harness::new(&data, None);
    crashed.app.open(&file).unwrap();
    crashed.command("shell.point.create position=[1, 2, 0]");
    crashed.menu(commands::FILE_NEW);
    crashed.command("shell.point.create position=[3, 4, 0]");
    assert_eq!(
        crashed.shell.invocations().len(),
        2,
        "{:?}",
        crashed.shell.errors()
    );
    drop(crashed);

    // The next start lists both sessions and changes nothing on its own.
    let mut h = Harness::new(&data, None);
    // The modal is measured in its first frame and shown from the second.
    h.frame(Vec::new());
    let pass = h.frame(Vec::new());
    assert_eq!(h.shell.recovery().len(), 2);
    assert_eq!(pass.all(accesskit::Role::Button, "Recover").len(), 2);
    // Fluent isolates arguments with direction marks, so fragments are matched.
    assert!(pass.shows("unsaved changes") && pass.shows("doc.arc"));
    assert!(h.app.tabs().is_empty());
    assert_eq!(h.app.recoverable().unwrap().len(), 2);
    let based = h
        .shell
        .recovery()
        .iter()
        .position(|e| e.orphan.base.is_some())
        .unwrap();

    // Compare shows the model's data and recovers nothing.
    let compare = center(pass.all(accesskit::Role::Button, "Compare")[based]);
    h.frame(click(compare));
    h.frame(Vec::new());
    let pass = h.frame(Vec::new());
    assert!(pass.shows("Saved file:") && pass.shows("The saved file is unchanged"));
    assert!(pass.shows("The saved file holds") && pass.shows("changes would be replayed."));
    assert!(h.app.tabs().is_empty());

    // Recover replays the session into a dirty tab of the file.
    let recover = center(pass.all(accesskit::Role::Button, "Recover")[based]);
    h.frame(click(recover));
    let tab = h.app.active().unwrap();
    assert_eq!(h.app.tab(tab).unwrap().path(), Some(file.as_path()));
    assert!(h.app.tab(tab).unwrap().is_dirty());
    assert_eq!(point_count(&h, tab), 1);
    assert_eq!(h.shell.recovery().len(), 1);

    // Discard deletes the other session; the prompt closes.
    h.press("Discard");
    assert!(h.shell.recovery().is_empty());
    assert_eq!(h.app.tabs().len(), 1);
    assert!(h.app.recoverable().unwrap().is_empty());
    let pass = h.frame(Vec::new());
    assert!(pass.find(accesskit::Role::Button, "Recover").is_none());
    assert!(h.shell.errors().is_empty(), "{:?}", h.shell.errors());
}

#[test]
fn banners_name_the_lock_holder_and_a_stale_lock_is_taken_over_on_request() {
    let dir = TempDir::new("banners");
    let file = dir.join("shared.arc");
    new_package(&file);

    // A second instance opens the file read-only with the holder in the banner.
    let mut first = Harness::new(&dir.join("data1"), None);
    first.app.open(&file).unwrap();
    let mut second = Harness::new(&dir.join("data2"), None);
    second.app.open(&file).unwrap();
    let pass = second.frame(Vec::new());
    assert!(
        pass.shows("is editing this file with") && pass.shows("tester"),
        "banner missing"
    );
    assert!(pass.find(accesskit::Role::Button, "Take over").is_none());
    // The banner belongs to its tab: a new document has none.
    second.menu(commands::FILE_NEW);
    assert!(!second.frame(Vec::new()).shows("is editing this file"));
    drop((first, second));

    // A lock left by a process of this host that no longer runs.
    let stale = dir.join("stale.arc");
    new_package(&stale);
    std::fs::write(
        arc_doc::package::lock_path(&stale),
        "{\"format\":1,\"user\":\"ghost\",\"host\":\"test-host\",\"pid\":4000000,\
         \"application\":\"old app 0\",\"time_unix_ms\":5}",
    )
    .unwrap();
    let mut h = Harness::new(&dir.join("data3"), None);
    let tab = h.app.open(&stale).unwrap();
    assert!(h.app.tab(tab).unwrap().is_read_only());
    let pass = h.frame(Vec::new());
    assert!(pass.shows("This file was left locked by") && pass.shows("ghost"));
    // Nothing changes until the user takes over.
    assert!(h.app.tab(tab).unwrap().is_read_only());
    h.press("Take over the lock");
    assert!(
        !h.app.tab(tab).unwrap().is_read_only(),
        "{:?}",
        h.shell.errors()
    );
    assert!(h.app.tab(tab).unwrap().banners().is_empty());
    assert!(!h.frame(Vec::new()).shows("left locked"));
}

#[test]
fn settings_edits_are_written_effective_at_once_and_after_a_restart() {
    let dir = TempDir::new("settings");
    let data = dir.join("data");
    let mut h = Harness::new(&data, None);
    h.shell.layout_mut().activate(arc_shell::SETTINGS).unwrap();
    assert!(h.frame(Vec::new()).shows("Settings"));

    // Scale and language through the settings panel, found by searching.
    h.type_into("Search settings", "ui.scale");
    h.type_into("ui.scale", "150");
    assert_eq!(
        h.shell.appearance().scale.percent(),
        150,
        "{:?}",
        h.shell.errors()
    );
    h.type_into("Search settings", "ui.language");
    h.type_into("ui.language", common::TEST_LANGUAGE);
    let table = arc_app::settings_file::load(&data.join("settings.toml"))
        .unwrap()
        .table;
    assert_eq!(table["ui"]["scale"].as_integer(), Some(150), "{table:?}");
    assert_eq!(
        table["ui"]["language"].as_str(),
        Some(common::TEST_LANGUAGE),
        "{table:?}"
    );
    // The language applies within the same run: the panel title changes.
    let pass = h.frame(Vec::new());
    assert!(
        pass.shows(common::TEST_SETTINGS_TITLE),
        "title not switched"
    );
    assert!(h.shell.errors().is_empty(), "{:?}", h.shell.errors());
    drop(h);

    // After a restart the file gives the same effective values.
    let mut h = Harness::new(&data, None);
    assert_eq!(h.shell.appearance().scale.percent(), 150);
    assert!(h.frame(Vec::new()).shows(common::TEST_SETTINGS_TITLE));

    // Reset removes the user value from the file.
    h.shell.layout_mut().activate(arc_shell::SETTINGS).unwrap();
    h.type_into("Search settings", "ui.");
    let pass = h.frame(Vec::new());
    let resets = pass.all(accesskit::Role::Button, "Reset");
    assert_eq!(resets.len(), 2);
    // The first is the language's; resetting it switches the labels back.
    let first = center(resets[0]);
    h.frame(click(first));
    let table = arc_app::settings_file::load(&data.join("settings.toml"))
        .unwrap()
        .table;
    assert_eq!(
        table["ui"].as_table().map(|t| t.len()),
        Some(1),
        "{table:?}"
    );
    assert_eq!(table["ui"]["scale"].as_integer(), Some(150));
    assert!(!h.frame(Vec::new()).shows(common::TEST_SETTINGS_TITLE));
}

#[test]
fn a_malformed_settings_file_is_set_aside_and_reported() {
    let dir = TempDir::new("malformed");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("settings.toml"), "ui = [unclosed").unwrap();
    let mut h = Harness::new(&data, None);
    assert!(data.join("settings.toml.malformed-1").exists());
    assert!(!data.join("settings.toml").exists());
    let pass = h.frame(Vec::new());
    assert!(pass.shows("is damaged. It was renamed"), "no notification");
}

#[test]
fn dropped_files_open_and_recent_files_are_written() {
    let dir = TempDir::new("drop");
    let data = dir.join("data");
    let file = dir.join("dropped.arc");
    new_package(&file);
    let mut h = Harness::new(&data, None);
    let ring = arc_app::shell::install_logging(&h.app, data.join("logs")).unwrap();
    h.shell.set_log_ring(ring);

    h.drop_files(&[file.clone(), dir.join("notes.txt")]);
    let tab = h.app.active().unwrap();
    assert_eq!(h.app.tab(tab).unwrap().path(), Some(file.as_path()));
    assert_eq!(h.shell.errors()[0].code(), "app.file.unsupported_type");
    let recent = arc_app::recent::RecentFiles::load(&data.join("recent.toml"), 20)
        .unwrap()
        .0;
    assert_eq!(recent.iter().next().unwrap().path, file);

    // The log viewer shows the record and never the redacted path.
    h.shell.layout_mut().activate(arc_shell::LOG).unwrap();
    let pass = h.frame(Vec::new());
    assert!(pass.shows("dropped file opened"), "record missing");
    assert!(pass.shows("path=[redacted]"));
    // The tab strip names the file; the log never shows its path.
    assert!(!pass.shows(&file.display().to_string()));
}

#[test]
fn a_floating_tree_is_laid_out_as_a_second_window() {
    let dir = TempDir::new("floating");
    let mut h = Harness::new(&dir.join("data"), None);
    h.shell
        .layout_mut()
        .float(arc_shell::LOG, [40.0, 30.0, 400.0, 300.0])
        .unwrap();
    let windows = h.shell.windows();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].id, "float.shell.log");
    assert_eq!(windows[0].panels, [arc_shell::LOG]);
    assert_eq!(windows[0].rect, [40.0, 30.0, 400.0, 300.0]);

    // The panel is drawn in its window, not in the main one.
    let empty = h.text("app-log-empty");
    assert!(!h.frame(Vec::new()).shows(&empty));
    let pass = h.window_frame("float.shell.log", Vec::new());
    assert!(pass.shows(&empty) && pass.shows("Log"));

    // Closing the window docks the panel back into the main tree.
    assert!(h.shell.dock_window("float.shell.log"));
    assert!(h.shell.windows().is_empty());
    h.shell.layout_mut().activate(arc_shell::LOG).unwrap();
    assert!(h.frame(Vec::new()).shows(&empty));
    assert!(!h.window_frame("float.shell.log", Vec::new()).shows(&empty));
}

/// Whether the outliner row of a point is listed when `filter` is typed in the outliner filter,
/// in a shell whose language setting is `language`.
fn point_row_listed(language: &str, filter: &str) -> bool {
    let dir = TempDir::new("filter");
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("settings.toml"),
        format!("[ui]\nlanguage = \"{language}\"\n"),
    )
    .unwrap();
    let mut h = Harness::new(&data, None);
    h.menu(commands::FILE_NEW);
    h.command("shell.point.create position=[0, 0, 0]");
    h.frame(Vec::new());
    let label = h.text("ui-tree-filter");
    h.type_into(&label, filter);
    let listed = h
        .frame(Vec::new())
        .find(accesskit::Role::Button, "Point")
        .is_some();
    assert!(h.shell.errors().is_empty(), "{:?}", h.shell.errors());
    listed
}

#[test]
fn the_outliner_filter_folds_case_by_the_active_language() {
    // English: `I` folds to `i`, a dotted capital does not match.
    assert!(point_row_listed("en-US", "POINT"));
    assert!(!point_row_listed("en-US", "POİNT"));
    // Turkish: `İ` folds to `i` and `I` to the dotless `ı`.
    assert!(point_row_listed("tr", "POİNT"));
    assert!(!point_row_listed("tr", "POINT"));
}

#[test]
fn a_recorded_macro_replays_the_session_on_a_copy_of_its_start() {
    use arc_command::macros::{parse_macro, play};
    use arc_command::{Permission, Session, Surface};

    let dir = TempDir::new("macro");
    let start = dir.join("start.arc");
    let copy = dir.join("copy.arc");
    new_package(&start);
    std::fs::copy(&start, &copy).unwrap();

    let mut h = Harness::new(&dir.join("data"), None);
    let tab = h.app.open(&start).unwrap();
    h.menu(commands::MACRO_RECORD_START);
    h.command("shell.layer.create name=walls");
    h.command("shell.point.create position=[1, 2, 0]");
    h.command("shell.line.create start=[5, 5, 0] end=[8, 5, 0]");
    assert_eq!(h.shell.invocations().len(), 3, "{:?}", h.shell.errors());
    let macro_file = dir.join("session.jsonl");
    h.dialogs.save = Some(macro_file.clone());
    h.menu(commands::MACRO_RECORD_STOP);
    assert_eq!(h.dialogs.asked, ["save"]);
    // Recording stopped: a further command is not in the macro.
    h.command("shell.point.create position=[9, 9, 0]");
    h.menu(commands::MACRO_RECORD_STOP);
    assert_eq!(
        h.dialogs.asked,
        ["save"],
        "stop without a recording asks nothing"
    );
    h.menu(commands::EDIT_UNDO);
    h.menu(commands::FILE_SAVE);
    let (_, saved) = objects(&h);
    assert_eq!(saved.len(), 3);
    assert!(!h.app.tab(tab).unwrap().is_dirty());

    // Play the macro on the copy of the starting package.
    let text = std::fs::read_to_string(&macro_file).unwrap();
    let parsed = parse_macro(&text, &arc_base::Limits::DEFAULT).unwrap();
    assert_eq!(parsed.steps.len(), 3);
    let mut other = common::app_in(&dir.join("data2"));
    let replay = other.open(&copy).unwrap();
    let session = Session::new(Surface::Ui, [Permission::Read, Permission::Edit]);
    let registry = &other.product().headless().registry;
    let document = other.tab(replay).unwrap().document();
    play(registry, document, &session, &parsed.steps, "macro").unwrap();
    let snapshot = document.snapshot();
    let mut replayed: Vec<String> = snapshot
        .iter()
        .map(|(_, o)| format!("{} {:?}", o.id, o.values))
        .collect();
    replayed.sort();
    assert_eq!(replayed, saved);
    let _ = &mut other;
}
