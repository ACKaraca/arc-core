//! The product's interaction controller (CORE-APP-011).
//!
//! A product that runs its own command line, prompts and viewport input registers a factory with
//! [`ProductBuilder::interaction`](crate::product::ProductBuilder::interaction). Each tab's view
//! then owns one [`Interaction`]. While it is registered the shell routes to it: pointer events of
//! the viewport are offered first (a consumed event is not handled by the widget), the command
//! panel's submitted lines, Escape and the option bar go to [`Interaction::line`], dynamic input
//! edits go to [`Interaction::dynamic`], and the panel, option bar, viewport and dynamic input
//! show what [`Interaction::output`] returns. The shell's own command line does not prompt while
//! a controller is registered. Commands that menus and the palette start still use the shell's
//! command line.
//!
//! Without a registered factory the shell behaves exactly as before.

use std::sync::Arc;

use arc_base::{ObjectId, Result};
use arc_command::prompt::Prompt;
use arc_command::{Invocation, Outcome, Session};
use arc_doc::Document;
use arc_geom::view::ViewFrame;
use arc_ui::{
    DynamicEvent, DynamicField, LineAction, LineStatus, PointerEvent, PointerHandler,
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

    /// An action of the command panel: a submitted line or option (`Submit`), Escape (`Cancel`,
    /// `Back`, `ClearInput`), undo or redo (`Invoke`, `CancelThen`) or repeat.
    ///
    /// # Errors
    ///
    /// Any error of the controller; the shell reports it.
    fn line(&mut self, cx: &InteractionContext<'_>, action: LineAction) -> Result<()>;

    /// What the shell shows this frame.
    fn output(&self) -> InteractionOutput;

    /// The tab of the controller is closing: the shell calls this once, before the tab's
    /// document is closed and the controller is dropped. It is the last call of the controller,
    /// so an undo group it began or a prompt it left open is ended here. The context's
    /// [`InteractionContext::document`] still works. A change the controller makes here is not
    /// part of a save made by the close, because the save has already happened.
    ///
    /// Not called when a close is cancelled or fails. Closing a tab through
    /// [`Application::close`] directly, without the shell, drops the controller without this call.
    fn closing(&mut self, _cx: &InteractionContext<'_>) {}
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
    /// The objects the controller has selected; the viewport highlights them and its selected
    /// count is their number.
    pub selection: Vec<ObjectId>,
}

/// What a controller reads: the application, the tab, the session the command line runs in, the
/// tab's selection and its camera (the view the controller's input is in).
pub struct InteractionContext<'a> {
    app: &'a Application,
    tab: TabId,
    session: &'a Session,
    selection: &'a [ObjectId],
    camera: ViewFrame,
}

impl<'a> InteractionContext<'a> {
    pub(crate) fn new(
        app: &'a Application,
        tab: TabId,
        session: &'a Session,
        selection: &'a [ObjectId],
        camera: ViewFrame,
    ) -> Self {
        Self {
            app,
            tab,
            session,
            selection,
            camera,
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

    /// The session the command line runs in. On a read-only tab the shell hands the controller a
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
    /// `edit` permission on a read-only tab.
    ///
    /// # Errors
    ///
    /// The errors of [`Application::invoke`].
    pub fn invoke(&self, invocation: Invocation) -> Result<Outcome> {
        self.app.invoke(self.tab, self.session, invocation)
    }
}

/// Offers viewport pointer events to a controller under one context.
pub(crate) struct Hook<'a, 'c> {
    pub(crate) controller: &'a mut dyn Interaction,
    pub(crate) cx: &'a InteractionContext<'c>,
}

impl PointerHandler for Hook<'_, '_> {
    fn pointer(&mut self, event: &PointerEvent) -> bool {
        self.controller.pointer(self.cx, event)
    }
}
