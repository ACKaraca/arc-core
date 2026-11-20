//! Menus and dialogs of the windowed shell through fakes (CORE-APP-009): a menu activation
//! emits the command intent, the open, save and close flows go through the dialog trait into
//! the application model, and panel factories of the wrong type are reported.

#![cfg(feature = "window")]
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arc_app::application::{AppConfig, Application, CloseAnswer, Closed, TabId};
use arc_app::commands;
use arc_app::product::{Entry, FileType, ProductBuilder};
use arc_app::recent::RecentFiles;
use arc_app::shell::platform::{self, Done, FileDialogs, NativeMenu};
use arc_app::shell::{
    self, CommandLinePanel, FrameInput, Request, Shell, ShellConfig, ViewportPanel,
};
use arc_base::error::Value;
use arc_base::{IdGenerator, Limits};
use arc_command::prompt::{Accept, PointDims, Prompt};
use arc_command::{FixedClock, Permission, Session, Surface};
use arc_doc::package::{LockOwner, Metadata, Package};
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    Dock, DockLayout, DynamicEvent, Entry as MenuEntry, Intent, LineAction, LineStatus, Localize,
    OptionControl, PointerEvent, PointerKind, PromptControls,
};

struct Names;
impl Localize for Names {
    fn message(&self, id: &str, _: &[(&str, Value)]) -> Option<String> {
        Some(
            match id {
                "app-menu-file" => "File",
                "app-menu-edit" => "Edit",
                "app-command-file-new" => "New",
                "app-command-file-open" => "Open",
                _ => return None,
            }
            .to_owned(),
        )
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("arc-app-shell-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(data: &Path, extra_panel: bool) -> Application {
    app_with(data, extra_panel, None)
}

/// The application, with a fake interaction controller that records into `seen` when given.
fn app_with(data: &Path, extra_panel: bool, seen: Option<Arc<Mutex<Seen>>>) -> Application {
    let mut b = ProductBuilder::new("arc-shell", "0.1.0")
        .with_types(arc_shell_sample::types().unwrap())
        .with_commands(arc_shell_sample::registry().unwrap());
    commands::register(b.commands_mut()).unwrap();
    b.file_type(FileType {
        extension: "arc".into(),
        message_id: "shell-file-arc".into(),
        open: true,
        save: true,
    })
    .unwrap();
    b.panel(Entry {
        id: "test.viewport".into(),
        message_id: "test-viewport".into(),
        factory: Arc::new(|| shell::panel(ViewportPanel::new())),
    })
    .unwrap();
    b.panel(Entry {
        id: "test.line".into(),
        message_id: "test-line".into(),
        factory: Arc::new(|| shell::panel(CommandLinePanel)),
    })
    .unwrap();
    if extra_panel {
        b.panel(Entry {
            id: "test.wrong".into(),
            message_id: "test-wrong".into(),
            factory: Arc::new(|| 7_u32),
        })
        .unwrap();
    }
    if let Some(seen) = seen {
        b.interaction(move || Box::new(FakeController { seen: seen.clone() }));
    }
    let owner = LockOwner {
        user: "tester".into(),
        host: "test-host".into(),
        application: "arc-shell".into(),
        time_unix_ms: 0,
    };
    let product = b.build(Arc::new(FixedClock(0)), owner).unwrap();
    let next = AtomicU64::new(7 << 32);
    Application::new(
        product,
        AppConfig {
            data_dir: data.to_path_buf(),
            limits: Limits::DEFAULT,
            id_generators: Box::new(move || {
                IdGenerator::seeded(next.fetch_add(1, Ordering::Relaxed))
            }),
            recent: RecentFiles::new(20),
        },
    )
}

fn config() -> ShellConfig {
    let root = Dock::Split {
        axis: arc_ui::Axis::Vertical,
        fraction: 0.8,
        first: Box::new(Dock::Tabs {
            tabs: vec!["test.viewport".into()],
            active: 0,
        }),
        second: Box::new(Dock::Tabs {
            tabs: vec!["test.line".into()],
            active: 0,
        }),
    };
    ShellConfig {
        layout: DockLayout::new(root, Vec::new()).unwrap(),
        menu: shell::default_menu().unwrap(),
        bindings: Vec::new(),
        appearance: arc_ui::Appearance::default(),
        seed: 1,
    }
}

#[derive(Default)]
struct FakeDialogs {
    open: Option<PathBuf>,
    save: Option<PathBuf>,
    close: Vec<CloseAnswer>,
    asked: Vec<String>,
}

impl FileDialogs for FakeDialogs {
    fn pick_open(&mut self, types: &[FileType]) -> Option<PathBuf> {
        self.asked.push(format!("open {}", types.len()));
        self.open.clone()
    }

    fn pick_save(&mut self, _: &[FileType], _: Option<&str>) -> Option<PathBuf> {
        self.asked.push("save".into());
        self.save.clone()
    }

    fn ask_close(&mut self, _: &str) -> CloseAnswer {
        self.asked.push("close".into());
        if self.close.is_empty() {
            CloseAnswer::Cancel
        } else {
            self.close.remove(0)
        }
    }
}

/// A native menu that records what it shows and activates items on request.
#[derive(Default)]
struct FakeMenu {
    shown: Vec<String>,
    activated: Vec<String>,
}

fn ids(entries: &[MenuEntry], out: &mut Vec<String>) {
    for e in entries {
        match e {
            MenuEntry::Command(view) => out.push(view.id.clone()),
            MenuEntry::Submenu { entries, .. } => ids(entries, out),
            MenuEntry::Separator => {}
        }
    }
}

impl NativeMenu for FakeMenu {
    fn install(&mut self, entries: &[MenuEntry]) -> bool {
        ids(entries, &mut self.shown);
        true
    }

    fn poll(&mut self) -> Vec<Intent> {
        self.activated.drain(..).map(Intent::new).collect()
    }
}

struct Run {
    requests: Vec<Request>,
    nodes: Vec<accesskit::Node>,
}

fn frame(
    shell: &mut Shell,
    app: &mut Application,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    menu_intents: Vec<Intent>,
    native_menu: bool,
) -> Run {
    let textures = BTreeMap::new();
    let mut out = shell.frame(
        app,
        ctx,
        FrameInput {
            raw: egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            time: 1.0,
            localize: &Names,
            language: "en",
            textures: &textures,
            menu_intents,
            native_menu,
        },
    );
    out.full.textures_delta.clear();
    let nodes = out
        .full
        .platform_output
        .accesskit_update
        .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
        .unwrap_or_default();
    Run {
        requests: out.requests,
        nodes,
    }
}

fn click(node: &accesskit::Node) -> Vec<egui::Event> {
    let b = node.bounds().unwrap();
    let at = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), button(true), button(false)]
}

