//! The windowed shell (feature `window`): CORE-APP-002, CORE-APP-009 and CORE-APP-025.
//!
//! [`Shell::frame`] is the frame composition and needs no window: given the [`Application`],
//! an [`egui::Context`], the frame's input and the textures of the viewport frames, it lays out
//! the menu bar and the dock with the panels the product registered, routes every intent
//! (widgets, menus, key bindings, the command line) either to a shell command or through the
//! command line to [`Application::invoke`], feeds viewport reactions to the tab's
//! [`arc_gpu::Viewport`], submits changed objects to extraction, and returns the
//! [`Request`]s the platform layer must carry out (dialogs, closing, quitting). Tests drive it
//! headless; [`event_loop`] runs it in a `winit` window.
//!
//! Panels are registered as product [`Entry`](crate::product::Entry)s whose factory returns a
//! `Box<dyn Panel>` (use [`panel`]); the standard panels of this module are the viewport,
//! property grid, outliner, command line, diagnostics, settings and log panels.
//!
//! [`Shell::start`] loads the user settings file, applies the appearance settings and lists
//! recoverable sessions in the recovery prompt; frames draw notifications, the active
//! document's banners and the prompt above the dock, open dropped files and write the recent
//! files after a change. Each floating dock tree is a secondary window: [`Shell::windows`]
//! models them and [`Shell::frame_window`] composes one.

pub mod chrome;
pub mod event_loop;
pub mod interaction;
mod over_viewport;
pub mod platform;
pub mod prefs;
pub mod prompts;

pub use chrome::{
    ChoiceState, DisplayProbe, InitialView, OverlayProbe, ShellAction, StatusToggle, TabView,
    ToolItem, WorkPlaneProbe,
};
pub use interaction::{
    ContextEntry, DynamicOutput, Interaction, InteractionContext, InteractionOutput, KeyInput,
    KeyReply, ViewRequest,
};

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_base::math::{Point3, Vector3};
use arc_base::{Error, Limits, ObjectId, Result, Tolerance};
use arc_command::macros::Recorder;
use arc_command::prompt::PromptInput;
use arc_command::{
    CommandLine, Invocation, LineStep, Outcome, Permission, Session, Submitted, Surface, Target,
};
use arc_doc::events::Subscription;
use arc_doc::events::{Changes, Event};
use arc_geom::view::{Projection, ViewFrame};
use arc_gpu::extract::{ExtractionDriver, Extractor as ExtractFn, ExtractorRegistry};
use arc_gpu::{Gpu, PickHit, PickRegion, Viewport, ViewportConfig};
use arc_ui::egui::{self, TextureId, Ui};
use arc_ui::gesture::ViewPicker;
use arc_ui::{
    Appearance, Binding, ChordResolver, Colors, CommandLineView, DiagnosticsPanel, Dock, DockArea,
    DockLayout, DynamicPanel, GridContext, Intent, Keymap, LineAction, LineCommands, LineContext,
    LineStatus, Localize, Lookup, Menu, Message, Notifications, OptionBar, PointerHandler,
    ProductInput, PropertyGrid, PropertyModel, PropertyObject, Severity, TreeCommands, TreeItem,
    TreeSource, TreeView, ViewCube, ViewCubeInput, ViewportCommands, ViewportInput, ViewportWidget,
};
use serde_json::{Map, Value as Json};

use crate::application::{Application, TabId, session_for};
use crate::commands::{self, SelectMode, SelectParams};
use crate::locale::Localizer;
use crate::logging::LogRing;
use crate::recent::RecentEntry;
use crate::settings::{Scope, SettingType, SettingValue};
use crate::settings_editor;
use interaction::Hook;
use prompts::{Choice, RecoveryEntry};

/// Lines of a controller's history the command panel shows, most recent last.
const MAX_SHOWN_HISTORY: usize = 500;
/// The most lines the shell keeps in a tab's command history of its own.
const MAX_CONSOLE: usize = 500;

/// How long a pick or a settle waits for the render and extraction threads.
pub const WAIT: Duration = Duration::from_secs(5);
/// The most errors the shell keeps for display.
pub const MAX_ERRORS: usize = 64;
/// The most invocations kept in the log of [`Shell::invocations`].
pub const MAX_LOG: usize = 4096;
/// The most objects a selection holds.
pub const MAX_SELECTION: usize = 100_000;
/// The most dropped files one frame opens.
pub const MAX_DROPPED: usize = 64;
/// The worker threads of each tab's extraction.
const EXTRACTION_WORKERS: usize = 2;

impl Localize for Localizer {
    fn message(&self, id: &str, args: &[(&str, arc_base::error::Value)]) -> Option<String> {
        let formatted = self.format(id, args);
        formatted.locale.map(|_| formatted.text)
    }
}

/// What a panel draws with.
pub struct PanelContext<'a> {
    /// The application, read-only: changes go through intents.
    pub app: &'a Application,
    /// The active tab.
    pub tab: Option<TabId>,
    /// The active tab's view state.
    pub view: Option<&'a mut View>,
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The active language tag, which selects the case-folding rule of filters.
    pub language: &'a str,
    /// The theme colors.
    pub colors: &'a Colors,
    /// The texture of the active tab's latest viewport frame, registered by the platform.
    pub texture: Option<TextureId>,
    /// The frame time in seconds on the caller's clock.
    pub time: f64,
    /// What the panel asks the shell to do.
    pub actions: &'a mut Vec<Action>,
    /// The recent log records, when the platform keeps them.
    pub log: Option<&'a LogRing>,
    /// What the product draws over the active tab's viewport (see [`OverlayProbe`]).
    pub overlays: &'a [arc_ui::ProductOverlay],
    /// The plane the active tab's point input lands on (see [`WorkPlaneProbe`]).
    pub work_plane: Option<arc_ui::WorkPlane>,
}

/// What a panel asks the shell to do after drawing.
#[derive(Debug)]
pub enum Action {
    /// Carry out an intent: a shell command, or a command started through the command line.
    Intent(Intent),
    /// A command-line action of the command-line widget.
    Line(LineAction),
    /// Start a command the user chose from a menu or a key binding: a shell command is carried
    /// out; any other command goes to the tab's controller as `LineAction::Invoke` when the
    /// tab has one (a toolbar button's command reaches it the same way), else it starts on the
    /// shell's command line as [`Action::Intent`] does.
    Start(Intent),
    /// Carry out an intent on the shell even when the tab has a controller: a shell command is
    /// carried out, any other command starts on the shell's own command line (what
    /// [`InteractionContext::ask`] produces).
    Dispatch(Intent),
    /// An answer for the active prompt from the viewport.
    Answer(PromptInput),
    /// A command a controller ran through [`InteractionContext::invoke`]; the history says so
    /// unless the controller reports its results itself.
    Ran(String),
    /// An error to show.
    Error(Error),
    /// A shell action: panel management or the camera.
    Shell(ShellAction),
    /// Set a setting in the user scope (`Some`) or remove it from there (`None`); the shell
    /// writes the user settings file and applies the appearance settings.
    Setting {
        /// The setting key.
        key: String,
        /// The new value, or `None` to reset.
        value: Option<SettingValue>,
    },
}

/// A panel of the dock. The factory of a registered panel returns it boxed (see [`panel`]).
pub trait Panel: Send {
    /// Draws the panel's content.
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>);
}

/// The factory value of a panel: what a product's panel factory returns.
#[must_use]
pub fn panel(p: impl Panel + 'static) -> Box<dyn Panel> {
    Box::new(p)
}

/// The factory value of an extractor: what a product's extractor factory returns.
#[must_use]
pub fn extractor(
    f: impl Fn(&arc_doc::Snapshot, &arc_doc::Object, &arc_base::Cancel) -> Result<arc_gpu::Extraction>
    + Send
    + Sync
    + 'static,
) -> ExtractFn {
    Arc::new(f)
}

/// What the platform layer must do for the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Ask for a file to open and open it.
    OpenDialog,
    /// Ask for a file to save the tab to and save it there.
    SaveDialog(TabId),
    /// The tab has unsaved changes: ask whether to save, discard or cancel, then close it.
    AskClose(TabId),
    /// Close every tab (asking for dirty ones) and end the application.
    Quit,
    /// Ask for a file and write this macro text to it.
    SaveMacro(String),
    /// A product command registered with
    /// [`ProductBuilder::path_dialog`](crate::product::ProductBuilder::path_dialog) started
    /// without a `path`: show `dialog` and run `then` with the chosen file as its `path`, through
    /// the tab's controller when it has one. A dialog that returns nothing leaves
    /// `app.file.none_chosen` in the history.
    PathDialog {
        /// The tab the command started on.
        tab: TabId,
        /// The dialog to show.
        dialog: crate::product::PathDialog,
        /// The command and its parameters; the chosen file is added as `path`.
        then: Intent,
    },
}

/// What one frame reads.
pub struct FrameInput<'a> {
    /// The egui input of the frame.
    pub raw: egui::RawInput,
    /// The caller's clock in seconds.
    pub time: f64,
    /// The message source.
    pub localize: &'a dyn Localize,
    /// The active language tag (for example `tr`), which selects the case-folding rule of the
    /// filters.
    pub language: &'a str,
    /// The registered texture of each tab's latest viewport frame.
    pub textures: &'a BTreeMap<TabId, TextureId>,
    /// Intents from the native menu bar since the last frame.
    pub menu_intents: Vec<Intent>,
    /// True when a native menu bar shows the menu, so no egui menu bar is drawn.
    pub native_menu: bool,
}

/// What one frame produced.
pub struct FrameOutput {
    /// The egui output to paint and to hand to the platform.
    pub full: egui::FullOutput,
    /// What the platform must do.
    pub requests: Vec<Request>,
    /// True when the shell has more to show soon (extraction or a viewport frame pending).
    pub busy: bool,
    /// True when a setting changed in this frame: the caller applies the language with
    /// [`prefs::apply_language`]; the shell applied the appearance itself.
    pub settings_changed: bool,
}

impl std::fmt::Debug for FrameOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameOutput")
            .field("requests", &self.requests)
            .field("busy", &self.busy)
            .field("settings_changed", &self.settings_changed)
            .finish_non_exhaustive()
    }
}

/// The shell's configuration from the product.
#[derive(Debug, Clone)]
pub struct ShellConfig {
    /// The dock layout of the panels; identifiers are panel entry identifiers.
    pub layout: DockLayout,
    /// The menu bar.
    pub menu: Menu,
    /// The key bindings.
    pub bindings: Vec<Binding>,
    /// Theme, density and scale.
    pub appearance: Appearance,
    /// The toolbar strip below the menu bar; empty draws none.
    pub toolbar: Vec<ToolItem>,
    /// The toggles of the status bar, after the cursor coordinates.
    pub status: Vec<StatusToggle>,
    /// What each viewport draws besides the scene; `None` draws the grid and the axes.
    pub display: Option<DisplayProbe>,
    /// What each viewport draws over the scene in model coordinates (labels); `None` draws
    /// nothing.
    pub overlays: Option<OverlayProbe>,
    /// The plane point input lands on in each viewport; `None` keeps the plane through the camera
    /// target (see [`WorkPlaneProbe`]).
    pub work_plane: Option<WorkPlaneProbe>,
    /// The view a new tab starts in; `None` looks straight down the z axis.
    pub initial_view: Option<InitialView>,
    /// The seed of invocation seeds: each command-line step uses the next value of a sequence
    /// from it, so identifiers that commands generate differ between steps. Pass entropy (the
    /// clock and the process), or a fixed value for a reproducible session.
    pub seed: u64,
}

/// A menu label message.
struct MenuLabel {
    message_id: &'static str,
}

const MENU_FILE: MenuLabel = MenuLabel {
    message_id: "app-menu-file",
};
const MENU_EDIT: MenuLabel = MenuLabel {
    message_id: "app-menu-edit",
};

/// The shell's menu bar: File (new, open, save, save as, close, quit) and Edit (undo, redo,
/// clear selection), each naming a [shell command](crate::commands).
///
/// # Errors
/// None for these items; the menu validation's errors otherwise.
pub fn default_menu() -> Result<Menu> {
    use arc_ui::MenuItem;
    let command = |c: &str| MenuItem::Command { command: c.into() };
    Menu::new(vec![
        MenuItem::Submenu {
            label: MENU_FILE.message_id.into(),
            items: vec![
                command(commands::FILE_NEW),
                command(commands::FILE_OPEN),
                command(commands::FILE_SAVE),
                command(commands::FILE_SAVE_AS),
                MenuItem::Separator,
                command(commands::FILE_CLOSE),
                command(commands::APP_QUIT),
            ],
        },
        MenuItem::Submenu {
            label: MENU_EDIT.message_id.into(),
            items: vec![
                command(commands::EDIT_UNDO),
                command(commands::EDIT_REDO),
                MenuItem::Separator,
                command(commands::SELECT_CLEAR),
            ],
        },
    ])
}

/// The view state of one tab: selection, camera, viewport, extraction and the per-document
/// widgets.
pub struct View {
    selection: Vec<ObjectId>,
    camera: ViewFrame,
    widget: ViewportWidget,
    viewport: Option<Viewport>,
    extraction: Option<ExtractionDriver>,
    events: Subscription,
    hover: Option<ObjectId>,
    line: CommandLine,
    line_view: CommandLineView,
    /// The product's interaction controller, when the product registered a factory.
    interaction: Option<Box<dyn Interaction>>,
    option_bar: OptionBar,
    grid: PropertyGrid,
    tree: TreeView,
    diagnostics: DiagnosticsPanel,
    /// Extraction results applied to the viewport, for [`Shell::busy`].
    pending: bool,
    /// The model point under the pointer in drawing units, for the status bar.
    cursor: Option<[f64; 2]>,
    /// The digits of the status bar coordinates.
    cursor_digits: usize,
    /// The revision of the document when the view was made; while the document is still at it
    /// the view frames the drawing once its scene is known.
    frame_revision: u64,
    /// The camera has been framed on the drawing (or the user moved it).
    framed: bool,
    /// Frame the camera on the drawing in the next viewport pass.
    zoom_extents: bool,
    /// The cameras before the zoom requests of the product, the last one on top.
    previous: Vec<ViewFrame>,
    /// Frames still wanted after the viewport went clean: the last rendered frame is registered
    /// as a texture by a redraw that follows the render.
    settle: std::cell::Cell<u8>,
    /// What the shell says in the command panel's history (typed lines, prompts, results and
    /// errors, at most [`MAX_CONSOLE`]) with the length of the controller's history each came
    /// after.
    console: Vec<(u64, ConsoleEntry)>,
    /// The prompt the history last showed, so a change of prompt adds one line.
    shown_prompt: Option<arc_command::prompt::Prompt>,
    /// Where the viewport was drawn in the last frame, in logical points: keys and typing go
    /// to the controller while the pointer is over it.
    viewport_rect: Option<egui::Rect>,
    /// The temporary hide and isolation the controller asked for.
    display: over_viewport::DisplayState,
}

