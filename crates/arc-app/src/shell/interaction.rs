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

/// The factory a product registers: it creates one controller per tab.
pub type Factory = Arc<dyn Fn() -> Box<dyn Interaction> + Send + Sync>;

/// A product's interaction controller of one tab. The methods run on the shell's frame, one
/// after another; a method must not block.
pub trait Interaction: Send {
    /// A pointer event of the viewport in model coordinates, offered before the widget handles
    /// it. Returns `true` when the event is consumed.
    fn pointer(&mut self, cx: &InteractionContext<'_>, event: &PointerEvent) -> bool;

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

/// A change of the camera a controller asks for. Corners are model coordinates in drawing units.
#[derive(Debug, Clone, Copy, PartialEq)]
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
}

/// The most commands one call of a controller can ask the shell for.
const MAX_ASKED: usize = 16;

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
        }
    }

    /// The commands the controller asked for during the call, taken once.
    pub(crate) fn take_asked(&self) -> Vec<Intent> {
        std::mem::take(&mut *self.asked.borrow_mut())
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

    /// The tab the controller belongs to.
    #[must_use]
    pub fn tab(&self) -> TabId {
        self.tab
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
        self.app.invoke(self.tab, self.session, invocation)
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
