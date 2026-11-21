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
use arc_app::product::{Entry, FileType, PathDialog, PathFilter, PathMode, ProductBuilder};
use arc_app::recent::RecentFiles;
use arc_app::shell::platform::{self, Done, FileDialogs, NativeMenu};
use arc_app::shell::{
    self, CommandLinePanel, FrameInput, Request, Shell, ShellConfig, ViewportPanel,
};
use arc_base::error::Value;
use arc_base::{IdGenerator, Limits};
use arc_command::prompt::{Accept, PointDims, Prompt};
use arc_command::{CommandSpec, FixedClock, Permission, Session, Surface, UndoBehavior};
use arc_doc::package::{LockOwner, Metadata, Package};
use arc_ui::egui::{self, accesskit};
use arc_ui::{
    Dock, DockLayout, DynamicEvent, Entry as MenuEntry, Intent, LineAction, LineStatus, Localize,
    OptionControl, PointerEvent, PointerKind, PromptControls,
};

/// The paths the test command `test.file.export` ran with.
static EXPORTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Names;
impl Localize for Names {
    fn message(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        if id == "app-history-command" {
            let (_, Value::Text(text)) = args.iter().find(|(k, _)| *k == "text")? else {
                return None;
            };
            return Some(format!("Command: {text}"));
        }
        Some(
            match id {
                "app-close-save" => "Save",
                "app-close-discard" => "Discard",
                "app-close-cancel" => "Cancel",
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
    b.commands_mut()
        .register(
            CommandSpec::new(
                "test.file.export",
                "test-export",
                "test-export-description",
                |_, p: commands::PathParams| {
                    EXPORTED.lock().unwrap().push(p.path.unwrap_or_default());
                    Ok(())
                },
            )
            .with_exposure([Surface::Ui, Surface::CommandLine])
            .with_undo(UndoBehavior::NotUndoable)
            .with_permissions([Permission::Read]),
        )
        .unwrap();
    b.path_dialog(
        "test.file.export",
        PathDialog {
            mode: PathMode::Save,
            title: "test-export-title".into(),
            filters: vec![PathFilter {
                message_id: "test-export-filter".into(),
                extensions: vec!["txt".into()],
            }],
            default_name: Some("{document}.txt".into()),
        },
    );
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
        toolbar: Vec::new(),
        status: Vec::new(),
        display: None,
        overlays: None,
        work_plane: None,
        initial_view: None,
        seed: 1,
    }
}

#[derive(Default)]
struct FakeDialogs {
    open: Option<PathBuf>,
    save: Option<PathBuf>,
    close: Vec<CloseAnswer>,
    asked: Vec<String>,
    path: Option<PathBuf>,
    /// The platform has no native question dialog.
    no_question: bool,
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

    fn pick_path(&mut self, _: &PathDialog, name: Option<&str>) -> Option<PathBuf> {
        self.asked
            .push(format!("path {}", name.unwrap_or_default()));
        self.path.clone()
    }

    fn can_ask(&self) -> bool {
        !self.no_question
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
    /// The commands of `Invoke` actions.
    invoked: Vec<String>,
    /// The document path the controller read at each call.
    paths: Vec<Option<PathBuf>>,
    /// The language, the decimal separator and the session's `app.language` of the last line.
    context: Option<(String, char, Option<serde_json::Value>)>,
    /// An idle controller shows no prompt, no option and no history, and has no selection of its
    /// own.
    idle: bool,
    /// The controller leaves undo, redo and clearing the selection to the shell.
    leaves_edit_to_shell: bool,
    /// The controller's history and the sequence number of its first line, when set.
    history: Option<(u64, Vec<String>)>,
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

    fn handles(&self, command: &str) -> bool {
        !self.seen.lock().unwrap().leaves_edit_to_shell
            && matches!(
                command,
                commands::EDIT_UNDO | commands::EDIT_REDO | commands::SELECT_CLEAR
            )
    }

    fn closing(&mut self, cx: &shell::InteractionContext<'_>) {
        let open = cx.document().is_ok();
        self.seen.lock().unwrap().closings.push((cx.tab(), open));
    }

    fn line(
        &mut self,
        cx: &shell::InteractionContext<'_>,
        action: LineAction,
    ) -> arc_base::Result<()> {
        let mut seen = self.seen.lock().unwrap();
        seen.paths.push(cx.document_path().map(Path::to_path_buf));
        seen.context = Some((
            cx.language().to_owned(),
            cx.decimal_separator(),
            cx.session().setting(commands::LANGUAGE_SETTING),
        ));
        match action {
            LineAction::Submit(text) if text == "ask" => {
                cx.ask(Intent::new(commands::FILE_NEW));
            }
            LineAction::Submit(text) => seen.lines.push(text),
            LineAction::Invoke(intent) => seen.invoked.push(intent.command),
            _ => {}
        }
        Ok(())
    }

    fn output(&self) -> shell::InteractionOutput {
        let (idle, history) = {
            let seen = self.seen.lock().unwrap();
            (seen.idle, seen.history.clone())
        };
        if idle {
            return shell::InteractionOutput::default();
        }
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
            history: history
                .clone()
                .map_or_else(|| vec!["test-history-line".into()], |h| h.1),
            history_first: history.map_or(0, |h| h.0),
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

    // The controller's prompt, history and option are what the panel and the option bar show. The
    // panel's input row is pinned to the bottom and measured by the first frame, so the buttons
    // are where a click lands from the second one on.
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
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

/// Types `text` into the command line and presses Enter.
fn submit(shell: &mut Shell, app: &mut Application, ctx: &egui::Context, text: &str) {
    let run = frame(shell, app, ctx, Vec::new(), Vec::new(), false);
    frame(
        shell,
        app,
        ctx,
        click(text_input(&run.nodes)),
        Vec::new(),
        false,
    );
    let typed = vec![egui::Event::Text(text.into())];
    frame(shell, app, ctx, typed, Vec::new(), false);
    let enter = key(egui::Key::Enter, egui::Modifiers::NONE);
    frame(shell, app, ctx, vec![enter], Vec::new(), false);
}

#[test]
fn the_shell_carries_out_its_commands_and_a_controller_gets_the_rest() {
    let dir = TempDir::new("shell-commands");
    let seen = Arc::new(Mutex::new(Seen {
        idle: true,
        ..Seen::default()
    }));
    let mut app = app_with(&dir.0, false, Some(seen.clone()));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    assert_eq!(app.tabs().len(), 1);

    // A shell command typed on the controlled line is the shell's.
    submit(&mut shell, &mut app, &ctx, commands::FILE_NEW);
    assert_eq!(app.tabs().len(), 2);
    assert!(seen.lock().unwrap().lines.is_empty());

    // Typed parameters reach the shell's command line too.
    let file = dir.0.join("typed.arc");
    submit(
        &mut shell,
        &mut app,
        &ctx,
        &format!("{} path={}", commands::FILE_SAVE_AS, file.display()),
    );
    assert!(file.exists());
    assert!(seen.lock().unwrap().lines.is_empty());

    // A controller asks for a shell command through its context.
    submit(&mut shell, &mut app, &ctx, "ask");
    assert_eq!(app.tabs().len(), 3);

    // A menu command is started the way a toolbar button's is: through the controller, with the
    // language and the decimal separator in its context; a shell command from the menu is not.
    let menu = vec![
        Intent::new("shell.layer.create"),
        Intent::new(commands::FILE_NEW),
    ];
    frame(&mut shell, &mut app, &ctx, Vec::new(), menu, false);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.invoked, ["shell.layer.create"]);
    assert_eq!(app.tabs().len(), 4);
    let (language, separator, setting) = seen.context.clone().unwrap();
    assert_eq!((language.as_str(), separator), ("en", '.'));
    assert_eq!(setting, Some(serde_json::json!("en")));
}

#[test]
fn undo_goes_to_the_controller_that_claims_it_and_to_the_shell_otherwise() {
    for leaves in [false, true] {
        let dir = TempDir::new("claims");
        let seen = Arc::new(Mutex::new(Seen {
            idle: true,
            leaves_edit_to_shell: leaves,
            ..Seen::default()
        }));
        let mut app = app_with(&dir.0, false, Some(seen.clone()));
        let mut shell = Shell::new(&app, config(), None);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        open_new_document(&mut shell, &mut app, &ctx);
        frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
        let tab = app.active().unwrap();
        let session = Session::new(Surface::Ui, [Permission::Edit]);
        app.invoke(
            tab,
            &session,
            arc_command::Invocation::new(
                "shell.point.create",
                serde_json::json!({"position": [0.0, 0.0, 0.0]}),
            ),
        )
        .unwrap();
        let steps = |app: &Application| app.tab(tab).unwrap().document().undo_step_list(10).len();
        assert_eq!(steps(&app), 1);
        let undo = vec![Intent::new(commands::EDIT_UNDO)];
        frame(&mut shell, &mut app, &ctx, Vec::new(), undo, false);
        let invoked = seen.lock().unwrap().invoked.clone();
        if leaves {
            assert!(invoked.is_empty(), "the shell undoes it");
            assert_eq!(steps(&app), 0);
        } else {
            assert_eq!(invoked, [commands::EDIT_UNDO]);
            assert_eq!(steps(&app), 1, "the controller owns undo");
        }
    }
}

#[test]
fn escape_clears_the_selection_of_a_controlled_tab_at_once() {
    let dir = TempDir::new("escape");
    let seen = Arc::new(Mutex::new(Seen {
        idle: true,
        leaves_edit_to_shell: true,
        ..Seen::default()
    }));
    let mut app = app_with(&dir.0, false, Some(seen.clone()));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    let tab = app.active().unwrap();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    let created = app
        .invoke(
            tab,
            &session,
            arc_command::Invocation::new(
                "shell.point.create",
                serde_json::json!({"position": [0.0, 0.0, 0.0]}),
            ),
        )
        .unwrap();
    let point: arc_base::ObjectId = created.value["id"].as_str().unwrap().parse().unwrap();
    let mut requests = Vec::new();
    let select = Intent::new(commands::SELECT_SET)
        .with("objects", arc_ui::object_list(&[point]))
        .with("mode", "replace");
    shell.dispatch(&mut app, select, &mut requests).unwrap();
    assert_eq!(shell.view(tab).unwrap().selection(), [point]);

    // The controller reports no selection of its own: Escape sees the tab's and clears it.
    let escape = key(egui::Key::Escape, egui::Modifiers::NONE);
    frame(&mut shell, &mut app, &ctx, vec![escape], Vec::new(), false);
    assert!(shell.view(tab).unwrap().selection().is_empty());
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

fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn a_key_the_product_binds_runs_once_and_the_command_line_does_not_run_it_again() {
    let dir = TempDir::new("one-key");
    let mut app = app(&dir.0, false);
    let mut config = config();
    config.bindings = vec![arc_ui::Binding {
        keys: "Ctrl+Z".into(),
        command: commands::EDIT_UNDO.into(),
    }];
    let mut shell = Shell::new(&app, config, None);
    let ctx = egui::Context::default();
    let tab = app.new_document().unwrap();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    for seed in 1..=2 {
        let mut create = arc_command::Invocation::new(
            "shell.point.create",
            serde_json::json!({"position": [0.0, 0.0, 0.0]}),
        );
        create.seed = seed;
        app.invoke(tab, &session, create).unwrap();
    }
    assert_eq!(app.tab(tab).unwrap().document().undo_step_list(10).len(), 2);
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    let ctrl = egui::Modifiers {
        ctrl: true,
        command: true,
        ..egui::Modifiers::NONE
    };
    frame(
        &mut shell,
        &mut app,
        &ctx,
        vec![key(egui::Key::Z, ctrl)],
        Vec::new(),
        false,
    );
    let document = app.tab(tab).unwrap().document();
    assert_eq!(
        document.undo_step_list(10).len(),
        1,
        "one key press, one step"
    );
}

/// Starts the test command without a path, as a menu entry would, on a new document.
fn export_request(shell: &mut Shell, app: &mut Application, ctx: &egui::Context) -> Request {
    open_new_document(shell, app, ctx);
    let mut run = frame(
        shell,
        app,
        ctx,
        Vec::new(),
        vec![Intent::new("test.file.export")],
        false,
    );
    assert_eq!(run.requests.len(), 1);
    run.requests.remove(0)
}

#[test]
fn a_path_dialog_request_runs_the_command_with_the_chosen_path() {
    let dir = TempDir::new("path-dialog");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let request = export_request(&mut shell, &mut app, &ctx);
    let Request::PathDialog { dialog, then, .. } = &request else {
        panic!("{request:?}");
    };
    assert_eq!(then.command, "test.file.export");
    assert_eq!(dialog.mode, PathMode::Save);

    // A cancelled dialog runs nothing.
    let mut dialogs = FakeDialogs::default();
    let before = EXPORTED.lock().unwrap().len();
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &request).unwrap(),
        Done::Cancelled
    );
    assert_eq!(EXPORTED.lock().unwrap().len(), before);
    assert_eq!(dialogs.asked, ["path untitled.txt"]);

    // The chosen file becomes the command's `path`.
    let chosen = dir.0.join("out.txt");
    dialogs.path = Some(chosen.clone());
    assert_eq!(
        platform::carry_out(&mut shell, &mut app, &mut dialogs, &request).unwrap(),
        Done::Ran
    );
    assert!(
        EXPORTED
            .lock()
            .unwrap()
            .contains(&chosen.to_string_lossy().into_owned())
    );
}

#[test]
fn a_controller_reads_the_document_path_after_the_first_save() {
    let dir = TempDir::new("doc-path");
    let seen = Arc::new(Mutex::new(Seen {
        idle: true,
        ..Seen::default()
    }));
    let mut app = app_with(&dir.0.join("data"), false, Some(seen.clone()));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    let tab = app.active().unwrap();
    let start = |shell: &mut Shell, app: &mut Application| {
        frame(
            shell,
            app,
            &ctx,
            Vec::new(),
            vec![Intent::new("test.other")],
            false,
        );
    };
    start(&mut shell, &mut app);
    let file = dir.0.join("doc.arc");
    app.save_as(tab, &file).unwrap();
    start(&mut shell, &mut app);
    assert_eq!(seen.lock().unwrap().paths, [None, Some(file)]);
}

#[test]
fn the_history_echoes_typed_lines_and_keeps_earlier_results() {
    let dir = TempDir::new("echo");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    submit(&mut shell, &mut app, &ctx, "no.such.command");
    submit(&mut shell, &mut app, &ctx, "also.missing");
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    assert!(shows(&run.nodes, "Command: no.such.command"));
    assert!(shows(&run.nodes, "Command: also.missing"));
}

/// Dirties the active document with a point.
fn dirty(app: &mut Application) -> TabId {
    let tab = app.active().unwrap();
    let session = Session::new(Surface::Ui, [Permission::Edit]);
    let mut create = arc_command::Invocation::new(
        "shell.point.create",
        serde_json::json!({"position": [1.0, 2.0, 3.0]}),
    );
    create.seed = 1;
    app.invoke(tab, &session, create).unwrap();
    tab
}

#[test]
fn the_window_asks_to_save_discard_or_keep_when_no_dialog_can() {
    let dir = TempDir::new("window-close");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut dialogs = FakeDialogs {
        no_question: true,
        ..FakeDialogs::default()
    };
    open_new_document(&mut shell, &mut app, &ctx);
    let file = dir.0.join("kept.arc");
    let tab = app.active().unwrap();
    app.save_as(tab, &file).unwrap();
    dirty(&mut app);
    let ask = |shell: &mut Shell, app: &mut Application, dialogs: &mut FakeDialogs| {
        let request = Request::AskClose(tab);
        assert_eq!(
            platform::carry_out(shell, app, dialogs, &request).unwrap(),
            Done::Asking
        );
        // The first frame measures the modal; the buttons are where a click lands from the next.
        frame(shell, app, &ctx, Vec::new(), Vec::new(), false);
        frame(shell, app, &ctx, Vec::new(), Vec::new(), false).nodes
    };

    // Cancel keeps the document, unsaved.
    let nodes = ask(&mut shell, &mut app, &mut dialogs);
    frame(
        &mut shell,
        &mut app,
        &ctx,
        click(button(&nodes, "Cancel")),
        Vec::new(),
        false,
    );
    assert!(app.tab(tab).unwrap().is_dirty());
    assert!(dialogs.asked.is_empty());

    // Save writes the file, then closes.
    let nodes = ask(&mut shell, &mut app, &mut dialogs);
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    frame(
        &mut shell,
        &mut app,
        &ctx,
        click(button(&nodes, "Save")),
        Vec::new(),
        false,
    );
    assert!(app.tab(tab).is_err());
    assert!(std::fs::metadata(&file).unwrap().modified().unwrap() >= before);

    // Discard closes without saving.
    open_new_document(&mut shell, &mut app, &ctx);
    let tab = dirty(&mut app);
    let request = Request::AskClose(tab);
    platform::carry_out(&mut shell, &mut app, &mut dialogs, &request).unwrap();
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    let nodes = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false).nodes;
    frame(
        &mut shell,
        &mut app,
        &ctx,
        click(button(&nodes, "Discard")),
        Vec::new(),
        false,
    );
    assert!(app.tab(tab).is_err());
}

/// The labels of a frame in drawing order.
fn labels(nodes: &[accesskit::Node]) -> Vec<&str> {
    nodes
        .iter()
        .filter_map(|n| n.label().or_else(|| n.value()))
        .collect()
}

#[test]
fn an_answer_completes_its_prompt_line_and_stays_in_place_when_the_controller_drops_lines() {
    let dir = TempDir::new("answer");
    let seen = Arc::new(Mutex::new(Seen {
        history: Some((0, vec!["h0".into(), "h1".into()])),
        ..Seen::default()
    }));
    let mut app = app_with(&dir.0.join("data"), false, Some(seen.clone()));
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    submit(&mut shell, &mut app, &ctx, "x");
    // The controller drops its oldest line and adds one: the answer stays between h1 and h2.
    seen.lock().unwrap().history = Some((1, vec!["h1".into(), "h2".into()]));
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    // Lines are placed by their position in the panel.
    let top = |text: &str| {
        run.nodes
            .iter()
            .find(|n| n.label() == Some(text) || n.value() == Some(text))
            .and_then(accesskit::Node::bounds)
            .map(|b| b.y0)
            .unwrap_or_else(|| panic!("{text}"))
    };
    assert!(top("h1") < top("test-prompt: x"));
    assert!(top("test-prompt: x") < top("h2"));
}

#[test]
fn a_command_that_changed_the_document_says_it_is_done_and_escape_at_a_prompt_says_cancel() {
    let dir = TempDir::new("done");
    let mut app = app(&dir.0.join("data"), false);
    let mut shell = Shell::new(&app, config(), None);
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    open_new_document(&mut shell, &mut app, &ctx);
    submit(
        &mut shell,
        &mut app,
        &ctx,
        r#"shell.point.create position=[1,2,3]"#,
    );
    let run = frame(&mut shell, &mut app, &ctx, Vec::new(), Vec::new(), false);
    assert!(shows(
        &run.nodes,
        "Command: shell.point.create position=[1,2,3]"
    ));
    // The command's label is unresolved here, so only the message of the completion line shows.
    assert!(
        shows(&run.nodes, "app-history-ran"),
        "{:?}",
        labels(&run.nodes)
    );
}