/// The document the window asks about, and whether the application ends once it is closed.
#[derive(Debug, Clone, Copy)]
struct PendingClose {
    tab: TabId,
    quit: bool,
}

/// Appends `line` anchored at `at`, dropping the oldest lines beyond [`MAX_CONSOLE`].
fn push_bounded<T>(lines: &mut Vec<(u64, T)>, at: u64, line: T) {
    if lines.len() >= MAX_CONSOLE {
        lines.remove(0);
    }
    lines.push((at, line));
}

/// A line the shell adds to the command panel's history.
#[derive(Debug, Clone)]
enum ConsoleEntry {
    /// A message: an error or a result.
    Message(ConsoleError),
    /// A command typed on an idle line: `Command: <text>`.
    Typed(String),
    /// A command a menu, a key or a button started: `Command: <display name>`.
    Started(String),
    /// A prompt, with the answer typed to it: `Specify next point: 5,4`.
    Prompt(arc_command::prompt::Prompt, Option<String>),
    /// A command that finished: `Done: <display name>`.
    Done(String),
}

/// An error kept for the command panel's history: its message and arguments, which the panel
/// formats in the language in effect when it draws.
#[derive(Debug, Clone)]
struct ConsoleError {
    message_id: &'static str,
    args: Vec<(&'static str, arc_base::error::Value)>,
    /// A result line that is no error (a file was saved); drawn in the text color.
    info: bool,
}

impl ConsoleError {
    fn of(error: &Error) -> Self {
        Self {
            message_id: error.message_id(),
            args: error
                .arguments()
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            info: false,
        }
    }

    fn info(message_id: &'static str, args: Vec<(&'static str, arc_base::error::Value)>) -> Self {
        Self {
            message_id,
            args,
            info: true,
        }
    }
}

impl std::fmt::Debug for View {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("View")
            .field("selection", &self.selection)
            .field("camera", &self.camera)
            .finish_non_exhaustive()
    }
}

impl View {
    /// The selected objects, in selection order.
    #[must_use]
    pub fn selection(&self) -> &[ObjectId] {
        &self.selection
    }

    /// The camera.
    #[must_use]
    pub fn camera(&self) -> &ViewFrame {
        &self.camera
    }

    /// The tab's viewport, once the viewport panel created it on a GPU.
    #[must_use]
    pub fn viewport(&self) -> Option<&Viewport> {
        self.viewport.as_ref()
    }

    /// The command line of the tab.
    #[must_use]
    pub fn command_line(&self) -> &CommandLine {
        &self.line
    }
}

/// The initial camera: 100 units from the origin along `view`'s direction toward the camera (the
/// z axis without one), 100 units high; framed on the drawing once its scene is known.
fn initial_camera(view: Option<InitialView>) -> Result<ViewFrame> {
    let [x, y, z] = view.map_or([0.0, 0.0, 1.0], |v| v.toward_camera);
    let length = (x * x + y * y + z * z).sqrt();
    let toward = if length.is_finite() && length > 1e-9 {
        Vector3::new(x / length, y / length, z / length)
    } else {
        Vector3::z()
    };
    // Looking along the vertical axis, y is up on the screen; otherwise z is.
    let up = if toward.x.abs() + toward.y.abs() < 1e-6 {
        Vector3::y()
    } else {
        Vector3::z()
    };
    ViewFrame::new(
        Point3::origin() + toward * 100.0,
        Point3::origin(),
        up,
        Projection::Orthographic { height: 100.0 },
        0.1,
        10_000.0,
    )
}

/// The windowed shell; see the [module documentation](self).
pub struct Shell {
    panels: Vec<(String, String, Box<dyn Panel>)>,
    layout: DockLayout,
    menu: Menu,
    keymap: Keymap,
    chords: ChordResolver,
    appearance: Appearance,
    extractors: ExtractorRegistry,
    gpu: Option<Gpu>,
    views: BTreeMap<TabId, View>,
    /// The session of the user-interface commands. Settings are per session: the user-interface
    /// and command-line sessions do not share the settings a command writes, only the `last` hook.
    ui_session: Session,
    /// The session of the command line; see `ui_session` for what the two do not share.
    line_session: Session,
    errors: Vec<Error>,
    log: Vec<String>,
    seed: u64,
    base_appearance: Appearance,
    notifications: Notifications,
    recovery: Vec<RecoveryEntry>,
    /// The close question shown in the window (see [`Shell::ask_in_window`]).
    closing: Option<PendingClose>,
    /// Requests that running a command after a file dialog produced; the next frame returns them.
    pending: Vec<Request>,
    files: Option<UserFiles>,
    recent_written: Option<Vec<RecentEntry>>,
    log_ring: Option<LogRing>,
    settings_changed: bool,
    /// Text a controller asked to put on the operating system clipboard this frame.
    copy_text: Option<String>,
    recorder: Option<Recorder>,
    default_layout: DockLayout,
    toolbar: Vec<ToolItem>,
    status: Vec<StatusToggle>,
    display: Option<DisplayProbe>,
    overlays: Option<OverlayProbe>,
    work_plane: Option<WorkPlaneProbe>,
    initial_view: Option<InitialView>,
    /// The active language tag of the last frame, which names a new document's content.
    language: String,
}

/// The user's files the shell reads at start and writes on change.
#[derive(Debug, Clone)]
pub struct UserFiles {
    /// The user settings file (the user scope).
    pub settings: PathBuf,
    /// The recent-files file.
    pub recent: PathBuf,
}

/// A secondary window: one floating dock tree.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowModel {
    /// The window identifier, also its key in the window-state file: `float.` and the tree's
    /// first panel, or `float-<n>` when that is not a valid window identifier or repeats.
    pub id: String,
    /// The tree's rectangle `[x, y, width, height]` in logical points, the window's placement
    /// when none was saved.
    pub rect: [f32; 4],
    /// The panels of the tree, in tree order.
    pub panels: Vec<String>,
}

fn dock_panels(dock: &Dock, out: &mut Vec<String>) {
    match dock {
        Dock::Tabs { tabs, .. } => out.extend(tabs.iter().cloned()),
        Dock::Split { first, second, .. } => {
            dock_panels(first, out);
            dock_panels(second, out);
        }
    }
}

