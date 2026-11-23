//! The application model against real packages (CORE-APP-001, 003, 004, 005, 006, 011), built
//! from the reference shell's sample set.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "tests"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_app::application::{AppConfig, Application, CloseAnswer, Closed, TabId};
use arc_app::locale::{Catalogs, Localizer};
use arc_app::product::{Entry, Extractor, FileType, Product, ProductBuilder};
use arc_app::recent::RecentFiles;
use arc_app::settings::{Scope, Setting, SettingType, SettingValue};
use arc_app::settings_editor;
use arc_base::{IdGenerator, Limits};
use arc_command::{FixedClock, Invocation, Permission, Session, Surface};
use arc_doc::package::{LockOwner, Metadata, Package, lock_path};
use serde_json::json;

/// A directory removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("arc-app-model-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn owner(user: &str) -> LockOwner {
    LockOwner {
        user: user.into(),
        host: "test-host".into(),
        application: "arc-shell".into(),
        time_unix_ms: 0,
    }
}

/// The sample product, as the windowed shell registers it.
fn product(user: &str) -> Product {
    let mut builder = ProductBuilder::new("arc-shell", "0.1.0")
        .with_types(arc_shell_sample::types().unwrap())
        .with_commands(arc_shell_sample::registry().unwrap());
    builder
        .file_type(FileType {
            extension: "arc".into(),
            message_id: "shell-file-arc".into(),
            open: true,
            save: true,
        })
        .unwrap();
    builder
        .panel(Entry {
            id: "shell.outliner".into(),
            message_id: "shell-panel-outliner".into(),
            factory: Arc::new(|| String::from("outliner state")),
        })
        .unwrap();
    builder
        .tool(Entry {
            id: "shell.point_tool".into(),
            message_id: "shell-tool-point".into(),
            factory: Arc::new(|| 7_u32),
        })
        .unwrap();
    builder
        .extractor(Extractor {
            id: "shell.lines".into(),
            types: vec![
                arc_shell_sample::LINE.into(),
                arc_shell_sample::POINT.into(),
            ],
            factory: Arc::new(|| ()),
        })
        .unwrap();
    builder
        .setting(Setting {
            key: "shell.grid.visible",
            ty: SettingType::Bool,
            default: SettingValue::Bool(true),
            scope: Scope::Document,
            message_id: "shell-setting-grid-visible",
        })
        .unwrap();
    builder.build(Arc::new(FixedClock(0)), owner(user)).unwrap()
}

fn app_in(data: &Path, user: &str, seed: u64) -> Application {
    let next = AtomicU64::new(seed << 32);
    let config = AppConfig {
        data_dir: data.to_path_buf(),
        limits: Limits::DEFAULT,
        id_generators: Box::new(move || IdGenerator::seeded(next.fetch_add(1, Ordering::Relaxed))),
        recent: RecentFiles::new(20),
    };
    Application::new(product(user), config)
}

fn new_package(path: &Path) {
    drop(Package::create(path, &Metadata::default(), &owner("creator")).unwrap());
}

fn edit_session() -> Session {
    Session::new(Surface::Automation, [Permission::Edit])
}

fn add_point(app: &Application, tab: TabId, x: f64) -> arc_base::Result<()> {
    let invocation = Invocation::new("shell.point.create", json!({ "position": [x, 0.0, 0.0] }));
    app.invoke(tab, &edit_session(), invocation).map(|_| ())
}

/// The objects of a tab's document as comparable data.
fn objects(app: &Application, tab: TabId) -> Vec<String> {
    let snapshot = app.tab(tab).unwrap().document().snapshot();
    let mut out: Vec<String> = snapshot
        .iter()
        .map(|(_, o)| format!("{} {:?}", o.id, o.values))
        .collect();
    out.sort();
    out
}

