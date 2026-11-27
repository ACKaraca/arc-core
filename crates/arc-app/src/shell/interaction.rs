//! The product's interaction controller (CORE-APP-011).
//!
//! A product that runs its own command line, prompts and viewport input registers a factory with
//! [`ProductBuilder::interaction`](crate::product::ProductBuilder::interaction). Each tab's view
//! then owns one [`Interaction`]. While it is registered the shell routes to it: pointer events of
//! the viewport are offered first (a consumed event is not handled by the widget), the command
//! panel's submitted lines, Escape and the option bar go to [`Interaction::line`], dynamic input
//! edits go to [`Interaction::dynamic`], and the panel, option bar, viewport and dynamic input
//! show what [`Interaction::output`] returns. The shell's own command line does not prompt while
//! a controller is registered.
//!
//! The shell carries out its own commands (`app.file.*`, `app.edit.*`, `app.select.*`, ...)
//! itself, whoever controls the line, unless the controller claims the command with
//! [`Interaction::handles`] (by default it claims none): a submitted line
//! or an intent that names one never reaches [`Interaction::line`] (a running tool is cancelled
//! first). Every other command a menu, a
//! key binding or a toolbar button starts reaches the controller as `LineAction::Invoke`, the same
//! way with or without parameters. A controller asks for a shell command, or for a command to
//! start on the shell's own command line, with [`InteractionContext::ask`].
//!
//! Without a registered factory the shell behaves exactly as before.

use std::cell::RefCell;
use std::sync::Arc;

use arc_base::{ObjectId, Result};
use arc_command::prompt::Prompt;
use arc_command::{Invocation, Outcome, Session};
use arc_doc::Document;
use arc_geom::view::ViewFrame;
use arc_ui::{
    DynamicEvent, DynamicField, Intent, LineAction, LineStatus, PointerEvent, PointerHandler,
    ProductOverlay, PromptControls,
};

use crate::application::{Application, TabId};
use crate::settings::SettingValue;

/// The factory a product registers: it creates one controller per tab.
pub type Factory = Arc<dyn Fn() -> Box<dyn Interaction> + Send + Sync>;