/// The windows of `layout`'s floating trees, in layout order.
fn window_models(layout: &DockLayout) -> Vec<WindowModel> {
    let mut out: Vec<WindowModel> = Vec::new();
    for (n, floating) in layout.floating().iter().enumerate() {
        let mut panels = Vec::new();
        dock_panels(&floating.dock, &mut panels);
        let mut id: String = panels
            .first()
            .map(|p| format!("float.{}", p.to_lowercase()))
            .unwrap_or_default();
        id.truncate(crate::window_state::MAX_WINDOW_ID_BYTES);
        if !crate::window_state::is_window_id(&id) || out.iter().any(|w| w.id == id) {
            id = format!("float-{n}");
        }
        out.push(WindowModel {
            id,
            rect: floating.rect,
            panels,
        });
    }
    out
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell")
            .field("views", &self.views)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

/// A session on `surface` for `app`'s product: its clock (so records carry the real time) and its
/// answer for the keyword `last`.
fn session(app: &Application, surface: Surface) -> Session {
    let core = app.product().headless();
    let mut session = Session::new(
        surface,
        [
            Permission::Read,
            Permission::Edit,
            Permission::FileRead,
            Permission::FileWrite,
        ],
    )
    .with_last_hook(core.last.clone());
    session.clock = Arc::clone(&core.clock);
    session
}

/// The controller's output; without a selection of its own the status reads the tab's selection.
fn controller_output(controller: &dyn Interaction, selection: &[ObjectId]) -> InteractionOutput {
    let mut out = controller.output();
    if out.selection.is_none() {
        out.status.has_selection = !selection.is_empty();
    }
    out
}

/// The session a controller of `tab` runs commands in: the command line's session with the
/// interface language set, without the `edit` permission on a read-only tab.
fn controller_session(app: &Application, tab: TabId, language: &str) -> Session {
    let session = session(app, Surface::CommandLine);
    // A tag within the setting limits never fails; the session just lacks the language.
    let _ = session.set_setting(commands::LANGUAGE_SETTING, serde_json::json!(language));
    session_for(app.is_read_only_tab(tab), &session).into_owned()
}

impl Shell {
    /// A shell for `app`'s product: calls each panel and extractor factory once. A factory that
    /// does not return the expected type, and key bindings the registry refuses, become errors
    /// in [`Shell::errors`]; the rest still works. `gpu` is the device viewports render on;
    /// without one the viewport panel paints its background only.
    #[must_use]
    pub fn new(app: &Application, config: ShellConfig, gpu: Option<Gpu>) -> Self {
        let product = app.product();
        let mut errors = Vec::new();
        let mut panels = Vec::new();
        for entry in product.panels() {
            match entry.factory.create().downcast::<Box<dyn Panel>>() {
                Ok(p) => panels.push((entry.id.clone(), entry.message_id.clone(), *p)),
                Err(_) => errors.push(factory_error(&entry.id)),
            }
        }
        let mut extractors = ExtractorRegistry::new();
        for entry in product.extractors() {
            match entry.factory.create().downcast::<ExtractFn>() {
                Ok(f) => {
                    for key in &entry.types {
                        if let Ok(key) = arc_doc::TypeKey::new(key.as_str()) {
                            let f = Arc::clone(&f);
                            extractors.register(key, move |s, o, c| f(s, o, c));
                        }
                    }
                }
                Err(_) => errors.push(factory_error(&entry.id)),
            }
        }
        let (keymap, problems) = Keymap::build(&config.bindings, &product.headless().registry);
        errors.extend(problems);
        Self {
            panels,
            layout: config.layout.clone(),
            menu: config.menu,
            keymap,
            chords: ChordResolver::default(),
            appearance: config.appearance,
            extractors,
            gpu,
            views: BTreeMap::new(),
            ui_session: session(app, Surface::Ui),
            line_session: session(app, Surface::CommandLine),
            errors,
            log: Vec::new(),
            seed: config.seed,
            base_appearance: config.appearance,
            notifications: Notifications::default(),
            recovery: Vec::new(),
            closing: None,
            pending: Vec::new(),
            files: None,
            recent_written: None,
            log_ring: None,
            settings_changed: false,
            copy_text: None,
            recorder: None,
            default_layout: config.layout,
            toolbar: config.toolbar,
            status: config.status,
            display: config.display,
            overlays: config.overlays,
            work_plane: config.work_plane,
            initial_view: config.initial_view,
            language: String::new(),
        }
    }

    /// Prepares the first frame: loads the user settings file into the user scope (a malformed
    /// file is set aside and reported as a notification), remembers the files for writing,
    /// applies the appearance settings and lists the recoverable sessions in the recovery
    /// prompt. Nothing is recovered or discarded until the user chooses it. Without `files`
    /// settings and recent files stay in memory.
    pub fn start(&mut self, app: &mut Application, files: Option<UserFiles>) {
        if let Some(files) = &files {
            match crate::settings_file::load(&files.settings) {
                Ok(loaded) => {
                    if let Some(aside) = &loaded.set_aside {
                        self.notify(Severity::Warning, &aside.problem);
                    }
                    if let Err(e) = app.settings_mut().set_layer(Scope::User, loaded.table) {
                        self.notify(Severity::Error, &e);
                    }
                }
                Err(e) => self.notify(Severity::Error, &e),
            }
        }
        self.files = files;
        self.appearance = prefs::appearance(app.settings(), self.base_appearance);
        match app.recoverable() {
            Ok(orphans) => {
                self.recovery = orphans
                    .into_iter()
                    .map(|orphan| RecoveryEntry {
                        orphan,
                        comparison: None,
                    })
                    .collect();
            }
            Err(e) => self.notify(Severity::Error, &e),
        }
        tracing::info!(recoverable = self.recovery.len(), "shell started");
    }

    fn notify(&mut self, severity: Severity, error: &Error) {
        let args = error
            .arguments()
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        self.notifications
            .post(severity, error.message_id(), args, Vec::new(), 0);
    }

    /// The notifications shown at the top of the main window.
    #[must_use]
    pub fn notifications(&self) -> &Notifications {
        &self.notifications
    }

    /// The sessions the recovery prompt lists; the prompt shows while this is not empty.
    #[must_use]
    pub fn recovery(&self) -> &[RecoveryEntry] {
        &self.recovery
    }

    /// Keeps `ring` for the log viewer panel.
    pub fn set_log_ring(&mut self, ring: LogRing) {
        self.log_ring = Some(ring);
    }

    /// The appearance in effect: the configuration's, with the appearance settings applied.
    #[must_use]
    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    /// The dock layout.
    #[must_use]
    pub fn layout(&self) -> &DockLayout {
        &self.layout
    }

    /// The dock layout, to float, move or close panels.
    pub fn layout_mut(&mut self) -> &mut DockLayout {
        &mut self.layout
    }

    /// The secondary windows: one per floating dock tree.
    #[must_use]
    pub fn windows(&self) -> Vec<WindowModel> {
        window_models(&self.layout)
    }

    /// Docks every panel of window `id` back into the main tree, as when its window closes.
    /// Returns false for an unknown window.
    pub fn dock_window(&mut self, id: &str) -> bool {
        let Some(window) = self.windows().into_iter().find(|w| w.id == id) else {
            return false;
        };
        for panel in window.panels {
            // A panel that vanished meanwhile has nothing to dock.
            let _ = self.layout.unfloat(&panel);
        }
        true
    }

    /// Writes the recent files when they changed since the last write (after an open or a
    /// save) and at quit. Without files from [`Shell::start`] nothing is written.
    pub fn save_recent(&mut self, app: &Application) {
        let Some(files) = &self.files else {
            return;
        };
        let now: Vec<RecentEntry> = app.recent().iter().cloned().collect();
        if self.recent_written.as_ref() == Some(&now) {
            return;
        }
        match app.recent().save(&files.recent) {
            Ok(()) => self.recent_written = Some(now),
            Err(e) => {
                // Reported once; the next change tries again.
                self.recent_written = Some(now);
                self.error(e);
            }
        }
    }

    /// The errors of recent frames, oldest first, at most [`MAX_ERRORS`].
    #[must_use]
    pub fn errors(&self) -> &[Error] {
        &self.errors
    }

    /// Clears [`Shell::errors`].
    pub fn clear_errors(&mut self) {
        self.errors.clear();
    }

    /// The identifiers of the commands that went through [`Application::invoke`], oldest
    /// first, at most [`MAX_LOG`]. Shell commands are not in it.
    #[must_use]
    pub fn invocations(&self) -> &[String] {
        &self.log
    }

    /// The view state of `tab`, once the tab was shown.
    #[must_use]
    pub fn view(&self, tab: TabId) -> Option<&View> {
        self.views.get(&tab)
    }

    /// The menu bar resolved against the product's commands for `localize`; items naming
    /// unknown commands are left out and recorded in [`Shell::errors`].
    pub fn menu_entries(
        &mut self,
        app: &Application,
        localize: &dyn Localize,
    ) -> Vec<arc_ui::Entry> {
        let lookup = Lookup {
            registry: &app.product().headless().registry,
            localize,
            state: &arc_ui::AllEnabled,
            keymap: Some(&self.keymap),
        };
        let (entries, problems) = lookup.menu(&self.menu);
        for p in problems {
            self.error(p);
        }
        entries
    }

    /// Each tab's latest viewport frame, for the platform to register as a texture.
    #[must_use]
    pub fn viewport_frames(&self) -> Vec<(TabId, Arc<arc_gpu::Frame>)> {
        self.views
            .iter()
            .filter_map(|(t, v)| Some((*t, v.viewport.as_ref()?.latest_frame()?)))
            .collect()
    }

    fn error(&mut self, error: Error) {
        if self.errors.len() >= MAX_ERRORS {
            self.errors.remove(0);
        }
        self.errors.push(error);
    }

    /// Records the error of an action on the active tab: in [`Shell::errors`] and in the command
    /// panel's history, after the lines that exist now.
    fn report(&mut self, app: &Application, error: Error) {
        self.push_result(app, ConsoleError::of(&error));
        self.error(error);
    }

    /// Says what a successful file action did in the command panel's history.
    fn announce(
        &mut self,
        app: &Application,
        message_id: &'static str,
        args: Vec<(&'static str, arc_base::error::Value)>,
    ) {
        self.push_result(app, ConsoleError::info(message_id, args));
    }

    /// Puts a result line at the end of the active tab's history.
    fn push_result(&mut self, app: &Application, line: ConsoleError) {
        self.push_entry(app, ConsoleEntry::Message(line));
    }

    /// Puts `entry` at the end of the active tab's history, after the lines of the controller
    /// that exist now (by their sequence numbers, so a controller that drops its oldest lines
    /// does not move the shell's). The oldest lines go beyond [`MAX_CONSOLE`].
    fn push_entry(&mut self, app: &Application, entry: ConsoleEntry) {
        if let Some(view) = app.active().and_then(|t| self.views.get_mut(&t)) {
            let at = view.interaction.as_ref().map_or(0, |i| {
                let out = i.output();
                out.history_first
                    .saturating_add(u64::try_from(out.history.len()).unwrap_or(u64::MAX))
            });
            push_bounded(&mut view.console, at, entry);
        }
    }

    /// Whether the controller of the active tab writes the completion and cancel lines itself.
    fn reports_results(&self, app: &Application) -> bool {
        app.active()
            .and_then(|t| self.views.get(&t))
            .and_then(|v| v.interaction.as_ref())
            .is_some_and(|i| i.output().reports_results)
    }

    /// Echoes the typed line or answer `text`: an answer to the active prompt completes the
    /// prompt's line (`Specify next point: 5,4`), anything else is a command (`Command: wall`).
    fn echo(&mut self, app: &Application, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // A toolbar button submits the identifier of its command: it reads as the command's name,
        // also while a prompt is active (the command then cancels it).
        if app.product().headless().registry.get(text).is_some() {
            self.push_entry(app, ConsoleEntry::Started(text.to_owned()));
            return;
        }
        let Some(view) = app.active().and_then(|t| self.views.get_mut(&t)) else {
            return;
        };
        let Some(prompt) = view.shown_prompt.clone() else {
            self.push_entry(app, ConsoleEntry::Typed(text.to_owned()));
            return;
        };
        if let Some((_, ConsoleEntry::Prompt(last, answer @ None))) = view.console.last_mut()
            && last.key == prompt.key
            && last.message.id == prompt.message.id
        {
            *answer = Some(text.to_owned());
            return;
        }
        self.push_entry(app, ConsoleEntry::Prompt(prompt, Some(text.to_owned())));
    }

    /// Adds a line when the active prompt of the active tab changed since the last frame.
    fn note_prompt(&mut self, app: &Application) {
        let Some(view) = app.active().and_then(|t| self.views.get_mut(&t)) else {
            return;
        };
        let now = match view.interaction.as_deref() {
            Some(c) => controller_output(c, &view.selection).prompt,
            None => view.line.active().and_then(|s| s.prompt()).cloned(),
        };
        let same = match (&now, &view.shown_prompt) {
            (Some(a), Some(b)) => a.key == b.key && a.message.id == b.message.id,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        view.shown_prompt.clone_from(&now);
        if let Some(prompt) = now {
            self.push_entry(app, ConsoleEntry::Prompt(prompt, None));
        }
    }

    /// Says that `command` finished, when it changed the document (a query or a selection
    /// leaves no line) and the controller does not say so itself.
    fn note_done(&mut self, app: &Application, command: &str) {
        let undoable = app
            .product()
            .headless()
            .registry
            .get(command)
            .is_some_and(|s| s.undo() != arc_command::UndoBehavior::NotUndoable);
        if undoable && !self.reports_results(app) {
            self.push_entry(app, ConsoleEntry::Done(command.to_owned()));
        }
    }

    /// Creates the view of every open tab that has none and drops the views of closed tabs.
    fn sync_views(&mut self, app: &Application) {
        self.views
            .retain(|id, _| app.tabs().iter().any(|t| t.id() == *id));
        for tab in app.tabs() {
            if self.views.contains_key(&tab.id()) {
                continue;
            }
            match self.new_view(tab.document()) {
                Ok(mut view) => {
                    view.interaction = app.product().interaction().map(|factory| factory());
                    self.views.insert(tab.id(), view);
                }
                Err(e) => self.error(e),
            }
        }
    }

    fn new_view(&self, document: &arc_doc::Document) -> Result<View> {
        let events = document.subscribe();
        let extraction = if self.gpu.is_some() {
            let driver = ExtractionDriver::new(self.extractors.clone(), EXTRACTION_WORKERS)?;
            driver.submit_changes(&document.snapshot(), &Changes::All);
            Some(driver)
        } else {
            None
        };
        Ok(View {
            selection: Vec::new(),
            camera: initial_camera(self.initial_view)?,
            widget: ViewportWidget::new(),
            viewport: None,
            extraction,
            events,
            hover: None,
            line: CommandLine::new(),
            line_view: CommandLineView::default(),
            interaction: None,
            option_bar: OptionBar::default(),
            grid: PropertyGrid::default(),
            tree: TreeView::default(),
            diagnostics: DiagnosticsPanel::default(),
            pending: false,
            cursor: None,
            cursor_digits: 3,
            frame_revision: document.snapshot().revision(),
            framed: false,
            zoom_extents: false,
            previous: Vec::new(),
            settle: std::cell::Cell::new(0),
            console: Vec::new(),
            shown_prompt: None,
            viewport_rect: None,
            display: over_viewport::DisplayState::default(),
        })
    }

    /// Submits committed changes to extraction, drops deleted objects from the selection and
    /// hands finished extraction results to the viewport. With `wait`, waits for extraction
    /// and for the viewport to draw.
    fn pump(&mut self, app: &Application, wait: Option<Duration>) {
        let mut errors = Vec::new();
        for (id, view) in &mut self.views {
            let mut changed = false;
            while let Some(event) = view.events.try_recv() {
                if let Event::Committed(c) = event {
                    if let Some(driver) = &view.extraction {
                        driver.submit_changes(&c.snapshot, &c.changes);
                        if let Some(dependents) = app.product().extraction_dependents() {
                            driver.submit(&c.snapshot, dependents(&c.snapshot, &c.changes));
                        }
                    }
                    changed = true;
                }
            }
            if changed && let Ok(tab) = app.tab(*id) {
                let snapshot = tab.document().snapshot();
                view.selection.retain(|o| snapshot.resolve(*o).is_ok());
            }
            if let Some(driver) = &view.extraction {
                if let Some(timeout) = wait {
                    let _ = driver.wait_idle(timeout);
                }
                // Results wait for the viewport: taking them before it exists would drop the
                // scene of a document that was open before its first frame.
                if let Some(viewport) = &view.viewport {
                    let results = driver.take_results();
                    view.pending = !results.is_empty();
                    if !results.is_empty()
                        && let Err(e) = viewport.update_scene(results)
                    {
                        errors.push(e);
                    }
                } else {
                    view.pending = true;
                }
            }
            if let (Some(timeout), Some(viewport)) = (wait, &view.viewport)
                && let Err(e) = viewport.sync(timeout)
            {
                errors.push(e);
            }
        }
        for e in errors {
            self.error(e);
        }
    }

    /// Waits at most `timeout` per tab until extraction is idle, its results are in the
    /// viewports and the viewports have drawn them. Tests and the platform's first frame use
    /// it; frames never wait.
    pub fn settle(&mut self, app: &Application, timeout: Duration) {
        self.sync_views(app);
        self.pump(app, Some(timeout));
    }

    /// True while extraction results or viewport frames are on their way.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.views.values().any(|v| {
            let working = v.pending || v.viewport.as_ref().is_some_and(|vp| vp.dirty().any());
            if working {
                v.settle.set(4);
                true
            } else if v.settle.get() > 0 {
                v.settle.set(v.settle.get() - 1);
                true
            } else {
                false
            }
        })
    }

    /// Composes one frame of the main window; see the [module documentation](self). Events
    /// with non-finite or huge positions, deltas or factors are dropped first: egui does not
    /// accept them.
    pub fn frame(
        &mut self,
        app: &mut Application,
        ctx: &egui::Context,
        input: FrameInput<'_>,
    ) -> FrameOutput {
        self.compose(app, ctx, input, None)
    }

    /// Composes one frame of secondary window `window` (see [`Shell::windows`]) in its own
    /// egui context: its floating tree with the same panels, actions and requests as the main
    /// window. An unknown window draws nothing. Moving a panel to a tree of another window from
    /// here is dropped.
    pub fn frame_window(
        &mut self,
        app: &mut Application,
        ctx: &egui::Context,
        input: FrameInput<'_>,
        window: &str,
    ) -> FrameOutput {
        self.compose(app, ctx, input, Some(window))
    }

    /// Opens the files dropped on a window through [`Application::open`].
    fn open_dropped(&mut self, app: &mut Application, raw: &mut egui::RawInput) {
        let dropped = std::mem::take(&mut raw.dropped_files);
        for file in dropped.iter().take(MAX_DROPPED) {
            match app.open(file.path()) {
                // Paths can name the user; the record keeps only that a file was opened.
                Ok(tab) => tracing::info!(
                    tab = ?tab,
                    path = %crate::logging::Redacted(file.path().display()),
                    "dropped file opened"
                ),
                Err(e) => self.error(e),
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one frame: the chrome, the dock and the panels"
    )]
    fn compose(
        &mut self,
        app: &mut Application,
        ctx: &egui::Context,
        mut input: FrameInput<'_>,
        window: Option<&str>,
    ) -> FrameOutput {
        input.raw.events.retain(finite);
        self.open_dropped(app, &mut input.raw);
        self.sync_views(app);
        self.pump(app, None);
        self.appearance.apply(ctx);
        let millis = millis(input.time);
        if window.is_none() && self.language != input.language {
            self.language.clear();
            self.language.push_str(input.language);
            let tag = serde_json::json!(input.language);
            for session in [&self.ui_session, &self.line_session] {
                // A tag within the setting limits never fails; the sessions just keep the old one.
                let _ = session.set_setting(commands::LANGUAGE_SETTING, tag.clone());
            }
        }
        // A key the keymap binds is handled by the binding alone: the press leaves the frame's
        // events, so no widget (the command line's undo keys, a text field) handles it again.
        let keyed = self.chords.take_events(
            &self.keymap,
            &mut input.raw.events,
            millis,
            ctx.text_edit_focused(),
        );
        let mut actions: Vec<Action> = keyed
            .into_iter()
            .chain(input.menu_intents)
            .map(Action::Start)
            .collect();
        // Escape that closes a menu or a popup closes only that: it is not also the viewport's or
        // the command line's Escape (which would clear the selection).
        if egui::Popup::is_any_open(ctx) {
            let escape = |e: &egui::Event| {
                matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    }
                )
            };
            if input.raw.events.iter().any(escape) {
                input.raw.events.retain(|e| !escape(e));
                egui::Popup::close_all(ctx);
            }
        }
        // Typing and Tab over the viewport go to the product's controller first.
        if window.is_none() {
            self.offer_viewport_keys(app, ctx, &mut input.raw, &mut actions);
            self.poll_line_input(app, &mut actions);
        }
        let entries = if input.native_menu || window.is_some() {
            Vec::new()
        } else {
            self.menu_entries(app, input.localize)
        };
        let colors = self.appearance.theme.theme().colors;
        let active = app.active();
        let texture = active.and_then(|t| input.textures.get(&t).copied());
        let gpu = self.gpu.clone();
        let titles: BTreeMap<String, String> = self
            .panels
            .iter()
            .map(|(id, message, _)| (id.clone(), arc_ui::resolve(input.localize, message, &[])))
            .collect();
        let (floating_at, mut own) = self.own_layout(window);
        let mut choices: Vec<Choice> = Vec::new();
        let main = window.is_none();
        let panel_titles: Vec<(String, String)> = self
            .panels
            .iter()
            .map(|(id, _, _)| {
                (
                    id.clone(),
                    titles.get(id).cloned().unwrap_or_else(|| id.clone()),
                )
            })
            .collect();
        let rows = chrome::panel_rows(&self.layout, &panel_titles);
        let tab_views: Vec<TabView> = if main {
            let untitled: Vec<TabId> = app
                .tabs()
                .iter()
                .filter(|t| t.title().is_none())
                .map(crate::application::Tab::id)
                .collect();
            app.tabs()
                .iter()
                .map(|t| TabView {
                    id: t.id(),
                    title: t.title().unwrap_or_else(|| {
                        let name = prompts::UNTITLED.text(input.localize);
                        match untitled.iter().position(|u| *u == t.id()) {
                            Some(n) if untitled.len() > 1 => format!("{name} {}", n + 1),
                            _ => name,
                        }
                    }),
                    modified: t.is_dirty(),
                    active: active == Some(t.id()),
                })
                .collect()
        } else {
            Vec::new()
        };
        let overlays = match (&self.overlays, active) {
            (Some(probe), Some(tab)) => (probe.0)(app, tab, input.language),
            _ => Vec::new(),
        };
        let work_plane = match (&self.work_plane, active) {
            (Some(probe), Some(tab)) => {
                let selection = self.views.get(&tab).map_or(&[][..], View::selection);
                (probe.0)(app, tab, selection)
            }
            _ => None,
        };
        let tools = chrome::tool_views(&self.toolbar, app, active, input.localize);
        let toggles = chrome::toggle_views(&self.status, app, active, input.localize);
        actions.extend(
            chrome::toggle_key_intents(&self.status, &toggles, &input.raw.events)
                .into_iter()
                .map(|i| Action::Line(LineAction::Invoke(i))),
        );
        let cursor_text = chrome::cursor_text(
            active
                .and_then(|t| self.views.get(&t))
                .and_then(|v| v.cursor),
            active
                .and_then(|t| self.views.get(&t))
                .map_or(3, |v| v.cursor_digits),
            input.localize,
        );
        if let (Some(probe), Some(tab)) = (&self.display, active) {
            let display = (probe.0)(app, tab);
            if let Some(view) = self.views.get_mut(&tab) {
                view.widget.display = display;
            }
        }
        // Typing outside a text field starts the command line.
        if main && let Some(view) = active.and_then(|t| self.views.get_mut(&t)) {
            view.line_view.defer_keys = ctx.text_edit_focused();
            let focused = ctx.memory(|m| m.focused());
            let other = focused.is_some() && focused != view.widget.id;
            // Escape ends a claimed drag where it is: the focus stays in the viewport.
            let dragging = view.widget.holds_product_press();
            let events: Vec<egui::Event> = input
                .raw
                .events
                .iter()
                .filter(|e| {
                    !(dragging
                        && matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Escape,
                                ..
                            }
                        ))
                })
                .cloned()
                .collect();
            if chrome::wants_command_line(&events, ctx.text_edit_focused(), other) {
                view.line_view.take_focus = true;
            }
        }
        let chrome_frame = chrome::ChromeFrame {
            tabs: &tab_views,
            entries: &entries,
            rows: &rows,
            tools: &tools,
            toggles: &toggles,
            cursor: &cursor_text,
            localize: input.localize,
        };
        let full = {
            let app_ref: &Application = app;
            let panels = &mut self.panels;
            let mut layout = match (&mut own, window) {
                (Some(own), _) => Some(own),
                (None, None) => Some(&mut self.layout),
                (None, Some(_)) => None,
            };
            let notifications = &mut self.notifications;
            let recovery = &self.recovery;
            let closing = self.closing;
            let log_ring = self.log_ring.as_ref();
            let mut view = active.and_then(|t| self.views.get_mut(&t));
            if let (Some(v), Some(gpu)) = (view.as_deref_mut(), gpu.as_ref())
                && v.viewport.is_none()
            {
                match Viewport::new(gpu, viewport_config(v.camera, &colors)) {
                    Ok(vp) => v.viewport = Some(vp),
                    Err(e) => actions.push(Action::Error(e)),
                }
            }
            ctx.run_ui(input.raw, |ui| {
                if main {
                    chrome::show_frame(ui, &chrome_frame, &mut actions);
                }
                if main {
                    if notifications.active().next().is_some() {
                        egui::Panel::top("arc-app-notifications").show(ui, |ui| {
                            if let Some(intent) = notifications.show(ui, &colors, input.localize) {
                                actions.push(Action::Intent(intent));
                            }
                        });
                    }
                    if let Some(tab) = active.and_then(|t| app_ref.tab(t).ok()) {
                        prompts::banners(ui, tab, input.localize, &mut choices);
                    }
                    prompts::recovery(ui, recovery, input.localize, &mut choices);
                    if let Some(title) = closing
                        .and_then(|p| tab_views.iter().find(|t| t.id == p.tab))
                        .map(|t| t.title.as_str())
                    {
                        prompts::close_question(ui, title, input.localize, &mut choices);
                    }
                }
                let Some(layout) = layout.as_deref_mut() else {
                    return;
                };
                let title = |id: &str| titles.get(id).cloned().unwrap_or_else(|| id.to_owned());
                DockArea::default().show(ui, layout, input.localize, &title, &mut |ui, id| {
                    let Some((_, _, p)) = panels.iter_mut().find(|(pid, _, _)| pid == id) else {
                        return;
                    };
                    let mut cx = PanelContext {
                        app: app_ref,
                        tab: active,
                        view: view.as_deref_mut(),
                        localize: input.localize,
                        language: input.language,
                        colors: &colors,
                        texture,
                        time: input.time,
                        actions: &mut actions,
                        log: log_ring,
                        overlays: &overlays,
                        work_plane,
                    };
                    p.show(ui, &mut cx);
                });
            })
        };
        if let (Some(at), Some(own)) = (floating_at, own) {
            self.write_back(at, &own);
        }
        // A focus request nobody took (no command line is shown) does not wait for later frames.
        if let Some(view) = active.and_then(|t| self.views.get_mut(&t)) {
            view.line_view.take_focus = false;
        }
        self.finish(app, full, choices, actions)
    }

    /// Offers the text typed in the active tab's command line to its controller, which may
    /// resolve it at once; the shell then clears the input and sends the action to the controller.
    fn poll_line_input(&mut self, app: &Application, actions: &mut Vec<Action>) {
        let Some(tab) = app.active() else {
            return;
        };
        let session = controller_session(app, tab, &self.language);
        let Some(view) = self.views.get_mut(&tab) else {
            return;
        };
        let View {
            interaction,
            selection,
            camera,
            line_view,
            ..
        } = view;
        if line_view.input.is_empty() {
            return;
        }
        let Some(controller) = interaction.as_deref_mut() else {
            return;
        };
        let cx = InteractionContext::new(app, tab, &session, selection, *camera, &self.language);
        if let Some(action) = controller.input_changed(&cx, &line_view.input) {
            line_view.input.clear();
            actions.push(Action::Line(action));
        }
        actions.extend(cx.take_asked().into_iter().map(Action::Dispatch));
    }

    /// Offers the Tab presses and typed text of the frame to the active tab's controller while
    /// the pointer is over the viewport or the viewport has the focus; what it consumes leaves
    /// the frame's events.
    fn offer_viewport_keys(
        &mut self,
        app: &Application,
        ctx: &egui::Context,
        raw: &mut egui::RawInput,
        actions: &mut Vec<Action>,
    ) {
        let Some(tab) = app.active() else {
            return;
        };
        let session = controller_session(app, tab, &self.language);
        let Some(view) = self.views.get_mut(&tab) else {
            return;
        };
        let over = view
            .viewport_rect
            .zip(ctx.pointer_latest_pos())
            .is_some_and(|(rect, at)| rect.contains(at));
        let focused = view.widget.id.is_some() && ctx.memory(|m| m.focused()) == view.widget.id;
        if !(over || focused) {
            return;
        }
        let View {
            interaction,
            selection,
            camera,
            ..
        } = view;
        let Some(controller) = interaction.as_deref_mut() else {
            return;
        };
        let cx = InteractionContext::new(app, tab, &session, selection, *camera, &self.language);
        let modifiers = ctx.input(|i| i.modifiers);
        // A text field keeps its typing, but Tab over the viewport still steps the candidates.
        let tab_only = ctx.text_edit_focused();
        if tab_only && !over {
            return;
        }
        over_viewport::offer_keys(controller, &cx, modifiers, &mut raw.events, tab_only);
        if let Some(text) = cx.take_clipboard_text() {
            self.copy_text = Some(text);
        }
        actions.extend(cx.take_ran().into_iter().map(Action::Ran));
        actions.extend(cx.take_asked().into_iter().map(Action::Dispatch));
        actions.extend(
            cx.take_settings()
                .into_iter()
                .map(|(key, value)| Action::Setting { key, value }),
        );
        actions.extend(
            cx.take_panels()
                .into_iter()
                .map(|p| Action::Shell(ShellAction::ShowPanel(p))),
        );
    }

    /// The position of window `window`'s floating tree and that tree as a layout of its own,
    /// which the window draws and [`Shell::write_back`] puts back.
    fn own_layout(&self, window: Option<&str>) -> (Option<usize>, Option<DockLayout>) {
        let at = window.and_then(|w| self.windows().iter().position(|m| m.id == w));
        let own = at.and_then(|at| {
            let tree = self.layout.floating().get(at)?;
            DockLayout::new(tree.dock.clone(), Vec::new()).ok()
        });
        (at, own)
    }

    /// Carries out what a frame's prompts and panels asked for, in order.
    fn finish(
        &mut self,
        app: &mut Application,
        mut full: egui::FullOutput,
        choices: Vec<Choice>,
        actions: Vec<Action>,
    ) -> FrameOutput {
        let mut requests = std::mem::take(&mut self.pending);
        for choice in choices {
            if let Err(e) = self.choose(app, choice, &mut requests) {
                self.error(e);
            }
        }
        // What an action changed shows in the next frame, which nothing else would ask for.
        let acted = !actions.is_empty();
        for action in actions {
            self.act(app, action, &mut requests);
        }
        self.pump(app, None);
        self.note_prompt(app);
        self.save_recent(app);
        // What a controller copied goes to the operating system clipboard.
        if let Some(text) = self.copy_text.take() {
            full.platform_output
                .commands
                .push(egui::OutputCommand::CopyText(text));
        }
        FrameOutput {
            full,
            requests,
            busy: self.busy() || acted,
            settings_changed: std::mem::take(&mut self.settings_changed),
        }
    }

    /// Puts floating tree `at` back after its window changed it: emptied trees are removed,
    /// trees it floated off are appended.
    fn write_back(&mut self, at: usize, own: &DockLayout) {
        let mut floating = self.layout.floating().to_vec();
        if own.panels().is_empty() {
            if at < floating.len() {
                floating.remove(at);
            }
        } else if let Some(tree) = floating.get_mut(at) {
            tree.dock = own.root().clone();
        }
        floating.extend(own.floating().iter().cloned());
        match DockLayout::new(self.layout.root().clone(), floating) {
            Ok(layout) => self.layout = layout,
            Err(e) => self.error(e),
        }
    }

    /// Carries out a choice of the recovery prompt or of a banner.
    fn choose(
        &mut self,
        app: &mut Application,
        choice: Choice,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        match choice {
            Choice::Close(answer) => self.answer_close(app, answer, requests),
            Choice::Later => {
                self.recovery.clear();
                Ok(())
            }
            Choice::TakeOver(tab) => app.take_over(tab),
            Choice::Compare(at) => {
                let Some(e) = self.recovery.get_mut(at) else {
                    return Ok(());
                };
                e.comparison = Some(app.compare(&e.orphan));
                Ok(())
            }
            Choice::Recover(at) | Choice::Discard(at) => {
                let Some(e) = self.recovery.get(at) else {
                    return Ok(());
                };
                if matches!(choice, Choice::Recover(_)) {
                    app.recover(&e.orphan)?;
                    tracing::info!("session recovered");
                } else {
                    app.discard_recovery(&e.orphan)?;
                    tracing::info!("session discarded");
                }
                self.recovery.remove(at);
                Ok(())
            }
        }
    }

    /// Shows the question about the unsaved document `tab` in the window (Save, Discard,
    /// Cancel) instead of a native dialog; `quit` ends the application after the close.
    pub(crate) fn ask_in_window(&mut self, app: &mut Application, tab: TabId, quit: bool) {
        // A tab that cannot be activated is closed already; the question then names nothing.
        if app.activate(tab).is_ok() {
            self.closing = Some(PendingClose { tab, quit });
        }
    }

    /// Carries out the answer to the question of [`Shell::ask_in_window`]. A document never
    /// saved asks for a file first and is asked about again when it is still unsaved.
    fn answer_close(
        &mut self,
        app: &mut Application,
        answer: crate::application::CloseAnswer,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        use crate::application::CloseAnswer;
        let Some(PendingClose { tab, quit }) = self.closing.take() else {
            return Ok(());
        };
        let title = app.tab(tab)?.title();
        match answer {
            CloseAnswer::Cancel => self.announce_kept(app),
            CloseAnswer::Save if app.tab(tab)?.path().is_none() => {
                requests.push(Request::SaveDialog(tab));
                requests.push(if quit {
                    Request::Quit
                } else {
                    Request::AskClose(tab)
                });
            }
            CloseAnswer::Save | CloseAnswer::Discard => {
                self.close_tab(app, tab, |_| answer)?;
                self.announce_closed(app, title);
                if quit {
                    requests.push(Request::Quit);
                }
            }
        }
        Ok(())
    }

    /// Sets or resets a setting in the user scope and writes the user settings file.
    fn set_setting(
        &mut self,
        app: &mut Application,
        key: &str,
        value: Option<SettingValue>,
    ) -> Result<()> {
        let file = self.files.as_ref().map(|f| f.settings.clone());
        let store = app.settings_mut();
        let result = match value {
            Some(v) => settings_editor::set(store, Scope::User, key, v, file.as_deref()),
            None => settings_editor::reset(store, Scope::User, key, file.as_deref()).map(drop),
        };
        self.settings_changed = true;
        self.appearance = prefs::appearance(app.settings(), self.base_appearance);
        result
    }

    /// Carries out one action of a frame.
    fn act(&mut self, app: &mut Application, action: Action, requests: &mut Vec<Request>) {
        match &action {
            Action::Line(LineAction::Submit(text)) => self.echo(app, text),
            Action::Answer(input) => {
                if let Some(text) = answer_text(input) {
                    self.echo(app, &text);
                }
            }
            Action::Start(intent)
                if !matches!(
                    intent.command.as_str(),
                    commands::SELECT_SET | commands::SELECT_CLEAR
                ) =>
            {
                self.push_entry(app, ConsoleEntry::Started(intent.command.clone()));
            }
            Action::Line(LineAction::Cancel | LineAction::CancelThen(_))
                if app
                    .active()
                    .and_then(|t| self.views.get(&t))
                    .is_some_and(|v| v.shown_prompt.is_some())
                    && !self.reports_results(app) =>
            {
                self.announce(app, HISTORY_CANCEL.message_id, Vec::new());
            }
            _ => {}
        }
        let result = match action {
            Action::Error(e) => Err(e),
            Action::Ran(command) => {
                self.note_done(app, &command);
                Ok(())
            }
            Action::Shell(action) => self.shell_action(app, action, requests),
            Action::Setting { key, value } => self.set_setting(app, &key, value),
            // A command of a menu or a key binding starts the way a toolbar button's does: through
            // the tab's controller when it has one, else on the shell's command line.
            Action::Start(intent) if self.controlled(app) => {
                self.controlled_line(app, LineAction::Invoke(intent), requests)
            }
            Action::Start(intent) | Action::Intent(intent) | Action::Dispatch(intent) => {
                self.dispatch(app, intent, requests)
            }
            Action::Answer(input) => self.line_step(app, LineStep::Answer(input), requests),
            Action::Line(line) if self.controlled(app) => self.controlled_line(app, line, requests),
            Action::Line(line) => match line {
                LineAction::ClearInput => Ok(()),
                // With no document open there is no command line, but a file command (a toolbar
                // button's New or Open) must still work.
                LineAction::Submit(text)
                    if app.active().is_none() && commands::is_shell_command(text.trim()) =>
                {
                    self.shell_command(app, text.trim(), Map::new(), requests)
                }
                LineAction::Submit(text) => self.line_step(app, LineStep::Submit(&text), requests),
                LineAction::Repeat => self.line_step(app, LineStep::Repeat, requests),
                LineAction::Back => self.line_step(app, LineStep::Back, requests),
                LineAction::Cancel => {
                    self.cancel_line(app);
                    Ok(())
                }
                LineAction::Invoke(intent) => self.dispatch(app, intent, requests),
                LineAction::CancelThen(intent) => {
                    self.cancel_line(app);
                    self.dispatch(app, intent, requests)
                }
            },
        };
        if let Err(e) = result {
            self.report(app, e);
        }
        self.note_prompt(app);
    }

    /// Carries out a panel or camera action of the View menu or the toolbar.
    fn shell_action(
        &mut self,
        app: &mut Application,
        action: ShellAction,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        match action {
            ShellAction::ActivateTab(tab) => app.activate(tab),
            ShellAction::CloseTab(tab) => {
                app.activate(tab)?;
                self.shell_command(app, commands::FILE_CLOSE, Map::new(), requests)
            }
            // A panel that is shown already comes to the front of its group.
            ShellAction::ShowPanel(panel) if self.layout.contains(&panel) => {
                self.layout.activate(&panel)
            }
            ShellAction::ShowPanel(panel) => {
                chrome::show_panel(&mut self.layout, &self.default_layout, &panel)
            }
            ShellAction::HidePanel(panel) => self.layout.close(&panel),
            ShellAction::ResetLayout => {
                self.layout = self.default_layout.clone();
                Ok(())
            }
            ShellAction::ZoomExtents => {
                if let Some(view) = app.active().and_then(|t| self.views.get_mut(&t)) {
                    view.zoom_extents = true;
                }
                Ok(())
            }
        }
    }

    /// Whether the active tab's view has a product's interaction controller.
    fn controlled(&self, app: &Application) -> bool {
        app.active()
            .and_then(|t| self.views.get(&t))
            .is_some_and(|v| v.interaction.is_some())
    }

    /// Closes tab `tab` through [`Application::close`], calling the tab's controller first (see
    /// [`Interaction::closing`]) when the close goes ahead.
    ///
    /// # Errors
    /// The errors of [`Application::close`].
    pub(crate) fn close_tab(
        &mut self,
        app: &mut Application,
        tab: TabId,
        ask: impl FnOnce(&crate::application::Tab) -> crate::application::CloseAnswer,
    ) -> Result<crate::application::Closed> {
        let views = &mut self.views;
        let language = self.language.as_str();
        app.close_with(tab, ask, |app, tab| {
            let Some(view) = views.get_mut(&tab) else {
                return;
            };
            let Some(controller) = view.interaction.as_deref_mut() else {
                return;
            };
            let session = controller_session(app, tab, language);
            let cx =
                InteractionContext::new(app, tab, &session, &view.selection, view.camera, language);
            controller.closing(&cx);
        })
    }

    /// Hands a command-panel action to the active tab's controller, unless it names a shell
    /// command: the shell carries those out itself, and cancels a running tool first (a save
    /// keeps it running).
    fn controlled_line(
        &mut self,
        app: &mut Application,
        line: LineAction,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        let (command, params, typed) = match &line {
            LineAction::Submit(text) => (
                text.split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                Map::new(),
                Some(text.clone()),
            ),
            LineAction::Invoke(i) | LineAction::CancelThen(i) => {
                (i.command.clone(), i.params.clone(), None)
            }
            _ => return self.controller_line(app, line, requests),
        };
        let claimed = app
            .active()
            .and_then(|t| self.views.get(&t))
            .and_then(|v| v.interaction.as_deref())
            .is_some_and(|c| c.handles(&command));
        if !commands::is_shell_command(&command) || claimed {
            return self.controller_line(app, line, requests);
        }
        if !matches!(
            command.as_str(),
            commands::FILE_SAVE | commands::FILE_SAVE_AS
        ) {
            self.controller_line(app, LineAction::Cancel, requests)?;
        }
        match typed {
            // A typed line may carry parameters (`app.file.open path=<file>`): the shell's own
            // command line reads them.
            Some(text) => self.line_step(app, LineStep::Submit(&text), requests),
            None => self.shell_command(app, &command, params, requests),
        }
    }

    /// Hands an action to the active tab's controller, then carries out what it asked for.
    fn controller_line(
        &mut self,
        app: &mut Application,
        line: LineAction,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        let tab = app
            .active()
            .ok_or_else(|| Error::new(crate::codes::DOCUMENT_UNKNOWN))?;
        let session = controller_session(app, tab, &self.language);
        let Some(view) = self.views.get_mut(&tab) else {
            return Err(Error::new(crate::codes::DOCUMENT_UNKNOWN));
        };
        let cx = InteractionContext::new(
            app,
            tab,
            &session,
            &view.selection,
            view.camera,
            &self.language,
        );
        let result = match view.interaction.as_deref_mut() {
            Some(controller) => controller.line(&cx, line),
            None => Ok(()),
        };
        let (asked, ran, settings) = (cx.take_asked(), cx.take_ran(), cx.take_settings());
        let panels = cx.take_panels();
        if let Some(text) = cx.take_clipboard_text() {
            self.copy_text = Some(text);
        }
        for command in ran {
            self.note_done(app, &command);
        }
        for panel in panels {
            self.shell_action(app, ShellAction::ShowPanel(panel), requests)?;
        }
        for (key, value) in settings {
            self.set_setting(app, &key, value)?;
        }
        for intent in asked {
            self.dispatch(app, intent, requests)?;
        }
        result
    }

    /// Runs `then` with the file a path dialog returned as its `path`, on `tab`: through the
    /// tab's controller when it has one, else on the shell's command line. A dialog never
    /// asks again for a command that has its path.
    pub(crate) fn run_with_path(
        &mut self,
        app: &mut Application,
        tab: TabId,
        mut then: Intent,
        path: &std::path::Path,
    ) -> Result<()> {
        app.activate(tab)?;
        // shortcut: a path that is not valid text loses the invalid parts; upgrade when a
        // command takes paths as bytes.
        then.params.insert(
            "path".to_owned(),
            Json::String(path.to_string_lossy().into_owned()),
        );
        let mut requests = Vec::new();
        let result = if self.controlled(app) {
            self.controlled_line(app, LineAction::Invoke(then), &mut requests)
        } else {
            self.dispatch(app, then, &mut requests)
        };
        if let Err(e) = result {
            self.report(app, e);
        }
        // What the command asked for (a file dialog of its own) goes out with the next frame.
        self.pending.extend(requests);
        self.note_prompt(app);
        Ok(())
    }

    fn cancel_line(&mut self, app: &Application) {
        if let Some(view) = app.active().and_then(|t| self.views.get_mut(&t)) {
            let _ = view.line.cancel();
        }
    }

    /// Carries out an intent: a shell command here, any other command started through the
    /// active tab's command line, which runs it through [`Application::invoke`] once its
    /// parameters are complete and prompts otherwise.
    ///
    /// # Errors
    /// `app.document.unknown` for a document command without an open document, and the errors
    /// of the command.
    pub fn dispatch(
        &mut self,
        app: &mut Application,
        intent: Intent,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        if commands::is_shell_command(&intent.command) {
            return self.shell_command(app, &intent.command, intent.params, requests);
        }
        if !intent.params.contains_key("path")
            && let (Some(dialog), Some(tab)) =
                (app.product().path_dialog(&intent.command), app.active())
        {
            requests.push(Request::PathDialog {
                tab,
                dialog: dialog.clone(),
                then: intent,
            });
            return Ok(());
        }
        self.line_step(
            app,
            LineStep::Start {
                command: &intent.command,
                preset: intent.params,
            },
            requests,
        )
    }

    /// Takes a command-line step on the active tab. Completed invocations of document commands
    /// go to [`Application::invoke`]; shell commands are carried out by the shell afterwards.
    ///
    /// # Errors
    /// `app.document.unknown` without an active tab, and the errors of the step.
    pub fn line_step(
        &mut self,
        app: &mut Application,
        step: LineStep<'_>,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        let tab = app
            .active()
            .ok_or_else(|| Error::new(crate::codes::DOCUMENT_UNKNOWN))?;
        self.sync_views(app);
        let Some(mut view) = self.views.remove(&tab) else {
            return Err(Error::new(crate::codes::DOCUMENT_UNKNOWN));
        };
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let seed = self.seed;
        let mut deferred: Vec<(String, Json)> = Vec::new();
        // The selection a command set (`Context::set_selection`) becomes the tab's selection.
        let mut chosen: Option<Vec<ObjectId>> = None;
        let result = {
            let app_ref: &Application = app;
            let document = app_ref.tab(tab)?.document();
            let registry = &app_ref.product().headless().registry;
            let localized = BTreeMap::new();
            let target = Target {
                registry,
                document,
                session: &self.line_session,
                selection: &view.selection,
                localized: &localized,
                seed,
            };
            let log = &mut self.log;
            let recorder = &self.recorder;
            let session = &self.ui_session;
            let mut runner = |invocation: Invocation| -> Result<Outcome> {
                if commands::is_shell_command(&invocation.command) {
                    deferred.push((invocation.command.clone(), invocation.params));
                    return Ok(Outcome {
                        command: invocation.command,
                        value: Json::Null,
                        revision: document.snapshot().revision(),
                        txn_id: None,
                        coalesce: None,
                        diagnostics: Vec::new(),
                        selection: None,
                    });
                }
                if log.len() >= MAX_LOG {
                    log.remove(0);
                }
                log.push(invocation.command.clone());
                let revision = document.snapshot().revision();
                let recorded = recorder.as_ref().map(|r| (r, invocation.clone()));
                let outcome = app_ref.invoke(tab, session, invocation)?;
                if let Some(ids) = &outcome.selection {
                    chosen = Some(ids.clone());
                }
                if let Some((recorder, invocation)) = recorded {
                    // A step the recorder refuses (a full recorder) does not fail the run.
                    let _ = recorder.record(registry, &invocation, revision);
                }
                Ok(outcome)
            };
            view.line.drive(&target, step, &mut runner)
        };
        if let Some(ids) = chosen {
            let live = app.tab(tab)?.document().snapshot();
            view.selection.clear();
            select(
                &mut view.selection,
                ids.into_iter().filter(|o| live.resolve(*o).is_ok()),
                SelectMode::Add,
            );
        }
        self.views.insert(tab, view);
        let submitted = result?;
        match submitted {
            Submitted::Rejected { error, .. } => self.report(app, error),
            Submitted::Ran(outcome) if !commands::is_shell_command(&outcome.command) => {
                self.note_done(app, &outcome.command);
            }
            _ => {}
        }
        for (command, params) in deferred {
            let params = match params {
                Json::Object(map) => map,
                _ => Map::new(),
            };
            self.shell_command(app, &command, params, requests)?;
        }
        Ok(())
    }

    fn shell_command(
        &mut self,
        app: &mut Application,
        command: &str,
        params: Map<String, Json>,
        requests: &mut Vec<Request>,
    ) -> Result<()> {
        let active = app.active();
        let need = || active.ok_or_else(|| Error::new(crate::codes::DOCUMENT_UNKNOWN));
        let path = params
            .get("path")
            .and_then(Json::as_str)
            .map(std::path::PathBuf::from);
        match command {
            commands::FILE_NEW => {
                let language = self.language.clone();
                app.new_document_in(&language)?;
                self.announce(app, FILE_NEW_DONE.message_id, Vec::new());
                Ok(())
            }
            commands::FILE_OPEN => match path {
                Some(p) => {
                    let tab = app.open(&p)?;
                    self.announced_file(app, tab, FILE_OPENED.message_id);
                    Ok(())
                }
                None => {
                    requests.push(Request::OpenDialog);
                    Ok(())
                }
            },
            commands::FILE_SAVE => {
                let tab = need()?;
                if app.tab(tab)?.path().is_some() {
                    app.save(tab)?;
                    self.announced_file(app, tab, FILE_SAVED.message_id);
                    Ok(())
                } else {
                    requests.push(Request::SaveDialog(tab));
                    Ok(())
                }
            }
            commands::FILE_SAVE_AS => {
                let tab = need()?;
                match path {
                    Some(p) => {
                        app.save_as(tab, &p)?;
                        self.announced_file(app, tab, FILE_SAVED.message_id);
                        Ok(())
                    }
                    None => {
                        requests.push(Request::SaveDialog(tab));
                        Ok(())
                    }
                }
            }
            commands::FILE_CLOSE => {
                let tab = need()?;
                if app.tab(tab)?.is_dirty() {
                    requests.push(Request::AskClose(tab));
                    Ok(())
                } else {
                    let title = app.tab(tab)?.title();
                    self.close_tab(app, tab, |_| crate::application::CloseAnswer::Cancel)?;
                    self.announce_closed(app, title);
                    Ok(())
                }
            }
            commands::APP_QUIT => {
                requests.push(Request::Quit);
                Ok(())
            }
            commands::MACRO_RECORD_START => {
                let core = app.product().headless();
                self.recorder = Some(Recorder::new(&core.name, &core.version, &Limits::DEFAULT));
                Ok(())
            }
            commands::MACRO_RECORD_STOP => {
                if let Some(recorder) = self.recorder.take() {
                    requests.push(Request::SaveMacro(recorder.text()));
                }
                Ok(())
            }
            commands::EDIT_UNDO => app.undo(need()?).map(drop),
            commands::EDIT_REDO => app.redo(need()?).map(drop),
            commands::SELECT_SET | commands::SELECT_CLEAR => {
                self.select_command(app, need()?, command, params)
            }
            _ => Ok(()),
        }
    }
}

