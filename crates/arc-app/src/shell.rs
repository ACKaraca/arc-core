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

pub mod event_loop;
pub mod interaction;
pub mod platform;
pub mod prefs;
pub mod prompts;

pub use interaction::{DynamicOutput, Interaction, InteractionContext, InteractionOutput};

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

/// Submitted lines of a controller the command panel shows, most recent last.
const MAX_SHOWN_HISTORY: usize = 16;

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
}

/// What a panel asks the shell to do after drawing.
#[derive(Debug)]
pub enum Action {
    /// Carry out an intent: a shell command, or a command started through the command line.
    Intent(Intent),
    /// A command-line action of the command-line widget.
    Line(LineAction),
    /// An answer for the active prompt from the viewport.
    Answer(PromptInput),
    /// An error to show.
    Error(Error),
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The initial camera: looking down the z axis at the origin, 20 m high.
fn initial_camera() -> Result<ViewFrame> {
    ViewFrame::new(
        Point3::new(0.0, 0.0, 100.0),
        Point3::origin(),
        Vector3::y(),
        Projection::Orthographic { height: 20.0 },
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
    files: Option<UserFiles>,
    recent_written: Option<Vec<RecentEntry>>,
    log_ring: Option<LogRing>,
    settings_changed: bool,
    recorder: Option<Recorder>,
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

fn session(surface: Surface) -> Session {
    Session::new(
        surface,
        [
            Permission::Read,
            Permission::Edit,
            Permission::FileRead,
            Permission::FileWrite,
        ],
    )
}

impl Shell {
    /// A shell for `app`'s product: calls each panel and extractor factory once. A factory that
    /// does not return the expected type, and key bindings the registry refuses, become errors
    /// in [`Shell::errors`]; the rest still works. `gpu` is the device viewports render on;
    /// without one the viewport panel paints its background only.
    #[must_use]
    pub fn new(app: &Application, config: ShellConfig, gpu: Option<Gpu>) -> Self {
        let product = app.product();
        let last = product.headless().last.clone();
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
            layout: config.layout,
            menu: config.menu,
            keymap,
            chords: ChordResolver::default(),
            appearance: config.appearance,
            extractors,
            gpu,
            views: BTreeMap::new(),
            ui_session: session(Surface::Ui).with_last_hook(last.clone()),
            line_session: session(Surface::CommandLine).with_last_hook(last),
            errors,
            log: Vec::new(),
            seed: config.seed,
            base_appearance: config.appearance,
            notifications: Notifications::default(),
            recovery: Vec::new(),
            files: None,
            recent_written: None,
            log_ring: None,
            settings_changed: false,
            recorder: None,
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
            camera: initial_camera()?,
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
                let results = driver.take_results();
                view.pending = !results.is_empty();
                if let Some(viewport) = &view.viewport
                    && !results.is_empty()
                    && let Err(e) = viewport.update_scene(results)
                {
                    errors.push(e);
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
        self.views
            .values()
            .any(|v| v.pending || v.viewport.as_ref().is_some_and(|vp| vp.dirty().any()))
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
        let mut actions: Vec<Action> = self
            .chords
            .handle_events(&self.keymap, &input.raw.events, millis)
            .into_iter()
            .chain(input.menu_intents)
            .map(Action::Intent)
            .collect();
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
            let log_ring = self.log_ring.as_ref();
            let main = window.is_none();
            let mut view = active.and_then(|t| self.views.get_mut(&t));
            if let (Some(v), Some(gpu)) = (view.as_deref_mut(), gpu.as_ref())
                && v.viewport.is_none()
            {
                match Viewport::new(gpu, viewport_config(v.camera)) {
                    Ok(vp) => v.viewport = Some(vp),
                    Err(e) => actions.push(Action::Error(e)),
                }
            }
            ctx.run_ui(input.raw, |ui| {
                if !entries.is_empty() {
                    egui::Panel::top("arc-app-menu").show(ui, |ui| {
                        if let Some(intent) = arc_ui::menu::show_menu_bar(ui, &entries) {
                            actions.push(Action::Intent(intent));
                        }
                    });
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
                    };
                    p.show(ui, &mut cx);
                });
            })
        };
        if let (Some(at), Some(own)) = (floating_at, own) {
            self.write_back(at, &own);
        }
        self.finish(app, full, choices, actions)
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
        full: egui::FullOutput,
        choices: Vec<Choice>,
        actions: Vec<Action>,
    ) -> FrameOutput {
        let mut requests = Vec::new();
        for choice in choices {
            if let Err(e) = self.choose(app, choice) {
                self.error(e);
            }
        }
        for action in actions {
            self.act(app, action, &mut requests);
        }
        self.pump(app, None);
        self.save_recent(app);
        FrameOutput {
            full,
            requests,
            busy: self.busy(),
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
    fn choose(&mut self, app: &mut Application, choice: Choice) -> Result<()> {
        match choice {
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
        let result = match action {
            Action::Error(e) => Err(e),
            Action::Setting { key, value } => self.set_setting(app, &key, value),
            Action::Intent(intent) => self.dispatch(app, intent, requests),
            Action::Answer(input) => self.line_step(app, LineStep::Answer(input), requests),
            Action::Line(line) if self.controlled(app) => self.controller_line(app, line),
            Action::Line(line) => match line {
                LineAction::ClearInput => Ok(()),
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
            self.error(e);
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
        app.close_with(tab, ask, |app, tab| {
            let Some(view) = views.get_mut(&tab) else {
                return;
            };
            let Some(controller) = view.interaction.as_deref_mut() else {
                return;
            };
            let session =
                session(Surface::CommandLine).with_last_hook(app.product().headless().last.clone());
            let session = session_for(app.is_read_only_tab(tab), &session);
            let cx = InteractionContext::new(app, tab, &session, &view.selection, view.camera);
            controller.closing(&cx);
        })
    }

    /// Hands a command-panel action to the active tab's controller.
    fn controller_line(&mut self, app: &Application, line: LineAction) -> Result<()> {
        let tab = app
            .active()
            .ok_or_else(|| Error::new(crate::codes::DOCUMENT_UNKNOWN))?;
        let session =
            session(Surface::CommandLine).with_last_hook(app.product().headless().last.clone());
        let session = session_for(app.is_read_only_tab(tab), &session);
        let Some(view) = self.views.get_mut(&tab) else {
            return Err(Error::new(crate::codes::DOCUMENT_UNKNOWN));
        };
        let cx = InteractionContext::new(app, tab, &session, &view.selection, view.camera);
        match view.interaction.as_deref_mut() {
            Some(controller) => controller.line(&cx, line),
            None => Ok(()),
        }
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
                if let Some((recorder, invocation)) = recorded {
                    // A step the recorder refuses (a full recorder) does not fail the run.
                    let _ = recorder.record(registry, &invocation, revision);
                }
                Ok(outcome)
            };
            view.line.drive(&target, step, &mut runner)
        };
        self.views.insert(tab, view);
        let submitted = result?;
        if let Submitted::Rejected { error, .. } = submitted {
            self.error(error);
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
            commands::FILE_NEW => app.new_document().map(drop),
            commands::FILE_OPEN => match path {
                Some(p) => app.open(&p).map(drop),
                None => {
                    requests.push(Request::OpenDialog);
                    Ok(())
                }
            },
            commands::FILE_SAVE => {
                let tab = need()?;
                if app.tab(tab)?.path().is_some() {
                    app.save(tab)
                } else {
                    requests.push(Request::SaveDialog(tab));
                    Ok(())
                }
            }
            commands::FILE_SAVE_AS => {
                let tab = need()?;
                match path {
                    Some(p) => app.save_as(tab, &p),
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
                    self.close_tab(app, tab, |_| crate::application::CloseAnswer::Cancel)
                        .map(drop)
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
                let tab = need()?;
                let snapshot = app.tab(tab)?.document().snapshot();
                let Some(view) = self.views.get_mut(&tab) else {
                    return Ok(());
                };
                if command == commands::SELECT_CLEAR {
                    view.selection.clear();
                    return Ok(());
                }
                let p: SelectParams =
                    serde_json::from_value(Json::Object(params)).map_err(|e| {
                        Error::new(arc_command::error::codes::PARAMS_MALFORMED).with_source(e)
                    })?;
                let objects = p
                    .objects
                    .into_iter()
                    .map(|o| o.0)
                    .filter(|o| snapshot.resolve(*o).is_ok());
                select(&mut view.selection, objects, p.mode);
                Ok(())
            }
            _ => Ok(()),
        }
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

fn viewport_config(camera: ViewFrame) -> ViewportConfig {
    let mut config = ViewportConfig::new(64, 64, camera);
    config.readback = false;
    config
}

/// Picking through a tab's [`Viewport`] on its render thread.
struct ViewportPicker<'a>(Option<&'a Viewport>);

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
        viewport.with_scene(WAIT, move |renderer, scene, settings| {
            renderer.pick(scene, &frame, width, height, settings, &region)
        })?
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
            ..
        } = view;
        let session =
            session(Surface::CommandLine).with_last_hook(cx.app.product().headless().last.clone());
        let controlled = session_for(cx.tab.is_some_and(|t| cx.app.is_read_only_tab(t)), &session);
        // With a controller the controller's prompt, selection and overlays are the viewport's.
        let frame = match (interaction.as_deref(), cx.tab) {
            (Some(controller), Some(_)) => Some(controller.output()),
            _ => None,
        };
        let icx = cx.tab.map(|tab| {
            InteractionContext::new(cx.app, tab, &controlled, selection.as_slice(), *camera)
        });
        let prompt = match frame {
            Some(_) => None,
            None => line.active().and_then(|s| s.prompt()),
        };
        let highlight: &[ObjectId] = match &frame {
            Some(f) => &f.selection,
            None => selection.as_slice(),
        };
        let selected = highlight.len();
        let mut picker = ViewportPicker(viewport.as_ref());
        let mut hook = match (interaction.as_deref_mut(), icx.as_ref()) {
            (Some(controller), Some(cx)) => Some(Hook { controller, cx }),
            _ => None,
        };
        let product = frame.as_ref().map(|f| ProductInput {
            pointer: hook.as_mut().map(|h| h as &mut dyn PointerHandler),
            overlays: &f.overlays,
            dynamic: f.dynamic.as_ref().map(|d| DynamicPanel {
                prompt: &d.prompt,
                fields: &d.fields,
            }),
        });
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
        if let (Some(controller), Some(icx)) = (interaction.as_deref_mut(), icx.as_ref()) {
            for event in &out.dynamic {
                controller.dynamic(icx, event);
            }
        }
        if let Some(c) = out.camera {
            *camera = c;
        }
        let cube = ViewCubeInput {
            frame: camera,
            localize: cx.localize,
            colors: cx.colors,
            size: ui.spacing().interact_size.y * 4.0,
        };
        // The shell has no animation tick, so the end frame of the transition is applied at once
        // (`interpolate` at t = 1).
        if let Some(intent) = ViewCube.show_over(ui, out.response.rect, &cube) {
            match intent
                .frame_from(camera)
                .and_then(|end| arc_gpu::navigation::interpolate(camera, &end, 1.0))
            {
                Ok(end) => *camera = end,
                Err(e) => cx.actions.push(Action::Error(e)),
            }
        }
        *hover = out.hover;
        let overlay = (!out.overlay.primitives.is_empty()).then_some(out.overlay);
        sync_viewport(
            viewport.as_ref(),
            out.size,
            camera,
            *hover,
            highlight,
            overlay,
            cx.actions,
        );
        cx.actions.extend(out.errors.into_iter().map(Action::Error));
        cx.actions.extend(out.intent.map(Action::Intent));
        cx.actions.extend(out.answer.map(Action::Answer));
    }
}

/// Hands the frame's size, camera, highlight and overlay to the viewport; failures become
/// actions.
fn sync_viewport(
    viewport: Option<&Viewport>,
    size: [u32; 2],
    camera: &ViewFrame,
    hover: Option<ObjectId>,
    highlight: &[ObjectId],
    overlay: Option<arc_gpu::Overlay>,
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
                TreeItem {
                    id: object.id,
                    label: name.unwrap_or_else(|| format!("{key} {}", object.id)),
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

/// The command-line panel of the active tab.
#[derive(Debug, Default)]
pub struct CommandLinePanel;

impl Panel for CommandLinePanel {
    fn show(&mut self, ui: &mut Ui, cx: &mut PanelContext<'_>) {
        let (Some(view), Some(tab_id)) = (cx.view.as_deref_mut(), cx.tab) else {
            return;
        };
        let Ok(tab) = cx.app.tab(tab_id) else {
            return;
        };
        let registry = &cx.app.product().headless().registry;
        let session =
            session(Surface::CommandLine).with_last_hook(cx.app.product().headless().last.clone());
        let commands = LineCommands {
            undo: commands::EDIT_UNDO.into(),
            redo: commands::EDIT_REDO.into(),
            clear_selection: commands::SELECT_CLEAR.into(),
        };
        if let Some(controller) = view.interaction.as_deref() {
            // The controller keeps the line: its prompt, history and options are what shows.
            let out = controller.output();
            let context = LineContext {
                localize: cx.localize,
                has_selection: !out.selection.is_empty(),
                commands: &commands,
            };
            let skip = out.history.len().saturating_sub(MAX_SHOWN_HISTORY);
            for line in out.history.iter().skip(skip) {
                ui.label(line.as_str());
            }
            let completion = arc_command::line::Completion::default();
            let status: LineStatus = out.status;
            if let Some(action) = view.line_view.show_with(
                ui,
                status,
                out.prompt.as_ref(),
                false,
                &completion,
                &context,
            ) {
                cx.actions.push(Action::Line(action));
            }
            if let Some(text) = view.option_bar.show(ui, &out.options, cx.localize) {
                cx.actions.push(Action::Line(LineAction::Submit(text)));
            }
            return;
        }
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
        if let Some(action) = view.line_view.show(ui, &view.line, &completion, &context) {
            cx.actions.push(Action::Line(action));
        }
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
        egui::ScrollArea::vertical().show(ui, |ui| {
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