#[test]
fn product_is_built_from_registrations_only() {
    let product = product("a");
    let headless = product.headless();
    assert_eq!(headless.name, "arc-shell");
    assert!(headless.registry.get("shell.point.create").is_some());
    assert_eq!(headless.types.len(), 3);
    assert_eq!(
        product.panels()[0]
            .factory
            .create()
            .downcast_ref::<String>()
            .unwrap(),
        "outliner state"
    );
    assert_eq!(
        *product.tools()[0]
            .factory
            .create()
            .downcast_ref::<u32>()
            .unwrap(),
        7
    );
    assert_eq!(product.extractors().len(), 1);
    assert!(product.file_type(Path::new("/x/a.ARC")).is_some());
    assert!(product.file_type(Path::new("/x/a.txt")).is_none());
    assert_eq!(product.settings().iter().count(), 6);

    let mut b = ProductBuilder::new("p", "1");
    let entry = |id: &str| Entry {
        id: id.into(),
        message_id: "m".into(),
        factory: Arc::new(|| ()),
    };
    let code = |r: arc_base::Result<()>| r.unwrap_err().code();
    assert_eq!(code(b.panel(entry("NoDots"))), "app.product.invalid_id");
    b.panel(entry("p.one")).unwrap();
    assert_eq!(code(b.panel(entry("p.one"))), "app.product.duplicate");
    let ft = |ext: &str, open, save| FileType {
        extension: ext.into(),
        message_id: "m".into(),
        open,
        save,
    };
    assert_eq!(
        code(b.file_type(ft("A", true, true))),
        "app.product.invalid_id"
    );
    assert_eq!(
        code(b.file_type(ft("", true, true))),
        "app.product.invalid_id"
    );
    assert_eq!(
        code(b.file_type(ft("x", false, false))),
        "app.product.invalid_id"
    );
    b.file_type(ft("x", true, false)).unwrap();
    assert_eq!(
        code(b.file_type(ft("x", false, true))),
        "app.product.duplicate"
    );
    b.extractor(Extractor {
        id: "p.ex".into(),
        types: vec!["p.missing".into()],
        factory: Arc::new(|| ()),
    })
    .unwrap();
    let error = b.build(Arc::new(FixedClock(0)), owner("a")).unwrap_err();
    assert_eq!(error.code(), "app.product.unknown_type");
}

#[test]
fn tabs_have_independent_dirty_state_and_undo_and_close_follows_the_decision_table() {
    let dir = TempDir::new("tabs");
    let mut app = app_in(&dir.join("data"), "a", 1);
    let a = app.new_document().unwrap();
    let b = app.new_document().unwrap();
    assert_eq!(app.active(), Some(b));
    assert!(!app.tab(a).unwrap().is_dirty());

    add_point(&app, a, 1.0).unwrap();
    add_point(&app, a, 2.0).unwrap();
    add_point(&app, b, 3.0).unwrap();
    assert!(app.tab(a).unwrap().is_dirty() && app.tab(b).unwrap().is_dirty());
    assert!(app.tab(a).unwrap().document().undo().wait().unwrap().value);
    assert_eq!(objects(&app, a).len(), 1);
    assert_eq!(objects(&app, b).len(), 1);
    assert!(app.tab(b).unwrap().document().can_undo());

    // Views are opaque state kept per tab.
    app.tab_mut(a).unwrap().views_mut().push(Box::new(42_i32));
    assert_eq!(app.tab(a).unwrap().views().len(), 1);
    assert!(app.tab(b).unwrap().views().is_empty());

    // Reorder and activate.
    app.move_tab(b, 0).unwrap();
    assert_eq!(app.tabs()[0].id(), b);
    app.activate(a).unwrap();
    assert_eq!(app.active(), Some(a));

    // Dirty + cancel: stays open.
    assert_eq!(
        app.close(a, |_| CloseAnswer::Cancel).unwrap(),
        Closed::Cancelled
    );
    // Dirty + save without a path: error, stays open and dirty.
    let error = app.close(a, |_| CloseAnswer::Save).unwrap_err();
    assert_eq!(error.code(), "app.document.no_path");
    assert!(app.tab(a).unwrap().is_dirty());
    // Dirty + save with a path: saved, then closed.
    let path = dir.join("b.arc");
    app.save_as(b, &path).unwrap();
    add_point(&app, b, 4.0).unwrap();
    assert_eq!(app.close(b, |_| CloseAnswer::Save).unwrap(), Closed::Closed);
    assert!(!lock_path(&path).exists());
    // Dirty + discard: closed, nothing left to recover.
    assert_eq!(
        app.close(a, |_| CloseAnswer::Discard).unwrap(),
        Closed::Closed
    );
    assert!(app.recoverable().unwrap().is_empty());
    assert_eq!(app.active(), None);
    // Clean: closes without asking.
    let c = app.open(&path).unwrap();
    assert_eq!(objects(&app, c).len(), 2);
    let closed = app
        .close(c, |_| panic!("a clean tab is not asked about"))
        .unwrap();
    assert_eq!(closed, Closed::Closed);
    assert_eq!(
        app.close(c, |_| CloseAnswer::Cancel).unwrap_err().code(),
        "app.document.unknown"
    );
}