impl Shell {
    /// Carries out `app.select.set` or `app.select.clear` on `tab`'s view.
    fn select_command(
        &mut self,
        app: &Application,
        tab: TabId,
        command: &str,
        params: Map<String, Json>,
    ) -> Result<()> {
        let snapshot = app.tab(tab)?.document().snapshot();
        let Some(view) = self.views.get_mut(&tab) else {
            return Ok(());
        };
        if command == commands::SELECT_CLEAR {
            view.selection.clear();
            return Ok(());
        }
        let p: SelectParams = serde_json::from_value(Json::Object(params))
            .map_err(|e| Error::new(arc_command::error::codes::PARAMS_MALFORMED).with_source(e))?;
        let objects = p
            .objects
            .into_iter()
            .map(|o| o.0)
            .filter(|o| snapshot.resolve(*o).is_ok());
        select(&mut view.selection, objects, p.mode);
        Ok(())
    }
}

const FILE_NEW_DONE: Message = Message {
    message_id: "app-file-new-done",
};
const FILE_OPENED: Message = Message {
    message_id: "app-file-opened",
};
const FILE_SAVED: Message = Message {
    message_id: "app-file-saved",
};
const FILE_CLOSED: Message = Message {
    message_id: "app-file-closed",
};
const FILE_CLOSED_NEW: Message = Message {
    message_id: "app-file-closed-new",
};
const HISTORY_COMMAND: Message = Message {
    message_id: "app-history-command",
};
const HISTORY_CANCEL: Message = Message {
    message_id: "app-history-cancel",
};
const HISTORY_RAN: Message = Message {
    message_id: "app-history-ran",
};
const FILE_KEPT: Message = Message {
    message_id: "app-file-kept-open",
};