/// A product's interaction controller of one tab. The methods run on the shell's frame, one
/// after another; a method must not block.
pub trait Interaction: Send {
    /// A pointer event of the viewport in model coordinates, offered before the widget handles
    /// it. Returns `true` when the event is consumed.
    fn pointer(&mut self, cx: &InteractionContext<'_>, event: &PointerEvent) -> bool;

    /// Called once per frame of the tab's viewport, before [`Self::output`] is read, with the
    /// document and selection of this frame. A controller refreshes what it derives from them
    /// here (handles, highlights). The default does nothing. It must not change the document.
    fn frame(&mut self, _cx: &InteractionContext<'_>) {}

    /// An edit of the dynamic input fields the controller supplied.
    fn dynamic(&mut self, cx: &InteractionContext<'_>, event: &DynamicEvent);

    /// An action of the command panel or a command start: a submitted line or option (`Submit`),
    /// Escape (`Cancel`, `Back`, `ClearInput`), a command of a menu, a key binding or a toolbar
    /// button (`Invoke`, with its parameters; `CancelThen` after Redo) or repeat. Shell commands
    /// never arrive here (see the module documentation).
    ///
    /// # Errors
    ///
    /// Any error of the controller; the shell reports it.
    fn line(&mut self, cx: &InteractionContext<'_>, action: LineAction) -> Result<()>;

    /// Whether the controller carries out the shell command `command` itself. The shell carries
    /// out the commands it owns whoever controls the line, except the ones a controller claims
    /// here: those arrive in [`Self::line`]. The default claims nothing. A controller with its own
    /// undo (one that steps back one answer) claims `app.edit.undo`, `app.edit.redo` and
    /// `app.select.clear`; file, view, macro and quit commands go through the same question.
    fn handles(&self, _command: &str) -> bool {
        false
    }

    /// A key press or typed text while the viewport has the keyboard focus or the pointer is over
    /// it, and no text field has the focus. The shell offers [`KeyInput`]s for the Tab key, for key
    /// presses with the command key that no binding took, and for typed text, after its own key
    /// bindings; `KeyReply::Consumed` removes the event from the
    /// frame, [`KeyReply::Text`] replaces typed text (what the controller held back, for the
    /// command line), `KeyReply::Pass` leaves it. The default passes everything, which keeps the
    /// shell's behavior (typing starts the command line).
    fn key(&mut self, _cx: &InteractionContext<'_>, _input: &KeyInput) -> KeyReply {
        KeyReply::Pass
    }

    /// The text typed in the command line, offered each frame while it is not empty. The
    /// controller may resolve it at once (a shortcut that is complete): it returns the action to
    /// carry out, and the shell clears the input and sends the action to [`Self::line`]. The
    /// default returns `None`, so the line keeps the text until Enter.
    fn input_changed(&mut self, _cx: &InteractionContext<'_>, _text: &str) -> Option<LineAction> {
        None
    }

    /// The entries of the context menu a secondary click opens over the viewport, in order. The
    /// shell computes them while the menu is open, so they follow the selection and the running
    /// command. An empty list (the default) opens no menu.
    fn context_menu(&self, _cx: &InteractionContext<'_>) -> Vec<ContextEntry> {
        Vec::new()
    }

    /// What the shell shows this frame.
    fn output(&self) -> InteractionOutput;

    /// A change of the view the controller asked for since the last call (a zoom command), taken
    /// once. It changes the camera only, never the document.
    fn take_view(&mut self) -> Option<ViewRequest> {
        None
    }

    /// The tab of the controller is closing: the shell calls this once, before the tab's
    /// document is closed and the controller is dropped. It is the last call of the controller,
    /// so an undo group it began or a prompt it left open is ended here. The context's
    /// [`InteractionContext::document`] still works. The hook must not commit a change to the
    /// document: it runs after any save, so the change would not be in the saved file. On the
    /// save path the tab's recovery journal is closed, not discarded; a change committed here
    /// leaves a record in it, which keeps the journal for recovery on the next start.
    ///
    /// Not called when a close is cancelled or fails. Closing a tab through
    /// [`Application::close`] directly, without the shell, drops the controller without this call.
    fn closing(&mut self, _cx: &InteractionContext<'_>) {}
}

/// A key or typed text the shell offers to [`Interaction::key`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct KeyInput {
    /// The key, for a key press (`Tab` or a key with the command key); `None` for typed text.
    pub key: Option<arc_ui::egui::Key>,
    /// The typed text (a printable character or more), for text input; `None` for a key press.
    pub text: Option<String>,
    /// The modifiers held.
    pub modifiers: arc_ui::egui::Modifiers,
}

impl KeyInput {
    /// A key press of `key` with no modifier.
    #[must_use]
    pub fn key(key: arc_ui::egui::Key) -> Self {
        Self {
            key: Some(key),
            text: None,
            modifiers: arc_ui::egui::Modifiers::NONE,
        }
    }

    /// Typed `text`.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            key: None,
            text: Some(text.into()),
            modifiers: arc_ui::egui::Modifiers::NONE,
        }
    }
}

/// What a controller did with a [`KeyInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyReply {
    /// Not the controller's: the shell handles the event as before.
    Pass,
    /// The controller took the event; the shell removes it from the frame.
    Consumed,
    /// The controller took the event and gives this typed text back in its place, so the
    /// command line receives what the controller had held (letters of a shortcut that did not
    /// complete).
    Text(String),
}

/// One entry of a context menu.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ContextEntry {
    /// The Fluent identifier of the label; empty for a separator.
    pub label: String,
    /// What choosing the entry does: the action goes to the controller like a command-line action
    /// (`LineAction::Submit(String::new())` is Enter, `LineAction::Invoke` starts a command).
    pub action: arc_ui::LineAction,
    /// Whether the entry can be chosen; a disabled entry is shown dimmed.
    pub enabled: bool,
    /// Whether the entry is a separator line (its label and action are unused).
    pub separator: bool,
}

impl ContextEntry {
    /// An enabled entry that does `action`.
    #[must_use]
    pub fn new(label: impl Into<String>, action: arc_ui::LineAction) -> Self {
        Self {
            label: label.into(),
            action,
            enabled: true,
            separator: false,
        }
    }

    /// An enabled entry that starts the command `command`.
    #[must_use]
    pub fn command(label: impl Into<String>, command: &str) -> Self {
        Self::new(label, arc_ui::LineAction::Invoke(Intent::new(command)))
    }

    /// A separator.
    #[must_use]
    pub fn separator() -> Self {
        Self {
            label: String::new(),
            action: arc_ui::LineAction::ClearInput,
            enabled: false,
            separator: true,
        }
    }