#[test]
fn open_save_save_as_and_reopen_equal_the_document() {
    let dir = TempDir::new("save");
    let first = dir.join("first.arc");
    let second = dir.join("second.arc");
    new_package(&first);
    let mut app = app_in(&dir.join("data"), "a", 2);

    assert_eq!(
        app.open(&dir.join("notes.txt")).unwrap_err().code(),
        "app.file.unsupported_type"
    );
    let tab = app.open(&first).unwrap();
    assert_eq!(
        app.open(&first).unwrap(),
        tab,
        "an open path activates its tab"
    );
    assert_eq!(app.tab(tab).unwrap().title().as_deref(), Some("first.arc"));
    add_point(&app, tab, 1.0).unwrap();
    app.save(tab).unwrap();
    assert!(!app.tab(tab).unwrap().is_dirty());
    add_point(&app, tab, 2.0).unwrap();

    app.save_as(tab, &second).unwrap();
    assert_eq!(app.tab(tab).unwrap().path(), Some(second.as_path()));
    assert!(!app.tab(tab).unwrap().is_dirty());
    assert!(
        !lock_path(&first).exists(),
        "the lock moved to the new file"
    );
    assert!(lock_path(&second).exists());
    let recent: Vec<_> = app.recent().iter().map(|e| e.path.clone()).collect();
    assert_eq!(recent, vec![second.clone(), first.clone()]);
    let expected = objects(&app, tab);
    assert_eq!(expected.len(), 2);
    drop(app);

    let mut again = app_in(&dir.join("data"), "a", 3);
    let reopened = again.open(&second).unwrap();
    assert_eq!(objects(&again, reopened), expected);
    let old = again.open(&first).unwrap();
    assert_eq!(objects(&again, old).len(), 1);
}

#[test]
fn failed_save_keeps_the_document_open_and_dirty() {
    let dir = TempDir::new("fail");
    let mut app = app_in(&dir.join("data"), "a", 4);
    let tab = app.new_document().unwrap();
    add_point(&app, tab, 1.0).unwrap();
    // The target's directory does not exist: the save fails and creates nothing.
    let target = dir.join("missing").join("x.arc");
    assert!(app.save_as(tab, &target).is_err());
    assert!(!target.exists());
    let tab = app.tab(tab).unwrap();
    assert!(tab.is_dirty() && tab.path().is_none());
    assert_eq!(app.recent().len(), 0);
}

#[test]
fn second_instance_opens_read_only_with_the_lock_holder_in_the_banner() {
    let dir = TempDir::new("lock");
    let path = dir.join("shared.arc");
    new_package(&path);
    let mut first = app_in(&dir.join("data1"), "first-user", 5);
    let mine = first.open(&path).unwrap();
    assert!(!first.tab(mine).unwrap().is_read_only());

    let mut second = app_in(&dir.join("data2"), "second-user", 6);
    let theirs = second.open(&path).unwrap();
    let tab = second.tab(theirs).unwrap();
    assert!(tab.is_read_only());
    let banner = &tab.banners()[0];
    assert_eq!(banner.message_id, "app-banner-locked");
    assert_eq!(banner.arguments["user"], "first-user".into());
    assert_eq!(banner.arguments["host"], "test-host".into());
    assert_eq!(
        add_point(&second, theirs, 1.0).unwrap_err().code(),
        "cmd.permission_denied"
    );
    assert_eq!(
        second.save(theirs).unwrap_err().code(),
        "app.document.read_only"
    );

    // Save as moves the read-only document to a file of its own, editable.
    let copy = dir.join("copy.arc");
    second.save_as(theirs, &copy).unwrap();
    let tab = second.tab(theirs).unwrap();
    assert!(!tab.is_read_only() && tab.banners().is_empty());
    add_point(&second, theirs, 1.0).unwrap();
}

#[test]
fn newer_major_format_is_refused_with_the_writing_version() {
    let dir = TempDir::new("newer");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/packages/newer-major.arc");
    let path = dir.join("newer-major.arc");
    std::fs::copy(fixture, &path).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut app = app_in(&dir.join("data"), "a", 7);
    let error = app.open(&path).unwrap_err();
    assert_eq!(error.code(), "doc.package.newer_format");
    assert!(error.arguments().contains_key("version"));
    assert!(app.tabs().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!lock_path(&path).exists());
}