impl Shell {
    /// Says that the file of `tab` was opened or saved (`message_id`), naming it.
    fn announced_file(&mut self, app: &Application, tab: TabId, message_id: &'static str) {
        let name = app.tab(tab).ok().and_then(crate::application::Tab::title);
        if let Some(name) = name {
            self.announce(app, message_id, vec![("name", name.into())]);
        }
    }

    /// Says that a document was closed; `title` is its file name, none for a new document.
    fn announce_closed(&mut self, app: &Application, title: Option<String>) {
        match title {
            Some(name) => self.announce(app, FILE_CLOSED.message_id, vec![("name", name.into())]),
            None => self.announce(app, FILE_CLOSED_NEW.message_id, Vec::new()),
        }
    }

    /// Says that closing a document with unsaved changes did not happen: the question was
    /// answered with Cancel, or no dialog could show it. Nothing is lost, and the line says
    /// how to go on.
    pub(crate) fn announce_kept(&mut self, app: &Application) {
        self.announce(app, FILE_KEPT.message_id, Vec::new());
    }
}

/// The name of the current log file in the log directory.
pub const LOG_FILE: &str = "shell.log";

/// Installs the process-wide log subscriber from the log settings of `app` (`log.level`,
/// `log.max_file_bytes`, `log.max_files`, read once: a change applies at the next start),
/// writing rotated files under `dir` and keeping recent records in the returned ring for the
/// log viewer.
///
/// # Errors
/// The errors of [`LogConfig::from_settings`](crate::logging::LogConfig::from_settings) and
/// [`subscriber_with_ring`](crate::logging::subscriber_with_ring); `app.log.io` when a
/// subscriber is installed already.
pub fn install_logging(app: &Application, dir: PathBuf) -> Result<LogRing> {
    let config = crate::logging::LogConfig::from_settings(dir, LOG_FILE, app.settings())?;
    let ring = LogRing::new();
    let subscriber = crate::logging::subscriber_with_ring(&config, ring.clone())?;
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|e| Error::new(crate::codes::LOG_IO).with_source(e))?;
    Ok(ring)
}