    /// This entry, shown dimmed and not choosable.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// A change of the camera a controller asks for. Corners are model coordinates in drawing units.
/// Variants may be added; a match needs a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum ViewRequest {
    /// Frame everything in the viewport's scene.
    Extents,
    /// Return to the camera before the last request.
    Previous,
    /// Frame the rectangle between two corners.
    Window {
        /// One corner.
        a: [f64; 2],
        /// The opposite corner.
        b: [f64; 2],
    },
    /// Scale the view about its center by a factor above zero.
    Scale(f64),
    /// Look straight down (along −z, with +y up on the screen) with an orthographic camera
    /// framed on the rectangle between two corners at the height `z`, and lock the orientation:
    /// until a [`ViewRequest::Camera`] request, navigation pans and zooms but does not rotate,
    /// and the view cube is not shown. A product shows a plan drawing this way.
    Plan {
        /// One corner.
        a: [f64; 2],
        /// The opposite corner.
        b: [f64; 2],
        /// The height of the drawing.
        z: f64,
    },
    /// Use this camera (one the controller read from [`InteractionContext::camera`] earlier) and
    /// release the orientation lock of [`ViewRequest::Plan`] and [`ViewRequest::Facing`].
    Camera(ViewFrame),
    /// Look along `forward` with `up` on the screen with an orthographic camera framed on the
    /// rectangle of `size` (width along the screen's x, height along its y) centered at `center`,
    /// and lock the orientation as [`ViewRequest::Plan`] does. A product shows a rotated plan, a
    /// section or an elevation drawing this way. Directions need not be unit vectors; `up` is
    /// made perpendicular to `forward`.
    Facing {
        /// The middle of the rectangle.
        center: [f64; 3],
        /// The direction the camera looks in.
        forward: [f64; 3],
        /// The direction that is up on the screen.
        up: [f64; 3],
        /// The width and height of the rectangle.
        size: [f64; 2],
    },
}

/// The dynamic input panel a controller asks for: its prompt message and fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DynamicOutput {
    /// The prompt's message identifier.
    pub prompt: String,
    /// The fields, in order.
    pub fields: Vec<DynamicField>,
}

/// What a controller shows in one frame.
#[derive(Debug, Clone, Default)]
pub struct InteractionOutput {
    /// The state the command line's keys read (active, answers, repeatable).
    pub status: LineStatus,
    /// The name of the running command, shown before its prompt.
    pub command: Option<String>,
    /// The active prompt, labeling the command line.
    pub prompt: Option<Prompt>,
    /// The option bar: keyword options and parameters of the active prompt.
    pub options: PromptControls,
    /// Submitted lines to show above the command line, oldest first.
    pub history: Vec<String>,
    /// Overlays painted over the viewport in model coordinates.
    pub overlays: Vec<ProductOverlay>,
    /// The dynamic input panel, when point input is active.
    pub dynamic: Option<DynamicOutput>,
    /// The model point the pointer resolves to (snap step, ortho, object snap), in drawing units:
    /// the status bar shows it and the crosshair sits on it. `None` shows the pointer itself.
    pub cursor: Option<[f64; 2]>,
    /// The digits after the decimal point of the status bar coordinates; `None` for the default.
    pub precision: Option<usize>,
    /// The objects the controller has selected of its own; the viewport highlights them and its
    /// selected count is their number. `None` (the default) shows the tab's selection, which
    /// follows a click, Escape and a command at once.
    pub selection: Option<Vec<ObjectId>>,
    /// The command line keeps a space as text instead of ending the answer at it, so an idle line
    /// takes inline arguments (`app.file.open path=<file>`). The default ends the input at a
    /// space like Enter, as a product with keyword answers and one-letter commands expects.
    pub inline_arguments: bool,
    /// The sequence number of `history[0]`: a controller that drops its oldest lines raises it
    /// by the number dropped, so the lines the shell keeps among them (typed commands, prompts)
    /// stay where they were. The default is 0.
    pub history_first: u64,
    /// The mouse pointer the viewport shows over the drawing (a resize or grab arrow over a
    /// handle, for example). `None` (the default) keeps the crosshair; a value hides the crosshair
    /// and sets this icon.
    pub pointer_icon: Option<arc_ui::egui::CursorIcon>,
    /// The controller writes the completion and cancel lines itself. The default is false: the
    /// shell adds `Done: <command>` for each command the controller ran through
    /// [`InteractionContext::invoke`] and `*Cancel*` for Escape at a prompt.
    pub reports_results: bool,
    /// A short text the viewport shows in a framed banner while a temporary display state is
    /// active (isolated or hidden objects), already localized. `None` (the default) shows none.
    pub banner: Option<String>,
    /// Objects the viewport does not draw and does not pick (a temporary hide; never saved).
    pub hidden: Vec<ObjectId>,
    /// When set, the only objects the viewport draws and picks (a temporary isolation); the
    /// other objects are hidden. `None` (the default) shows every object that is not in
    /// [`Self::hidden`].
    pub isolated: Option<Vec<ObjectId>>,
    /// How the viewport draws faces and edges (a view's visual style). `None` (the default) is
    /// [`arc_gpu::ShadingMode::Shaded`].
    pub shading: Option<arc_gpu::ShadingMode>,
    /// The section planes and box that clip the viewport's meshes (a 3D view's section box).
    /// `None` (the default) clips nothing.
    pub section: Option<arc_gpu::display::Section>,
}