#[test]
fn recovery_lists_recovers_and_discards_sessions_of_a_dropped_application() {
    let dir = TempDir::new("recovery");
    let data = dir.join("data");
    let path = dir.join("doc.arc");
    new_package(&path);

    // A dropped application leaves its unsaved changes behind.
    let mut crashed = app_in(&data, "a", 8);
    let tab = crashed.open(&path).unwrap();
    assert!(crashed.tab(tab).unwrap().is_protected());
    add_point(&crashed, tab, 1.0).unwrap();
    add_point(&crashed, tab, 2.0).unwrap();
    let expected = objects(&crashed, tab);
    let untitled = crashed.new_document().unwrap();
    add_point(&crashed, untitled, 9.0).unwrap();
    drop(crashed);

    let mut app = app_in(&data, "a", 9);
    let orphans = app.recoverable().unwrap();
    assert_eq!(orphans.len(), 2);
    let based = orphans.iter().find(|o| o.base.is_some()).unwrap();
    assert_eq!(based.records, 2);
    assert!(based.document.is_some() && based.last_unix_ms.is_some());
    let comparison = app.compare(based);
    assert_eq!(comparison.base.as_deref(), Some(path.as_path()));
    assert!(comparison.base_unchanged);
    assert_eq!((comparison.base_objects, comparison.records), (Some(0), 2));

    let recovered = app.recover(based).unwrap();
    let tab = app.tab(recovered).unwrap();
    assert!(tab.is_dirty() && !tab.is_read_only());
    assert_eq!(tab.path(), Some(path.as_path()));
    assert_eq!(objects(&app, recovered), expected);
    app.save(recovered).unwrap();
    assert!(!app.tab(recovered).unwrap().is_dirty());

    let other = app.recoverable().unwrap();
    assert_eq!(other.len(), 1);
    assert!(other[0].base.is_none());
    assert_eq!(app.compare(&other[0]).base_objects, None);
    app.discard_recovery(&other[0]).unwrap();
    assert!(app.recoverable().unwrap().is_empty());
    assert_eq!(
        app.close(recovered, |_| CloseAnswer::Cancel).unwrap(),
        Closed::Closed
    );
    drop(app);
    let reopened = app_in(&data, "a", 10);
    assert!(reopened.recoverable().unwrap().is_empty());
}

fn row(app: &Application, key: &str) -> settings_editor::Row {
    settings_editor::rows(app.settings())
        .into_iter()
        .find(|r| r.key == key)
        .unwrap()
}

#[test]
fn settings_editor_lists_searches_sets_and_resets() {
    let dir = TempDir::new("settings");
    let user_file = dir.join("user.toml");
    let mut app = app_in(&dir.join("data"), "a", 11);
    let mut catalogs = Catalogs::new();
    catalogs
        .add("en-US", include_str!("../../../locales/en-US/arc-app.ftl"))
        .unwrap();
    catalogs
        .add(
            "en-US",
            "shell-setting-grid-visible = Whether the drawing grid is shown.\n",
        )
        .unwrap();
    let localizer = Localizer::new(catalogs, &[]).unwrap();

    let rows = settings_editor::rows(app.settings());
    assert_eq!(rows.len(), 6);
    assert!(rows.iter().all(|r| r.source == Scope::Default));

    let keys = |q: &str| -> Vec<&str> {
        settings_editor::search(app.settings(), &localizer, q)
            .iter()
            .map(|r| r.key)
            .collect()
    };
    assert_eq!(keys("LOG.MAX"), vec!["log.max_file_bytes", "log.max_files"]);
    assert_eq!(keys("drawing GRID"), vec!["shell.grid.visible"]);
    assert_eq!(keys("  ").len(), 6);
    assert!(keys("no such words").is_empty());

    let store = app.settings_mut();
    settings_editor::set(
        store,
        Scope::User,
        "log.max_files",
        SettingValue::Int(9),
        Some(&user_file),
    )
    .unwrap();
    settings_editor::set(
        store,
        Scope::Document,
        "shell.grid.visible",
        SettingValue::Bool(false),
        None,
    )
    .unwrap();
    settings_editor::set(
        store,
        Scope::User,
        "shell.grid.visible",
        SettingValue::Bool(false),
        Some(&user_file),
    )
    .unwrap();
    let saved = arc_app::settings_file::load(&user_file).unwrap().table;
    assert_eq!(saved["log"]["max_files"].as_integer(), Some(9));

    assert_eq!(row(&app, "shell.grid.visible").source, Scope::Document);
    let store = app.settings_mut();
    assert!(settings_editor::reset(store, Scope::Document, "shell.grid.visible", None).unwrap());
    assert_eq!(row(&app, "shell.grid.visible").source, Scope::User);
    let store = app.settings_mut();
    assert!(settings_editor::reset(store, Scope::User, "log.max_files", Some(&user_file)).unwrap());
    assert!(
        !settings_editor::reset(
            app.settings_mut(),
            Scope::User,
            "log.max_files",
            Some(&user_file)
        )
        .unwrap()
    );
    let r = row(&app, "log.max_files");
    assert_eq!((r.value, r.source), (SettingValue::Int(5), Scope::Default));
    let saved = arc_app::settings_file::load(&user_file).unwrap().table;
    assert!(saved.get("log").is_none());
    assert_eq!(
        settings_editor::reset(app.settings_mut(), Scope::User, "nope", None)
            .unwrap_err()
            .code(),
        "app.settings.unknown_key"
    );
}