fn button<'a>(nodes: &'a [accesskit::Node], name: &str) -> &'a accesskit::Node {
    nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::Button && n.label() == Some(name))
        .unwrap_or_else(|| panic!("no button {name}"))
}

#[test]
fn native_menu_activation_emits_the_command_intent() {
    let dir = TempDir::new("native-menu");
    let mut app = app(&dir.0, false);
    let mut shell = Shell::new(&app, config(), None);
    let mut menu = FakeMenu::default();
    let entries = shell.menu_entries(&app, &Names);
    assert!(menu.install(&entries));
    assert!(menu.shown.contains(&commands::FILE_NEW.to_owned()));
    assert_eq!(menu.shown.len(), 9, "{:?}", menu.shown);

    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    menu.activated.push(commands::FILE_NEW.into());
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), menu.poll(), true);
    assert_eq!(app.tabs().len(), 1);
    assert!(run.requests.is_empty());
    // With a native menu bar, the egui menu bar is not drawn.
    assert!(run.nodes.iter().all(|n| n.label() != Some("File")));
}

#[test]
fn egui_menu_bar_activation_emits_the_command_intent() {
    let dir = TempDir::new("egui-menu");
    let mut app = app(&dir.0, false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    let file = button(&run.nodes, "File");
    frame(&mut shell, &mut app, &ctx, click(file), Vec::new(), false);
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    let new = button(&run.nodes, "New");
    frame(&mut shell, &mut app, &ctx, click(new), Vec::new(), false);
    assert_eq!(app.tabs().len(), 1);
}

#[test]
fn open_save_and_close_go_through_the_dialogs_into_the_application() {
    let dir = TempDir::new("dialogs");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    let mut dialogs = FakeDialogs::default();
    let step = |shell: &mut Shell, app: &mut Application, command: &str| {
        frame(
            shell,
            app,
            &ctx,
            Vec::new(),
            vec![Intent::new(command)],
            false,
        )
        .requests
    };

    // Open without a path asks the dialog; a cancelled dialog changes nothing.
    assert_eq!(
        step(&mut shell, &mut app, commands::FILE_OPEN),
        [Request::OpenDialog]
    );
    let done =
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &Request::OpenDialog).unwrap();
    assert_eq!(done, Done::Cancelled);
    assert!(app.tabs().is_empty());

    // A new document saved through the save dialog, then opened through the open dialog.
    step(&mut shell, &mut app, commands::FILE_NEW);
    let tab = app.active().unwrap();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    let mut create = arc_command::Invocation::new(
        "shell.point.create",
        serde_json::json!({"position": [1.0, 2.0, 3.0]}),
    );
    create.seed = 1;
    app.invoke(tab, &session, create).unwrap();
    let requests = step(&mut shell, &mut app, commands::FILE_SAVE);
    assert_eq!(requests, [Request::SaveDialog(tab)]);
    let file = dir.0.join("saved.arc");
    dialogs.save = Some(file.clone());
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &requests[0]).unwrap(),
        Done::Saved
    );
    assert_eq!(app.tab(tab).unwrap().path(), Some(file.as_path()));

    // Closing a clean tab asks nothing; the open dialog then opens the file.
    assert!(step(&mut shell, &mut app, commands::FILE_CLOSE).is_empty());
    assert!(app.tabs().is_empty());
    dialogs.open = Some(file.clone());
    let done =
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &Request::OpenDialog).unwrap();
    let Done::Opened(opened) = done else {
        panic!("{done:?}")
    };
    assert_eq!(app.tab(opened).unwrap().document().snapshot().len(), 1);
    assert_eq!(dialogs.asked, ["open 1", "save", "open 1"]);

    // A dirty tab asks before closing: cancel keeps it, discard closes it.
    // Another seed: the reopened document starts at the revision the first point was made at.
    let mut second = arc_command::Invocation::new(
        "shell.point.create",
        serde_json::json!({"position": [0.0, 0.0, 0.0]}),
    );
    second.seed = 2;
    app.invoke(opened, &session, second).unwrap();
    let requests = step(&mut shell, &mut app, commands::FILE_CLOSE);
    assert_eq!(requests, [Request::AskClose(opened)]);
    dialogs.close = vec![CloseAnswer::Cancel];
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &requests[0]).unwrap(),
        Done::Closed(Closed::Cancelled)
    );
    assert_eq!(app.tabs().len(), 1);
    // Quit asks for the dirty tab; discarding ends the application.
    assert_eq!(
        step(&mut shell, &mut app, commands::APP_QUIT),
        [Request::Quit]
    );
    dialogs.close = vec![CloseAnswer::Discard];
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &Request::Quit).unwrap(),
        Done::Quit
    );
    assert!(app.tabs().is_empty());
    assert!(shell.errors().is_empty(), "{:?}", shell.errors());
}