/// The most commands one call of a controller can ask the shell for.
const MAX_ASKED: usize = 16;

/// The most objects one call of a controller can ask the shell to extract again.
const MAX_EXTRACT: usize = 10_000;

/// What a controller reads: the application, the tab, the session the command line runs in, the
/// tab's selection, its camera (the view the controller's input is in) and the interface language.
pub struct InteractionContext<'a> {
    app: &'a Application,
    tab: TabId,
    session: &'a Session,
    selection: &'a [ObjectId],
    camera: ViewFrame,
    language: &'a str,
    asked: RefCell<Vec<Intent>>,
    ran: RefCell<Vec<String>>,
    settings: RefCell<Vec<(String, Option<SettingValue>)>>,
    panels: RefCell<Vec<String>>,
    clipboard: RefCell<Option<String>>,
    extract: RefCell<Vec<ObjectId>>,
}

impl<'a> InteractionContext<'a> {
    pub(crate) fn new(
        app: &'a Application,
        tab: TabId,
        session: &'a Session,
        selection: &'a [ObjectId],
        camera: ViewFrame,
        language: &'a str,
    ) -> Self {
        Self {
            app,
            tab,
            session,
            selection,
            camera,
            language,
            asked: RefCell::new(Vec::new()),
            ran: RefCell::new(Vec::new()),
            settings: RefCell::new(Vec::new()),
            panels: RefCell::new(Vec::new()),
            clipboard: RefCell::new(None),
            extract: RefCell::new(Vec::new()),
        }
    }

    /// The objects the controller asked to extract again during the call, taken once.
    pub(crate) fn take_extraction(&self) -> Vec<ObjectId> {
        std::mem::take(&mut *self.extract.borrow_mut())
    }

    /// The text the controller asked to put on the operating system clipboard, taken once.
    pub(crate) fn take_clipboard_text(&self) -> Option<String> {
        self.clipboard.borrow_mut().take()
    }

    /// The panels the controller asked to show during the call, taken once.
    pub(crate) fn take_panels(&self) -> Vec<String> {
        std::mem::take(&mut *self.panels.borrow_mut())
    }

    /// The settings the controller asked to change during the call, taken once.
    pub(crate) fn take_settings(&self) -> Vec<(String, Option<SettingValue>)> {
        std::mem::take(&mut *self.settings.borrow_mut())
    }

    /// The commands the controller asked for during the call, taken once.
    pub(crate) fn take_asked(&self) -> Vec<Intent> {
        std::mem::take(&mut *self.asked.borrow_mut())
    }

    /// The commands the controller ran through [`Self::invoke`] during the call, taken once.
    pub(crate) fn take_ran(&self) -> Vec<String> {
        std::mem::take(&mut *self.ran.borrow_mut())
    }