/// The caller's clock in milliseconds; a clock beyond u64 milliseconds saturates.
fn millis(time: f64) -> u64 {
    if time.is_finite() && time > 0.0 {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "saturating"
        )]
        let m = (time * 1000.0) as u64;
        m
    } else {
        0
    }
}

/// The largest event coordinate, delta or factor magnitude accepted, in points.
pub const MAX_EVENT_MAGNITUDE: f32 = 1e6;

/// True when every number of `event` is finite and at most [`MAX_EVENT_MAGNITUDE`] in
/// magnitude: egui sums wheel deltas, which overflows to infinity and NaN from finite input.
fn finite(event: &egui::Event) -> bool {
    use egui::Event;
    let ok = |v: f32| v.is_finite() && v.abs() <= MAX_EVENT_MAGNITUDE;
    match event {
        Event::PointerMoved(p) | Event::PointerButton { pos: p, .. } => ok(p.x) && ok(p.y),
        Event::MouseMoved(v) | Event::MouseWheel { delta: v, .. } => ok(v.x) && ok(v.y),
        Event::Zoom(f) | Event::Rotate(f) => ok(*f),
        Event::Touch { pos, force, .. } => ok(pos.x) && ok(pos.y) && force.is_none_or(ok),
        _ => true,
    }
}

/// Applies a selection change; the selection stays free of repeats and within
/// [`MAX_SELECTION`].
fn select(
    selection: &mut Vec<ObjectId>,
    objects: impl Iterator<Item = ObjectId>,
    mode: SelectMode,
) {
    if mode == SelectMode::Replace {
        selection.clear();
    }
    for o in objects {
        match selection.iter().position(|s| *s == o) {
            Some(at) if mode == SelectMode::Toggle => {
                selection.remove(at);
            }
            Some(_) => {}
            None if selection.len() < MAX_SELECTION => selection.push(o),
            None => {}
        }
    }
}

fn factory_error(id: &str) -> Error {
    Error::new(crate::codes::PRODUCT_INVALID_ID).with_arg("id", id)
}

fn viewport_config(camera: ViewFrame, colors: &Colors) -> ViewportConfig {
    let mut config = ViewportConfig::new(64, 64, camera);
    config.readback = false;
    config.settings = render_settings(colors);
    config
}

/// The render settings of a viewport: a transparent background, so the widget's grid and axes
/// show through under the drawing.
fn render_settings(_colors: &Colors) -> arc_gpu::RenderSettings {
    arc_gpu::RenderSettings {
        // The egui renderer samples the frame as premultiplied: transparent black adds nothing.
        background: arc_base::Color::from_srgb8([0, 0, 0, 0]),
        ..arc_gpu::RenderSettings::DEFAULT
    }
}

/// Frames the camera on everything in the viewport's scene: after a document opens (until its
/// first change) and on a zoom-extents request. A scene that is not known yet waits for a later
/// pass; an empty one ends an explicit request.
fn frame_drawing(
    viewport: Option<&Viewport>,
    size: [u32; 2],
    camera: &mut ViewFrame,
    framed: &mut bool,
    requested: &mut bool,
    actions: &mut Vec<Action>,
) {
    let Some(viewport) = viewport else {
        return;
    };
    let bounds = ViewportPicker(Some(viewport), None).bounds();
    let Some((center, radius)) = bounds else {
        *requested = false;
        return;
    };
    let aspect = f64::from(size[0].max(1)) / f64::from(size[1].max(1));
    match arc_gpu::navigation::zoom_to_sphere(
        camera,
        &center,
        radius * 1.1,
        aspect,
        &Tolerance::DEFAULT,
    ) {
        Ok(next) => {
            *camera = next;
            *framed = true;
            *requested = false;
        }
        Err(e) => {
            *requested = false;
            actions.push(Action::Error(e));
        }
    }
}

/// The screen position of the viewport pixel `px`, in logical points.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a position in logical points, far inside the range of f32"
)]
fn point_at(rect: egui::Rect, px: [f64; 2], ppp: f64) -> egui::Pos2 {
    egui::Pos2::new(
        rect.min.x + (px[0] / ppp) as f32,
        rect.min.y + (px[1] / ppp) as f32,
    )
}

/// Applies a view request of the product to the camera: the framing itself is the zoom-extents pass.
fn apply_view(
    request: ViewRequest,
    camera: &mut ViewFrame,
    previous: &mut Vec<ViewFrame>,
    zoom_extents: &mut bool,
    size: [u32; 2],
    actions: &mut Vec<Action>,
) {
    /// The most cameras kept for the Previous form.
    const KEPT: usize = 32;
    if request == ViewRequest::Previous {
        if let Some(last) = previous.pop() {
            *camera = last;
        }
        return;
    }
    if previous.len() >= KEPT {
        previous.remove(0);
    }
    previous.push(*camera);
    let aspect = f64::from(size[0].max(1)) / f64::from(size[1].max(1));
    let next = match request {
        ViewRequest::Extents | ViewRequest::Previous => {
            *zoom_extents = true;
            return;
        }
        ViewRequest::Window { a, b } => {
            let (w, h) = ((a[0] - b[0]).abs(), (a[1] - b[1]).abs());
            // The sphere that frames the window: the fit takes the larger of the two directions.
            let height = h.max(w / aspect);
            let center = Point3::new((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, camera.target().z);
            arc_gpu::navigation::zoom_to_sphere(
                camera,
                &center,
                height / 2.0 * aspect.min(1.0),
                aspect,
                &Tolerance::DEFAULT,
            )
        }
        ViewRequest::Scale(factor) => {
            arc_gpu::navigation::zoom_at(camera, factor, &camera.target())
        }
    };
    match next {
        Ok(frame) => *camera = frame,
        Err(e) => actions.push(Action::Error(e)),
    }
}

/// Picking through a tab's [`Viewport`] on its render thread.
struct ViewportPicker<'a>(
    Option<&'a Viewport>,
    Option<&'a over_viewport::DisplayState>,
);

impl ViewPicker for ViewportPicker<'_> {
    fn pick(
        &mut self,
        frame: &ViewFrame,
        width: u32,
        height: u32,
        region: &PickRegion,
    ) -> Result<Vec<PickHit>> {
        let Some(viewport) = self.0 else {
            return Ok(Vec::new());
        };
        let (frame, region) = (*frame, *region);
        let mut hits = viewport.with_scene(WAIT, move |renderer, scene, settings| {
            renderer.pick(scene, &frame, width, height, settings, &region)
        })??;
        // Hidden and isolated-out objects are not picked.
        if let Some(display) = self.1 {
            hits.retain(|hit| display.visible(&hit.key.object));
        }
        Ok(hits)
    }

    fn bounds(&mut self) -> Option<(Point3, f64)> {
        self.0?
            .with_scene(WAIT, |_, scene, _| scene.bounding_sphere(None))
            .ok()
            .flatten()
    }
}

/// The viewport panel (CORE-UI-005 in the shell): the active tab's viewport with selection
/// gestures, prompt answers and navigation.
#[derive(Debug, Default)]
pub struct ViewportPanel {
    /// The commands selection intents name.
    pub commands: ViewportCommands,
}

impl ViewportPanel {
    /// A viewport panel whose intents name the shell's selection commands.
    #[must_use]
    pub fn new() -> Self {
        Self {
            commands: ViewportCommands {
                select: commands::SELECT_SET.into(),
                clear_selection: commands::SELECT_CLEAR.into(),
            },
        }
    }
}