#[test]
fn save_of_a_new_document_on_close_asks_for_a_file() {
    let dir = TempDir::new("close-save");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let tab = app.new_document().unwrap();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    app.invoke(
        tab,
        &session,
        arc_command::Invocation::new("shell.layer.create", serde_json::json!({"name": "a"})),
    )
    .unwrap();
    let mut dialogs = FakeDialogs {
        close: vec![CloseAnswer::Save],
        save: Some(dir.0.join("new.arc")),
        ..FakeDialogs::default()
    };
    let done =
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &Request::AskClose(tab)).unwrap();
    assert_eq!(done, Done::Closed(Closed::Closed));
    assert!(dir.0.join("new.arc").exists());
    assert_eq!(dialogs.asked, ["close", "save"]);
}

#[test]
fn factories_of_the_wrong_type_are_reported() {
    let dir = TempDir::new("factory");
    let app = app(&dir.0, true);
    let shell = Shell::new(&app, config(), None);
    assert_eq!(
        shell
            .errors()
            .iter()
            .map(arc_base::Error::code)
            .collect::<Vec<_>>(),
        ["app.product.invalid_id"]
    );
}

/// What the fake controller saw.
#[derive(Default)]
struct Seen {
    lines: Vec<String>,
    clicks: usize,
    /// For each pointer event: whether the tab is read-only and whether the session has `edit`.
    access: Vec<(bool, bool)>,
    /// For each `closing` call: the tab and whether its document was still open.
    closings: Vec<(TabId, bool)>,
}

/// A controller that keeps a prompt, one option and one history line, consumes viewport clicks
/// and records submitted lines.
struct FakeController {
    seen: Arc<Mutex<Seen>>,
}