/// A product whose new-document command creates one layer, as a drafting product's creates the
/// first space.
fn product_with_start() -> Product {
    let mut commands = arc_shell_sample::registry().unwrap();
    commands
        .register(arc_command::CommandSpec::new(
            "test.start.make",
            "test-start-make",
            "test-start-make-description",
            |ctx, _: arc_app::commands::NoParams| -> arc_base::Result<()> {
                let id: arc_base::ObjectId = ctx.new_id();
                ctx.txn()?.create(
                    arc_shell_sample::LAYER,
                    id,
                    arc_doc::Header::default(),
                    vec![Some(arc_doc::Value::String("Start".into()))],
                )?;
                Ok(())
            },
        ))
        .unwrap();
    let mut builder = ProductBuilder::new("arc-shell", "0.1.0")
        .with_types(arc_shell_sample::types().unwrap())
        .with_commands(commands);
    builder
        .file_type(FileType {
            extension: "arc".into(),
            message_id: "shell-file-arc".into(),
            open: true,
            save: true,
        })
        .unwrap();
    builder.new_document_command("test.start.make");
    builder.build(Arc::new(FixedClock(0)), owner("a")).unwrap()
}

#[test]
fn a_new_document_starts_with_its_command_content_which_is_no_undo_step_and_saves() {
    let dir = TempDir::new("start-content");
    let config = |seed: u64| {
        let next = AtomicU64::new(seed << 32);
        AppConfig {
            data_dir: dir.join("data"),
            limits: Limits::DEFAULT,
            id_generators: Box::new(move || {
                IdGenerator::seeded(next.fetch_add(1, Ordering::Relaxed))
            }),
            recent: RecentFiles::new(20),
        }
    };
    let mut app = Application::new(product_with_start(), config(1));
    let tab = app.new_document().unwrap();
    assert_eq!(app.tab(tab).unwrap().document().snapshot().len(), 1);
    assert!(
        !app.tab(tab).unwrap().is_dirty(),
        "the content is the start"
    );
    assert!(!app.undo(tab).unwrap(), "nothing to undo: it is no step");
    assert_eq!(app.tab(tab).unwrap().document().snapshot().len(), 1);
    // Saving used to fail with `doc.package.last_txn_missing`: the objects had no transaction.
    let file = dir.join("start.arc");
    app.save_as(tab, &file).unwrap();
    assert!(!app.tab(tab).unwrap().is_dirty());
    app.close(tab, |_| CloseAnswer::Discard).unwrap();
    let mut again = Application::new(product_with_start(), config(2));
    let reopened = again.open(&file).unwrap();
    assert_eq!(again.tab(reopened).unwrap().document().snapshot().len(), 1);
}

#[test]
fn an_edit_undone_back_to_the_saved_state_is_not_dirty_and_a_redo_is() {
    let dir = TempDir::new("undo-clean");
    let mut app = app_in(&dir.join("data"), "a", 3);
    let tab = app.new_document().unwrap();
    add_point(&app, tab, 1.0).unwrap();
    let path = dir.join("saved.arc");
    app.save_as(tab, &path).unwrap();
    assert!(!app.tab(tab).unwrap().is_dirty());
    add_point(&app, tab, 2.0).unwrap();
    assert!(app.tab(tab).unwrap().is_dirty());
    // Undone back to what was saved: the revision moved on, the document is as saved.
    assert!(
        app.tab(tab)
            .unwrap()
            .document()
            .undo()
            .wait()
            .unwrap()
            .value
    );
    assert!(!app.tab(tab).unwrap().is_dirty());
    // Undone past the save: dirty.
    assert!(
        app.tab(tab)
            .unwrap()
            .document()
            .undo()
            .wait()
            .unwrap()
            .value
    );
    assert!(app.tab(tab).unwrap().is_dirty());
    assert!(
        app.tab(tab)
            .unwrap()
            .document()
            .redo()
            .wait()
            .unwrap()
            .value
    );
    assert!(
        app.tab(tab)
            .unwrap()
            .document()
            .redo()
            .wait()
            .unwrap()
            .value
    );
    assert!(app.tab(tab).unwrap().is_dirty());
}