impl Panel for ViewportPanel {
    #[allow(clippy::too_many_lines, reason = "one frame of the viewport")]
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let Some(view) = cx.view.as_deref_mut() else {
            return;
        };
        let View {
            widget,
            viewport,
            camera,
            line,
            selection,
            hover,
            interaction,
            cursor,
            frame_revision,
            framed,
            zoom_extents,
            previous,
            cursor_digits,
            viewport_rect,
            display,
            ..
        } = view;
        // With a controller the controller's prompt and overlays are the viewport's; its
        // selection is too, when it reports one of its own.
        let controlled = cx
            .tab
            .map(|tab| controller_session(cx.app, tab, cx.language));
        let icx = cx.tab.zip(controlled.as_ref()).map(|(tab, session)| {
            InteractionContext::new(
                cx.app,
                tab,
                session,
                selection.as_slice(),
                *camera,
                cx.language,
            )
        });
        // The controller sees the document and the selection as they are this frame before it
        // says what to show, so overlays follow an undo or a selection made elsewhere.
        if let (Some(controller), Some(icx)) = (interaction.as_deref_mut(), icx.as_ref()) {
            controller.frame(icx);
        }
        let frame = match (interaction.as_deref(), cx.tab) {
            (Some(controller), Some(_)) => Some(controller_output(controller, selection)),
            _ => None,
        };
        let prompt = match frame {
            Some(_) => None,
            None => line.active().and_then(|s| s.prompt()),
        };
        let highlight: &[ObjectId] = frame
            .as_ref()
            .and_then(|f| f.selection.as_deref())
            .unwrap_or(selection.as_slice());
        let selected = highlight.len();
        let mut picker = ViewportPicker(viewport.as_ref(), Some(&*display));
        let mut hook = match (interaction.as_deref_mut(), icx.as_ref()) {
            (Some(controller), Some(cx)) => Some(Hook {
                controller,
                cx,
                used: false,
            }),
            _ => None,
        };
        // The product's own overlays (labels) show with the controller's.
        let overlays: Vec<arc_ui::ProductOverlay> = match &frame {
            Some(f) if !cx.overlays.is_empty() => {
                cx.overlays.iter().chain(&f.overlays).cloned().collect()
            }
            Some(f) => f.overlays.clone(),
            None => Vec::new(),
        };
        let product = match &frame {
            Some(f) => Some(ProductInput {
                pointer: hook.as_mut().map(|h| h as &mut dyn PointerHandler),
                overlays: &overlays,
                dynamic: f.dynamic.as_ref().map(|d| DynamicPanel {
                    prompt: &d.prompt,
                    fields: &d.fields,
                }),
                work_plane: cx.work_plane,
            }),
            None => (!cx.overlays.is_empty() || cx.work_plane.is_some()).then_some(ProductInput {
                pointer: None,
                overlays: cx.overlays,
                dynamic: None,
                work_plane: cx.work_plane,
            }),
        };
        widget.blocked = Some(ViewCube::rect(
            ui.available_rect_before_wrap(),
            ui.spacing().interact_size.y * 4.0,
        ));
        let out = widget.show(
            ui,
            ViewportInput {
                texture: cx.texture,
                frame: camera,
                prompt,
                selected,
                picker: &mut picker,
                commands: &self.commands,
                localize: cx.localize,
                colors: cx.colors,
                scale: arc_ui::UiScale::default(),
                tolerance: &Tolerance::DEFAULT,
                name: "",
                time: cx.time,
                product,
            },
        );
        if out.claim_cancelled {
            cx.actions.push(Action::Line(LineAction::Cancel));
        }
        if hook.as_ref().is_some_and(|h| h.used) {
            ui.ctx().request_repaint();
        }
        if let (Some(controller), Some(icx)) = (interaction.as_deref_mut(), icx.as_ref()) {
            for event in &out.dynamic {
                controller.dynamic(icx, event);
            }
        }
        // What the controller asked for during a pointer event or a dynamic input edit.
        if let Some(icx) = icx.as_ref() {
            cx.actions
                .extend(icx.take_ran().into_iter().map(Action::Ran));
            cx.actions
                .extend(icx.take_asked().into_iter().map(Action::Dispatch));
            cx.actions.extend(
                icx.take_settings()
                    .into_iter()
                    .map(|(key, value)| Action::Setting { key, value }),
            );
            cx.actions.extend(
                icx.take_panels()
                    .into_iter()
                    .map(|p| Action::Shell(ShellAction::ShowPanel(p))),
            );
        }
        *viewport_rect = Some(out.response.rect);
        // The controller's context menu, its temporary display state and the banner that says so.
        if let (Some(controller), Some(icx)) = (interaction.as_deref(), icx.as_ref()) {
            over_viewport::context_menu(
                &out.response,
                cx.localize,
                &|| controller.context_menu(icx),
                cx.actions,
            );
        }
        if let Some(f) = &frame {
            display.update(&f.hidden, f.isolated.as_deref());
            if let Some(text) = &f.banner {
                over_viewport::paint_banner(ui, out.response.rect, text, cx.colors);
            }
        }
        if let (Some(vp), Some(overrides)) = (viewport.as_ref(), display.pending())
            && let Err(e) = vp.set_overrides(overrides)
        {
            cx.actions.push(Action::Error(e));
        }
        if let Some(c) = out.camera {
            *camera = c;
            // The user's own navigation settles the framing.
            *framed = true;
        }
        if let Some(request) = interaction.as_deref_mut().and_then(|c| c.take_view()) {
            apply_view(
                request,
                camera,
                previous,
                zoom_extents,
                out.size,
                cx.actions,
            );
            *framed = true;
            ui.ctx().request_repaint();
        }
        // The pointer the controller resolved (snap step, ortho, object snap) is where the
        // crosshair sits and what the status bar reads.
        let resolved = frame.as_ref().and_then(|f| f.cursor);
        *cursor_digits = frame.as_ref().and_then(|f| f.precision).unwrap_or(3);
        let ppp = f64::from(ui.ctx().pixels_per_point());
        let at = resolved
            .filter(|_| out.pointer_model.is_some())
            .and_then(|[x, y]| out.projection.model_to_pixel(&Point3::new(x, y, 0.0)))
            .map(|px| point_at(out.response.rect, px, ppp));
        cube_and_crosshair(
            ui,
            &out.response.rect,
            camera,
            (cx.localize, cx.colors),
            at,
            frame.as_ref().and_then(|f| f.pointer_icon),
            cx.actions,
        );
        *cursor = out.pointer_model.map(|p| resolved.unwrap_or([p.x, p.y]));
        *hover = out.hover;
        let overlay = (!out.overlay.primitives.is_empty()).then_some(out.overlay);
        let revision = cx
            .tab
            .and_then(|t| cx.app.tab(t).ok())
            .map(|t| t.document().snapshot().revision());
        if *zoom_extents || (!*framed && revision == Some(*frame_revision)) {
            frame_drawing(
                viewport.as_ref(),
                out.size,
                camera,
                framed,
                zoom_extents,
                cx.actions,
            );
        }
        sync_viewport(
            viewport.as_ref(),
            out.size,
            camera,
            *hover,
            highlight,
            overlay,
            cx.colors,
            cx.actions,
        );
        cx.actions.extend(out.errors.into_iter().map(Action::Error));
        cx.actions.extend(out.intent.map(Action::Intent));
        cx.actions.extend(out.answer.map(Action::Answer));
    }
}

/// Draws the view cube and the crosshair over the viewport at `rect` and applies what the cube
/// asked for: a click snaps the camera to that view at once (the shell has no animation tick, so
/// the end frame of the transition is `interpolate` at t = 1), a drag orbits it.
fn cube_and_crosshair(
    ui: &mut Ui,
    rect: &egui::Rect,
    camera: &mut ViewFrame,
    (localize, colors): (&dyn Localize, &Colors),
    at: Option<egui::Pos2>,
    icon: Option<egui::CursorIcon>,
    actions: &mut Vec<Action>,
) {
    let cube = ViewCubeInput {
        frame: camera,
        localize,
        colors,
        size: ui.spacing().interact_size.y * 4.0,
    };
    let out = ViewCube.show_over_dragged(ui, *rect, &cube);
    if let Some(intent) = out.intent {
        match intent
            .frame_from(camera)
            .and_then(|end| arc_gpu::navigation::interpolate(camera, &end, 1.0))
        {
            Ok(end) => *camera = end,
            Err(e) => actions.push(Action::Error(e)),
        }
    }
    if let Some(drag) = out.drag.filter(|d| *d != [0.0, 0.0]) {
        match arc_ui::orbit_by_drag(camera, drag, &Tolerance::DEFAULT) {
            Ok(next) => *camera = next,
            Err(e) => actions.push(Action::Error(e)),
        }
    }
    if !out.over
        && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        && rect.contains(pointer)
    {
        let pointer = at.filter(|p| rect.contains(*p)).unwrap_or(pointer);
        match icon {
            Some(icon) => ui.ctx().set_cursor_icon(icon),
            None => arc_ui::paint_crosshair(ui, *rect, colors, pointer),
        }
    }
}

/// Hands the frame's size, camera, highlight and overlay to the viewport; failures become
/// actions.
#[allow(
    clippy::too_many_arguments,
    reason = "one hand-over of the frame's state"
)]
fn sync_viewport(
    viewport: Option<&Viewport>,
    size: [u32; 2],
    camera: &ViewFrame,
    hover: Option<ObjectId>,
    highlight: &[ObjectId],
    overlay: Option<arc_gpu::Overlay>,
    colors: &Colors,
    actions: &mut Vec<Action>,
) {
    let Some(vp) = viewport else {
        return;
    };
    let results = [
        vp.resize(size[0], size[1]),
        vp.set_camera(*camera),
        vp.set_highlight(hover, highlight),
        vp.set_overlay(overlay),
        vp.set_settings(render_settings(colors)),
    ];
    actions.extend(
        results
            .into_iter()
            .filter_map(Result::err)
            .map(Action::Error),
    );
}

/// The property grid panel: the fields of the selected objects; an edit becomes an intent
/// for the product's edit command.
#[derive(Debug)]
pub struct PropertiesPanel {
    /// The command an edit names, with `objects`, `field` and `value`.
    pub edit_command: String,
}

impl PropertiesPanel {
    /// A grid whose edits name `edit_command`.
    #[must_use]
    pub fn new(edit_command: impl Into<String>) -> Self {
        Self {
            edit_command: edit_command.into(),
        }
    }
}

impl Panel for PropertiesPanel {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let (Some(view), Some(tab)) = (cx.view.as_deref_mut(), cx.tab) else {
            return;
        };
        let Ok(tab) = cx.app.tab(tab) else {
            return;
        };
        let snapshot = tab.document().snapshot();
        let registry = snapshot.registry();
        let objects: Vec<(
            ObjectId,
            Arc<arc_doc::Object>,
            &[arc_doc::schema::FieldDecl],
        )> = view
            .selection
            .iter()
            .filter_map(|id| {
                let handle = snapshot.resolve(*id).ok()?;
                let object = Arc::clone(snapshot.get(handle).ok()?);
                let decl = registry.get(snapshot.type_key(handle)?.as_str())?;
                Some((*id, object, decl.schema.fields.as_slice()))
            })
            .collect();
        let shown: Vec<PropertyObject<'_>> = objects
            .iter()
            .map(|(id, object, fields)| PropertyObject {
                id: *id,
                fields,
                values: &object.values,
                read_only: &[],
                overridden: &[],
            })
            .collect();
        let model = match PropertyModel::build(&shown) {
            Ok(m) => m,
            Err(e) => {
                cx.actions.push(Action::Error(e));
                return;
            }
        };
        let units = arc_base::DisplayUnits::default();
        let label = |name: &str| name.to_owned();
        let context = GridContext {
            localize: cx.localize,
            label: &label,
            units: &units,
            decimal_separator: '.',
            edit_command: &self.edit_command,
        };
        if let Some(intent) = view.grid.show(ui, &model, &context) {
            cx.actions.push(Action::Intent(intent));
        }
    }
}

/// The last part of a type key with underscores as spaces and a capital: `core.draft.line` is
/// `Line`.
fn readable_type(key: &str) -> String {
    let last = key.rsplit('.').next().unwrap_or(key).replace('_', " ");
    let mut chars = last.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// The objects of a snapshot as a flat outliner, in identifier order. Labels are the type key
/// and the identifier, or the `name` text field when the object has one.
struct Objects<'a> {
    language: &'a str,
    items: Vec<TreeItem>,
    _snapshot: &'a arc_doc::Snapshot,
}

impl<'a> Objects<'a> {
    fn new(snapshot: &'a arc_doc::Snapshot, selection: &[ObjectId], language: &'a str) -> Self {
        let registry = snapshot.registry();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut items: Vec<TreeItem> = snapshot
            .iter()
            .map(|(handle, object)| {
                let key = snapshot.type_key(handle).map_or("", |k| k.as_str());
                let name = registry.get(key).and_then(|decl| {
                    let at = decl.schema.fields.iter().position(|f| f.name == "name")?;
                    match object.values.get(at)? {
                        Some(arc_doc::Value::String(s)) => Some(s.clone()),
                        _ => None,
                    }
                });
                // A readable type name and a count: the last part of the type key, capitalized.
                let kind = readable_type(key);
                let n = counts.entry(kind.clone()).or_insert(0);
                *n += 1;
                TreeItem {
                    id: object.id,
                    label: name.unwrap_or_else(|| format!("{kind} {n}")),
                    has_children: false,
                    selected: selection.contains(&object.id),
                    visible: None,
                    locked: None,
                }
            })
            .collect();
        items.sort_by_key(|i| i.id);
        Self {
            language,
            items,
            _snapshot: snapshot,
        }
    }

    fn matching<'s>(&'s self, filter: &'s str) -> impl Iterator<Item = &'s TreeItem> + 's {
        self.items
            .iter()
            .filter(move |i| arc_ui::text::matches_filter(&i.label, filter, self.language))
    }
}

impl TreeSource for Objects<'_> {
    fn child_count(&self, parent: Option<ObjectId>, filter: &str) -> usize {
        if parent.is_some() {
            return 0;
        }
        self.matching(filter).count()
    }

    fn child(&self, parent: Option<ObjectId>, index: usize, filter: &str) -> Option<TreeItem> {
        if parent.is_some() {
            return None;
        }
        self.matching(filter).nth(index).cloned()
    }

    fn index_of(&self, parent: Option<ObjectId>, id: ObjectId, filter: &str) -> Option<usize> {
        if parent.is_some() {
            return None;
        }
        self.matching(filter).position(|i| i.id == id)
    }
}

/// The outliner panel: every object of the document, selectable.
#[derive(Debug)]
pub struct OutlinerPanel {
    commands: TreeCommands,
}

impl Default for OutlinerPanel {
    fn default() -> Self {
        // Rename, visibility, lock and move need product commands; the sample set has none,
        // so those intents name the selection command and are refused by its parameters.
        Self {
            commands: TreeCommands {
                select: commands::SELECT_SET.into(),
                rename: commands::SELECT_SET.into(),
                set_visible: commands::SELECT_SET.into(),
                set_locked: commands::SELECT_SET.into(),
                move_to: commands::SELECT_SET.into(),
            },
        }
    }
}

impl OutlinerPanel {
    /// An outliner whose intents name `commands`.
    #[must_use]
    pub fn new(commands: TreeCommands) -> Self {
        Self { commands }
    }
}

impl Panel for OutlinerPanel {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let (Some(view), Some(tab)) = (cx.view.as_deref_mut(), cx.tab) else {
            return;
        };
        let Ok(tab) = cx.app.tab(tab) else {
            return;
        };
        let snapshot = tab.document().snapshot();
        let source = Objects::new(&snapshot, &view.selection, cx.language);
        if let Some(intent) = view.tree.show(ui, &source, &self.commands, cx.localize) {
            cx.actions.push(Action::Intent(intent));
        }
    }
}

/// A line of the command panel's history.
enum ConsoleLine {
    /// A line of the history.
    Text(String),
    /// An error, already in the language in effect.
    Error(String),
}