impl shell::Interaction for FakeController {
    fn pointer(&mut self, cx: &shell::InteractionContext<'_>, event: &PointerEvent) -> bool {
        let mut seen = self.seen.lock().unwrap();
        let edit = cx.session().permissions.contains(&Permission::Edit);
        seen.access.push((cx.read_only(), edit));
        if event.kind == PointerKind::Click {
            seen.clicks += 1;
            return true;
        }
        false
    }

    fn dynamic(&mut self, _: &shell::InteractionContext<'_>, _: &DynamicEvent) {}

    fn closing(&mut self, cx: &shell::InteractionContext<'_>) {
        let open = cx.document().is_ok();
        self.seen.lock().unwrap().closings.push((cx.tab(), open));
    }

    fn line(
        &mut self,
        _: &shell::InteractionContext<'_>,
        action: LineAction,
    ) -> arc_base::Result<()> {
        if let LineAction::Submit(text) = action {
            self.seen.lock().unwrap().lines.push(text);
        }
        Ok(())
    }

    fn output(&self) -> shell::InteractionOutput {
        shell::InteractionOutput {
            status: LineStatus {
                active: true,
                answers: 0,
                has_selection: false,
                has_last: false,
            },
            prompt: Some(Prompt::new(
                "point",
                "test-prompt",
                Accept::Point {
                    dims: PointDims::Two,
                },
            )),
            options: PromptControls {
                options: vec![OptionControl {
                    id: "yes".into(),
                    label: "test-option-yes".into(),
                    shortcut: Some('y'),
                }],
                parameters: Vec::new(),
            },
            history: vec!["test-history-line".into()],
            ..shell::InteractionOutput::default()
        }
    }
}

fn open_new_document(shell: &mut Shell, app: &mut Application, ctx: &egui::Context) {
    let run = frame(shell, app, ctx, Vec::new(), Vec::new(), false);
    let file = button(&run.nodes, "File");
    frame(shell, app, ctx, click(file), Vec::new(), false);
    let run = frame(shell, app, ctx, Vec::new(), Vec::new(), false);
    let new = button(&run.nodes, "New");
    frame(shell, app, ctx, click(new), Vec::new(), false);
}

fn text_input(nodes: &[accesskit::Node]) -> &accesskit::Node {
    nodes
        .iter()
        .find(|n| n.role() == accesskit::Role::TextInput)
        .unwrap_or_else(|| panic!("no text input"))
}

fn shows(nodes: &[accesskit::Node], text: &str) -> bool {
    nodes
        .iter()
        .any(|n| n.label() == Some(text) || n.value() == Some(text))
}

#[test]
fn a_registered_controller_owns_the_command_panel_option_bar_and_viewport_clicks() {
    let dir = TempDir::new("controller");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut app = app_with(&dir.0, false, Some(seen.clone()));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);

    // The controller's prompt, history and option are what the panel and the option bar show.
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    assert!(shows(&run.nodes, "test-prompt"));
    assert!(shows(&run.nodes, "test-history-line"));
    let option = button(&run.nodes, "test-option-yes (y)");

    // A click in the viewport goes to the controller, which consumes it.
    let at = egui::pos2(200.0, 200.0);
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let click_viewport = vec![egui::Event::PointerMoved(at), press(true), press(false)];
    frame(
        &mut shell,
        &mut app,
        &ctx,
        click_viewport,
        Vec::new(),
        false,
    );
    assert_eq!(seen.lock().unwrap().clicks, 1);

    // The option button submits its identifier to the controller.
    frame(&mut shell, &mut app, &ctx, click(option), Vec::new(), false);
    assert_eq!(seen.lock().unwrap().lines, ["yes"]);

    // A typed line is submitted by Enter, in the frame after the text was typed.
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    frame(
        &mut shell,
        &mut app,
        &ctx,
        click(text_input(&run.nodes)),
        Vec::new(),
        false,
    );
    let typed = vec![egui::Event::Text("hello".into())];
    frame(&mut shell, &mut app, &ctx, typed, Vec::new(), false);
    let enter = egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut shell, &mut app, &ctx, vec![enter], Vec::new(), false);
    assert_eq!(seen.lock().unwrap().lines, ["yes", "hello"]);
    assert_eq!(seen.lock().unwrap().clicks, 1);
}

fn owner(user: &str) -> LockOwner {
    LockOwner {
        user: user.into(),
        host: "test-host".into(),
        application: "arc-shell".into(),
        time_unix_ms: 0,
    }
}