    /// The interface language tag in effect (for example `tr`), also the session setting
    /// `app.language`.
    #[must_use]
    pub fn language(&self) -> &'a str {
        self.language
    }

    /// The decimal separator numbers are typed and shown with in the interface language.
    #[must_use]
    pub fn decimal_separator(&self) -> char {
        arc_ui::decimal_separator(self.language)
    }

    /// Asks the shell to carry out `intent` after the controller's call returns: a shell command
    /// (`app.file.save`, `app.select.clear`, a view command) is carried out by the shell; any other
    /// command is started on the shell's own command line, which runs it when its parameters are
    /// complete (that line does not prompt while a controller is registered, so the command must
    /// not need prompts). At most 16 commands per call are kept.
    pub fn ask(&self, intent: Intent) {
        let mut asked = self.asked.borrow_mut();
        if asked.len() < MAX_ASKED {
            asked.push(intent);
        }
    }

    /// Asks the shell to set the setting `key` in the user scope (`Some`) or to remove it from
    /// there (`None`) after the controller's call returns; the shell writes the user settings
    /// file as it does for an edit in the settings panel. At most 16 changes per call are kept.
    pub fn ask_setting(&self, key: &str, value: Option<SettingValue>) {
        let mut settings = self.settings.borrow_mut();
        if settings.len() < MAX_ASKED {
            settings.push((key.to_owned(), value));
        }
    }

    /// Asks the shell to put `text` (at most 4096 bytes; a longer text is cut) on the operating
    /// system clipboard when the frame ends. A controller that keeps its own clipboard does this
    /// on copy: the platform reports a paste shortcut only while the system clipboard holds
    /// something, so the internal clipboard is pasted by that shortcut.
    pub fn ask_clipboard_text(&self, text: &str) {
        let mut end = text.len().min(4096);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        *self.clipboard.borrow_mut() = text.get(..end).map(str::to_owned);
    }

    /// Asks the shell to show the panel `panel` (its identifier among the product's panels) after
    /// the controller's call returns: a hidden panel opens where it belongs, one in a tab group
    /// comes to the front. At most 16 requests per call are kept.
    pub fn ask_panel(&self, panel: &str) {
        let mut panels = self.panels.borrow_mut();
        if panels.len() < MAX_ASKED {
            panels.push(panel.to_owned());
        }
    }

    /// Asks the shell to extract `objects` again from the tab's current snapshot after the
    /// controller's call returns, as a commit that changed them would: on the extraction
    /// workers, cancelling an older extraction of the same object, and drawn when the result
    /// arrives. A controller uses it when what an object draws depends on state outside the
    /// document (a drawing filtered by a temporary display state, a view opened). Honored from
    /// [`Interaction::frame`], [`Interaction::line`] and [`Interaction::key`]; at most
    /// 10 000 objects per call are kept.
    pub fn ask_extraction(&self, objects: &[ObjectId]) {
        let mut extract = self.extract.borrow_mut();
        let room = MAX_EXTRACT.saturating_sub(extract.len());
        extract.extend(objects.iter().take(room));
    }

    /// The tab the controller belongs to.
    #[must_use]
    pub fn tab(&self) -> TabId {
        self.tab
    }

    /// The file of the tab's document; `None` before its first save (and when the tab is closed).
    #[must_use]
    pub fn document_path(&self) -> Option<&'a std::path::Path> {
        self.app.tab(self.tab).ok()?.path()
    }

    /// The tab's document.
    ///
    /// # Errors
    ///
    /// `app.document.unknown` when the tab is closed.
    pub fn document(&self) -> Result<&'a Document> {
        Ok(self.app.tab(self.tab)?.document())
    }

    /// True when the tab is read-only. A read-only tab must not be changed: a controller that
    /// writes to the document itself checks this first, because [`Self::invoke`] refuses edits
    /// but direct changes do not pass through it.
    #[must_use]
    pub fn read_only(&self) -> bool {
        self.app.is_read_only_tab(self.tab)
    }

    /// The session the command line runs in, with the session setting `app.language` set to the
    /// interface language. On a read-only tab the shell hands the controller a
    /// session without the `edit` permission.
    #[must_use]
    pub fn session(&self) -> &'a Session {
        self.session
    }

    /// The effective value of the setting `key`, or `None` when it is not registered.
    #[must_use]
    pub fn setting(&self, key: &str) -> Option<crate::settings::SettingValue> {
        self.app.settings().get(key).ok().map(|entry| entry.value)
    }

    /// The tab's selection in selection order.
    #[must_use]
    pub fn selection(&self) -> &'a [ObjectId] {
        self.selection
    }

    /// The camera of the tab's viewport.
    #[must_use]
    pub fn camera(&self) -> &ViewFrame {
        &self.camera
    }

    /// Runs a document command for the tab through [`Application::invoke`], which removes the
    /// `edit` permission on a read-only tab. An invocation that names no selection sees the tab's.
    ///
    /// # Errors
    ///
    /// The errors of [`Application::invoke`].
    pub fn invoke(&self, mut invocation: Invocation) -> Result<Outcome> {
        if invocation.selection.is_empty() {
            invocation.selection = self.selection.to_vec();
        }
        let command = invocation.command.clone();
        let outcome = self.app.invoke(self.tab, self.session, invocation)?;
        self.ran.borrow_mut().push(command);
        Ok(outcome)
    }
}

/// Offers viewport pointer events to a controller under one context.
pub(crate) struct Hook<'a, 'c> {
    pub(crate) controller: &'a mut dyn Interaction,
    pub(crate) cx: &'a InteractionContext<'c>,
    /// A press, release or click reached the controller: what it shows changed after the
    /// frame drew, so the shell asks for another frame.
    pub(crate) used: bool,
}

impl PointerHandler for Hook<'_, '_> {
    fn pointer(&mut self, event: &PointerEvent) -> bool {
        self.used |= !matches!(event.kind, arc_ui::PointerKind::Move);
        self.controller.pointer(self.cx, event)
    }
}