impl ConsoleLine {
    fn of(entry: &ConsoleEntry, localize: &dyn Localize, registry: &arc_command::Registry) -> Self {
        // The name a person reads for a command: its label, else the identifier.
        let name = |id: &str| {
            registry.get(id).map_or_else(
                || id.to_owned(),
                |s| arc_ui::resolve(localize, s.label(), &[]),
            )
        };
        match entry {
            ConsoleEntry::Message(error) => {
                let args: Vec<(&str, arc_base::error::Value)> =
                    error.args.iter().map(|(k, v)| (*k, v.clone())).collect();
                let text = arc_ui::resolve(localize, error.message_id, &args);
                if error.info {
                    Self::Text(text)
                } else {
                    Self::Error(text)
                }
            }
            ConsoleEntry::Typed(text) => Self::Text(arc_ui::resolve(
                localize,
                HISTORY_COMMAND.message_id,
                &[("text", text.as_str().into())],
            )),
            ConsoleEntry::Started(id) => Self::Text(arc_ui::resolve(
                localize,
                HISTORY_COMMAND.message_id,
                &[("text", name(id).into())],
            )),
            ConsoleEntry::Prompt(prompt, answer) => {
                let mut text = arc_ui::prompt_text(prompt, localize);
                if let Some(answer) = answer {
                    if !text.ends_with(':') {
                        text.push(':');
                    }
                    text.push(' ');
                    text.push_str(answer);
                }
                Self::Text(text)
            }
            ConsoleEntry::Done(id) => Self::Text(arc_ui::resolve(
                localize,
                HISTORY_RAN.message_id,
                &[("command", name(id).into())],
            )),
        }
    }
}

/// The text of an answer the viewport or the option bar gave, for the history; `None` for the
/// answers that are points or objects, which the viewport shows itself.
fn answer_text(input: &PromptInput) -> Option<String> {
    match input {
        PromptInput::Keyword(t) | PromptInput::Text(t) | PromptInput::Expression(t) => {
            Some(t.clone())
        }
        PromptInput::Distance(d) => Some(d.to_string()),
        PromptInput::Angle(a) => Some(a.to_degrees().to_string()),
        _ => None,
    }
}

/// Draws the history in the space that is left: monospace lines, errors in the error color,
/// scrolled to the newest line.
fn show_console(ui: &mut Ui, lines: &[ConsoleLine], colors: &Colors) {
    egui::ScrollArea::vertical()
        .id_salt("arc-command-console")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in lines {
                let (text, color) = match line {
                    ConsoleLine::Text(t) => (t.as_str(), colors.text),
                    ConsoleLine::Error(t) => (t.as_str(), colors.error),
                };
                ui.add(
                    egui::Label::new(egui::RichText::new(text).monospace().color(color))
                        .wrap()
                        .selectable(true),
                );
            }
        });
}

/// Lays out the command panel: the input pinned at the bottom, whatever the panel's height, and
/// the history in the space above it, which scrolls.
fn command_panel(ui: &mut Ui, colors: &Colors, lines: &[ConsoleLine], input: impl FnOnce(&mut Ui)) {
    egui::Panel::bottom("arc-command-input")
        .show_separator_line(false)
        .resizable(false)
        .frame(egui::Frame::NONE)
        .show(ui, input);
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ui, |ui| show_console(ui, lines, colors));
}

/// The command-line panel of the active tab.
#[derive(Debug, Default)]
pub struct CommandLinePanel;

impl Panel for CommandLinePanel {
    #[allow(
        clippy::too_many_lines,
        reason = "the controlled line, then the shell's own"
    )]
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let (Some(view), Some(tab_id)) = (cx.view.as_deref_mut(), cx.tab) else {
            return;
        };
        let Ok(tab) = cx.app.tab(tab_id) else {
            return;
        };
        let registry = &cx.app.product().headless().registry;
        let session = session(cx.app, Surface::CommandLine);
        let commands = LineCommands {
            undo: commands::EDIT_UNDO.into(),
            redo: commands::EDIT_REDO.into(),
            clear_selection: commands::SELECT_CLEAR.into(),
        };
        if let Some(controller) = view.interaction.as_deref() {
            // The controller keeps the line: its prompt, history and options are what shows.
            let out = controller_output(controller, &view.selection);
            let context = LineContext {
                localize: cx.localize,
                has_selection: out.status.has_selection,
                commands: &commands,
            };
            let skip = out.history.len().saturating_sub(MAX_SHOWN_HISTORY);
            let mut lines: Vec<ConsoleLine> = Vec::new();
            let errors = view.console.iter();
            let mut pending = errors.peekable();
            for (index, line) in out.history.iter().enumerate() {
                // Sequence numbers, so lines the controller dropped do not move the shell's.
                let number = out
                    .history_first
                    .saturating_add(u64::try_from(index).unwrap_or(u64::MAX));
                while let Some((_, e)) = pending.next_if(|(at, _)| *at <= number) {
                    lines.push(ConsoleLine::of(e, cx.localize, registry));
                }
                if index >= skip {
                    lines.push(ConsoleLine::Text(line.clone()));
                }
            }
            for (_, e) in pending {
                lines.push(ConsoleLine::of(e, cx.localize, registry));
            }
            view.line_view.command.clone_from(&out.command);
            view.line_view.space_submits = !out.inline_arguments;
            let completion = arc_command::line::Completion::default();
            let status: LineStatus = out.status;
            let viewport_id = view.widget.id;
            let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let line_view = &mut view.line_view;
            let option_bar = &mut view.option_bar;
            let actions = &mut *cx.actions;
            let localize = cx.localize;
            command_panel(ui, cx.colors, &lines, |ui| {
                if let Some(action) = line_view.show_with(
                    ui,
                    status,
                    out.prompt.as_ref(),
                    false,
                    &completion,
                    &context,
                ) {
                    actions.push(Action::Line(action));
                }
                actions.extend(line_view.take_queued().into_iter().map(Action::Line));
                // Escape on an empty line gives the keyboard back to the viewport.
                if escaped
                    && line_view.input.is_empty()
                    && let Some(id) = viewport_id
                {
                    ui.memory_mut(|m| m.request_focus(id));
                }
                if let Some(text) = option_bar.show(ui, &out.options, localize) {
                    actions.push(Action::Line(LineAction::Submit(text)));
                }
            });
            return;
        }
        let lines: Vec<ConsoleLine> = view
            .console
            .iter()
            .map(|(_, e)| ConsoleLine::of(e, cx.localize, registry))
            .collect();
        let localized = BTreeMap::new();
        let target = Target {
            registry,
            document: tab.document(),
            session: &session,
            selection: &view.selection,
            localized: &localized,
            seed: 0,
        };
        let input = view.line_view.input.clone();
        let completion = view.line.complete(&target, &input, input.len());
        let context = LineContext {
            localize: cx.localize,
            has_selection: !view.selection.is_empty(),
            commands: &commands,
        };
        let line_view = &mut view.line_view;
        let line = &view.line;
        let actions = &mut *cx.actions;
        command_panel(ui, cx.colors, &lines, |ui| {
            if let Some(action) = line_view.show(ui, line, &completion, &context) {
                actions.push(Action::Line(action));
            }
            actions.extend(line_view.take_queued().into_iter().map(Action::Line));
        });
    }
}

/// The diagnostics panel of the active document.
#[derive(Debug, Default)]
pub struct DiagnosticsPanelView;

impl Panel for DiagnosticsPanelView {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let (Some(view), Some(tab)) = (cx.view.as_deref_mut(), cx.tab) else {
            return;
        };
        let Ok(tab) = cx.app.tab(tab) else {
            return;
        };
        let snapshot = tab.document().diagnostics_snapshot();
        cx.language.clone_into(&mut view.diagnostics.language);
        if let Some(intent) =
            view.diagnostics
                .show(ui, &snapshot, cx.localize, commands::SELECT_SET)
        {
            cx.actions.push(Action::Intent(intent));
        }
    }
}

const SETTINGS_SEARCH: Message = Message {
    message_id: "app-settings-search",
};
const SETTINGS_RESET: Message = Message {
    message_id: "app-settings-reset",
};
const LOG_EMPTY: Message = Message {
    message_id: "app-log-empty",
};

fn setting_text(value: &SettingValue) -> String {
    match value {
        SettingValue::Bool(v) => v.to_string(),
        SettingValue::Int(v) => v.to_string(),
        SettingValue::Float(v) => v.to_string(),
        SettingValue::Text(v) => v.clone(),
    }
}

/// Reads typed text as a value of `ty`; `None` when it does not parse. The store checks ranges
/// and choices.
fn parse_setting(ty: &SettingType, text: &str) -> Option<SettingValue> {
    let text = text.trim();
    match ty {
        SettingType::Bool => text.parse().ok().map(SettingValue::Bool),
        SettingType::Int { .. } => text.parse().ok().map(SettingValue::Int),
        SettingType::Float => text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(SettingValue::Float),
        SettingType::Text | SettingType::Choice(_) => Some(SettingValue::Text(text.to_owned())),
        SettingType::Internal => None,
    }
}

/// The settings panel (CORE-APP-011): every setting with its effective value, filtered by a
/// search text; values settable in the user scope are edited here (a check box, a choice list
/// or a text field committed when it loses focus) and reset to what lower scopes give. Edits
/// become [`Action::Setting`]s, which the shell writes to the user settings file.
#[derive(Debug, Default)]
pub struct SettingsPanel {
    /// The search text.
    pub query: String,
    /// The text fields being edited, by key.
    drafts: BTreeMap<&'static str, String>,
}

impl SettingsPanel {
    fn row(
        &mut self,
        ui: &mut Ui,
        row: &settings_editor::Row,
        ty: &SettingType,
        localize: &dyn Localize,
        actions: &mut Vec<Action>,
    ) {
        let label = ui.label(row.key);
        let mut edit = |value: Option<SettingValue>| {
            actions.push(Action::Setting {
                key: row.key.to_owned(),
                value,
            });
        };
        match (ty, &row.value) {
            (SettingType::Bool, SettingValue::Bool(on)) => {
                let mut v = *on;
                if ui.checkbox(&mut v, "").labelled_by(label.id).changed() {
                    edit(Some(SettingValue::Bool(v)));
                }
            }
            (SettingType::Choice(choices), SettingValue::Text(current)) => {
                egui::ComboBox::from_id_salt(("arc-app-setting", row.key))
                    .selected_text(current.as_str())
                    .show_ui(ui, |ui| {
                        for choice in *choices {
                            if ui.selectable_label(*choice == current, *choice).clicked()
                                && *choice != current
                            {
                                edit(Some(SettingValue::Text((*choice).to_owned())));
                            }
                        }
                    });
            }
            (ty, value) => {
                let draft = self
                    .drafts
                    .entry(row.key)
                    .or_insert_with(|| setting_text(value));
                let response = ui.text_edit_singleline(draft).labelled_by(label.id);
                if response.lost_focus() {
                    match parse_setting(ty, draft) {
                        Some(v) if v != *value => edit(Some(v)),
                        Some(_) => {}
                        None => actions.push(Action::Error(
                            Error::new(crate::codes::SETTINGS_TYPE_MISMATCH)
                                .with_arg("key", row.key),
                        )),
                    }
                    self.drafts.remove(row.key);
                } else if !response.has_focus() {
                    *draft = setting_text(value);
                }
            }
        }
        if row.source == Scope::User && ui.button(SETTINGS_RESET.text(localize)).clicked() {
            actions.push(Action::Setting {
                key: row.key.to_owned(),
                value: None,
            });
        }
    }
}

impl Panel for SettingsPanel {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let search = ui.label(SETTINGS_SEARCH.text(cx.localize));
        ui.text_edit_singleline(&mut self.query)
            .labelled_by(search.id);
        let store = cx.app.settings();
        let query = self.query.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("arc-app-settings-rows")
            .show(ui, |ui| {
                for row in settings_editor::rows(store) {
                    let description = arc_ui::resolve(cx.localize, row.message_id, &[]);
                    if !query.is_empty()
                        && !row.key.contains(&query)
                        && !description.to_lowercase().contains(&query)
                    {
                        continue;
                    }
                    let Some(setting) = store.registry().get(row.key) else {
                        continue;
                    };
                    // Values above the user scope's reach are shown, not edited.
                    let editable = row.scope >= Scope::User && !row.locked;
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(editable, |ui| {
                            self.row(ui, &row, &setting.ty, cx.localize, cx.actions);
                        })
                        .response
                        .on_hover_text(description);
                    });
                }
            });
    }
}

/// The log viewer panel (CORE-APP-015): the recent records the platform keeps in its
/// [`LogRing`], newest at the bottom. Records are the formatted log lines, so values logged
/// as [`Redacted`](crate::logging::Redacted) show as `[redacted]`.
#[derive(Debug, Default)]
pub struct LogPanel;

impl Panel for LogPanel {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let Some(ring) = cx.log.filter(|r| !r.is_empty()) else {
            ui.label(LOG_EMPTY.text(cx.localize));
            return;
        };
        let height = ui.text_style_height(&egui::TextStyle::Body);
        egui::ScrollArea::both().stick_to_bottom(true).show_rows(
            ui,
            height,
            ring.len(),
            |ui, rows| {
                for record in ring.range(rows) {
                    ui.add(egui::Label::new(record).extend());
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_base::id::Identifier;

    #[test]
    fn the_shell_history_keeps_the_newest_lines() {
        let mut lines = Vec::new();
        for n in 0..MAX_CONSOLE + 10 {
            push_bounded(&mut lines, 0, n);
        }
        assert_eq!(lines.len(), MAX_CONSOLE);
        assert_eq!(lines.first().map(|l| l.1), Some(10));
        assert_eq!(lines.last().map(|l| l.1), Some(MAX_CONSOLE + 9));
    }

    #[test]
    fn selection_modes() {
        let [a, b, c] = [1u128, 2, 3].map(ObjectId::from_u128);
        let mut s = vec![a];
        select(&mut s, [b, b].into_iter(), SelectMode::Add);
        assert_eq!(s, [a, b]);
        select(&mut s, [a, c].into_iter(), SelectMode::Toggle);
        assert_eq!(s, [b, c]);
        select(&mut s, [a].into_iter(), SelectMode::Replace);
        assert_eq!(s, [a]);
    }
}