#[test]
fn a_controller_sees_the_read_only_state_of_its_tab_and_no_edit_permission_on_it() {
    let dir = TempDir::new("read-only-controller");
    let path = dir.0.join("shared.arc");
    drop(Package::create(&path, &Metadata::default(), &owner("holder")).unwrap());
    // The holder opens the package first, so the second application opens it read-only.
    let holder_seen = Arc::new(Mutex::new(Seen::default()));
    let mut holder = app_with(&dir.0.join("holder"), false, Some(holder_seen.clone()));
    holder.open(&path).unwrap();
    let reader_seen = Arc::new(Mutex::new(Seen::default()));
    let mut reader = app_with(&dir.0.join("reader"), false, Some(reader_seen.clone()));
    reader.open(&path).unwrap();
    assert!(
        reader
            .active()
            .is_some_and(|t| reader.tab(t).unwrap().is_read_only())
    );

    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let at = egui::pos2(200.0, 200.0);
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let click_viewport = || vec![egui::Event::PointerMoved(at), press(true), press(false)];

    let mut holder_shell = Shell::new(&holder, config(), None);
    frame(
        &mut holder_shell,
        &mut holder,
        &ctx,
        click_viewport(),
        Vec::new(),
        false,
    );
    let mut reader_shell = Shell::new(&reader, config(), None);
    frame(
        &mut reader_shell,
        &mut reader,
        &ctx,
        click_viewport(),
        Vec::new(),
        false,
    );

    let holder_access = holder_seen.lock().unwrap().access.clone();
    let reader_access = reader_seen.lock().unwrap().access.clone();
    assert!(!holder_access.is_empty() && !reader_access.is_empty());
    // An editable tab: not read-only, and the session keeps `edit`.
    assert!(holder_access.iter().all(|a| *a == (false, true)));
    // A read-only tab: read-only, and the session has no `edit`.
    assert!(reader_access.iter().all(|a| *a == (true, false)));
}

#[test]
fn without_a_controller_the_shell_shows_its_own_command_line() {
    let dir = TempDir::new("no-controller");
    let mut app = app(&dir.0, false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    assert!(!shows(&run.nodes, "test-prompt"));
    assert!(!shows(&run.nodes, "test-option-yes (y)"));
}

#[test]
fn closing_reaches_the_controller_once_while_its_document_is_open() {
    let dir = TempDir::new("closing");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut app = app_with(&dir.0.join("data"), false, Some(Arc::clone(&seen)));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    let mut dialogs = FakeDialogs::default();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    // A command frame, then an idle frame: the view, and with it the controller, of a new tab
    // is made by the frame after the one that opened it.
    let run = |shell: &mut Shell, app: &mut Application, command: &str| {
        let requests = frame(
            shell,
            app,
            &ctx,
            Vec::new(),
            vec![Intent::new(command)],
            false,
        )
        .requests;
        frame(shell, app, &ctx, Vec::new(), Vec::new(), false);
        requests
    };

    // A dirty tab asks first; a cancelled close does not reach the controller.
    run(&mut shell, &mut app, commands::FILE_NEW);
    let dirty = app.active().unwrap();
    let mut create = arc_command::Invocation::new(
        "shell.point.create",
        serde_json::json!({"position": [1.0, 2.0, 3.0]}),
    );
    create.seed = 1;
    app.invoke(dirty, &session, create).unwrap();
    dialogs.close = vec![CloseAnswer::Cancel];
    let request = Request::AskClose(dirty);
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &request).unwrap(),
        Done::Closed(Closed::Cancelled)
    );
    assert!(seen.lock().unwrap().closings.is_empty());

    // Discarding closes it: the controller is told once, with the document still open.
    dialogs.close = vec![CloseAnswer::Discard];
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &request).unwrap(),
        Done::Closed(Closed::Closed)
    );
    assert_eq!(seen.lock().unwrap().closings, [(dirty, true)]);
    assert!(app.tab(dirty).is_err());

    // A clean tab closed by the close command, then quitting with a clean tab left.
    run(&mut shell, &mut app, commands::FILE_NEW);
    let clean = app.active().unwrap();
    assert!(run(&mut shell, &mut app, commands::FILE_CLOSE).is_empty());
    run(&mut shell, &mut app, commands::FILE_NEW);
    let quit = app.active().unwrap();
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &Request::Quit).unwrap(),
        Done::Quit
    );
    assert_eq!(
        seen.lock().unwrap().closings,
        [(dirty, true), (clean, true), (quit, true)]
    );
    assert!(app.tabs().is_empty());
}
